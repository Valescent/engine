//! Edit queue and incremental rebuild: one block change relights and remeshes
//! only the sections it can reach.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use glam::IVec3;
use tracing::warn;

use crate::description;
use crate::editlog;
use crate::light2;
use crate::mesher2;
use crate::section;
use crate::world_save;

use super::{COLUMN_SECTIONS, TerrainDemo, demo_light_table, demo_section};
/// One block change fed to an incremental rebuild.
pub(crate) struct DemoEdit {
    pub(crate) cell: IVec3,
}

/// One section in a rebuild result; `mesh: None` removes it.
pub(crate) struct DemoPatch {
    pub(crate) origin: [i32; 3],
    pub(crate) mesh: Option<mesher2::SectionMesh>,
}

/// One finished background rebuild of the patch (`L3.3`).
pub(crate) struct DemoRebuild {
    pub(crate) light: BTreeMap<(i32, i32), light2::ColumnLight>,
    pub(crate) patches: BTreeMap<(i32, i32), Vec<DemoPatch>>,
    /// Columns the edits touched, for the coarse-region refresh.
    pub(crate) columns: BTreeSet<(i32, i32)>,
    pub(crate) light_time: std::time::Duration,
    pub(crate) mesh_time: std::time::Duration,
}

/// Repairs light and remeshes only what an edit can reach.
///
/// Light changes stay within `LIGHT_LEVELS` laterally, so only the 3x3 columns
/// around an edit are relit; within a column the edit's vertical span is
/// repaired by [`light2::ColumnLight::relight_column`]. A section is remeshed
/// when its light changed or when the edit touched its geometry.
#[allow(clippy::too_many_lines)]
pub(crate) fn demo_rebuild(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    previous_light: &BTreeMap<(i32, i32), light2::ColumnLight>,
    materials: &mesher2::MaterialProps,
    edits: &[DemoEdit],
) -> DemoRebuild {
    let size = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
    let mut light = previous_light.clone();
    let mut affected_columns = BTreeSet::new();
    let mut queued = BTreeSet::new();
    for edit in edits {
        let (cx, cz) = (edit.cell.x.div_euclid(size), edit.cell.z.div_euclid(size));
        for dz in -1..=1_i32 {
            for dx in -1..=1_i32 {
                affected_columns.insert((cx + dx, cz + dz));
            }
        }
        // Faces and AO read the 26 neighbours of the changed cell, so only the
        // sections containing that block neighbourhood can change geometry.
        // An interior block costs one section, not a 3x3x3 window.
        for dy in -1..=1_i32 {
            for dz in -1..=1_i32 {
                for dx in -1..=1_i32 {
                    let x = edit.cell.x + dx;
                    let y = edit.cell.y + dy;
                    let z = edit.cell.z + dz;
                    queued.insert((x.div_euclid(size), y.div_euclid(size), z.div_euclid(size)));
                }
            }
        }
    }

    let started = std::time::Instant::now();
    let table = demo_light_table();
    for &(sx, sz) in &affected_columns {
        let Some(stack) = columns.get(&(sx, sz)) else {
            continue;
        };
        let mut borders: [Option<&[section::Section; COLUMN_SECTIONS]>; 8] = [None; 8];
        let mut slot = 0;
        for dz in -1..=1_i32 {
            for dx in -1..=1_i32 {
                if dx == 0 && dz == 0 {
                    continue;
                }
                borders[slot] = columns.get(&(sx + dx, sz + dz)).map(|stack| &**stack);
                slot += 1;
            }
        }
        let view = light2::ColumnView::new(stack, borders);

        let mut owns_edit = false;
        let mut low = i32::MAX;
        let mut high = i32::MIN;
        for edit in edits {
            let (ex, ez) = (edit.cell.x.div_euclid(size), edit.cell.z.div_euclid(size));
            if (ex, ez) == (sx, sz) {
                owns_edit = true;
            }
            low = low.min(edit.cell.y);
            high = high.max(edit.cell.y);
        }
        if high < low {
            continue;
        }
        let (y_lo, y_hi) = if owns_edit {
            // A changed block moves the sky seed for every cell below it.
            let section_top = high - high.rem_euclid(size);
            (0, section_top + size - 1 + light2::RELIGHT_RADIUS)
        } else {
            // A neighbour keeps its sky seed; only lateral light moves.
            (low - light2::RELIGHT_RADIUS, high + light2::RELIGHT_RADIUS)
        };
        if let Some(entry) = light.get_mut(&(sx, sz)) {
            entry.relight_span_view(&view, &table, y_lo, y_hi);
        }
    }
    let light_time = started.elapsed();

    for &(sx, sz) in &affected_columns {
        let (Some(before), Some(after)) = (previous_light.get(&(sx, sz)), light.get(&(sx, sz)))
        else {
            continue;
        };
        for sy in 0..COLUMN_SECTIONS {
            if before.section_differs(after, sy) {
                queued.insert((sx, i32::try_from(sy).unwrap_or(0), sz));
            }
        }
    }

    let mesh_started = std::time::Instant::now();
    let mut patches: BTreeMap<(i32, i32), Vec<DemoPatch>> = BTreeMap::new();
    for &(sx, sy, sz) in &queued {
        let Ok(sy) = usize::try_from(sy) else {
            continue;
        };
        if sy >= COLUMN_SECTIONS || !columns.contains_key(&(sx, sz)) {
            continue;
        }
        let origin = [sx * size, i32::try_from(sy).unwrap_or(0) * size, sz * size];
        let mesh = demo_section(columns, &light, materials, sx, sz, sy).map(|section| section.mesh);
        patches
            .entry((sx, sz))
            .or_default()
            .push(DemoPatch { origin, mesh });
    }
    for list in patches.values_mut() {
        list.sort_by_key(|patch| patch.origin);
    }
    DemoRebuild {
        light,
        patches,
        columns: affected_columns,
        light_time,
        mesh_time: mesh_started.elapsed(),
    }
}

/// The save key for a world cell (`J4.1`).
pub(crate) fn section_key(cell: IVec3) -> section::SectionKey {
    let size = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
    section::SectionKey {
        column: [cell.x.div_euclid(size), cell.z.div_euclid(size)],
        section: u8::try_from(cell.y.div_euclid(size)).unwrap_or(0),
    }
}

/// Queues one edit's journal batch and section write.
///
/// The caller has already written the block into `demo.columns`; `baseline`
/// and both names must come from before the write.
pub(crate) fn queue_demo_edit(
    save: &world_save::SaveWorker,
    demo: &mut TerrainDemo,
    cell: IVec3,
    key: section::SectionKey,
    baseline: [u8; 32],
    old_name: String,
    new_name: String,
) {
    demo.revision = demo.revision.saturating_add(1);
    let revision = demo.revision;
    let section = demo
        .columns
        .get(&(key.column[0], key.column[1]))
        .and_then(|stack| stack.get(usize::from(key.section)))
        .cloned();
    let Some(section) = section else {
        return;
    };
    let Some(record) = editlog::Edit::record(
        [cell.x, cell.y, cell.z],
        old_name,
        new_name,
        revision,
        0,
        0,
        0,
    ) else {
        return;
    };
    let batch = editlog::EditBatch::new(revision, 0, vec![record]);
    if let Err(error) = save.record_batch(batch) {
        warn!(%error, "terrain journal write failed");
    }
    if let Err(error) = save.save_section(key, baseline, true, section, revision) {
        warn!(%error, "terrain section save failed");
    }
}

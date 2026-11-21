//! Save replay and repair: the prompt that blocks saves after a corrupt
//! section, and the journal replay applied before the ring opens.

use std::collections::{BTreeMap, BTreeSet};

#[cfg(test)]
use glam::IVec2;
use glam::IVec3;
use tracing::warn;

use crate::content;
use crate::description;
use crate::editlog;
#[cfg(test)]
use crate::error;
use crate::pipeline;
#[cfg(test)]
use crate::residency;
use crate::section;
use crate::world_save;

use super::COLUMN_SECTIONS;
#[cfg(test)]
use super::lod::demo_lod_sets;
#[cfg(test)]
use super::patch::{
    TerrainDemo, demo_column_light, demo_fine_all, demo_materials, solid_table, worker_count,
};
use super::section_key;

/// A corrupt section that must be repaired before saving can resume.
pub(crate) struct RepairPrompt {
    pub(crate) world: world_save::WorldSave,
    pub(crate) key: section::SectionKey,
}

/// The result of replaying recovered journal frames over the saved sections.
pub(crate) struct ReplayOutcome {
    pub(crate) saved: Vec<section::SavedSection>,
    pub(crate) revision: u64,
    pub(crate) read_only: bool,
    /// Saved section records the world holds.
    pub(crate) sections: usize,
    /// Journal edits this replay applied.
    pub(crate) applied: usize,
}

/// Replays recovered journal frames over saved sections before the ring opens.
///
/// Each frame is applied to the section record it covers, in revision order, so
/// a section file is ahead of the journal exactly like the normal edit path.
/// Frames for a column that has no saved record are left in the journal: the
/// header does not advance past them. Unknown block names make the world
/// read-only (`A16`).
#[allow(clippy::too_many_lines)]
pub(crate) fn replay_saved_sections(
    mut saved: Vec<section::SavedSection>,
    recovered: &[editlog::EditBatch],
    last_saved: u64,
    pack: &content::ContentPack,
    save: Option<&world_save::SaveWorker>,
    terrain: Option<&pipeline::Terrain>,
) -> ReplayOutcome {
    let mut revision = last_saved;
    let mut read_only = false;
    let mut applied = 0;
    let mut index: BTreeMap<section::SectionKey, usize> = saved
        .iter()
        .enumerate()
        .map(|(index, record)| (record.key, index))
        .collect();
    let mut batches: Vec<&editlog::EditBatch> = recovered
        .iter()
        .filter(|batch| batch.revision > last_saved)
        .collect();
    batches.sort_by_key(|batch| batch.revision);
    'replay: for batch in batches {
        let mut wanted = Vec::with_capacity(batch.edits.len());
        for record in &batch.edits {
            let (Some(old), Some(new)) = (pack.id(&record.old), pack.id(&record.new)) else {
                read_only = true;
                break 'replay;
            };
            let cell = IVec3::new(record.x, record.y, record.z);
            let key = section_key(cell);
            let slot = if let Some(&slot) = index.get(&key) {
                slot
            } else {
                // The journal can be one write ahead of the section store. A
                // crash there leaves a frame with no saved record; generate the
                // section so the frame still lands and the header can advance.
                let Some(terrain) = terrain else {
                    break 'replay;
                };
                let section_count = i32::try_from(description::SECTIONS_PER_COLUMN).unwrap_or(12);
                let mut stack = Vec::with_capacity(COLUMN_SECTIONS);
                for sy in 0..section_count {
                    match terrain.section(key.column[0], sy, key.column[1]) {
                        Ok(section) => stack.push(section),
                        Err(error) => {
                            warn!(%error, "recovered section could not be regenerated");
                            break 'replay;
                        }
                    }
                }
                let Some(section) = stack.get(usize::from(key.section)) else {
                    break 'replay;
                };
                let record = section::SavedSection {
                    key,
                    edited: false,
                    baseline_hash: section::baseline_hash(section),
                    section: section.clone(),
                    unknown_names: Vec::new(),
                };
                saved.push(record);
                let slot = saved.len() - 1;
                index.insert(key, slot);
                slot
            };
            wanted.push((slot, key, cell, old, new));
        }
        if wanted.is_empty() {
            continue;
        }
        let mut touched = BTreeSet::new();
        for (slot, key, cell, old, new) in &wanted {
            let size = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
            let x = u32::try_from(cell.x.rem_euclid(size)).unwrap_or(0);
            let y = u32::try_from(cell.y.rem_euclid(size)).unwrap_or(0);
            let z = u32::try_from(cell.z.rem_euclid(size)).unwrap_or(0);
            let section = &mut saved[*slot];
            let current = section.section.get(x, y, z);
            if current == *old {
                section.section.set(x, y, z, *new);
                section.edited = true;
                touched.insert(*key);
                applied += 1;
            } else if current != *new {
                warn!(
                    x = cell.x,
                    y = cell.y,
                    z = cell.z,
                    "journal edit conflicts with the saved section; keeping the section"
                );
            }
        }
        if let Some(save) = save {
            // Queue the whole frame's sections with no revision advance, then
            // one more write advances the header: the trim happens only after
            // every section the frame touches is on disk.
            let keys: Vec<section::SectionKey> = wanted.iter().map(|edit| edit.1).collect();
            for (position, key) in keys.iter().enumerate() {
                let Some(&slot) = index.get(key) else {
                    continue;
                };
                let record = &saved[slot];
                let through = if position + 1 == keys.len() {
                    batch.revision
                } else {
                    0
                };
                if let Err(error) = save.save_section(
                    *key,
                    record.baseline_hash,
                    record.edited,
                    record.section.clone(),
                    through,
                ) {
                    warn!(%error, "replayed terrain section could not be saved");
                }
            }
        }
        revision = revision.max(batch.revision);
    }
    ReplayOutcome {
        sections: saved.len(),
        saved,
        revision,
        read_only,
        applied,
    }
}

/// Builds a `(2 * radius + 1)`-column patch of [`pipeline::Terrain`] output and
/// meshes it with [`mesher2`], ready for the renderer's packed-quad path.
///
/// Every column generates all [`description::SECTIONS_PER_COLUMN`] sections
/// (`F1.5`), so the patch is the full world height. AO reads the 26 neighbours
/// (`M1.2`) from the same static region; outside it, `None` keeps the `I2.3`
/// world-edge mirror rule.
///
/// Lighting comes from [`light2`]: one job per column with the 8 horizontal
/// neighbours, so caves, overhangs and water shade correctly (`L1.2`, `L2.4`).
///
/// Block coordinates are exact in `f32` up to `2^24`, so the spawn conversion
/// below is lossless for any world this engine generates.
#[cfg(test)]
#[allow(clippy::cast_precision_loss)]
pub(crate) fn build_terrain_demo(
    seed: u64,
    column: IVec2,
    radius: i32,
) -> Result<TerrainDemo, error::TerrainError> {
    build_terrain_demo_saved(seed, column, radius, &[], &[], 0).map(|(demo, _)| demo)
}

/// What a load had to replay from the journal.
#[cfg(test)]
pub(crate) struct DemoReplay {
    /// Sections the replay changed, to be written before the header advances.
    pub(crate) keys: Vec<section::SectionKey>,
    /// Greatest journal revision now reflected in the patch.
    pub(crate) revision: u64,
}

/// [`build_terrain_demo`], with saved sections and recovered journal frames.
///
/// A saved section is accepted only when its `baseline_hash` matches the
/// generated section's, so a record from another generator never replaces this
/// one. Recovered frames newer than `last_saved` are replayed in revision order:
/// a record applies when the block still holds its `old` name, is skipped when
/// it already holds `new`, and leaves the saved section alone on a conflict.
/// Unknown names make the world read-only (`A16`).
#[cfg(test)]
#[allow(clippy::cast_precision_loss, clippy::too_many_lines)]
pub(crate) fn build_terrain_demo_saved(
    seed: u64,
    column: IVec2,
    radius: i32,
    saved: &[section::SavedSection],
    recovered: &[editlog::EditBatch],
    last_saved: u64,
) -> Result<(TerrainDemo, DemoReplay), error::TerrainError> {
    build_terrain_demo_extent(
        seed,
        column,
        column.x - radius,
        column.x + radius,
        column.y - radius,
        column.y + radius,
        saved,
        recovered,
        last_saved,
    )
}

/// [`build_terrain_demo_saved`] over an explicit inclusive column rectangle.
///
/// Test-only: the live demo streams a ring instead of building a rectangle.
#[cfg(test)]
#[allow(clippy::cast_precision_loss, clippy::too_many_lines)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_terrain_demo_extent(
    seed: u64,
    column: IVec2,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    saved: &[section::SavedSection],
    recovered: &[editlog::EditBatch],
    last_saved: u64,
) -> Result<(TerrainDemo, DemoReplay), error::TerrainError> {
    let world_hydro = pipeline::Terrain::new(seed)?.solve_world_hydro()?;
    let terrain =
        pipeline::Terrain::new(seed)?.with_world_hydro(std::sync::Arc::clone(&world_hydro));
    let materials = demo_materials();
    let pack = content::v1_default();

    let mut columns = demo_columns_extent(seed, min_x, max_x, min_z, max_z, &world_hydro)?;
    let mut baselines = BTreeMap::new();
    let mut read_only = false;
    for record in saved {
        let key = record.key;
        let generated = columns
            .get(&(key.column[0], key.column[1]))
            .and_then(|stack| stack.get(usize::from(key.section)))
            .map_or([0; 32], section::baseline_hash);
        if generated != record.baseline_hash {
            warn!(
                column = ?key.column,
                section = key.section,
                "saved terrain section does not match this generator; ignored"
            );
            continue;
        }
        if !record.unknown_names.is_empty() {
            read_only = true;
        }
        let Some(stack) = columns.get_mut(&(key.column[0], key.column[1])) else {
            continue;
        };
        let Some(slot) = stack.get_mut(usize::from(key.section)) else {
            continue;
        };
        *slot = record.section.clone();
        baselines.insert(key, record.baseline_hash);
    }

    let mut replay_keys = BTreeSet::new();
    let mut replay_revision = last_saved;
    let mut batches: Vec<&editlog::EditBatch> = recovered
        .iter()
        .filter(|batch| batch.revision > last_saved)
        .collect();
    batches.sort_by_key(|batch| batch.revision);
    'replay: for batch in batches {
        let mut changes = Vec::new();
        for record in &batch.edits {
            let (Some(old), Some(new)) = (pack.id(&record.old), pack.id(&record.new)) else {
                read_only = true;
                break 'replay;
            };
            let cell = IVec3::new(record.x, record.y, record.z);
            let key = section_key(cell);
            if columns
                .get(&(key.column[0], key.column[1]))
                .and_then(|stack| stack.get(usize::from(key.section)))
                .is_none()
            {
                break 'replay;
            }
            let generated = terrain
                .section(key.column[0], i32::from(key.section), key.column[1])
                .ok()
                .map_or([0; 32], |section| section::baseline_hash(&section));
            baselines.entry(key).or_insert(generated);
            changes.push((key, cell, old, new));
        }

        for (key, cell, old, new) in changes {
            let Some(stack) = columns.get_mut(&(key.column[0], key.column[1])) else {
                continue;
            };
            let Some(section) = stack.get_mut(usize::from(key.section)) else {
                continue;
            };
            let size = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
            let x = u32::try_from(cell.x.rem_euclid(size)).unwrap_or(0);
            let y = u32::try_from(cell.y.rem_euclid(size)).unwrap_or(0);
            let z = u32::try_from(cell.z.rem_euclid(size)).unwrap_or(0);
            let current = section.get(x, y, z);
            if current == old {
                section.set(x, y, z, new);
                replay_keys.insert(key);
            } else if current != new {
                warn!(
                    x = cell.x,
                    y = cell.y,
                    z = cell.z,
                    "journal edit conflicts with the saved section; keeping the section"
                );
            }
        }
        replay_revision = replay_revision.max(batch.revision);
    }

    // `L1.1`/`L2.4`: light from the content pack, one job per column.
    let mut demo = TerrainDemo::empty(
        seed,
        column,
        residency::DEMO_VIEW_RADIUS,
        std::sync::Arc::clone(&world_hydro),
    );
    demo.materials = materials;
    demo.pack = pack;
    demo.solid = solid_table(&demo.pack);
    demo.air = demo.pack.id("core:air").unwrap_or(0);
    demo.baselines = baselines;
    demo.revision = replay_revision;
    demo.read_only = read_only;
    demo.columns = columns
        .into_iter()
        .map(|(key, stack)| (key, std::sync::Arc::new(stack)))
        .collect();
    demo.light = demo_column_light(&demo.columns);
    demo.fine = demo_fine_all(&demo.columns, &demo.light, &demo.materials);
    let sets = demo_lod_sets(
        &demo.columns,
        &demo.materials,
        &demo.pack,
        (column.x, column.y),
        demo.view_radius,
        true,
    );
    demo.coarse = sets.coarse;
    demo.coarse.extend(sets.partial_sections);
    demo.coarse.sort_by_key(|section| section.origin);
    demo.partial_columns = sets.partial;
    demo.far = sets.far;
    demo.settle_spawn();
    demo.select_lod((column.x, column.y));

    debug_assert!(demo.quads > 0, "the spawn patch emits no geometry");
    Ok((
        demo,
        DemoReplay {
            keys: replay_keys.into_iter().collect(),
            revision: replay_revision,
        },
    ))
}

/// Generates every column in an inclusive rectangle.
///
/// Test-only: the live demo generates one column per worker job.
#[cfg(test)]
#[allow(clippy::too_many_lines)]
pub(crate) fn demo_columns_extent(
    seed: u64,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    world_hydro: &std::sync::Arc<crate::hydro::WorldHydro>,
) -> Result<BTreeMap<(i32, i32), [section::Section; COLUMN_SECTIONS]>, error::TerrainError> {
    let section_count = i32::try_from(description::SECTIONS_PER_COLUMN).unwrap_or(12);
    let mut keys = Vec::new();
    for sz in min_z..=max_z {
        for sx in min_x..=max_x {
            keys.push((sx, sz));
        }
    }
    if keys.is_empty() {
        return Ok(BTreeMap::new());
    }
    let workers = worker_count().min(keys.len());
    let chunk = keys.len().div_ceil(workers);
    let mut columns = BTreeMap::new();
    std::thread::scope(|scope| -> Result<(), error::TerrainError> {
        let handles: Vec<_> = keys
            .chunks(chunk)
            .map(|chunk| {
                let world_hydro = std::sync::Arc::clone(world_hydro);
                scope.spawn(move || -> Result<Vec<_>, error::TerrainError> {
                    let terrain = pipeline::Terrain::new(seed)?.with_world_hydro(world_hydro);
                    let mut out = Vec::with_capacity(chunk.len());
                    for &(sx, sz) in chunk {
                        let mut stack = Vec::with_capacity(COLUMN_SECTIONS);
                        for sy in 0..section_count {
                            stack.push(terrain.section(sx, sy, sz)?);
                        }
                        let Ok(stack) = <[section::Section; COLUMN_SECTIONS]>::try_from(stack)
                        else {
                            return Err(error::TerrainError::new(
                                error::Stage::Height,
                                error::ErrorKind::InvariantViolated {
                                    check: "column_section_count",
                                    coord: None,
                                },
                            ));
                        };
                        out.push(((sx, sz), stack));
                    }
                    Ok(out)
                })
            })
            .collect();
        for handle in handles {
            let batch = handle.join().expect("a column job panicked")?;
            for (key, stack) in batch {
                columns.insert(key, stack);
            }
        }
        Ok(())
    })?;
    Ok(columns)
}

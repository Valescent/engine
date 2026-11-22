//! The column ring: a fixed worker pool, generation and light/mesh jobs, the
//! coarse LOD job, and the in-flight bookkeeping around them.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::{debug, warn};

use crate::content;
use crate::description;
use crate::error;
use crate::light2;
use crate::mesher2;
use crate::pipeline;
use crate::residency;
use crate::section;

use super::lod::demo_lod_sets;
use super::patch::mesh_bytes;
use super::{
    COLUMN_SECTIONS, DemoPatch, DemoSection, PartialBuild, TerrainDemo, demo_fine_column,
    demo_light_table, demo_section,
};

/// A column index to its generated sections. Shared with build jobs by `Arc`.
pub(crate) type ColumnMap = BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>;

/// Tables every ring job needs, cloned by `Arc`.
#[derive(Clone)]
pub(crate) struct RingAssets {
    pub(crate) materials: Arc<mesher2::MaterialProps>,
    pub(crate) light_table: Arc<light2::LightTable>,
    pub(crate) pack: Arc<content::ContentPack>,
}

impl RingAssets {
    pub(crate) fn from_demo(demo: &TerrainDemo) -> Self {
        Self {
            materials: Arc::new(demo.materials.clone()),
            light_table: Arc::new(demo_light_table()),
            pack: Arc::new(demo.pack.clone()),
        }
    }
}

/// One background column generation job.
pub(crate) struct GenerateJob {
    pub(crate) column: (i32, i32),
    /// Saved sections for this column, already replayed.
    pub(crate) saved: Vec<section::SavedSection>,
}

/// One background light/mesh job for a generated column.
pub(crate) struct BuildJob {
    pub(crate) centre: (i32, i32),
    /// Recompute light for the centre and every present neighbour.
    pub(crate) full: bool,
    /// Mesh the centre column at LOD0.
    pub(crate) detailed: bool,
    /// Half-width of the column window this job reads: `2` for a full light
    /// pass (centre and neighbour repairs), `1` for a mesh-only upgrade.
    pub(crate) margin: i32,
    /// Columns within the build window, borrowed by `Arc`.
    pub(crate) columns: ColumnMap,
    /// Light the caller already had, for the section-diff test.
    pub(crate) previous_light: BTreeMap<(i32, i32), light2::ColumnLight>,
    /// Columns that currently have a fine mesh (or want one).
    pub(crate) fine_columns: BTreeSet<(i32, i32)>,
    pub(crate) materials: Arc<mesher2::MaterialProps>,
    pub(crate) light_table: Arc<light2::LightTable>,
}

/// One whole-ring coarse LOD rebuild.
pub(crate) struct CoarseJob {
    pub(crate) columns: ColumnMap,
    /// Player column the LOD1/LOD2 split was built for.
    pub(crate) player: (i32, i32),
    /// Chebyshev radius of the visible ring, for clipped rim regions.
    pub(crate) radius: i32,
    /// Rebuild clipped LOD1 sections in this pass.
    pub(crate) partial: bool,
    pub(crate) materials: Arc<mesher2::MaterialProps>,
    pub(crate) pack: Arc<content::ContentPack>,
}

/// A finished generated column.
pub(crate) struct GeneratedColumn {
    pub(crate) column: (i32, i32),
    pub(crate) stack: [section::Section; COLUMN_SECTIONS],
    pub(crate) applied: Vec<(section::SectionKey, [u8; 32])>,
    pub(crate) read_only: bool,
    /// Wall time spent generating this column, for the fill breakdown.
    pub(crate) generate_ms: f64,
}

/// A finished light/mesh pass.
pub(crate) struct BuiltColumn {
    pub(crate) centre: (i32, i32),
    /// Half-width of the column window the job read, for the stale test.
    pub(crate) margin: i32,
    pub(crate) snapshot: ColumnMap,
    pub(crate) light: BTreeMap<(i32, i32), light2::ColumnLight>,
    /// Whole-column replacements (the centre when it was meshed).
    pub(crate) replace: BTreeMap<(i32, i32), Vec<DemoSection>>,
    /// Per-section replacements for neighbours whose border changed.
    pub(crate) patches: BTreeMap<(i32, i32), Vec<DemoPatch>>,
    pub(crate) light_ms: f64,
    pub(crate) mesh_ms: f64,
}

/// A finished coarse rebuild.
pub(crate) struct CoarseBuilt {
    pub(crate) snapshot: ColumnMap,
    pub(crate) player: (i32, i32),
    /// LOD1 sections for the middle band.
    pub(crate) coarse: Vec<DemoSection>,
    /// Clipped LOD1 sections and masks, present only when this pass rebuilt them.
    pub(crate) partial: Option<PartialBuild>,
    /// LOD2 sections for the far band.
    pub(crate) far: Vec<DemoSection>,
}

pub(crate) enum PoolJob {
    Generate(GenerateJob),
    Build(BuildJob),
    Coarse(CoarseJob),
}

pub(crate) enum PoolResult {
    Generated(
        (i32, i32),
        Box<Result<GeneratedColumn, error::TerrainError>>,
    ),
    Built(BuiltColumn),
    Coarse(CoarseBuilt),
}

/// A fixed fan-out of workers, one `Terrain` each (`F1.5`, `A7.1`).
pub(crate) struct ColumnPool {
    pub(crate) jobs: std::sync::mpsc::Sender<PoolJob>,
    pub(crate) results: std::sync::mpsc::Receiver<PoolResult>,
}

impl ColumnPool {
    pub(crate) fn new(
        seed: u64,
        world_hydro: &Arc<crate::hydro::WorldHydro>,
        workers: usize,
    ) -> Self {
        let (job_sender, job_receiver) = std::sync::mpsc::channel();
        let (result_sender, result_receiver) = std::sync::mpsc::channel();
        let job_receiver = Arc::new(std::sync::Mutex::new(job_receiver));
        for index in 0..workers.max(1) {
            let job_receiver = Arc::clone(&job_receiver);
            let result_sender = result_sender.clone();
            let world_hydro = Arc::clone(world_hydro);
            let _ = std::thread::Builder::new()
                .name(format!("terrain-column-{index}"))
                .spawn(move || {
                    let terrain = pipeline::Terrain::new(seed)
                        .ok()
                        .map(|terrain| terrain.with_world_hydro(world_hydro));
                    loop {
                        let Ok(job) = job_receiver.lock().expect("pool lock").recv() else {
                            break;
                        };
                        let result = match job {
                            PoolJob::Generate(job) => {
                                let column = job.column;
                                PoolResult::Generated(
                                    column,
                                    Box::new(generate_column(terrain.as_ref(), &job)),
                                )
                            }
                            PoolJob::Build(job) => PoolResult::Built(build_column(job)),
                            PoolJob::Coarse(job) => PoolResult::Coarse(build_coarse(job)),
                        };
                        if result_sender.send(result).is_err() {
                            break;
                        }
                    }
                });
        }
        Self {
            jobs: job_sender,
            results: result_receiver,
        }
    }

    pub(crate) fn submit(&self, job: PoolJob) -> bool {
        self.jobs.send(job).is_ok()
    }

    pub(crate) fn try_recv(&self) -> Option<PoolResult> {
        self.results.try_recv().ok()
    }
}

/// Generates one column and layers saved sections over the generated baseline.
pub(crate) fn generate_column(
    terrain: Option<&pipeline::Terrain>,
    job: &GenerateJob,
) -> Result<GeneratedColumn, error::TerrainError> {
    let started = Instant::now();
    let Some(terrain) = terrain else {
        return Err(error::TerrainError::new(
            error::Stage::Height,
            error::ErrorKind::InvariantViolated {
                check: "worker_terrain",
                coord: None,
            },
        ));
    };
    let section_count = i32::try_from(description::SECTIONS_PER_COLUMN).unwrap_or(12);
    let mut stack = Vec::with_capacity(COLUMN_SECTIONS);
    for sy in 0..section_count {
        stack.push(terrain.section(job.column.0, sy, job.column.1)?);
    }
    let Ok(mut stack) = <[section::Section; COLUMN_SECTIONS]>::try_from(stack) else {
        return Err(error::TerrainError::new(
            error::Stage::Height,
            error::ErrorKind::InvariantViolated {
                check: "column_section_count",
                coord: None,
            },
        ));
    };
    let mut applied = Vec::new();
    let mut read_only = false;
    for record in &job.saved {
        let key = record.key;
        if key.column != [job.column.0, job.column.1] {
            continue;
        }
        let Some(slot) = stack.get_mut(usize::from(key.section)) else {
            continue;
        };
        if section::baseline_hash(slot) != record.baseline_hash {
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
        *slot = record.section.clone();
        applied.push((key, record.baseline_hash));
    }
    Ok(GeneratedColumn {
        column: job.column,
        stack,
        applied,
        read_only,
        generate_ms: started.elapsed().as_secs_f64() * 1000.0,
    })
}

/// The eight horizontal neighbours, in `ColumnView` order.
pub(crate) const COLUMN_NEIGHBOURS: [(i32, i32); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];

/// Recomputes light and meshes the sections a new column can change.
pub(crate) fn build_column(job: BuildJob) -> BuiltColumn {
    let started = Instant::now();
    let mut light = job.previous_light.clone();
    let centre = job.centre;
    let mut targets = vec![centre];
    if job.full {
        for (dx, dz) in COLUMN_NEIGHBOURS {
            let column = (centre.0 + dx, centre.1 + dz);
            if job.columns.contains_key(&column) {
                targets.push(column);
            }
        }
        for &target in &targets {
            // A neighbour was already lit without this column, so its own
            // blocks and sky seed are unchanged: only the band its new border
            // can reach has to be repaired. The centre (and any neighbour
            // without previous light) still needs a full pass.
            if target != centre
                && let Some(previous) = job.previous_light.get(&target)
            {
                light.insert(
                    target,
                    relight_border(previous, &job.columns, target, centre, &job.light_table),
                );
                continue;
            }
            let stack = &job.columns[&target];
            let mut borders: [Option<&[section::Section; COLUMN_SECTIONS]>; 8] = [None; 8];
            for (slot, (dx, dz)) in COLUMN_NEIGHBOURS.into_iter().enumerate() {
                borders[slot] = job
                    .columns
                    .get(&(target.0 + dx, target.1 + dz))
                    .map(|stack| &**stack);
            }
            let blocks = light2::ColumnBlocks::new((**stack).clone(), &borders);
            light.insert(target, light2::compute_column(&blocks, &job.light_table));
        }
    }
    let light_ms = started.elapsed().as_secs_f64() * 1000.0;

    let mesh_started = Instant::now();
    let mut replace = BTreeMap::new();
    if job.detailed {
        let sections = demo_fine_column(&job.columns, &light, &job.materials, centre.0, centre.1);
        replace.insert(centre, sections);
    }
    let mut patches: BTreeMap<(i32, i32), Vec<DemoPatch>> = BTreeMap::new();
    if job.full {
        for &neighbour in &targets[1..] {
            if !job.fine_columns.contains(&neighbour) {
                continue;
            }
            let before = job.previous_light.get(&neighbour);
            let after = light.get(&neighbour);
            let delta = (centre.0 - neighbour.0, centre.1 - neighbour.1);
            let mut list = Vec::new();
            for sy in 0..COLUMN_SECTIONS {
                let light_changed = before
                    .zip(after)
                    .is_some_and(|(before, after)| before.section_differs(after, sy));
                let geometry_changed = border_section_changed(
                    &job.columns[&neighbour],
                    &job.columns[&centre],
                    delta,
                    sy,
                );
                if !light_changed && !geometry_changed {
                    continue;
                }
                let size = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
                let origin = [
                    neighbour.0 * size,
                    i32::try_from(sy).unwrap_or(0) * size,
                    neighbour.1 * size,
                ];
                let mesh = demo_section(
                    &job.columns,
                    &light,
                    &job.materials,
                    neighbour.0,
                    neighbour.1,
                    sy,
                )
                .map(|section| section.mesh);
                list.push(DemoPatch { origin, mesh });
            }
            if !list.is_empty() {
                patches.insert(neighbour, list);
            }
        }
    }
    let mesh_ms = mesh_started.elapsed().as_secs_f64() * 1000.0;

    BuiltColumn {
        centre,
        margin: job.margin,
        snapshot: job.columns,
        light,
        replace,
        patches,
        light_ms,
        mesh_ms,
    }
}

/// Relights only the part of `target` that the new `centre` column can change.
///
/// `previous` was computed with `centre` absent, so the missing border mirrored
/// `target`'s own edge. Once the column exists, only cells whose border blocks
/// have different light properties can move light. The repair starts at `y = 0`
/// because skylight that arrives at the border can descend losslessly through a
/// shaft below it; it stops [`light2::RELIGHT_RADIUS`] above the highest
/// differing cell, which is as far as lateral light can climb.
pub(crate) fn relight_border(
    previous: &light2::ColumnLight,
    columns: &ColumnMap,
    target: (i32, i32),
    centre: (i32, i32),
    table: &light2::LightTable,
) -> light2::ColumnLight {
    let Some(high) = border_light_top(
        &columns[&target],
        &columns[&centre],
        (centre.0 - target.0, centre.1 - target.1),
        table,
    ) else {
        return previous.clone();
    };
    let mut borders: [Option<&[section::Section; COLUMN_SECTIONS]>; 8] = [None; 8];
    for (slot, (dx, dz)) in COLUMN_NEIGHBOURS.into_iter().enumerate() {
        borders[slot] = columns
            .get(&(target.0 + dx, target.1 + dz))
            .map(|stack| &**stack);
    }
    let view = light2::ColumnView::new(&columns[&target], borders);
    let mut light = previous.clone();
    light.relight_span_view(&view, table, 0, high + light2::RELIGHT_RADIUS);
    light
}

/// The highest `y` where a new `centre` column changes `neighbour`'s border.
///
/// The neighbour was lit with the centre missing, which mirrors the neighbour's
/// own edge across that border. A cell only matters when the real border block
/// has different light properties or emission, so ids that light identically
/// (stone, dirt, ores, ...) do not widen the repair.
pub(crate) fn border_light_top(
    neighbour: &[section::Section; COLUMN_SECTIONS],
    centre: &[section::Section; COLUMN_SECTIONS],
    delta: (i32, i32),
    table: &light2::LightTable,
) -> Option<i32> {
    let diagonal = delta.0 != 0 && delta.1 != 0;
    let (neighbour_x, centre_x) = if delta.0 > 0 { (31, 0) } else { (0, 31) };
    let (neighbour_z, centre_z) = if delta.1 > 0 { (31, 0) } else { (0, 31) };
    let mut high = None;
    for (section_index, (neighbour_section, centre_section)) in
        neighbour.iter().zip(centre.iter()).enumerate()
    {
        let base = i32::try_from(section_index * 32).unwrap_or(0);
        for local_y in 0..32_u32 {
            let y = base + i32::try_from(local_y).unwrap_or(0);
            let differs = if diagonal {
                light_cell_differs(
                    neighbour_section.get(neighbour_x, local_y, neighbour_z),
                    centre_section.get(centre_x, local_y, centre_z),
                    table,
                )
            } else if delta.0 != 0 {
                (0..32_u32).any(|u| {
                    light_cell_differs(
                        neighbour_section.get(neighbour_x, local_y, u),
                        centre_section.get(centre_x, local_y, u),
                        table,
                    )
                })
            } else {
                (0..32_u32).any(|u| {
                    light_cell_differs(
                        neighbour_section.get(u, local_y, neighbour_z),
                        centre_section.get(u, local_y, centre_z),
                        table,
                    )
                })
            };
            if differs {
                high = Some(y);
            }
        }
    }
    high
}

/// Whether two block ids can affect light differently.
pub(crate) fn light_cell_differs(
    a: section::BlockId,
    b: section::BlockId,
    table: &light2::LightTable,
) -> bool {
    table.props(a) != table.props(b) || table.emission(a) != table.emission(b)
}

/// Builds the coarse LOD sets for the whole resident ring.
pub(crate) fn build_coarse(job: CoarseJob) -> CoarseBuilt {
    let sets = demo_lod_sets(
        &job.columns,
        &job.materials,
        &job.pack,
        job.player,
        job.radius,
        job.partial,
    );
    let rim_x = job.player.0 + job.radius;
    let rim_z = job.player.1 + job.radius;
    let on_rim =
        |section: &DemoSection| section.origin[0] / 32 == rim_x || section.origin[2] / 32 == rim_z;
    let partial_sections = sets.partial_sections.len();
    let rim_sections = sets
        .partial_sections
        .iter()
        .filter(|section| on_rim(section))
        .count();
    let partial_quads: usize = sets
        .partial_sections
        .iter()
        .map(|section| mesher2::quad_count(&section.mesh))
        .sum();
    let rim_quads: usize = sets
        .partial_sections
        .iter()
        .filter(|section| on_rim(section))
        .map(|section| mesher2::quad_count(&section.mesh))
        .sum();
    let partial_bytes: usize = sets.partial_sections.iter().map(mesh_bytes).sum();
    let rim_bytes: usize = sets
        .partial_sections
        .iter()
        .filter(|section| on_rim(section))
        .map(mesh_bytes)
        .sum();
    debug!(
        player = ?job.player,
        coarse = sets.coarse.len(),
        far = sets.far.len(),
        columns = job.columns.len(),
        partial = sets.partial.len(),
        partial_sections,
        rim_sections,
        boundary_sections = partial_sections - rim_sections,
        partial_quads,
        rim_quads,
        partial_bytes,
        rim_bytes,
        "terrain coarse built"
    );
    CoarseBuilt {
        snapshot: job.columns,
        player: job.player,
        coarse: sets.coarse,
        partial: job.partial.then_some((sets.partial_sections, sets.partial)),
        far: sets.far,
    }
}

/// Whether a section's block border toward `delta` differs from `a`'s mirror
/// edge, which is what a newly generated column can change.
pub(crate) fn border_section_changed(
    a: &[section::Section; COLUMN_SECTIONS],
    b: &[section::Section; COLUMN_SECTIONS],
    delta: (i32, i32),
    sy: usize,
) -> bool {
    let Some(a_section) = a.get(sy) else {
        return false;
    };
    let Some(b_section) = b.get(sy) else {
        return false;
    };
    let base_y = i32::try_from(sy).unwrap_or(0) * 32;
    let diagonal = delta.0 != 0 && delta.1 != 0;
    for y in 0..32 {
        let world_y = u32::try_from(base_y + y).unwrap_or(0);
        if diagonal {
            let (ax, az, bx, bz) = match delta {
                (1, 1) => (31, 31, 0, 0),
                (1, -1) => (31, 0, 0, 31),
                (-1, 1) => (0, 31, 31, 0),
                _ => (0, 0, 31, 31),
            };
            if a_section.get(ax, world_y, az) != b_section.get(bx, world_y, bz) {
                return true;
            }
            continue;
        }
        for u in 0..32 {
            let (ax, az, bx, bz) = match delta {
                (1, 0) => (31, u, 0, u),
                (-1, 0) => (0, u, 31, u),
                (0, 1) => (u, 31, u, 0),
                _ => (u, 0, u, 31),
            };
            if a_section.get(ax, world_y, az) != b_section.get(bx, world_y, bz) {
                return true;
            }
        }
    }
    false
}

/// Half-width of the column window a build job reads.
///
/// A full light pass reads the centre's 5x5: it also repairs the eight
/// neighbours, whose light and meshes depend on the next ring out. A mesh-only
/// upgrade reads just the centre's 3x3.
pub(crate) fn build_margin(full: bool) -> i32 {
    if full { 2 } else { 1 }
}

/// Whether every column a build reads that the ring will generate is resident.
///
/// Columns outside the visible ring are never generated at a still camera, so a
/// missing one is the permanent world edge the mesher mirrors. Waiting for the
/// in-ring columns means the snapshot cannot change while the job runs, so a
/// column is not rebuilt repeatedly while its neighbourhood is still filling.
pub(crate) fn build_window_ready(
    columns: &ColumnMap,
    centre: (i32, i32),
    margin: i32,
    player: (i32, i32),
    radius: i32,
) -> bool {
    for dx in -margin..=margin {
        for dz in -margin..=margin {
            let key = (centre.0 + dx, centre.1 + dz);
            if residency::in_ring([key.0, key.1], [player.0, player.1], radius)
                && !columns.contains_key(&key)
            {
                return false;
            }
        }
    }
    true
}

/// Copies the columns in `centre +- margin` for a background job.
pub(crate) fn snapshot_around(columns: &ColumnMap, centre: (i32, i32), margin: i32) -> ColumnMap {
    let mut map = ColumnMap::new();
    for dx in -margin..=margin {
        for dz in -margin..=margin {
            let key = (centre.0 + dx, centre.1 + dz);
            if let Some(stack) = columns.get(&key) {
                map.insert(key, Arc::clone(stack));
            }
        }
    }
    map
}

/// Whether the `centre +- margin` window still matches a job's snapshot.
pub(crate) fn neighbourhood_intact(
    columns: &ColumnMap,
    snapshot: &ColumnMap,
    centre: (i32, i32),
    margin: i32,
) -> bool {
    for dx in -margin..=margin {
        for dz in -margin..=margin {
            let key = (centre.0 + dx, centre.1 + dz);
            match (columns.get(&key), snapshot.get(&key)) {
                (Some(current), Some(snap)) if Arc::ptr_eq(current, snap) => {}
                (None, None) => {}
                _ => return false,
            }
        }
    }
    true
}

/// Whether every column a coarse snapshot read is still the same `Arc`.
///
/// New columns are allowed: the coarse set is disposable and the next job picks
/// them up. A removed or edited column invalidates the result.
pub(crate) fn coarse_snapshot_intact(columns: &ColumnMap, snapshot: &ColumnMap) -> bool {
    snapshot
        .iter()
        .all(|(key, stack)| columns.get(key).is_some_and(|now| Arc::ptr_eq(now, stack)))
}

/// Streaming state for the column ring.
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct RingStream {
    pub(crate) pool: ColumnPool,
    /// Every column with a generation or build job running.
    pub(crate) in_flight: BTreeSet<(i32, i32)>,
    /// Saved sections grouped by the column generation will need.
    pub(crate) saved: BTreeMap<(i32, i32), Vec<section::SavedSection>>,
    pub(crate) assets: RingAssets,
    /// Coarse LOD needs a rebuild after a column or edit change.
    pub(crate) coarse_dirty: bool,
    /// A coarse job is running.
    pub(crate) coarse_busy: bool,
    pub(crate) last_coarse: Instant,
    /// The renderer needs a terrain submission.
    pub(crate) upload_dirty: bool,
    pub(crate) last_upload: Instant,
    /// Whether the player has been placed on the centre column.
    pub(crate) spawned: bool,
    /// Logs one line when the ring first fills.
    pub(crate) announced_full: bool,
}

impl RingStream {
    pub(crate) fn new(
        demo: &TerrainDemo,
        saved: Vec<section::SavedSection>,
        workers: usize,
    ) -> Self {
        let mut grouped: BTreeMap<(i32, i32), Vec<section::SavedSection>> = BTreeMap::new();
        for record in saved {
            grouped
                .entry((record.key.column[0], record.key.column[1]))
                .or_default()
                .push(record);
        }
        let now = Instant::now();
        Self {
            pool: ColumnPool::new(demo.seed, &demo.world_hydro, workers),
            in_flight: BTreeSet::new(),
            saved: grouped,
            assets: RingAssets::from_demo(demo),
            coarse_dirty: true,
            coarse_busy: false,
            last_coarse: now.checked_sub(Duration::from_secs(1)).unwrap_or(now),
            upload_dirty: true,
            last_upload: now.checked_sub(Duration::from_secs(1)).unwrap_or(now),
            spawned: false,
            announced_full: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;

    use crate::light2;
    use crate::pipeline;
    use crate::section;

    use super::super::content::demo_materials;
    use super::super::patch::{demo_column_light, demo_fine_column};
    use super::super::persist::demo_columns_extent;

    use super::*;

    #[test]
    fn a_new_column_relights_and_remeshes_its_neighbour_border() {
        let stone: Arc<[section::Section; COLUMN_SECTIONS]> =
            Arc::new(std::array::from_fn(|_| section::Section::uniform(1)));
        let air: Arc<[section::Section; COLUMN_SECTIONS]> =
            Arc::new(std::array::from_fn(|_| section::Section::uniform(0)));
        let mut columns = ColumnMap::new();
        columns.insert((0, 0), stone);
        columns.insert((1, 0), air);
        let built = build_column(BuildJob {
            centre: (1, 0),
            full: true,
            detailed: true,
            margin: 2,
            columns,
            previous_light: BTreeMap::new(),
            fine_columns: BTreeSet::from([(0, 0)]),
            materials: Arc::new(demo_materials()),
            light_table: Arc::new(demo_light_table()),
        });

        assert!(
            built.light.contains_key(&(0, 0)),
            "the present neighbour is relit"
        );
        assert!(
            built
                .patches
                .get(&(0, 0))
                .is_some_and(|patches| !patches.is_empty()),
            "the neighbour's border sections are remeshed"
        );
        assert!(
            built.replace.contains_key(&(1, 0)),
            "the new column is meshed at LOD0"
        );
    }

    #[test]
    fn narrow_border_relight_matches_a_full_relight() {
        let seed = 0x5EED;
        let hydro = pipeline::Terrain::new(seed)
            .expect("the v1 description compiles")
            .solve_world_hydro()
            .expect("the hydrology solves");
        let columns: ColumnMap = demo_columns_extent(seed, -1, 1, -1, 1, &hydro)
            .expect("the tile generates")
            .into_iter()
            .map(|(key, stack)| (key, Arc::new(stack)))
            .collect();
        let table = demo_light_table();
        let materials = demo_materials();
        let centre = (0, 0);

        // Before the centre is generated, every neighbour sees its own edge
        // mirrored across that border. That is the state a build job starts
        // from, and the one the narrow repair must match.
        let mut previous = BTreeMap::new();
        for (&key, stack) in &columns {
            if key == centre {
                continue;
            }
            let mut borders: [Option<&[section::Section; COLUMN_SECTIONS]>; 8] = [None; 8];
            for (slot, (dx, dz)) in COLUMN_NEIGHBOURS.into_iter().enumerate() {
                let border = (key.0 + dx, key.1 + dz);
                if border == centre {
                    continue;
                }
                borders[slot] = columns.get(&border).map(|stack| &**stack);
            }
            let blocks = light2::ColumnBlocks::new((**stack).clone(), &borders);
            previous.insert(key, light2::compute_column(&blocks, &table));
        }

        let mut narrow = previous.clone();
        let mut full = previous.clone();
        let mut centre_borders: [Option<&[section::Section; COLUMN_SECTIONS]>; 8] = [None; 8];
        for (slot, (dx, dz)) in COLUMN_NEIGHBOURS.into_iter().enumerate() {
            centre_borders[slot] = columns
                .get(&(centre.0 + dx, centre.1 + dz))
                .map(|stack| &**stack);
        }
        let centre_blocks =
            light2::ColumnBlocks::new((**columns.get(&centre).unwrap()).clone(), &centre_borders);
        let centre_light = light2::compute_column(&centre_blocks, &table);
        narrow.insert(centre, centre_light.clone());
        full.insert(centre, centre_light);

        for (dx, dz) in COLUMN_NEIGHBOURS {
            let key = (centre.0 + dx, centre.1 + dz);
            narrow.insert(
                key,
                relight_border(&previous[&key], &columns, key, centre, &table),
            );
            let mut borders: [Option<&[section::Section; COLUMN_SECTIONS]>; 8] = [None; 8];
            for (slot, (nx, nz)) in COLUMN_NEIGHBOURS.into_iter().enumerate() {
                borders[slot] = columns.get(&(key.0 + nx, key.1 + nz)).map(|stack| &**stack);
            }
            let blocks =
                light2::ColumnBlocks::new((**columns.get(&key).unwrap()).clone(), &borders);
            full.insert(key, light2::compute_column(&blocks, &table));
        }

        for (dx, dz) in COLUMN_NEIGHBOURS {
            let key = (centre.0 + dx, centre.1 + dz);
            assert_eq!(
                narrow[&key], full[&key],
                "neighbour {dx},{dz} matches a full relight"
            );
            for sy in 0..COLUMN_SECTIONS {
                let narrow_mesh = demo_section(&columns, &narrow, &materials, key.0, key.1, sy)
                    .map(|section| section.mesh);
                let full_mesh = demo_section(&columns, &full, &materials, key.0, key.1, sy)
                    .map(|section| section.mesh);
                assert_eq!(
                    narrow_mesh, full_mesh,
                    "neighbour {dx},{dz} section {sy} mesh matches"
                );
            }
        }
    }

    #[test]
    fn a_build_waits_for_its_in_ring_dependency_window() {
        let air: Arc<[section::Section; COLUMN_SECTIONS]> =
            Arc::new(std::array::from_fn(|_| section::Section::uniform(0)));
        let mut columns = ColumnMap::new();
        for dx in -2..=2 {
            for dz in -2..=2 {
                columns.insert((dx, dz), Arc::clone(&air));
            }
        }
        assert!(build_window_ready(&columns, (0, 0), 2, (0, 0), 2));

        columns.remove(&(2, 0));
        assert!(
            !build_window_ready(&columns, (0, 0), 2, (0, 0), 2),
            "a missing in-ring column defers the build"
        );
        assert!(
            build_window_ready(&columns, (0, 0), 2, (0, 0), 1),
            "outside the visible ring the absence is the permanent edge"
        );
        assert!(
            build_window_ready(&columns, (0, 0), 1, (0, 0), 2),
            "a mesh-only upgrade does not read the outer ring"
        );
    }

    #[test]
    fn a_mesh_only_build_ignores_the_outer_ring() {
        let air: Arc<[section::Section; COLUMN_SECTIONS]> =
            Arc::new(std::array::from_fn(|_| section::Section::uniform(0)));
        let mut snapshot = ColumnMap::new();
        for dx in -1..=1 {
            for dz in -1..=1 {
                snapshot.insert((dx, dz), Arc::clone(&air));
            }
        }
        let mut current = snapshot.clone();
        current.insert((2, 0), Arc::clone(&air));

        assert!(neighbourhood_intact(&current, &snapshot, (0, 0), 1));
        assert!(!neighbourhood_intact(&current, &snapshot, (0, 0), 2));
    }

    #[test]
    fn a_complete_window_build_matches_a_full_rebuild() {
        let seed = 0x5EED;
        let hydro = pipeline::Terrain::new(seed)
            .expect("the v1 description compiles")
            .solve_world_hydro()
            .expect("the hydrology solves");
        let columns: ColumnMap = demo_columns_extent(seed, -2, 2, -2, 2, &hydro)
            .expect("the tile generates")
            .into_iter()
            .map(|(key, stack)| (key, Arc::new(stack)))
            .collect();
        let centre = (0, 0);
        assert!(
            build_window_ready(&columns, centre, 2, centre, 2),
            "the accepted fixture has its whole dependency window"
        );

        let materials = Arc::new(demo_materials());
        let light_table = Arc::new(demo_light_table());
        let fine_columns = COLUMN_NEIGHBOURS
            .into_iter()
            .map(|(dx, dz)| (centre.0 + dx, centre.1 + dz))
            .collect();
        let built = build_column(BuildJob {
            centre,
            full: true,
            detailed: true,
            margin: 2,
            columns: columns.clone(),
            previous_light: BTreeMap::new(),
            fine_columns,
            materials: Arc::clone(&materials),
            light_table,
        });
        let full_light = demo_column_light(&columns);

        for (dx, dz) in std::iter::once((0, 0)).chain(COLUMN_NEIGHBOURS) {
            let key = (centre.0 + dx, centre.1 + dz);
            assert_eq!(
                built.light[&key], full_light[&key],
                "column {dx},{dz} light matches a full rebuild"
            );
        }

        let expected = demo_fine_column(&columns, &full_light, &materials, centre.0, centre.1);
        let actual = &built.replace[&centre];
        assert_eq!(
            actual.len(),
            expected.len(),
            "the centre mesh has the same sections"
        );
        for (actual, expected) in actual.iter().zip(&expected) {
            assert_eq!(actual.origin, expected.origin);
            assert_eq!(actual.scale, expected.scale);
            assert_eq!(
                actual.mesh, expected.mesh,
                "the accepted centre mesh matches a full remesh"
            );
        }
    }
}

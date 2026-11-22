//! The v1 terrain demo runtime: resident columns, LOD bands, the column ring,
//! edits, and the save/replay wiring.
//!
//! Nothing in this tree names `winit` or `wgpu`; the app translates the demo's
//! state into renderer submissions. [`DemoRuntime`] is the one seam the app
//! holds: it owns the patch, the ring, the save worker, the repair prompt, the
//! pending rebuild, and the edit queue.
//!
//! [`build`] holds the incremental relight/remesh, [`content`] the material and
//! collision tables, and each other child module its named responsibility.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::{IVec2, IVec3, Vec3};
use tracing::{debug, error, info, warn};

use crate::error;
use crate::pipeline;
use crate::player::BlockSource;
use crate::residency;
use crate::save;
use crate::terrain_section as section;
use crate::world_save;

mod build;
mod content;
mod edit;
mod lod;
mod patch;
mod persist;
mod stream;

pub(crate) use build::{DemoEdit, DemoPatch, DemoRebuild, demo_rebuild};
pub(crate) use edit::{queue_demo_edit, section_key};
pub(crate) use lod::coarse_bands;
pub(crate) use patch::{
    DemoSection, PartialBuild, TerrainDemo, demo_fine_column, demo_light_table, demo_section,
    worker_count,
};
pub(crate) use persist::{RepairPrompt, replay_saved_sections};
pub(crate) use stream::{
    BuildJob, COLUMN_NEIGHBOURS, CoarseBuilt, CoarseJob, GenerateJob, PoolJob, PoolResult,
    RingStream, build_margin, build_window_ready, coarse_snapshot_intact, neighbourhood_intact,
    snapshot_around,
};

pub(crate) const TERRAIN_DEMO_ENV: &str = "TERRAIN_DEMO";
/// Directory for the v1 terrain demo's saved world (`A16`).
pub(crate) const TERRAIN_SAVE_DIR: &str = "world/terrain-demo";
/// Seed for the stress scene, which has no generated world seed.
pub(crate) const TERRAIN_DEMO_STRESS_SEED: u64 = 0x7E44_5EED;
/// Fine LOD radius in columns. Columns beyond this ring are drawn as LOD1.
const TERRAIN_DEMO_FINE_RADIUS: i32 = 2;
/// Fine meshes are kept one ring beyond the fine radius, so a column change
/// does not leave a gap while the new fine mesh builds.
const TERRAIN_DEMO_FINE_MARGIN: i32 = 1;
/// Chebyshev radius drawn at LOD0: the fine radius plus its transition buffer.
const TERRAIN_DEMO_FINE_CUTOFF: i32 = TERRAIN_DEMO_FINE_RADIUS + TERRAIN_DEMO_FINE_MARGIN;
/// Outer radius of the LOD1 band in columns. A complete LOD2 region is a
/// `4x4` column square entirely beyond this radius.
///
/// The radius is one short of the first 4-aligned LOD2 base (`8`), so every
/// complete LOD2 region inside the ring is clear of the fine transition. At the
/// spec cap (`render_distance_chunks = 16`) LOD2 covers the outer half of the
/// 33-column ring (columns `8..15` and `-16..-9`); at the default 12 the `8..11`
/// band still forms complete far squares; at 8 there is no complete far square
/// and the middle band falls back to LOD1.
const TERRAIN_DEMO_LOD1_RADIUS: i32 = 7;
/// Minimum interval between coarse LOD uploads, in milliseconds.
pub(crate) const TERRAIN_DEMO_UPLOAD_MS: u64 = 100;
/// Minimum interval between whole-ring coarse rebuilds, in milliseconds. LOD2
/// is rebuilt by the same job, so it shares this throttle.
pub(crate) const TERRAIN_DEMO_COARSE_MS: u64 = 250;

/// Sections per column as a `usize`, for array types (`K1.5`).
pub(crate) const COLUMN_SECTIONS: usize = crate::description::SECTIONS_PER_COLUMN as usize;

/// The demo's per-frame result for the app.
///
/// The runtime decides *what* must happen; `App` performs the renderer and
/// player calls. `demo` never stores a renderer.
#[derive(Default)]
pub(crate) struct DemoFrame {
    /// Re-submit the active section set to the renderer.
    pub(crate) upload: bool,
    /// The player was placed on the freshly measured centre column.
    pub(crate) spawn: Option<Vec3>,
    /// A background rebuild landed this frame.
    pub(crate) rebuild: Option<DemoRebuildTiming>,
}

/// Timings for the rebuild log line, which `App` emits after the upload.
pub(crate) struct DemoRebuildTiming {
    pub(crate) light_ms: f64,
    pub(crate) mesh_ms: f64,
    pub(crate) patches: usize,
}

/// The overlay counters the app logs once per second.
pub(crate) struct DemoStats {
    pub(crate) wanted: usize,
    pub(crate) resident: usize,
    pub(crate) ready: usize,
    pub(crate) fine: usize,
    pub(crate) coarse: usize,
    pub(crate) lod2: usize,
    pub(crate) quads: usize,
    pub(crate) bytes: usize,
    pub(crate) in_flight: usize,
    /// Set once, on the sample that first sees the whole ring resident.
    pub(crate) complete: bool,
}

/// The v1 demo's whole runtime: patch, ring, save worker, repair prompt, the
/// in-flight rebuild, and the edits waiting for it.
///
/// `App` holds one of these instead of six fields, and calls [`Self::tick`] once
/// per frame. The child modules keep their own boundaries; this type only
/// orchestrates them in the order the frame loop needs.
pub(crate) struct DemoRuntime {
    patch: TerrainDemo,
    ring: RingStream,
    save_worker: Option<world_save::SaveWorker>,
    repair_prompt: Option<RepairPrompt>,
    /// In-flight background rebuild of the patch.
    build: Option<std::sync::mpsc::Receiver<DemoRebuild>>,
    /// Edits waiting for the in-flight rebuild to finish, so clicks coalesce.
    edits: Vec<DemoEdit>,
}

impl BlockSource for DemoRuntime {
    fn solid_blocks_in(&self, min: IVec3, max: IVec3) -> Vec<IVec3> {
        self.patch.solid_blocks_in(min, max)
    }
}

impl DemoRuntime {
    /// Opens or creates the saved demo world and starts the column ring.
    ///
    /// `column` is the player's starting column and `view_radius` the ring
    /// radius the app derived from the render distance.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn open(
        seed: u64,
        column: IVec2,
        view_radius: i32,
    ) -> Result<Self, error::TerrainError> {
        let (saved, mut world, last_saved, recovered, repair) =
            match world_save::WorldSave::open_or_create(
                TERRAIN_SAVE_DIR,
                seed,
                "Valescent terrain demo",
            ) {
                Ok(world) => {
                    let last_saved = world.header.revision;
                    let recovered = world.recovered().to_vec();
                    match world.read_sections() {
                        Ok(saved) => (saved, Some(world), last_saved, recovered, None),
                        Err(error) => {
                            let key = if let error::ErrorKind::CorruptSection { path, .. } =
                                &error.kind
                            {
                                world.sections.key_from_path(path)
                            } else {
                                None
                            };
                            if let Some(key) = key {
                                warn!(%error, "terrain world needs repair before saves resume");
                                (
                                    Vec::new(),
                                    None,
                                    last_saved,
                                    Vec::new(),
                                    Some(RepairPrompt { world, key }),
                                )
                            } else {
                                warn!(
                                    %error,
                                    "saved terrain could not be read; running without saves"
                                );
                                (Vec::new(), None, last_saved, Vec::new(), None)
                            }
                        }
                    }
                }
                Err(error) => {
                    if matches!(error.kind, error::ErrorKind::SeedMismatch { .. }) {
                        warn!(
                            %error,
                            path = TERRAIN_SAVE_DIR,
                            "terrain demo world belongs to another seed; move that directory aside to start a new demo world"
                        );
                    } else if let error::ErrorKind::UnknownGeneratorVersion(version) = error.kind {
                        warn!(
                            %error,
                            path = TERRAIN_SAVE_DIR,
                            version,
                            ships = world_save::GENERATOR_VERSION,
                            "terrain demo world was generated by another version; move that directory aside to start a new demo world"
                        );
                    } else {
                        warn!(%error, "terrain world unavailable; running without saves");
                    }
                    (Vec::new(), None, 0, Vec::new(), None)
                }
            };
        let world_hydro = pipeline::Terrain::new(seed)?.solve_world_hydro()?;
        let replay_terrain =
            pipeline::Terrain::new(seed)?.with_world_hydro(Arc::clone(&world_hydro));
        let mut patch = TerrainDemo::empty(seed, column, view_radius, world_hydro);
        patch.revision = last_saved;
        let save_worker = match world.take() {
            Some(world) => match world_save::SaveWorker::start(world, 64) {
                Ok(worker) => Some(worker),
                Err(error) => {
                    warn!(%error, "terrain save worker unavailable; edits will not persist");
                    None
                }
            },
            None => None,
        };
        let replayed = replay_saved_sections(
            saved,
            &recovered,
            last_saved,
            &patch.pack,
            save_worker.as_ref(),
            Some(&replay_terrain),
        );
        let read_only = replayed.read_only;
        let saved_count = replayed.sections;
        let applied_count = replayed.applied;
        patch.revision = replayed.revision;
        patch.read_only = read_only;
        let ring = RingStream::new(&patch, replayed.saved, worker_count().min(12));
        info!(
            seed = patch.seed,
            column = ?patch.origin_column,
            view_radius = patch.view_radius,
            fine_radius = patch.fine_radius,
            saved_sections = saved_count,
            replayed_edits = applied_count,
            read_only = patch.read_only,
            repair_needed = repair.is_some(),
            "TERRAIN_DEMO=1: column ring streaming starts"
        );
        Ok(Self {
            patch,
            ring,
            save_worker,
            repair_prompt: repair,
            build: None,
            edits: Vec::new(),
        })
    }

    /// Advances the demo by one frame.
    ///
    /// Polls the pending rebuild, then (while focused) drains ring jobs, plans
    /// the bounded generate/build/evict batch, runs the coarse and upload
    /// throttles, and reports what the app must submit.
    pub(crate) fn tick(
        &mut self,
        now: Instant,
        player_column: (i32, i32),
        focused: bool,
    ) -> DemoFrame {
        let mut frame = DemoFrame::default();
        self.poll_rebuild(player_column, &mut frame);
        if focused {
            self.poll_ring(now, player_column, &mut frame);
        }
        frame
    }

    /// Applies one block change and queues its light/mesh rebuild.
    ///
    /// `y = 0` is indestructible (`C4.2`) and a placement needs an empty cell.
    /// Returns whether the patch changed.
    pub(crate) fn edit(&mut self, cell: IVec3, id: section::BlockId) -> bool {
        if id == self.patch.air && cell.y == 0 {
            return false;
        }
        if id != self.patch.air && !self.patch.is_empty(cell) {
            return false;
        }
        let key = section_key(cell);
        let baseline = self.patch.baseline_for(key);
        let old_name = self
            .patch
            .block_at(cell)
            .and_then(|old| self.patch.pack.name(old).map(str::to_owned));
        let new_name = self.patch.pack.name(id).map(str::to_owned);
        if !self.patch.set_block(cell, id) {
            return false;
        }
        let size = i32::try_from(crate::description::SECTION_SIZE).unwrap_or(32);
        let column = (cell.x.div_euclid(size), cell.z.div_euclid(size));
        self.patch.unsaved.insert(column);
        let queued = if let (false, Some(save), Some(old_name), Some(new_name)) = (
            self.patch.read_only,
            self.save_worker.as_ref(),
            old_name,
            new_name,
        ) {
            queue_demo_edit(
                save,
                &mut self.patch,
                cell,
                key,
                baseline,
                old_name,
                new_name,
            );
            true
        } else {
            false
        };
        if queued {
            self.patch.unsaved.remove(&column);
        }
        self.ring.coarse_dirty = true;
        self.ring.upload_dirty = true;
        self.edits.push(DemoEdit { cell });
        if self.build.is_none() {
            self.start_rebuild();
        }
        true
    }

    /// Id of air, which is the demo's "break" block.
    pub(crate) fn air(&self) -> section::BlockId {
        self.patch.air
    }

    /// Flushes the save worker. Closing the window and `Ctrl+S` both call it.
    pub(crate) fn save(&mut self) {
        if let Some(save) = self.save_worker.take() {
            match save.shutdown() {
                Ok(()) => debug!("terrain world saved"),
                Err(error) => error!(%error, "failed to save the terrain world"),
            }
        }
    }

    /// Final flush on exit.
    pub(crate) fn shutdown(&mut self) {
        self.save();
    }

    /// Applies the player's `world-repair` choice and restarts the save worker.
    ///
    /// Returns whether the reloaded sections need a renderer upload.
    pub(crate) fn repair(&mut self, action: save::RepairAction, player_column: (i32, i32)) -> bool {
        let Some(prompt) = self.repair_prompt.take() else {
            return false;
        };
        let key = prompt.key;
        let world = prompt.world;
        match world.sections.repair(key, action) {
            Ok(outcome) => info!(?outcome, "world repair applied"),
            Err(error) => {
                error!(%error, "world repair failed");
                self.repair_prompt = Some(RepairPrompt { world, key });
                return false;
            }
        }
        match world.read_sections() {
            Ok(saved) => {
                self.patch.apply_saved(&saved);
                let mut grouped: BTreeMap<(i32, i32), Vec<section::SavedSection>> = BTreeMap::new();
                for record in saved {
                    grouped
                        .entry((record.key.column[0], record.key.column[1]))
                        .or_default()
                        .push(record);
                }
                self.ring.saved = grouped;
                self.ring.coarse_dirty = true;
                self.ring.upload_dirty = true;
                match world_save::SaveWorker::start(world, 64) {
                    Ok(worker) => self.save_worker = Some(worker),
                    Err(error) => {
                        warn!(%error, "terrain save worker unavailable; edits will not persist");
                    }
                }
                self.advance_lod(player_column);
                true
            }
            Err(error) => {
                error!(%error, "saved terrain is still unreadable after repair");
                self.repair_prompt = Some(RepairPrompt { world, key });
                false
            }
        }
    }

    /// The section key the repair overlay names, if a prompt is blocking saves.
    pub(crate) fn repair_key(&self) -> Option<section::SectionKey> {
        self.repair_prompt.as_ref().map(|prompt| prompt.key)
    }

    /// Whether a player AABB at `position` overlaps only ready columns.
    pub(crate) fn is_ready(&self, position: Vec3) -> bool {
        self.patch.aabb_ready(position)
    }

    /// Whether a cell is solid for raycasts and collision.
    pub(crate) fn is_solid(&self, cell: IVec3) -> bool {
        self.patch.is_solid(cell)
    }

    /// The active LOD set the app submits, fine and coarse by distance.
    pub(crate) fn sections(&self) -> &[DemoSection] {
        &self.patch.sections
    }

    /// Overlay counters, and whether the ring just filled.
    pub(crate) fn stats(&mut self) -> DemoStats {
        let view_radius = self.patch.view_radius;
        let wanted = usize::try_from((2 * view_radius + 1) * (2 * view_radius + 1)).unwrap_or(0);
        let resident = self.patch.columns.len();
        let complete = resident >= wanted && !self.ring.announced_full;
        if complete {
            self.ring.announced_full = true;
        }
        DemoStats {
            wanted,
            resident,
            ready: self.patch.light.len(),
            fine: self.patch.fine.len(),
            coarse: self.patch.coarse.len(),
            lod2: self.patch.far.len(),
            quads: self.patch.quads,
            bytes: self.patch.footprint_bytes(),
            in_flight: self.ring.in_flight.len(),
            complete,
        }
    }

    /// Re-selects the LOD set when the player changed column.
    ///
    /// Returns whether the active set changed, so the caller can upload after
    /// the player step.
    pub(crate) fn advance_lod(&mut self, player_column: (i32, i32)) -> bool {
        if self.patch.lod_column == player_column {
            return false;
        }
        self.patch.select_lod(player_column);
        true
    }

    /// Starts a worker for every edit queued since the last rebuild.
    fn start_rebuild(&mut self) {
        if self.edits.is_empty() {
            return;
        }
        let columns = self.patch.columns.clone();
        let light = self.patch.light.clone();
        let materials = self.patch.materials.clone();
        let edits = std::mem::take(&mut self.edits);
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(demo_rebuild(&columns, &light, &materials, &edits));
        });
        self.build = Some(receiver);
    }

    /// Installs a finished rebuild if it is still current.
    fn poll_rebuild(&mut self, player_column: (i32, i32), frame: &mut DemoFrame) {
        let received = self.build.as_ref().map(std::sync::mpsc::Receiver::try_recv);
        match received {
            Some(Ok(rebuild)) => {
                let timing = DemoRebuildTiming {
                    light_ms: rebuild.light_time.as_secs_f64() * 1000.0,
                    mesh_ms: rebuild.mesh_time.as_secs_f64() * 1000.0,
                    patches: rebuild.patches.len(),
                };
                self.build = None;
                self.patch.apply_rebuild(rebuild);
                self.advance_lod(player_column);
                frame.upload = true;
                frame.rebuild = Some(timing);
                if !self.edits.is_empty() {
                    self.start_rebuild();
                }
            }
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                self.build = None;
                if !self.edits.is_empty() {
                    self.start_rebuild();
                }
            }
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) | None => {}
        }
    }

    /// Advances the column ring: applies finished jobs, then plans and starts a
    /// bounded batch of generate, build, and evict work for this frame.
    #[allow(clippy::too_many_lines)]
    fn poll_ring(&mut self, now: Instant, player_column: (i32, i32), frame: &mut DemoFrame) {
        let mut results = Vec::new();
        while results.len() < 16 {
            let Some(result) = self.ring.pool.try_recv() else {
                break;
            };
            results.push(result);
        }
        for result in results {
            self.apply_ring_result(result, player_column, frame);
        }

        let demo = &self.patch;
        let resident: BTreeSet<[i32; 2]> = demo.columns.keys().map(|&(x, z)| [x, z]).collect();
        let needs_build: BTreeSet<[i32; 2]> = resident
            .iter()
            .copied()
            .filter(|&column| {
                let ring = (column[0] - player_column.0)
                    .abs()
                    .max((column[1] - player_column.1).abs());
                let key = (column[0], column[1]);
                let lit = demo.light.contains_key(&key);
                // Wait until the columns this job reads are resident, so a
                // build is not repeatedly discarded while its neighbourhood
                // fills. A mesh-only upgrade reads one column less.
                (!lit || (ring <= demo.fine_radius && !demo.fine.contains_key(&key)))
                    && build_window_ready(
                        &demo.columns,
                        key,
                        build_margin(!lit),
                        player_column,
                        demo.view_radius,
                    )
            })
            .collect();
        let in_flight: BTreeSet<[i32; 2]> =
            self.ring.in_flight.iter().map(|&(x, z)| [x, z]).collect();
        let protected: BTreeSet<[i32; 2]> = demo.unsaved.iter().map(|&(x, z)| [x, z]).collect();
        let plan = residency::plan_ring(
            [player_column.0, player_column.1],
            demo.view_radius,
            &resident,
            &in_flight,
            &needs_build,
            &protected,
            residency::RingBudget::v1(),
        );
        let assets = self.ring.assets.clone();
        let saved = self.ring.saved.clone();
        let mut jobs = Vec::new();
        for &column in &plan.generate {
            let column = (column[0], column[1]);
            jobs.push((
                column,
                PoolJob::Generate(GenerateJob {
                    column,
                    saved: saved.get(&column).cloned().unwrap_or_default(),
                }),
            ));
        }
        for &column in &plan.build {
            let column = (column[0], column[1]);
            let full = !demo.light.contains_key(&column);
            let ring_distance = (column.0 - player_column.0)
                .abs()
                .max((column.1 - player_column.1).abs());
            let detailed = !full || ring_distance <= demo.fine_radius;
            let margin = build_margin(full);
            let mut previous_light = BTreeMap::new();
            let mut fine_columns = BTreeSet::new();
            for (dx, dz) in COLUMN_NEIGHBOURS {
                let key = (column.0 + dx, column.1 + dz);
                if let Some(light) = demo.light.get(&key) {
                    previous_light.insert(key, light.clone());
                }
                if demo.fine.contains_key(&key) {
                    fine_columns.insert(key);
                }
            }
            if let Some(light) = demo.light.get(&column) {
                previous_light.insert(column, light.clone());
            }
            if demo.fine.contains_key(&column) {
                fine_columns.insert(column);
            }
            jobs.push((
                column,
                PoolJob::Build(BuildJob {
                    centre: column,
                    full,
                    detailed,
                    margin,
                    columns: snapshot_around(&demo.columns, column, margin),
                    previous_light,
                    fine_columns,
                    materials: Arc::clone(&assets.materials),
                    light_table: Arc::clone(&assets.light_table),
                }),
            ));
        }
        let mut evicted = Vec::new();
        for &column in &plan.evict {
            evicted.push((column[0], column[1]));
        }
        for (column, job) in jobs {
            if self.ring.in_flight.contains(&column) {
                continue;
            }
            self.ring.in_flight.insert(column);
            if !self.ring.pool.submit(job) {
                self.ring.in_flight.remove(&column);
                warn!(?column, "terrain ring job could not be submitted");
            }
        }
        for &column in &evicted {
            debug!(?column, "evicting terrain column");
            self.patch.remove_column(column);
        }
        if !evicted.is_empty() {
            self.ring.coarse_dirty = true;
            self.ring.upload_dirty = true;
            self.patch.select_lod(player_column);
        }

        // Coarse LOD is disposable and rebuilt whole; keep one job in flight and
        // let it lag a frame behind the fine ring. LOD2 shares this job and
        // throttle; an edit only marks it dirty so the edit path stays cheap.
        let changed = self.patch.lod_column != player_column;
        let dirty = self.ring.coarse_dirty || self.patch.far_dirty;
        let busy = self.ring.coarse_busy;
        let elapsed = now.saturating_duration_since(self.ring.last_coarse);
        if changed {
            self.ring.coarse_dirty = true;
            self.advance_lod(player_column);
            frame.upload = true;
        }
        if (dirty || changed) && !busy && elapsed >= Duration::from_millis(TERRAIN_DEMO_COARSE_MS) {
            // Clipped sections are the costly part of a coarse pass. Reuse them
            // while their masks are unchanged; a new rim column, an eviction, or
            // an edit makes the next pass rebuild them.
            let partial = self.patch.partial_dirty
                || coarse_bands(&self.patch.columns, player_column, self.patch.view_radius).partial
                    != self.patch.partial_columns;
            let job = PoolJob::Coarse(CoarseJob {
                columns: self.patch.columns.clone(),
                player: player_column,
                radius: self.patch.view_radius,
                partial,
                materials: Arc::clone(&self.ring.assets.materials),
                pack: Arc::clone(&self.ring.assets.pack),
            });
            self.ring.coarse_busy = true;
            self.ring.last_coarse = Instant::now();
            if !self.ring.pool.submit(job) {
                self.ring.coarse_busy = false;
                warn!("terrain coarse job could not be submitted");
            }
        }
        if self.ring.upload_dirty
            && now.saturating_duration_since(self.ring.last_upload)
                >= Duration::from_millis(TERRAIN_DEMO_UPLOAD_MS)
        {
            self.ring.upload_dirty = false;
            self.ring.last_upload = now;
            self.advance_lod(player_column);
            frame.upload = true;
        }
    }

    /// Installs one finished ring job.
    #[allow(clippy::too_many_lines)]
    fn apply_ring_result(
        &mut self,
        result: PoolResult,
        player_column: (i32, i32),
        frame: &mut DemoFrame,
    ) {
        match result {
            PoolResult::Generated(column, result) => {
                self.ring.in_flight.remove(&column);
                match *result {
                    Ok(generated) => {
                        debug!(
                            column = ?generated.column,
                            generate_ms = generated.generate_ms,
                            "terrain column generated"
                        );
                        let radius = self.patch.view_radius + residency::UNLOAD_MARGIN;
                        let ring_position = (generated.column.0 - player_column.0)
                            .abs()
                            .max((generated.column.1 - player_column.1).abs());
                        if ring_position > radius {
                            debug!(?generated.column, "late column is outside the ring");
                            return;
                        }
                        for (key, baseline) in generated.applied {
                            self.patch.baselines.insert(key, baseline);
                        }
                        self.patch.read_only |= generated.read_only;
                        self.patch.insert_column(generated.column, generated.stack);
                        if generated.column
                            == (self.patch.origin_column.x, self.patch.origin_column.y)
                        {
                            self.patch.settle_spawn();
                        }
                        self.ring.coarse_dirty = true;
                        self.ring.upload_dirty = true;
                    }
                    Err(error) => {
                        warn!(%error, "terrain column generation failed");
                    }
                }
            }
            PoolResult::Built(built) => {
                self.ring.in_flight.remove(&built.centre);
                let mut spawn = None;
                if neighbourhood_intact(
                    &self.patch.columns,
                    &built.snapshot,
                    built.centre,
                    built.margin,
                ) {
                    self.patch.apply_light(built.light);
                    for (column, sections) in built.replace {
                        self.patch.fine.insert(column, sections);
                    }
                    self.patch.apply_patches(built.patches);
                    // A new or repaired fine mesh must join the active set
                    // immediately; otherwise it waits for the next coarse
                    // rebuild even though the player may be standing on it.
                    self.patch.select_lod(self.patch.lod_column);
                    if built.centre == (self.patch.origin_column.x, self.patch.origin_column.y)
                        && self.patch.spawn_ready
                    {
                        spawn = Some(self.patch.spawn);
                    }
                    debug!(
                        column = ?built.centre,
                        light_ms = built.light_ms,
                        mesh_ms = built.mesh_ms,
                        "terrain column ready"
                    );
                } else {
                    debug!(column = ?built.centre, "discarded stale terrain build");
                }
                self.ring.upload_dirty = true;
                if let Some(spawn) = spawn
                    && !self.ring.spawned
                {
                    self.ring.spawned = true;
                    frame.spawn = Some(spawn);
                }
            }
            PoolResult::Coarse(coarse) => {
                let intact = self.patch.lod_column == coarse.player
                    && coarse_snapshot_intact(&self.patch.columns, &coarse.snapshot);
                if intact {
                    let changed = self.patch.columns.len() != coarse.snapshot.len();
                    let CoarseBuilt {
                        coarse: complete,
                        partial,
                        far,
                        ..
                    } = coarse;
                    let kept: Vec<DemoSection> = if partial.is_none() {
                        self.patch
                            .coarse
                            .iter()
                            .filter(|section| {
                                self.patch
                                    .partial_columns
                                    .contains_key(&(section.origin[0] / 32, section.origin[2] / 32))
                            })
                            .cloned()
                            .collect()
                    } else {
                        Vec::new()
                    };
                    self.patch.coarse = complete;
                    self.patch.coarse.extend(kept);
                    if let Some((sections, masks)) = partial {
                        self.patch.coarse.extend(sections);
                        self.patch.partial_columns = masks;
                        self.patch.partial_dirty = false;
                    }
                    self.patch.coarse.sort_by_key(|section| section.origin);
                    self.patch.far = far;
                    self.patch.far_dirty = false;
                    self.patch.select_lod(self.patch.lod_column);
                    self.ring.coarse_busy = false;
                    self.ring.coarse_dirty = changed;
                    self.ring.upload_dirty = true;
                } else {
                    self.ring.coarse_busy = false;
                    self.ring.coarse_dirty = true;
                }
            }
        }
    }

    /// A test-only runtime over an already-built patch and ring.
    #[cfg(test)]
    pub(crate) fn from_parts(patch: TerrainDemo, ring: RingStream) -> Self {
        Self {
            patch,
            ring,
            save_worker: None,
            repair_prompt: None,
            build: None,
            edits: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use glam::{IVec2, Vec3};

    use crate::pipeline;
    use crate::residency;
    use crate::terrain_section as section;

    use super::*;

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn the_ring_caps_generation_and_gates_on_the_centre() {
        let seed = 0x5EED;
        let column = IVec2::new(2, -3);
        let hydro = pipeline::Terrain::new(seed)
            .expect("the description compiles")
            .solve_world_hydro()
            .expect("the hydrology solves");
        let demo = TerrainDemo::empty(seed, column, residency::DEMO_VIEW_RADIUS, hydro);
        let ring = RingStream::new(&demo, Vec::new(), 2);
        let mut runtime = DemoRuntime::from_parts(demo, ring);
        let position = Vec3::new(
            column.x as f32 * 32.0 + 16.5,
            64.0,
            column.y as f32 * 32.0 + 16.5,
        );

        // The centre column starts unready, so physics refuses movement.
        assert!(!runtime.is_ready(position));
        runtime.tick(Instant::now(), (column.x, column.y), true);
        let in_flight = &runtime.ring.in_flight;
        assert_eq!(in_flight.len(), 2, "two columns per frame");
        assert!(
            in_flight.contains(&(column.x, column.y)),
            "the centre column is generated first"
        );

        let deadline = Instant::now() + Duration::from_secs(30);
        while !(runtime.patch.column_ready((column.x, column.y))
            && runtime.patch.fine.contains_key(&(column.x, column.y)))
        {
            assert!(
                Instant::now() < deadline,
                "the centre column did not become ready"
            );
            runtime.tick(Instant::now(), (column.x, column.y), true);
            std::thread::sleep(Duration::from_millis(5));
        }
        let demo = &runtime.patch;
        assert!(demo.spawn_ready, "the spawn surface was measured");
        assert!(
            demo.fine.contains_key(&(column.x, column.y)),
            "the centre column was meshed at LOD0"
        );
        assert!(
            demo.sections.iter().any(|active| {
                active.scale == 1
                    && active.origin[0].div_euclid(32) == column.x
                    && active.origin[2].div_euclid(32) == column.y
            }),
            "the centre column's mesh is in the active render set"
        );
        assert!(
            runtime.is_ready(runtime.patch.spawn),
            "the centre column now accepts movement"
        );
    }

    #[test]
    fn the_ring_evicts_beyond_the_margin_but_keeps_unsaved_edits() {
        let seed = 0x5EED;
        let column = IVec2::new(0, 0);
        let hydro = pipeline::Terrain::new(seed)
            .expect("the description compiles")
            .solve_world_hydro()
            .expect("the hydrology solves");
        let mut demo = TerrainDemo::empty(seed, column, residency::DEMO_VIEW_RADIUS, hydro);
        demo.view_radius = 0;
        for key in [(0, 0), (5, 0), (6, 0)] {
            demo.insert_column(key, std::array::from_fn(|_| section::Section::uniform(0)));
        }
        demo.unsaved.insert((6, 0));
        let ring = RingStream::new(&demo, Vec::new(), 1);
        let mut runtime = DemoRuntime::from_parts(demo, ring);

        runtime.tick(Instant::now(), (0, 0), true);
        let demo = &runtime.patch;
        assert!(
            !demo.columns.contains_key(&(5, 0)),
            "a column beyond radius + 2 is evicted"
        );
        assert!(
            demo.columns.contains_key(&(6, 0)),
            "an edit whose save is not queued is never evicted"
        );
        assert!(
            demo.columns.contains_key(&(0, 0)),
            "the player's column stays resident"
        );
    }
}

//! The `winit` application: window and renderer lifecycle, the frame loop,
//! input mapping, overlay/stats, and the demo-to-renderer upload adapter.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::{IVec2, IVec3, Vec2, Vec3};
use thiserror::Error;
use tracing::{debug, error, info, warn};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::block::Block;
use crate::camera::Camera;
use crate::config::{Config, ConfigError};
use crate::content;
use crate::demo::{
    BuildJob, COLUMN_NEIGHBOURS, CoarseBuilt, CoarseJob, DemoEdit, DemoRebuild, DemoSection,
    GenerateJob, PoolJob, PoolResult, RepairPrompt, RingStream, TERRAIN_DEMO_COARSE_MS,
    TERRAIN_DEMO_UPLOAD_MS, TerrainDemo, build_margin, build_window_ready, coarse_bands,
    coarse_snapshot_intact, demo_rebuild, neighbourhood_intact, queue_demo_edit, section_key,
    snapshot_around,
};
use crate::description;
use crate::frustum::Frustum;
use crate::mesh::Lod;
use crate::mesher::{MeshKind, MeshPool, MeshQueue, Priority, collect_finished, create_pool};
use crate::metrics;
use crate::player::{Player, PlayerInput};
use crate::raycast::{RaycastHit, raycast, raycast_where};
use crate::renderer::{FrameStats, Renderer, RendererError, TerrainSection};
use crate::residency;
use crate::save;
use crate::section;
use crate::sky::Sky;
use crate::startup::{insert_generated_chunk, no_vsync_requested, release_column, save_directory};
use crate::streaming::Streaming;
use crate::voxel::build_highlight_mesh;
use crate::world::{CHUNK_COLUMN, ChunkCoord, DirtyChunks, SaveError, World};
use crate::world_save;

const WINDOW_TITLE: &str = concat!("Valescent Engine ", env!("CARGO_PKG_VERSION"));
const MAX_FRAME_DELTA: Duration = Duration::from_millis(100);
const FRAME_LOG_INTERVAL: Duration = Duration::from_secs(1);
const MAX_COLUMNS_PER_FRAME: usize = 2;
const INTERACTION_DISTANCE: f32 = 6.0;
const SPAWN_X: i32 = 32;
const SPAWN_Z: i32 = 32;
const MAX_UPLOADS_PER_FRAME: usize = 2;

#[derive(Debug, Error)]
pub enum RunError {
    #[error(transparent)]
    EventLoop(#[from] winit::error::EventLoopError),
    #[error(transparent)]
    Window(#[from] winit::error::OsError),
    #[error(transparent)]
    Renderer(#[from] RendererError),
    #[error(transparent)]
    Save(#[from] SaveError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Terrain(#[from] crate::error::TerrainError),
}

/// Uploads a built demo patch through the renderer's packed-quad path.
fn upload_terrain_demo(renderer: &mut Renderer, demo: &TerrainDemo) -> Result<(), RendererError> {
    let sections: Vec<TerrainSection<'_>> = demo
        .sections
        .iter()
        .map(|section| TerrainSection {
            origin: section.origin,
            scale: section.scale,
            opaque: &section.mesh.opaque,
            cutout: &section.mesh.cutout,
            water: &section.mesh.water,
        })
        .collect();
    renderer.submit_terrain(&sections)
}

#[allow(clippy::cast_precision_loss)]
fn spawn_position(world: &World) -> Vec3 {
    let mut highest = None;
    for (chunk, data) in world.iter_chunks() {
        let origin = chunk.world_origin();
        for (local, _) in data.iter() {
            if origin.x + local.x == SPAWN_X && origin.z + local.z == SPAWN_Z {
                let y = origin.y + local.y;
                highest = Some(highest.map_or(y, |highest: i32| highest.max(y)));
            }
        }
    }

    let feet_y = highest.map_or(3.0, |y| y as f32 + 1.5);
    Vec3::new(SPAWN_X as f32 + 0.5, feet_y, SPAWN_Z as f32 + 0.5)
}

fn can_place(world: &World, player: &Player, position: IVec3) -> bool {
    !world.is_solid(position) && !player.aabb().intersects_block(position)
}

fn place_position(hit: RaycastHit) -> Option<IVec3> {
    Some(IVec3::new(
        hit.block.x.checked_add(hit.normal.x)?,
        hit.block.y.checked_add(hit.normal.y)?,
        hit.block.z.checked_add(hit.normal.z)?,
    ))
}

/// The repair action a shortcut key selects.
fn repair_action(code: KeyCode) -> Option<save::RepairAction> {
    match code {
        KeyCode::KeyR => Some(save::RepairAction::RestoreBackup),
        KeyCode::KeyQ => Some(save::RepairAction::Quarantine),
        KeyCode::KeyS => Some(save::RepairAction::Salvage),
        _ => None,
    }
}

/// The stable content name of a hotbar block (`A5.1`).
///
/// The hotbar still selects legacy [`Block`] values; the new terrain only knows
/// names, so the mapping lives here.
fn legacy_block_name(block: Block) -> &'static str {
    match block {
        Block::Air => "core:air",
        Block::Stone => "core:stone",
        Block::Dirt => "core:dirt",
        Block::Grass => "core:grass",
        Block::Wood => "core:wood",
        Block::Leaves => "core:leaves",
        Block::Lamp => "core:lamp",
        Block::Water => "core:water",
        Block::Sand => "core:sand",
    }
}

#[allow(clippy::struct_excessive_bools)]
pub(crate) struct App {
    pub(crate) window: Option<Arc<Window>>,
    pub(crate) renderer: Option<Renderer>,
    pub(crate) mesher: Option<Arc<dyn MeshPool>>,
    pub(crate) queue: MeshQueue,
    pub(crate) config: Config,
    pub(crate) streaming: Option<Streaming>,
    pub(crate) seed: Option<u64>,
    pub(crate) terrain_demo: Option<TerrainDemo>,
    /// Background writer for the v1 terrain world, when the demo has one.
    pub(crate) terrain_save: Option<world_save::SaveWorker>,
    /// Column-ring streaming state and workers.
    pub(crate) terrain_ring: Option<RingStream>,
    /// A corrupt section blocking saves until the player chooses an action.
    pub(crate) repair_prompt: Option<RepairPrompt>,
    /// In-flight background rebuild of the patch.
    pub(crate) demo_build: Option<std::sync::mpsc::Receiver<DemoRebuild>>,
    /// Edits waiting for the in-flight rebuild to finish, so clicks coalesce.
    pub(crate) demo_edits: Vec<DemoEdit>,
    pub(crate) save_directory: Option<PathBuf>,
    pub(crate) level_work: HashSet<ChunkCoord>,
    pub(crate) level_player_chunk: ChunkCoord,
    pub(crate) stats: FrameAccumulator,
    pub(crate) stress: bool,
    pub(crate) world: World,
    pub(crate) player: Player,
    pub(crate) camera: Camera,
    pub(crate) sky: Sky,
    pub(crate) unlit: bool,
    pub(crate) selected_block: Block,
    pub(crate) error: Option<RunError>,
    pub(crate) pressed: HashSet<KeyCode>,
    pub(crate) mouse_captured: bool,
    /// Last window-relative cursor position, for the repair prompt.
    pub(crate) cursor_position: (f64, f64),
    pub(crate) modifiers: ModifiersState,
    pub(crate) jump_requested: bool,
    pub(crate) highlighted_block: Option<IVec3>,
    pub(crate) focused: bool,
    pub(crate) occluded: bool,
    pub(crate) show_overlay: bool,
    pub(crate) overlay_fps: f64,
    pub(crate) overlay_frame_ms: f64,
    pub(crate) last_frame: Instant,
    pub(crate) frame_accum: Duration,
    pub(crate) frame_count: u32,
}

/// Per-second accumulation of the v0.0.5 metrics.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FrameAccumulator {
    pub(crate) totals: FrameStats,
    pub(crate) rebuild_ms: f64,
    pub(crate) upload_ms: f64,
    pub(crate) last_rebuild_ms: f64,
    pub(crate) last_upload_ms: f64,
    pub(crate) pending_jobs: usize,
    pub(crate) generated_columns: u32,
    pub(crate) generation_ms: f64,
}

impl FrameAccumulator {
    fn reset(&mut self) {
        *self = Self::default();
    }
}

impl App {
    #[cfg(test)]
    fn new(world: World) -> Self {
        Self::with_world(world, false)
    }

    pub(crate) fn with_world(world: World, stress: bool) -> Self {
        let spawn = if stress {
            Vec3::new(16.5, 3.0, 16.5)
        } else {
            spawn_position(&world)
        };
        let player = Player::new(spawn);
        let mut camera = Camera::new(1.0);
        camera.set_position(player.eye_position());
        if stress {
            camera.set_yaw(std::f32::consts::FRAC_PI_4);
        }
        debug!(spawn = ?player.position, stress, "player spawned");

        let mut queue = MeshQueue::default();
        for (chunk, _) in world.iter_chunks() {
            queue.mark_light_built(chunk);
        }

        Self {
            window: None,
            renderer: None,
            mesher: None,
            queue,
            config: Config::default(),
            streaming: None,
            seed: None,
            terrain_demo: None,
            terrain_save: None,
            terrain_ring: None,
            repair_prompt: None,
            demo_build: None,
            demo_edits: Vec::new(),
            save_directory: save_directory(stress).map(Path::to_path_buf),
            level_work: world.iter_chunks().map(|(chunk, _)| chunk).collect(),
            level_player_chunk: ChunkCoord::from_world(spawn.floor().as_ivec3()).0,
            stats: FrameAccumulator::default(),
            stress,
            world,
            player,
            camera,
            sky: Sky::new(),
            unlit: false,
            selected_block: Block::Stone,
            error: None,
            pressed: HashSet::new(),
            mouse_captured: false,
            cursor_position: (0.0, 0.0),
            modifiers: ModifiersState::empty(),
            jump_requested: false,
            highlighted_block: None,
            focused: true,
            occluded: false,
            show_overlay: cfg!(debug_assertions),
            overlay_fps: 0.0,
            overlay_frame_ms: 0.0,
            last_frame: Instant::now(),
            frame_accum: Duration::ZERO,
            frame_count: 0,
        }
    }

    fn fail(&mut self, error: RunError, event_loop: &ActiveEventLoop) {
        error!(%error, "engine startup failed");
        self.error = Some(error);
        event_loop.exit();
    }

    /// Invalidates the edited chunks and queues urgent light work.
    ///
    /// Meshes wait for the accepted light field, then rebuild through the
    /// normal request path.
    fn invalidate_chunks(&mut self, dirty: &DirtyChunks) {
        let started = Instant::now();
        let light_dirty = self.world.light_dirty(dirty);
        self.queue.invalidate(&light_dirty);
        self.level_work.extend(light_dirty.iter().copied());

        if let Some(pool) = self.mesher.clone() {
            for chunk in &light_dirty {
                self.queue
                    .request_light(&*pool, &self.world, *chunk, Priority::Urgent);
            }
        }
        for chunk in dirty {
            if self.world.chunk(*chunk).is_none() {
                self.queue.remove(*chunk);
                if let Some(renderer) = &mut self.renderer {
                    renderer.remove_chunk(*chunk);
                }
            }
        }

        if let Some(window) = &self.window {
            window.request_redraw();
        }

        debug!(
            dirty_chunks = light_dirty.len(),
            vertices = self.renderer.as_ref().map_or(0, Renderer::total_vertices),
            rebuild_ms = started.elapsed().as_secs_f64() * 1000.0,
            "rebuilt chunks"
        );
    }

    pub(crate) fn request_initial_meshes(&mut self) {
        let Some(pool) = self.mesher.clone() else {
            return;
        };
        for (chunk, _) in self.world.iter_chunks() {
            self.queue.request(
                &*pool,
                &self.world,
                chunk,
                Lod::Lod0,
                MeshKind::Opaque,
                Priority::Urgent,
            );
            self.queue.request(
                &*pool,
                &self.world,
                chunk,
                Lod::Lod0,
                MeshKind::Water,
                Priority::Background,
            );
        }
    }

    /// Recalculates every chunk's LOD and requests any missing mesh.
    ///
    /// Only chunks with outstanding level work are visited. Generation commits,
    /// edits, and a player chunk change feed the set; a chunk stays in it until
    /// its light, opaque mesh, and water mesh are built.
    pub(crate) fn request_levels(&mut self, pool: &dyn MeshPool) {
        profiling::scope!("mesh request");
        let feet = self.player.position.floor().as_ivec3();
        let player_chunk = ChunkCoord::from_world(feet).0;
        if player_chunk != self.level_player_chunk {
            self.level_player_chunk = player_chunk;
            self.level_work
                .extend(self.world.iter_chunks().map(|(chunk, _)| chunk));
        }

        let work: Vec<ChunkCoord> = self.level_work.drain().collect();
        for chunk in work {
            if self.world.chunk(chunk).is_none() {
                continue;
            }
            let mut done = true;
            if !self.queue.light_is_current(chunk) {
                done = false;
                if !self.queue.light_pending(chunk) {
                    self.queue
                        .request_light(pool, &self.world, chunk, Priority::Background);
                }
            }
            if self.queue.light_is_current(chunk) {
                let lod = self.queue.desired_lod(chunk, player_chunk);
                if !self.queue.is_built(chunk, lod, MeshKind::Opaque) {
                    done = false;
                    if !self.queue.is_pending(chunk, lod, MeshKind::Opaque) {
                        self.queue.request(
                            pool,
                            &self.world,
                            chunk,
                            lod,
                            MeshKind::Opaque,
                            Priority::Background,
                        );
                    }
                }

                if !self.queue.is_built(chunk, Lod::Lod0, MeshKind::Water) {
                    done = false;
                    if !self.queue.is_pending(chunk, Lod::Lod0, MeshKind::Water) {
                        self.queue.request(
                            pool,
                            &self.world,
                            chunk,
                            Lod::Lod0,
                            MeshKind::Water,
                            Priority::Background,
                        );
                    }
                }
            }
            if !done {
                self.level_work.insert(chunk);
            }
        }
        self.stats.pending_jobs = self.queue.pending_len();
    }

    pub(crate) fn upload_finished_meshes(&mut self, pool: &dyn MeshPool) {
        profiling::scope!("mesh upload");
        let (results, lights, generations, rebuild) =
            collect_finished(&mut self.queue, pool, MAX_UPLOADS_PER_FRAME);
        let rebuild_ms = rebuild.as_secs_f64() * 1000.0;
        self.stats.rebuild_ms += rebuild_ms;
        self.stats.last_rebuild_ms = rebuild_ms;

        for light in lights {
            self.world.set_light_field(light.chunk, light.light);
        }

        let chunk_directory = save_directory(self.stress);
        for generation in generations {
            self.stats.generated_columns += 1;
            self.stats.generation_ms += generation.elapsed.as_secs_f64() * 1000.0;
            let mut loaded = true;
            for (chunk, blocks) in generation.chunks {
                let saved = if let Some(directory) = chunk_directory {
                    insert_generated_chunk(&mut self.world, directory, chunk, blocks)
                } else {
                    self.world.insert_chunk(chunk, blocks);
                    Ok(())
                };
                if let Err(error) = saved {
                    error!(%error, chunk = ?chunk, "failed to load a saved chunk");
                    loaded = false;
                }
                self.level_work.insert(chunk);
            }
            if loaded && let Some(streaming) = &mut self.streaming {
                streaming.mark_loaded(generation.column);
            }
        }

        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let mut upload_ms = 0.0;
        for result in &results {
            let started = Instant::now();
            match result.kind {
                MeshKind::Opaque => {
                    renderer.upload_chunk(result.chunk, result.lod, result.revision, &result.mesh);
                }
                MeshKind::Water => {
                    renderer.upload_water_chunk(result.chunk, result.revision, &result.mesh);
                }
            }
            upload_ms += started.elapsed().as_secs_f64() * 1000.0;
        }
        self.stats.upload_ms += upload_ms;
        self.stats.last_upload_ms = upload_ms;
    }

    fn select_active_levels(&mut self) {
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        for (chunk, _) in self.world.iter_chunks() {
            if let Some(lod) = self.queue.desired(chunk) {
                renderer.select_active(chunk, lod);
            }
        }
    }

    fn target_hit(&self) -> Option<RaycastHit> {
        let origin = self.player.eye_position();
        let direction = self.camera.forward();
        match &self.terrain_demo {
            // The patch is what the player can see and stand on, so interaction
            // must target it, not the legacy world underneath (`F1.5`).
            Some(demo) => raycast_where(
                |cell| demo.is_solid(cell),
                origin,
                direction,
                INTERACTION_DISTANCE,
            ),
            None => raycast(&self.world, origin, direction, INTERACTION_DISTANCE),
        }
    }

    fn break_block(&mut self) {
        if self.terrain_demo.is_some() {
            self.demo_edit(true);
            return;
        }
        let Some(hit) = self.target_hit() else {
            return;
        };
        let dirty = self.world.set_block(hit.block, Block::Air);
        debug!(block = ?hit.block, "broke block");
        self.invalidate_chunks(&dirty);
    }

    fn place_block(&mut self) {
        if self.terrain_demo.is_some() {
            self.demo_edit(false);
            return;
        }
        let Some(hit) = self.target_hit() else {
            return;
        };
        let Some(position) = place_position(hit) else {
            return;
        };
        if !can_place(&self.world, &self.player, position) {
            return;
        }
        let dirty = self.world.set_block(position, self.selected_block);
        debug!(block = ?position, selected = ?self.selected_block, "placed block");
        self.invalidate_chunks(&dirty);
    }

    /// Applies the player's `world-repair` choice, reloads sections, and starts
    /// the save worker.
    fn resolve_repair(&mut self, action: save::RepairAction) {
        let Some(prompt) = self.repair_prompt.take() else {
            return;
        };
        let key = prompt.key;
        let world = prompt.world;
        match world.sections.repair(key, action) {
            Ok(outcome) => info!(?outcome, "world repair applied"),
            Err(error) => {
                error!(%error, "world repair failed");
                self.repair_prompt = Some(RepairPrompt { world, key });
                return;
            }
        }
        match world.read_sections() {
            Ok(saved) => {
                if let Some(demo) = &mut self.terrain_demo {
                    demo.apply_saved(&saved);
                }
                if let Some(ring) = &mut self.terrain_ring {
                    let mut grouped: BTreeMap<(i32, i32), Vec<section::SavedSection>> =
                        BTreeMap::new();
                    for record in saved {
                        grouped
                            .entry((record.key.column[0], record.key.column[1]))
                            .or_default()
                            .push(record);
                    }
                    ring.saved = grouped;
                    ring.coarse_dirty = true;
                    ring.upload_dirty = true;
                }
                match world_save::SaveWorker::start(world, 64) {
                    Ok(worker) => self.terrain_save = Some(worker),
                    Err(error) => {
                        warn!(%error, "terrain save worker unavailable; edits will not persist");
                    }
                }
                self.upload_demo();
            }
            Err(error) => {
                error!(%error, "saved terrain is still unreadable after repair");
                self.repair_prompt = Some(RepairPrompt { world, key });
            }
        }
    }

    fn save_world(&mut self) {
        if let Some(save) = self.terrain_save.take() {
            match save.shutdown() {
                Ok(()) => debug!("terrain world saved"),
                Err(error) => error!(%error, "failed to save the terrain world"),
            }
        }
        let Some(directory) = &self.save_directory else {
            debug!("stress scene is not saved");
            return;
        };
        match self.world.save_chunks(directory) {
            Ok(()) => debug!("world saved"),
            Err(error) => error!(%error, "failed to save the world"),
        }
    }

    fn toggle_vsync(&mut self) {
        if let Some(renderer) = &mut self.renderer {
            let vsync = renderer.toggle_vsync();
            info!(vsync, "vsync toggled");
        }
    }

    fn toggle_overlay(&mut self) {
        self.show_overlay = !self.show_overlay;
        debug!(show_overlay = self.show_overlay, "debug overlay toggled");
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Applies one break or place to the demo patch and re-uploads it.
    ///
    /// `C4.2`: `y = 0` is indestructible, and a placement must not intersect the
    /// player. The block is resolved by stable name (`A5.1`), never by the legacy
    /// numeric id.
    fn demo_edit(&mut self, break_it: bool) {
        let Some(hit) = self.target_hit() else {
            return;
        };
        if break_it && hit.block.y == 0 {
            return;
        }
        let (cell, selected) = if break_it {
            (hit.block, None)
        } else {
            let Some(cell) = place_position(hit) else {
                return;
            };
            if self.player.aabb().intersects_block(cell) {
                return;
            }
            (cell, Some(self.selected_block))
        };
        let Some(demo) = &mut self.terrain_demo else {
            return;
        };
        let id = match selected {
            None => demo.air,
            Some(block) => {
                let Some(id) = content::v1_default().id(legacy_block_name(block)) else {
                    return;
                };
                id
            }
        };
        if !break_it && !demo.is_empty(cell) {
            return;
        }
        let key = section_key(cell);
        let baseline = demo.baseline_for(key);
        let old_name = demo
            .block_at(cell)
            .and_then(|old| demo.pack.name(old).map(str::to_owned));
        let new_name = demo.pack.name(id).map(str::to_owned);
        if !demo.set_block(cell, id) {
            return;
        }
        let size = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
        let column = (cell.x.div_euclid(size), cell.z.div_euclid(size));
        demo.unsaved.insert(column);
        let queued = if let (false, Some(save), Some(old_name), Some(new_name)) = (
            demo.read_only,
            self.terrain_save.as_ref(),
            old_name,
            new_name,
        ) {
            queue_demo_edit(save, demo, cell, key, baseline, old_name, new_name);
            true
        } else {
            false
        };
        if queued {
            demo.unsaved.remove(&column);
        }
        if let Some(ring) = &mut self.terrain_ring {
            ring.coarse_dirty = true;
            ring.upload_dirty = true;
        }
        self.demo_edits.push(DemoEdit { cell });
        self.rebuild_demo();
    }

    /// Queues a rebuild; rapid clicks coalesce into one worker at a time.
    ///
    /// The click has already mutated the section, so the frame stays responsive;
    /// [`Self::poll_demo_rebuild`] starts the next worker when this one lands.
    fn rebuild_demo(&mut self) {
        if self.demo_build.is_some() {
            return;
        }
        self.start_demo_rebuild();
    }

    /// Starts a worker for every edit queued since the last rebuild.
    fn start_demo_rebuild(&mut self) {
        let Some(demo) = &self.terrain_demo else {
            return;
        };
        if self.demo_edits.is_empty() {
            return;
        }
        let columns = demo.columns.clone();
        let light = demo.light.clone();
        let materials = demo.materials.clone();
        let edits = std::mem::take(&mut self.demo_edits);
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(demo_rebuild(&columns, &light, &materials, &edits));
        });
        self.demo_build = Some(receiver);
    }

    /// Installs a finished rebuild if it is still current.
    fn poll_demo_rebuild(&mut self) {
        let received = self
            .demo_build
            .as_ref()
            .map(std::sync::mpsc::Receiver::try_recv);
        match received {
            Some(Ok(rebuild)) => {
                let light_ms = rebuild.light_time.as_secs_f64() * 1000.0;
                let mesh_ms = rebuild.mesh_time.as_secs_f64() * 1000.0;
                let patches = rebuild.patches.len();
                self.demo_build = None;
                if let Some(demo) = &mut self.terrain_demo {
                    demo.apply_rebuild(rebuild);
                }
                let upload_started = Instant::now();
                self.upload_demo();
                let upload_ms = upload_started.elapsed().as_secs_f64() * 1000.0;
                debug!(
                    light_ms,
                    mesh_ms, upload_ms, patches, "terrain rebuild applied"
                );
                if !self.demo_edits.is_empty() {
                    self.start_demo_rebuild();
                }
            }
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                self.demo_build = None;
                if !self.demo_edits.is_empty() {
                    self.start_demo_rebuild();
                }
            }
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) | None => {}
        }
    }

    /// Rebuilds the renderer's demo buffers after an edit.
    fn upload_demo(&mut self) {
        let column = Streaming::column_at(self.player.position);
        let column = (column.x, column.y);
        if let Some(demo) = &mut self.terrain_demo
            && demo.lod_column != column
        {
            demo.select_lod(column);
        }
        let (Some(demo), Some(renderer)) = (self.terrain_demo.as_ref(), self.renderer.as_mut())
        else {
            return;
        };
        if let Err(error) = upload_terrain_demo(renderer, demo) {
            error!(%error, "terrain upload failed");
        }
    }

    /// Advances the column ring: applies finished jobs, then plans and starts a
    /// bounded batch of generate, build, and evict work for this frame.
    ///
    /// Returns whether the player's current column has light, so the caller can
    /// keep movement frozen while the centre column streams in.
    #[allow(clippy::too_many_lines)]
    fn poll_ring(&mut self) -> bool {
        let Some(demo) = self.terrain_demo.as_ref() else {
            return true;
        };
        let player = Streaming::column_at(self.player.position);
        let player_column = (player.x, player.y);
        let centre_ready = demo.column_ready(player_column);

        let mut results = Vec::new();
        if let Some(ring) = &self.terrain_ring {
            while results.len() < 16 {
                let Some(result) = ring.pool.try_recv() else {
                    break;
                };
                results.push(result);
            }
        }
        for result in results {
            self.apply_ring_result(result);
        }

        let (Some(demo), Some(ring)) = (self.terrain_demo.as_ref(), self.terrain_ring.as_ref())
        else {
            return centre_ready;
        };
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
        let in_flight: BTreeSet<[i32; 2]> = ring.in_flight.iter().map(|&(x, z)| [x, z]).collect();
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
        let assets = ring.assets.clone();
        let saved = ring.saved.clone();
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
        if let Some(ring) = self.terrain_ring.as_mut() {
            for (column, job) in jobs {
                if ring.in_flight.contains(&column) {
                    continue;
                }
                ring.in_flight.insert(column);
                if !ring.pool.submit(job) {
                    ring.in_flight.remove(&column);
                    warn!(?column, "terrain ring job could not be submitted");
                }
            }
        }
        if let Some(demo) = self.terrain_demo.as_mut() {
            for &column in &evicted {
                debug!(?column, "evicting terrain column");
                demo.remove_column(column);
            }
            if let Some(ring) = self.terrain_ring.as_mut()
                && !evicted.is_empty()
            {
                ring.coarse_dirty = true;
                ring.upload_dirty = true;
                demo.select_lod(player_column);
            }
        }

        // Coarse LOD is disposable and rebuilt whole; keep one job in flight and
        // let it lag a frame behind the fine ring. LOD2 shares this job and
        // throttle; an edit only marks it dirty so the edit path stays cheap.
        let now = Instant::now();
        let (dirty, busy, elapsed, changed) =
            self.terrain_ring
                .as_ref()
                .map_or((false, true, Duration::ZERO, false), |ring| {
                    let changed = self
                        .terrain_demo
                        .as_ref()
                        .is_some_and(|demo| demo.lod_column != player_column);
                    let far_dirty = self
                        .terrain_demo
                        .as_ref()
                        .is_some_and(|demo| demo.far_dirty);
                    (
                        ring.coarse_dirty || far_dirty,
                        ring.coarse_busy,
                        now.saturating_duration_since(ring.last_coarse),
                        changed,
                    )
                });
        if changed {
            if let Some(ring) = self.terrain_ring.as_mut() {
                ring.coarse_dirty = true;
            }
            self.upload_demo();
        }
        if (dirty || changed) && !busy && elapsed >= Duration::from_millis(TERRAIN_DEMO_COARSE_MS) {
            let (Some(demo), Some(ring)) = (self.terrain_demo.as_ref(), self.terrain_ring.as_mut())
            else {
                return centre_ready;
            };
            // Clipped sections are the costly part of a coarse pass. Reuse them
            // while their masks are unchanged; a new rim column, an eviction, or
            // an edit makes the next pass rebuild them.
            let partial = demo.partial_dirty
                || coarse_bands(&demo.columns, player_column, demo.view_radius).partial
                    != demo.partial_columns;
            let job = PoolJob::Coarse(CoarseJob {
                columns: demo.columns.clone(),
                player: player_column,
                radius: demo.view_radius,
                partial,
                materials: Arc::clone(&ring.assets.materials),
                pack: Arc::clone(&ring.assets.pack),
            });
            ring.coarse_busy = true;
            ring.last_coarse = Instant::now();
            if !ring.pool.submit(job) {
                ring.coarse_busy = false;
                warn!("terrain coarse job could not be submitted");
            }
        }
        if let Some(ring) = self.terrain_ring.as_mut()
            && ring.upload_dirty
            && now.saturating_duration_since(ring.last_upload)
                >= Duration::from_millis(TERRAIN_DEMO_UPLOAD_MS)
        {
            ring.upload_dirty = false;
            ring.last_upload = now;
            self.upload_demo();
        }
        centre_ready
    }

    /// Installs one finished ring job.
    #[allow(clippy::too_many_lines)]
    fn apply_ring_result(&mut self, result: PoolResult) {
        let player = Streaming::column_at(self.player.position);
        let player_column = (player.x, player.y);
        match result {
            PoolResult::Generated(column, result) => {
                if let Some(ring) = &mut self.terrain_ring {
                    ring.in_flight.remove(&column);
                }
                match *result {
                    Ok(generated) => {
                        debug!(
                            column = ?generated.column,
                            generate_ms = generated.generate_ms,
                            "terrain column generated"
                        );
                        if let Some(demo) = &mut self.terrain_demo {
                            let radius = demo.view_radius + residency::UNLOAD_MARGIN;
                            let ring_position = (generated.column.0 - player_column.0)
                                .abs()
                                .max((generated.column.1 - player_column.1).abs());
                            if ring_position > radius {
                                debug!(?generated.column, "late column is outside the ring");
                                return;
                            }
                            for (key, baseline) in generated.applied {
                                demo.baselines.insert(key, baseline);
                            }
                            demo.read_only |= generated.read_only;
                            demo.insert_column(generated.column, generated.stack);
                            if generated.column == (demo.origin_column.x, demo.origin_column.y) {
                                demo.settle_spawn();
                            }
                        }
                        if let Some(ring) = &mut self.terrain_ring {
                            ring.coarse_dirty = true;
                            ring.upload_dirty = true;
                        }
                    }
                    Err(error) => {
                        warn!(%error, "terrain column generation failed");
                    }
                }
            }
            PoolResult::Built(built) => {
                if let Some(ring) = &mut self.terrain_ring {
                    ring.in_flight.remove(&built.centre);
                }
                let mut spawn = None;
                if let Some(demo) = &mut self.terrain_demo {
                    if neighbourhood_intact(
                        &demo.columns,
                        &built.snapshot,
                        built.centre,
                        built.margin,
                    ) {
                        demo.apply_light(built.light);
                        for (column, sections) in built.replace {
                            demo.fine.insert(column, sections);
                        }
                        demo.apply_patches(built.patches);
                        // A new or repaired fine mesh must join the active set
                        // immediately; otherwise it waits for the next coarse
                        // rebuild even though the player may be standing on it.
                        demo.select_lod(demo.lod_column);
                        if built.centre == (demo.origin_column.x, demo.origin_column.y)
                            && demo.spawn_ready
                        {
                            spawn = Some(demo.spawn);
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
                }
                if let Some(ring) = &mut self.terrain_ring {
                    ring.upload_dirty = true;
                }
                if let Some(spawn) = spawn {
                    let announce = self.terrain_ring.as_ref().is_some_and(|ring| !ring.spawned);
                    if announce {
                        self.set_spawn(spawn);
                        if let Some(ring) = &mut self.terrain_ring {
                            ring.spawned = true;
                        }
                        info!(?spawn, "standing on the centre terrain column");
                    }
                }
            }
            PoolResult::Coarse(coarse) => {
                let intact = self.terrain_demo.as_ref().is_some_and(|demo| {
                    demo.lod_column == coarse.player
                        && coarse_snapshot_intact(&demo.columns, &coarse.snapshot)
                });
                if let Some(demo) = &mut self.terrain_demo {
                    if intact {
                        let changed = demo.columns.len() != coarse.snapshot.len();
                        let CoarseBuilt {
                            coarse: complete,
                            partial,
                            far,
                            ..
                        } = coarse;
                        let kept: Vec<DemoSection> = if partial.is_none() {
                            demo.coarse
                                .iter()
                                .filter(|section| {
                                    demo.partial_columns.contains_key(&(
                                        section.origin[0] / 32,
                                        section.origin[2] / 32,
                                    ))
                                })
                                .cloned()
                                .collect()
                        } else {
                            Vec::new()
                        };
                        demo.coarse = complete;
                        demo.coarse.extend(kept);
                        if let Some((sections, masks)) = partial {
                            demo.coarse.extend(sections);
                            demo.partial_columns = masks;
                            demo.partial_dirty = false;
                        }
                        demo.coarse.sort_by_key(|section| section.origin);
                        demo.far = far;
                        demo.far_dirty = false;
                        demo.select_lod(demo.lod_column);
                        if let Some(ring) = &mut self.terrain_ring {
                            ring.coarse_busy = false;
                            ring.coarse_dirty = changed;
                            ring.upload_dirty = true;
                        }
                    } else if let Some(ring) = &mut self.terrain_ring {
                        ring.coarse_busy = false;
                        ring.coarse_dirty = true;
                    }
                }
            }
        }
    }

    fn update_highlight(&mut self) {
        if self.renderer.is_none() {
            return;
        }
        profiling::scope!("highlight");
        let hit = self.target_hit().map(|hit| hit.block);
        if self.highlighted_block == hit {
            return;
        }
        self.highlighted_block = hit;
        let mesh = hit.map_or_else(Vec::new, build_highlight_mesh);
        if let Some(renderer) = &mut self.renderer {
            renderer.set_highlight(&mesh);
        }
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        self.camera.set_aspect(size.width, size.height);
        if let Some(renderer) = &mut self.renderer {
            renderer.resize(size, &self.camera);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    #[allow(clippy::cast_precision_loss)]
    /// Builds this frame's movement input and advances the player against
    /// whichever block source is live: the demo patch, or the legacy world.
    fn step_player(&mut self, delta_seconds: f32) {
        let forward_axis = axis(
            self.pressed.contains(&KeyCode::KeyW),
            self.pressed.contains(&KeyCode::KeyS),
        );
        let right_axis = axis(
            self.pressed.contains(&KeyCode::KeyD),
            self.pressed.contains(&KeyCode::KeyA),
        );
        let jump_pressed = std::mem::take(&mut self.jump_requested);
        let input = PlayerInput {
            forward: forward_axis,
            right: right_axis,
            jump: if self.player.flying {
                self.pressed.contains(&KeyCode::Space)
            } else {
                jump_pressed
            },
            descend: self.player.flying && self.modifiers.control_key(),
            yaw: self.camera.yaw(),
        };
        profiling::scope!("player update");
        let streaming = self.streaming.as_ref();
        let demo = self.terrain_demo.as_ref();
        let ready = |position: Vec3| match demo {
            Some(demo) => demo.aabb_ready(position),
            None => streaming.is_none_or(|streaming| streaming.aabb_ready(position)),
        };
        let source: &dyn crate::player::BlockSource = match &self.terrain_demo {
            Some(demo) => demo,
            None => &self.world,
        };
        self.player
            .update_guarded(source, input, delta_seconds, &ready);
    }

    #[allow(clippy::cast_precision_loss)]
    fn redraw(&mut self) {
        if self.occluded {
            return;
        }

        profiling::scope!("frame");

        self.poll_demo_rebuild();

        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.last_frame);
        self.last_frame = now;

        if self.focused {
            self.record_frame(elapsed);
            self.poll_ring();
            let delta_seconds = elapsed.min(MAX_FRAME_DELTA).as_secs_f32();
            self.step_player(delta_seconds);
            self.sky.advance(delta_seconds);
            self.camera.set_position(self.player.eye_position());
            self.update_highlight();
        }

        let slow0 = Instant::now();
        if let Some(pool) = self.mesher.clone() {
            self.stream_columns(&*pool);
            let slow1 = Instant::now();
            self.request_levels(&*pool);
            let slow2 = Instant::now();
            self.upload_finished_meshes(&*pool);
            let slow3 = Instant::now();
            profiling::scope!("select active");
            self.select_active_levels();
            let slow4 = Instant::now();
            if slow4.saturating_duration_since(slow0).as_millis() > 5 {
                let ms =
                    |a: Instant, b: Instant| b.saturating_duration_since(a).as_secs_f64() * 1000.0;
                debug!(
                    stream = ms(slow0, slow1),
                    levels = ms(slow1, slow2),
                    upload = ms(slow2, slow3),
                    select = ms(slow3, slow4),
                    "slow phases"
                );
            }
        }

        let player_column = Streaming::column_at(self.player.position);
        let player_column = (player_column.x, player_column.y);
        if self
            .terrain_demo
            .as_ref()
            .is_some_and(|demo| demo.lod_column != player_column)
        {
            self.upload_demo();
        }
        let overlay = if let Some(prompt) = &self.repair_prompt {
            Some(format!(
                "REPAIR c.{}.{}.{}: R RESTORE  Q QUARANTINE  S SALVAGE  (OR CLICK LEFT/MID/RIGHT)",
                prompt.key.column[0], prompt.key.column[1], prompt.key.section
            ))
        } else {
            self.show_overlay.then(|| {
                format!(
                    "FPS {:.0}  {:.2} MS",
                    self.overlay_fps, self.overlay_frame_ms
                )
            })
        };
        // One render origin for the frame: the same value builds the frustum
        // and rebases every submitted section/chunk origin.
        let view = self.camera.render_view();
        let frustum = {
            profiling::scope!("frustum");
            Frustum::from_view_projection(view.view_projection)
        };
        let slow_render = Instant::now();
        if let Some(renderer) = &mut self.renderer {
            renderer.set_camera(&view);
            renderer.set_sky(&self.sky.sample(), self.unlit);
            renderer.set_overlay(overlay.as_deref());
            self.stats.totals = renderer.render(&frustum);
        }
        if slow_render.elapsed().as_millis() > 5 {
            debug!(
                render_ms = slow_render.elapsed().as_secs_f64() * 1000.0,
                "slow render"
            );
        }

        metrics::frame_ms(elapsed.as_secs_f64() * 1000.0);
        metrics::visible_chunks(f64::from(self.stats.totals.visible_chunks));
        metrics::draw_calls(f64::from(self.stats.totals.draw_calls));
        metrics::visible_vertices(self.stats.totals.visible_vertices as f64);
        metrics::rebuild_ms(self.stats.last_rebuild_ms);
        metrics::upload_ms(self.stats.last_upload_ms);
        metrics::mesh_jobs(self.stats.pending_jobs as f64);
        metrics::generated_columns(f64::from(self.stats.generated_columns));
        metrics::generation_ms(self.stats.generation_ms);
        metrics::terrain_sections_uploaded(f64::from(self.stats.totals.terrain_sections_uploaded));
        profiling::finish_frame!();

        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn record_frame(&mut self, delta: Duration) {
        self.frame_accum += delta;
        self.frame_count += 1;
        if self.frame_accum >= FRAME_LOG_INTERVAL {
            let seconds = self.frame_accum.as_secs_f64();
            let frames = f64::from(self.frame_count);
            debug!(
                fps = frames / seconds,
                frame_ms = seconds * 1000.0 / frames,
                visible_chunks = self.stats.totals.visible_chunks,
                total_chunks = self.stats.totals.total_chunks,
                visible_vertices = self.stats.totals.visible_vertices,
                draw_calls = self.stats.totals.draw_calls,
                rebuild_ms = self.stats.rebuild_ms,
                upload_ms = self.stats.upload_ms,
                generated_columns = self.stats.generated_columns,
                generation_ms = self.stats.generation_ms,
                player = ?self.player.position,
                on_ground = self.player.on_ground,
                "frame timing"
            );
            if let Some(demo) = &self.terrain_demo {
                let wanted =
                    usize::try_from((2 * demo.view_radius + 1) * (2 * demo.view_radius + 1))
                        .unwrap_or(0);
                let resident = demo.columns.len();
                let ready = demo.light.len();
                let fine = demo.fine.len();
                let coarse: usize = demo.coarse.len();
                let lod2: usize = demo.far.len();
                let quads = demo.quads;
                let bytes = demo.footprint_bytes();
                let in_flight = self
                    .terrain_ring
                    .as_ref()
                    .map_or(0, |ring| ring.in_flight.len());
                info!(
                    wanted,
                    resident, ready, fine, coarse, lod2, quads, in_flight, bytes, "terrain ring"
                );
                let full = resident >= wanted;
                if let Some(ring) = &mut self.terrain_ring
                    && full
                    && !ring.announced_full
                {
                    ring.announced_full = true;
                    info!(wanted, bytes, "terrain ring complete");
                }
            }
            self.overlay_fps = frames / seconds;
            self.overlay_frame_ms = seconds * 1000.0 / frames;
            self.frame_accum = Duration::ZERO;
            self.frame_count = 0;
            self.stats.reset();
        }
    }

    fn handle_key(&mut self, event: &KeyEvent) {
        let PhysicalKey::Code(code) = event.physical_key else {
            return;
        };

        match event.state {
            ElementState::Pressed => {
                if event.repeat {
                    return;
                }
                if self.repair_prompt.is_some() {
                    if let Some(action) = repair_action(code) {
                        self.resolve_repair(action);
                    }
                    return;
                }
                if code == KeyCode::KeyS && self.modifiers.control_key() {
                    self.save_world();
                } else if code == KeyCode::Space {
                    self.jump_requested = true;
                    // Held while flying: `Space` ascends continuously.
                    self.pressed.insert(code);
                } else if code == KeyCode::Escape {
                    self.release_cursor();
                } else if code == KeyCode::KeyV {
                    self.toggle_vsync();
                } else if code == KeyCode::F3 {
                    self.toggle_overlay();
                } else if code == KeyCode::KeyF {
                    self.player.toggle_fly();
                    info!(flying = self.player.flying, "debug fly toggled");
                } else if code == KeyCode::KeyT {
                    self.sky.skip_hour();
                    debug!(time_of_day = self.sky.time_of_day(), "time advanced");
                } else if code == KeyCode::F4 {
                    self.unlit = !self.unlit;
                    debug!(unlit = self.unlit, "unlit shading toggled");
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                } else if let Some(block) = hotbar_block(code) {
                    self.selected_block = block;
                    debug!(?block, "selected block");
                } else if is_movement_key(code) {
                    self.pressed.insert(code);
                }
            }
            ElementState::Released => {
                self.pressed.remove(&code);
            }
        }
    }

    fn capture_cursor(&mut self) {
        if self.mouse_captured {
            return;
        }
        let Some(window) = &self.window else {
            return;
        };

        if let Err(locked_error) = window.set_cursor_grab(CursorGrabMode::Locked) {
            debug!(%locked_error, "locked cursor grab unavailable; trying confined grab");
            if let Err(error) = window.set_cursor_grab(CursorGrabMode::Confined) {
                warn!(%error, "failed to capture the cursor");
                return;
            }
        }
        window.set_cursor_visible(false);
        self.mouse_captured = true;
        debug!("cursor captured");
    }

    fn release_cursor(&mut self) {
        if self.mouse_captured {
            debug!("cursor released");
        }
        if let Some(window) = &self.window {
            if let Err(error) = window.set_cursor_grab(CursorGrabMode::None) {
                warn!(%error, "failed to release the cursor");
            }
            window.set_cursor_visible(true);
        }
        self.mouse_captured = false;
    }

    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn apply_render_distance(&mut self) {
        let distance = self.config.render_distance_chunks as f32 * 32.0;
        self.camera.set_far_plane(distance + 64.0);
        self.camera.set_fog_end(distance);
    }

    /// Whether all four chunks of a column are lit and meshed.
    /// Whether all four chunks of a column are lit and meshed.
    ///
    /// A chunk with no stored blocks is empty air and needs no light or mesh.
    fn column_ready(&self, column: IVec2) -> bool {
        (0..CHUNK_COLUMN).all(|y| {
            let chunk = ChunkCoord::new(column.x, y, column.y);
            self.world.chunk(chunk).is_none()
                || (self.queue.light_is_current(chunk)
                    && (self.queue.is_built(chunk, Lod::Lod0, MeshKind::Opaque)
                        || self.queue.is_built(chunk, Lod::Lod1, MeshKind::Opaque)
                        || self.queue.is_built(chunk, Lod::Lod2, MeshKind::Opaque)))
        })
    }

    /// Marks ready columns, unloads distant ones, and requests the nearest missing.
    pub(crate) fn stream_columns(&mut self, pool: &dyn MeshPool) {
        let Some(seed) = self.seed else {
            return;
        };
        let player_column = Streaming::column_at(self.player.position);

        let unloadable = self
            .streaming
            .as_ref()
            .map(|streaming| streaming.unloadable(player_column))
            .unwrap_or_default();
        for column in unloadable {
            self.unload_column(column);
        }

        let loaded: Vec<IVec2> = self
            .streaming
            .as_ref()
            .map(|streaming| streaming.loaded().collect())
            .unwrap_or_default();
        for column in loaded {
            if self.column_ready(column)
                && let Some(streaming) = &mut self.streaming
            {
                streaming.mark_ready(column);
            }
        }

        let forward = self.camera.forward();
        let forward = Vec2::new(forward.x, forward.z).normalize_or_zero();
        let wanted = self
            .streaming
            .as_ref()
            .map(|streaming| streaming.wanted(self.player.position, forward))
            .unwrap_or_default();
        for column in wanted.into_iter().take(MAX_COLUMNS_PER_FRAME) {
            self.queue
                .request_generation(pool, seed, column, Priority::Background);
        }
    }

    /// Saves and drops one column that left the unload margin.
    fn unload_column(&mut self, column: IVec2) {
        let Some(directory) = &self.save_directory else {
            return;
        };
        if let Err(error) = release_column(&mut self.world, &mut self.queue, directory, column) {
            error!(%error, ?column, "failed to save an unloaded chunk");
            return;
        }
        if let Some(renderer) = &mut self.renderer {
            for y in 0..CHUNK_COLUMN {
                renderer.remove_chunk(ChunkCoord::new(column.x, y, column.y));
            }
        }
        for y in 0..CHUNK_COLUMN {
            self.level_work
                .remove(&ChunkCoord::new(column.x, y, column.y));
        }
        if let Some(streaming) = &mut self.streaming {
            streaming.mark_unloaded(column);
        }
        debug!(?column, "unloaded column");
    }

    pub(crate) fn set_spawn(&mut self, position: Vec3) {
        self.player = Player::new(position);
        self.camera.set_position(self.player.eye_position());
    }

    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if focused {
            self.last_frame = Instant::now();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        } else {
            self.pressed.clear();
            self.jump_requested = false;
            self.highlighted_block = None;
            if let Some(renderer) = &mut self.renderer {
                renderer.set_highlight(&[]);
            }
            self.release_cursor();
        }
    }

    fn apply_mouse_look(&mut self, delta_x: f32, delta_y: f32) {
        if self.mouse_captured && self.focused {
            self.camera.look(delta_x, delta_y);
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }

        let attributes = Window::default_attributes()
            .with_title(WINDOW_TITLE)
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(error) => {
                self.fail(RunError::Window(error), event_loop);
                return;
            }
        };

        let size = window.inner_size();
        self.camera.set_aspect(size.width, size.height);
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle_from_env(
                Box::new(event_loop.owned_display_handle()),
            ));
        let mut renderer =
            match pollster::block_on(Renderer::new(window.clone(), instance, &self.camera)) {
                Ok(renderer) => renderer,
                Err(error) => {
                    self.fail(RunError::Renderer(error), event_loop);
                    return;
                }
            };

        // One upload at startup; the patch is static, so nothing rebuilds it.
        let upload = match &self.terrain_demo {
            Some(demo) => upload_terrain_demo(&mut renderer, demo),
            None => Ok(()),
        };
        if let Err(error) = upload {
            self.fail(RunError::Renderer(error), event_loop);
            return;
        }

        info!(width = size.width, height = size.height, "window ready");
        self.window = Some(window.clone());
        self.renderer = Some(renderer);
        self.mesher = Some(create_pool());
        self.request_initial_meshes();
        if no_vsync_requested() {
            self.toggle_vsync();
        }
        self.last_frame = Instant::now();
        window.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                self.save_world();
                self.release_cursor();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => self.resize(size),
            WindowEvent::KeyboardInput { event, .. } => self.handle_key(&event),
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_position = (position.x, position.y);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if self.repair_prompt.is_some() {
                    let width = self
                        .window
                        .as_ref()
                        .map_or(1.0, |window| f64::from(window.inner_size().width).max(1.0));
                    let x = self.cursor_position.0;
                    let action = if x < width / 3.0 {
                        save::RepairAction::RestoreBackup
                    } else if x < width * 2.0 / 3.0 {
                        save::RepairAction::Quarantine
                    } else {
                        save::RepairAction::Salvage
                    };
                    self.resolve_repair(action);
                    return;
                }
                if self.mouse_captured {
                    self.break_block();
                } else {
                    self.capture_cursor();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Right,
                ..
            } => {
                if self.mouse_captured {
                    self.place_block();
                }
            }
            WindowEvent::Focused(focused) => self.set_focused(focused),
            WindowEvent::Occluded(occluded) => {
                self.occluded = occluded;
                if !occluded && let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        if let DeviceEvent::MouseMotion { delta } = event {
            self.apply_mouse_look(delta.0 as f32, delta.1 as f32);
        }
    }
}

fn axis(positive: bool, negative: bool) -> f32 {
    match (positive, negative) {
        (true, false) => 1.0,
        (false, true) => -1.0,
        _ => 0.0,
    }
}

fn is_movement_key(code: KeyCode) -> bool {
    matches!(
        code,
        KeyCode::KeyW | KeyCode::KeyA | KeyCode::KeyS | KeyCode::KeyD
    )
}

fn hotbar_block(code: KeyCode) -> Option<Block> {
    let slot = match code {
        KeyCode::Digit1 => 1,
        KeyCode::Digit2 => 2,
        KeyCode::Digit3 => 3,
        KeyCode::Digit4 => 4,
        KeyCode::Digit5 => 5,
        _ => return None,
    };
    Block::from_hotbar(slot)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::generation::GenResult;
    use crate::light;
    use crate::mesh;
    use crate::mesher::{Job, JobResult, MeshResult};
    use crate::pipeline;
    use crate::section;
    use crate::startup::stress_world;

    use super::*;

    fn temp_directory(name: &str) -> std::path::PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "valescent-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    /// Builds the demo world used by the tests.
    fn demo_world() -> World {
        let mut world = World::default();
        for x in 28..=35 {
            for z in 28..=35 {
                world.set_block(IVec3::new(x, 0, z), Block::Dirt);
                world.set_block(IVec3::new(x, 1, z), Block::Grass);
            }
        }
        for coordinate in [
            IVec3::new(29, 2, 29),
            IVec3::new(30, 2, 29),
            IVec3::new(29, 2, 30),
        ] {
            world.set_block(coordinate, Block::Stone);
        }
        for x in 28..=30 {
            for z in 28..=30 {
                world.set_block(IVec3::new(x, 1, z), Block::Water);
            }
        }
        world.set_block(IVec3::new(30, 2, 30), Block::Lamp);
        world.set_block(IVec3::new(34, 2, 34), Block::Wood);
        world.set_block(IVec3::new(34, 3, 34), Block::Wood);
        world.set_block(IVec3::new(34, 4, 34), Block::Leaves);
        world
    }

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
        let mut app = App::with_world(World::default(), true);
        app.terrain_ring = Some(RingStream::new(&demo, Vec::new(), 2));
        app.terrain_demo = Some(demo);
        app.player.position = Vec3::new(
            column.x as f32 * 32.0 + 16.5,
            64.0,
            column.y as f32 * 32.0 + 16.5,
        );

        // The centre column starts unready, so physics refuses movement.
        assert!(
            !app.terrain_demo
                .as_ref()
                .is_some_and(|demo| demo.aabb_ready(app.player.position))
        );
        app.poll_ring();
        let in_flight = &app
            .terrain_ring
            .as_ref()
            .expect("the ring exists")
            .in_flight;
        assert_eq!(in_flight.len(), 2, "two columns per frame");
        assert!(
            in_flight.contains(&(column.x, column.y)),
            "the centre column is generated first"
        );

        let deadline = Instant::now() + Duration::from_secs(30);
        while !app.terrain_demo.as_ref().is_some_and(|demo| {
            demo.column_ready((column.x, column.y)) && demo.fine.contains_key(&(column.x, column.y))
        }) {
            assert!(
                Instant::now() < deadline,
                "the centre column did not become ready"
            );
            app.poll_ring();
            std::thread::sleep(Duration::from_millis(5));
        }
        let demo = app.terrain_demo.as_ref().expect("the demo exists");
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
            demo.aabb_ready(demo.spawn),
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
        let mut app = App::with_world(World::default(), true);
        app.terrain_ring = Some(RingStream::new(&demo, Vec::new(), 1));
        app.terrain_demo = Some(demo);
        app.player.position = Vec3::new(16.5, 64.0, 16.5);

        app.poll_ring();
        let demo = app.terrain_demo.as_ref().expect("the demo exists");
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

    #[test]
    fn repair_shortcuts_map_to_actions() {
        assert_eq!(
            repair_action(KeyCode::KeyR),
            Some(save::RepairAction::RestoreBackup)
        );
        assert_eq!(
            repair_action(KeyCode::KeyQ),
            Some(save::RepairAction::Quarantine)
        );
        assert_eq!(
            repair_action(KeyCode::KeyS),
            Some(save::RepairAction::Salvage)
        );
        assert_eq!(repair_action(KeyCode::KeyX), None);
    }
    #[test]
    fn mouse_look_requires_capture_and_focus() {
        let mut app = App::new(demo_world());
        let initial = app.camera.view_projection();

        app.apply_mouse_look(100.0, 0.0);
        assert_eq!(app.camera.view_projection(), initial);

        app.mouse_captured = true;
        app.focused = false;
        app.apply_mouse_look(100.0, 0.0);
        assert_eq!(app.camera.view_projection(), initial);

        app.focused = true;
        app.apply_mouse_look(100.0, 0.0);
        assert_ne!(app.camera.view_projection(), initial);
    }

    #[test]
    fn overlay_values_update_once_per_second() {
        let mut app = App::new(demo_world());
        assert_eq!(app.overlay_fps, 0.0);

        app.record_frame(Duration::from_millis(400));
        assert_eq!(app.overlay_fps, 0.0);

        app.record_frame(Duration::from_millis(600));
        assert!((app.overlay_fps - 2.0).abs() < 1e-5);
        assert!((app.overlay_frame_ms - 500.0).abs() < 1e-5);
    }

    #[test]
    fn demo_world_has_the_required_blocks_and_clear_spawn() {
        let world = demo_world();

        assert!(
            world
                .iter_chunks()
                .any(|(_, chunk)| chunk.iter().any(|(_, block)| block == Block::Stone))
        );
        assert!(
            world
                .iter_chunks()
                .any(|(_, chunk)| chunk.iter().any(|(_, block)| block == Block::Wood))
        );
        assert_eq!(world.block(IVec3::new(SPAWN_X, 0, SPAWN_Z)), Block::Dirt);
        assert_eq!(world.block(IVec3::new(SPAWN_X, 1, SPAWN_Z)), Block::Grass);

        let spawn = spawn_position(&world);
        let player = Player::new(spawn);
        assert!((spawn.y - 2.5).abs() < 1e-5);
        assert!(
            !player
                .aabb()
                .intersects_block(IVec3::new(SPAWN_X, 1, SPAWN_Z))
        );
    }
    #[derive(Debug, Default)]
    struct InlinePool {
        light_jobs: AtomicUsize,
        mesh_jobs: AtomicUsize,
        results: Mutex<Vec<JobResult>>,
    }

    impl MeshPool for InlinePool {
        fn can_submit(&self, _priority: Priority) -> bool {
            true
        }

        fn submit(&self, job: Job, _priority: Priority) -> bool {
            match job {
                Job::Light(job) => {
                    self.light_jobs.fetch_add(1, Ordering::Relaxed);
                    let light = light::compute(&job.snapshot, job.chunk);
                    self.results.lock().unwrap().push(JobResult::Light(
                        crate::light::LightResult {
                            chunk: job.chunk,
                            revision: job.revision,
                            light,
                            elapsed: Duration::ZERO,
                        },
                    ));
                }
                Job::Mesh(_) => {
                    self.mesh_jobs.fetch_add(1, Ordering::Relaxed);
                }
                Job::Generate(_) => {}
            }
            true
        }

        fn try_recv(&self) -> Option<JobResult> {
            self.results.lock().unwrap().pop()
        }
    }

    #[test]
    fn an_edit_waits_for_light_before_requesting_meshes() {
        let mut app = App::new(demo_world());
        app.player = Player::new(Vec3::new(32.5, 2.5, 32.5));
        app.camera.set_position(app.player.eye_position());
        app.camera.look(0.0, 100_000.0);

        let pool = Arc::new(InlinePool::default());
        app.mesher = Some(pool.clone());
        let target = IVec3::new(SPAWN_X, 1, SPAWN_Z);
        let chunk = ChunkCoord::from_world(target).0;

        app.break_block();
        assert!(!app.queue.light_is_current(chunk));
        assert!(pool.light_jobs.load(Ordering::Relaxed) >= 1);
        assert_eq!(pool.mesh_jobs.load(Ordering::Relaxed), 0);

        app.upload_finished_meshes(&*pool);
        assert!(app.queue.light_is_current(chunk));
        assert!(app.world.light_field(chunk).is_some());

        app.request_levels(&*pool);
        assert!(pool.mesh_jobs.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn breaking_a_block_invalidates_light_and_waits_for_rebuild() {
        let mut app = App::new(demo_world());
        app.player = Player::new(Vec3::new(32.5, 2.5, 32.5));
        app.camera.set_position(app.player.eye_position());
        app.camera.look(0.0, 100_000.0);

        let target = IVec3::new(SPAWN_X, 1, SPAWN_Z);
        let chunk = ChunkCoord::from_world(target).0;
        assert!(app.queue.light_is_current(chunk));

        app.break_block();

        assert_eq!(app.world.block(target), Block::Air);
        assert!(!app.queue.light_is_current(chunk));
    }
    #[test]
    fn placement_rejects_occupied_cells_and_the_player_box() {
        let mut world = World::default();
        world.set_block(IVec3::new(1, 0, 0), Block::Stone);
        let player = Player::new(Vec3::new(0.5, 0.0, 0.5));

        assert!(!can_place(&world, &player, IVec3::new(1, 0, 0)));
        assert!(!can_place(&world, &player, IVec3::ZERO));
        assert!(can_place(&world, &player, IVec3::new(2, 0, 0)));
    }

    #[test]
    fn breaking_a_targeted_block_clears_it() {
        let mut app = App::new(demo_world());
        app.player = Player::new(Vec3::new(32.5, 2.5, 32.5));
        app.camera.set_position(app.player.eye_position());
        app.camera.look(0.0, 100_000.0);

        assert_eq!(
            app.world.block(IVec3::new(SPAWN_X, 1, SPAWN_Z)),
            Block::Grass
        );
        app.break_block();
        assert_eq!(app.world.block(IVec3::new(SPAWN_X, 1, SPAWN_Z)), Block::Air);
    }
    #[test]
    fn a_late_generation_result_is_discarded_after_unload() {
        let pool = InlinePool::default();
        let column = IVec2::new(2, 0);
        let result = GenResult {
            column,
            chunks: Vec::new(),
            elapsed: Duration::ZERO,
        };
        let mut queue = MeshQueue::default();

        queue.request_generation(&pool, 7, column, Priority::Background);
        assert!(queue.accept_generation(&result));

        queue.request_generation(&pool, 7, column, Priority::Background);
        queue.cancel_generation(column);
        assert!(!queue.accept_generation(&result));
    }
    #[test]
    fn the_render_distance_sets_the_far_plane_and_fog_range() {
        let mut app = App::new(demo_world());
        app.config.render_distance_chunks = 12;

        app.apply_render_distance();

        assert_eq!(app.camera.far_plane(), 448.0);
        assert_eq!(app.camera.fog_end(), 384.0);
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn the_ring_radius_never_exceeds_the_camera_visible_radius() {
        let mut app = App::new(demo_world());
        for render_distance in [8, 12, 16, 24, 32] {
            app.config.render_distance_chunks = render_distance;
            app.apply_render_distance();

            let radius = residency::demo_view_radius(render_distance);
            let ring_extent = radius as f32 * 32.0;
            assert!(
                ring_extent <= app.camera.fog_end(),
                "radius {radius} ends at {ring_extent} blocks, past fog end {}",
                app.camera.fog_end()
            );
            assert!(
                ring_extent <= app.camera.far_plane(),
                "radius {radius} ends at {ring_extent} blocks, past far plane {}",
                app.camera.far_plane()
            );
        }

        // The default selects the 625-column ring; the spec distance selects
        // the 1,089-column cap.
        assert_eq!(residency::demo_view_radius(12), 12);
        assert_eq!(residency::demo_view_radius(16), residency::DEMO_VIEW_RADIUS);
    }

    #[test]
    fn a_column_built_at_lod2_is_ready() {
        let mut app = App::with_world(World::default(), false);
        let chunk = ChunkCoord::new(0, 0, 0);
        app.world.set_block(IVec3::new(0, 0, 0), Block::Stone);
        app.queue.mark_light_built(chunk);
        let pool = InlinePool::default();
        app.queue.request(
            &pool,
            &app.world,
            chunk,
            Lod::Lod2,
            MeshKind::Opaque,
            Priority::Background,
        );
        let revision = app.queue.revision(chunk);

        assert!(app.queue.accept(&MeshResult {
            chunk,
            lod: Lod::Lod2,
            kind: MeshKind::Opaque,
            revision,
            mesh: mesh::MeshData::default(),
            elapsed: Duration::ZERO,
        }));
        assert!(app.column_ready(IVec2::ZERO));
    }

    #[test]
    fn an_all_air_column_is_ready() {
        let app = App::with_world(World::default(), false);

        assert!(app.column_ready(IVec2::new(3, -2)));
    }

    #[test]
    fn absent_chunks_are_ignored_but_stored_chunks_must_be_lit() {
        let mut app = App::with_world(World::default(), false);
        app.world.set_block(IVec3::new(0, 0, 0), Block::Stone);

        assert!(!app.column_ready(IVec2::ZERO));

        app.queue.mark_light_built(ChunkCoord::new(0, 0, 0));

        assert!(!app.column_ready(IVec2::ZERO));
    }

    #[test]
    fn streaming_unloads_a_distant_column_after_saving_it() {
        let directory = temp_directory("unload-stream");
        let chunk = ChunkCoord::new(0, 0, 0);
        let coordinate = IVec3::new(5, 0, 5);
        let mut app = App::with_world(World::default(), false);
        app.save_directory = Some(directory.clone());
        app.seed = Some(42);
        app.world.set_block(coordinate, Block::Stone);
        let mut streaming = Streaming::new(8);
        streaming.mark_loaded(IVec2::ZERO);
        streaming.mark_ready(IVec2::ZERO);
        app.streaming = Some(streaming);
        app.player.position = Vec3::new(60.0 * 32.0, 40.0, 0.5);

        app.stream_columns(&InlinePool::default());

        assert!(app.world.chunk(chunk).is_none());
        let saved = World::load_saved_chunk(&directory, chunk).unwrap().unwrap();
        assert_eq!(saved.block(coordinate), Block::Stone);
        assert!(!app.streaming.as_ref().unwrap().is_ready(IVec2::ZERO));
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn stress_mode_spawns_on_the_flat_half_facing_the_uneven_half() {
        let app = App::with_world(stress_world(), true);

        assert_eq!(app.player.position, Vec3::new(16.5, 3.0, 16.5));
        assert!((app.camera.yaw() - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
        assert_eq!(app.world.block(IVec3::new(16, 2, 16)), Block::Grass);
    }
    #[test]
    fn the_stress_spawn_frustum_contains_the_nearby_chunks() {
        let app = App::with_world(stress_world(), true);
        let view = app.camera.render_view();
        let frustum = Frustum::from_view_projection(view.view_projection);

        let inside = ChunkCoord::from_world(IVec3::new(16, 0, 16)).0.origin();
        let inside = view.relative_point(inside);
        assert!(frustum.intersects_aabb(inside, inside + Vec3::splat(32.0)));

        let behind = ChunkCoord::from_world(IVec3::new(16, 0, 240)).0.origin();
        let behind = view.relative_point(behind);
        assert!(!frustum.intersects_aabb(behind, behind + Vec3::splat(32.0)));
    }
}

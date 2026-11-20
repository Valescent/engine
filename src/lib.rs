//! Valescent Engine: a voxel engine written in Rust.
//!
//! The binary in `src/main.rs` is a thin shell around this crate. Everything
//! the engine does lives here so it can be tested, benchmarked, and reused
//! without a window.

mod block;
mod camera;
mod config;
mod frustum;
mod generation;
mod headless;
mod horizon;
mod light;
mod mesh;
mod mesher;
mod metrics;
mod overlay;
mod player;
mod pool;
mod quad;
mod quad_renderer;
mod raycast;
mod renderer;
mod sky;
mod streaming;
mod voxel;
mod world;

#[cfg(test)]
mod bench;

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::{IVec2, IVec3, Vec2, Vec3};
use thiserror::Error;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use block::Block;
use camera::Camera;
use config::{Config, ConfigError};
use frustum::Frustum;
use generation::Generator;
use mesh::Lod;
use mesher::{MeshKind, MeshPool, MeshQueue, Priority, collect_finished, create_pool};
use player::{Player, PlayerInput};
use raycast::{RaycastHit, raycast};
pub use renderer::RendererError;
use renderer::{FrameStats, Renderer};
use sky::Sky;
use streaming::Streaming;
use voxel::build_highlight_mesh;
use world::{CHUNK_COLUMN, CHUNK_SIZE, Chunk, DirtyChunks, WORLD_DIRECTORY, World};
pub use world::{ChunkCoord, SaveError};

const WINDOW_TITLE: &str = concat!("Valescent Engine ", env!("CARGO_PKG_VERSION"));
const MAX_FRAME_DELTA: Duration = Duration::from_millis(100);
const FRAME_LOG_INTERVAL: Duration = Duration::from_secs(1);
const WORLD_CHUNKS: &str = "world/chunks";
const CONFIG_FILE: &str = "config.toml";
const MAX_COLUMNS_PER_FRAME: usize = 2;
const INTERACTION_DISTANCE: f32 = 6.0;
const SPAWN_X: i32 = 32;
const SPAWN_Z: i32 = 32;
/// Mesh uploads collected per frame. `2` made distance-32 first-load settle in 33 s against 22 s at
/// `64`, so above this value the cap stops limiting; `8` stays above the worker rate while keeping
/// each frame's upload burst small.
const MAX_UPLOADS_PER_FRAME: usize = 8;

/// Main-thread budget for draining the chunk work set each frame.
const LEVEL_WORK_BUDGET: Duration = Duration::from_millis(2);
const STRESS_SCENE_ENV: &str = "ENGINE_STRESS_SCENE";
const STRESS_CHUNKS: i32 = 8;
const STRESS_FLAT_CHUNKS: i32 = 4;

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
}

/// Runs the engine until the window is closed.
///
/// # Errors
///
/// Returns an error if world loading, the event loop, window, or GPU
/// initialization fails.
pub fn run() -> Result<(), RunError> {
    init_tracing();

    #[cfg(feature = "tracy")]
    let _tracy_client = tracy_client::Client::start();

    let config = config::load(Path::new(CONFIG_FILE))?;
    let stress = stress_requested();
    let setup = world_for(
        stress,
        Path::new(WORLD_DIRECTORY),
        config.render_distance_chunks,
    )?;
    let mut app = App::with_world(setup.world, stress);
    if let Some(spawn) = setup.spawn {
        app.set_spawn(spawn);
    }
    app.config = config;
    app.streaming = setup.streaming;
    app.seed = setup.seed;
    app.apply_render_distance();
    if let Some(options) = headless::options() {
        headless::run(app, options);
        return Ok(());
    }
    let event_loop = EventLoop::new()?;
    let result = event_loop.run_app(&mut app);
    if let Some(error) = app.error {
        return Err(error);
    }
    result.map_err(RunError::from)
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

/// Returns whether the in-memory stress scene replaces the saved world.
fn stress_requested() -> bool {
    std::env::var(STRESS_SCENE_ENV).is_ok_and(|value| value == "1")
}

/// Returns the chunk directory to load and save, or `None` in stress mode.
fn save_directory(stress: bool) -> Option<&'static Path> {
    if stress {
        None
    } else {
        Some(Path::new(WORLD_CHUNKS))
    }
}

/// Returns whether to start with vsync off, for benchmark runs.
fn no_vsync_requested() -> bool {
    std::env::var("ENGINE_NO_VSYNC").is_ok_and(|value| value == "1")
}

/// Seed from `ENGINE_SEED`, when set and valid.
fn generated_seed() -> Option<u64> {
    let value = std::env::var("ENGINE_SEED").ok()?;
    match value.trim().parse() {
        Ok(seed) => Some(seed),
        Err(error) => {
            warn!(%error, %value, "ignoring invalid ENGINE_SEED");
            None
        }
    }
}

/// Builds the spawn column and streaming state for a generated world.
#[allow(clippy::cast_precision_loss)]
fn generated_setup(
    seed: u64,
    render_distance: i32,
    directory: &Path,
) -> Result<WorldSetup, SaveError> {
    let generator = Generator::new(seed);
    let (spawn_x, spawn_z, spawn_height) = generator.spawn_column();
    let spawn = Vec3::new(
        spawn_x as f32 + 0.5,
        (spawn_height + 1) as f32,
        spawn_z as f32 + 0.5,
    );
    let column = Streaming::column_at(spawn);

    let mut world = World::default();
    for (chunk, blocks) in generator.generate_column(column) {
        insert_generated_chunk(&mut world, directory, chunk, blocks)?;
    }
    world.relight();

    let mut streaming = Streaming::new(render_distance);
    streaming.mark_loaded(column);
    streaming.mark_ready(column);

    info!(seed, ?spawn, "generated world ready");
    Ok(WorldSetup {
        world,
        spawn: Some(spawn),
        streaming: Some(streaming),
        seed: Some(seed),
    })
}

/// Inserts generated blocks unless a saved chunk overrides them.
fn insert_generated_chunk(
    world: &mut World,
    directory: &Path,
    chunk: ChunkCoord,
    generated: Chunk,
) -> Result<(), SaveError> {
    match World::load_saved_chunk(directory, chunk)? {
        Some(saved) if saved.is_empty() => {
            debug!(?chunk, "applied an empty saved chunk");
            world.mark_modified(chunk);
        }
        Some(saved) => {
            debug!(?chunk, blocks = saved.len(), "applied a saved chunk");
            world.insert_chunk(chunk, saved);
            world.mark_modified(chunk);
        }
        None => world.insert_chunk(chunk, generated),
    }
    Ok(())
}

/// Saves a column's modified chunks, then drops the column from the world and queue.
fn release_column(
    world: &mut World,
    queue: &mut MeshQueue,
    directory: &Path,
    column: IVec2,
) -> Result<(), SaveError> {
    for y in 0..CHUNK_COLUMN {
        world.save_chunk(directory, ChunkCoord::new(column.x, y, column.y))?;
    }
    queue.cancel_generation(column);
    for y in 0..CHUNK_COLUMN {
        let chunk = ChunkCoord::new(column.x, y, column.y);
        queue.remove(chunk);
        world.remove_chunk(chunk);
    }
    Ok(())
}

/// One startup world choice and its streaming state.
struct WorldSetup {
    world: World,
    spawn: Option<Vec3>,
    streaming: Option<Streaming>,
    seed: Option<u64>,
}

/// Loads the stress scene or a generated world.
///
/// Stress mode never touches `world_directory`. The returned spawn overrides the
/// default player position.
fn world_for(
    stress: bool,
    world_directory: &Path,
    render_distance: i32,
) -> Result<WorldSetup, SaveError> {
    if stress {
        let mut world = stress_world();
        world.relight();
        Ok(WorldSetup {
            world,
            spawn: None,
            streaming: None,
            seed: None,
        })
    } else {
        let seed = world::load_or_create_seed(world_directory, generated_seed())?;
        Ok(generated_setup(
            seed,
            render_distance,
            &world_directory.join("chunks"),
        )?)
    }
}

/// Builds the deterministic 8x8 chunk stress fixture.
///
/// Chunks sit at `(cx, 0, cz)` for `cx, cz` in `0..8`. The first four chunk
/// columns are flat at height two; the rest vary between two and five.
fn stress_world() -> World {
    let mut world = World::default();
    let extent = STRESS_CHUNKS * CHUNK_SIZE;
    for x in 0..extent {
        for z in 0..extent {
            let height = if x.div_euclid(CHUNK_SIZE) < STRESS_FLAT_CHUNKS {
                2
            } else {
                2 + (((x * 31) ^ (z * 17)) & 3)
            };
            for y in 0..=height {
                let block = if y == height {
                    Block::Grass
                } else if y == height - 1 {
                    Block::Dirt
                } else {
                    Block::Stone
                };
                world.set_block(IVec3::new(x, y, z), block);
            }
        }
    }
    world
}

/// Builds the demo world used by the tests.
#[cfg(test)]
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

#[allow(clippy::struct_excessive_bools)]
struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    mesher: Option<Arc<dyn MeshPool>>,
    queue: MeshQueue,
    config: Config,
    streaming: Option<Streaming>,
    seed: Option<u64>,
    save_directory: Option<PathBuf>,
    level_work: BTreeSet<ChunkCoord>,
    level_player_chunk: ChunkCoord,
    stats: FrameAccumulator,
    stress: bool,
    world: World,
    player: Player,
    camera: Camera,
    sky: Sky,
    unlit: bool,
    selected_block: Block,
    error: Option<RunError>,
    pressed: HashSet<KeyCode>,
    mouse_captured: bool,
    modifiers: ModifiersState,
    jump_requested: bool,
    highlighted_block: Option<IVec3>,
    focused: bool,
    occluded: bool,
    show_overlay: bool,
    horizon_cull: bool,
    horizon: horizon::Horizon,
    horizon_at: Instant,
    overlay_fps: f64,
    overlay_frame_ms: f64,
    last_frame: Instant,
    frame_accum: Duration,
    frame_count: u32,
}

/// Per-second accumulation of the v0.0.5 metrics.
#[derive(Clone, Copy, Debug, Default)]
struct FrameAccumulator {
    totals: FrameStats,
    rebuild_ms: f64,
    upload_ms: f64,
    last_rebuild_ms: f64,
    last_upload_ms: f64,
    pending_jobs: usize,
    generated_columns: u32,
    generation_ms: f64,
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

    fn with_world(world: World, stress: bool) -> Self {
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
            modifiers: ModifiersState::empty(),
            jump_requested: false,
            highlighted_block: None,
            focused: true,
            occluded: false,
            show_overlay: cfg!(debug_assertions),
            horizon_cull: std::env::var("ENGINE_HORIZON_CULL").is_ok_and(|value| value == "1"),
            horizon: horizon::Horizon::disabled(),
            horizon_at: Instant::now(),
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
            } else if !self.world.chunk(*chunk).is_some_and(Chunk::has_water)
                && let Some(renderer) = &mut self.renderer
            {
                // No water job will run for a dry chunk, so clear any water mesh
                // left over from before the last water block was removed.
                let revision = self.queue.revision(*chunk);
                renderer.upload_water_chunk(*chunk, revision, &mesh::MeshData::default());
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

    fn request_initial_meshes(&mut self) {
        let Some(pool) = self.mesher.clone() else {
            return;
        };
        for (chunk, data) in self.world.iter_chunks() {
            self.queue.request(
                &*pool,
                &self.world,
                chunk,
                Lod::Lod0,
                MeshKind::Opaque,
                Priority::Urgent,
            );
            if data.has_water() {
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
    }

    /// Recalculates every chunk's LOD and requests any missing mesh.
    ///
    /// Only chunks with outstanding level work are visited, in chunk order, and
    /// at most [`LEVEL_WORK_BUDGET`] of main-thread time is spent per frame. A
    /// chunk is dropped from the set while it waits for a job result; accepting
    /// the result re-inserts it, so the set stays a queue of actionable work
    /// rather than a set of blocked chunks.
    fn request_levels(&mut self, pool: &dyn MeshPool) {
        profiling::scope!("mesh request");
        let feet = self.player.position.floor().as_ivec3();
        let player_chunk = ChunkCoord::from_world(feet).0;
        if player_chunk != self.level_player_chunk {
            self.level_player_chunk = player_chunk;
            self.level_work
                .extend(self.world.iter_chunks().map(|(chunk, _)| chunk));
        }

        let started = Instant::now();
        while let Some(chunk) = self.level_work.pop_first() {
            let chunk_started = Instant::now();
            if let Some(data) = self.world.chunk(chunk) {
                let has_water = data.has_water();
                let mut retry = false;
                if self.queue.light_is_current(chunk) {
                    let lod = self.queue.desired_lod(chunk, player_chunk);
                    if !self.queue.is_built(chunk, lod, MeshKind::Opaque)
                        && !self.queue.is_pending(chunk, lod, MeshKind::Opaque)
                    {
                        self.queue.request(
                            pool,
                            &self.world,
                            chunk,
                            lod,
                            MeshKind::Opaque,
                            Priority::Background,
                        );
                        retry = !self.queue.is_pending(chunk, lod, MeshKind::Opaque);
                    }

                    if has_water
                        && !self.queue.is_built(chunk, Lod::Lod0, MeshKind::Water)
                        && !self.queue.is_pending(chunk, Lod::Lod0, MeshKind::Water)
                    {
                        self.queue.request(
                            pool,
                            &self.world,
                            chunk,
                            Lod::Lod0,
                            MeshKind::Water,
                            Priority::Background,
                        );
                        retry = retry || !self.queue.is_pending(chunk, Lod::Lod0, MeshKind::Water);
                    }
                } else if !self.queue.light_pending(chunk) {
                    self.queue
                        .request_light(pool, &self.world, chunk, Priority::Background);
                    retry = !self.queue.light_pending(chunk);
                }
                if retry {
                    self.level_work.insert(chunk);
                }
            }
            let took = chunk_started.elapsed();
            if took.as_millis() >= 2 {
                debug!(
                    chunk = ?chunk,
                    ms = took.as_secs_f64() * 1000.0,
                    "slow level request"
                );
            }
            if started.elapsed() >= LEVEL_WORK_BUDGET {
                break;
            }
        }
        self.stats.pending_jobs = self.queue.pending_len();
    }

    fn upload_finished_meshes(&mut self, pool: &dyn MeshPool) {
        profiling::scope!("mesh upload");
        let (results, lights, generations, rebuild) =
            collect_finished(&mut self.queue, pool, MAX_UPLOADS_PER_FRAME);
        let rebuild_ms = rebuild.as_secs_f64() * 1000.0;
        self.stats.rebuild_ms += rebuild_ms;
        self.stats.last_rebuild_ms = rebuild_ms;

        for light in lights {
            self.level_work.insert(light.chunk);
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
            let desired = self.queue.desired(result.chunk);
            let uploaded = match result.kind {
                MeshKind::Opaque => {
                    let uploaded = renderer.upload_chunk(
                        result.chunk,
                        result.lod,
                        result.revision,
                        &result.mesh,
                    );
                    if uploaded && let Some(lod) = desired {
                        renderer.select_active(result.chunk, lod);
                    }
                    uploaded
                }
                MeshKind::Water => {
                    renderer.upload_water_chunk(result.chunk, result.revision, &result.mesh)
                }
            };
            if !uploaded {
                self.queue
                    .forget_built(result.chunk, result.lod, result.kind);
            }
            self.level_work.insert(result.chunk);
            upload_ms += started.elapsed().as_secs_f64() * 1000.0;
        }
        self.stats.upload_ms += upload_ms;
        self.stats.last_upload_ms = upload_ms;
    }

    fn target_hit(&self) -> Option<RaycastHit> {
        raycast(
            &self.world,
            self.player.eye_position(),
            self.camera.forward(),
            INTERACTION_DISTANCE,
        )
    }

    fn break_block(&mut self) {
        let Some(hit) = self.target_hit() else {
            return;
        };
        let dirty = self.world.set_block(hit.block, Block::Air);
        debug!(block = ?hit.block, "broke block");
        self.invalidate_chunks(&dirty);
    }

    fn place_block(&mut self) {
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

    fn save_world(&self) {
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
    fn redraw(&mut self) {
        if self.occluded {
            return;
        }

        profiling::scope!("frame");

        let now = Instant::now();
        let elapsed = now.saturating_duration_since(self.last_frame);
        self.last_frame = now;

        if self.focused {
            self.record_frame(elapsed);

            let delta_seconds = elapsed.min(MAX_FRAME_DELTA).as_secs_f32();
            let forward_axis = axis(
                self.pressed.contains(&KeyCode::KeyW),
                self.pressed.contains(&KeyCode::KeyS),
            );
            let right_axis = axis(
                self.pressed.contains(&KeyCode::KeyD),
                self.pressed.contains(&KeyCode::KeyA),
            );
            let input = PlayerInput {
                forward: forward_axis,
                right: right_axis,
                jump: std::mem::take(&mut self.jump_requested),
                yaw: self.camera.yaw(),
            };
            {
                profiling::scope!("player update");
                let streaming = self.streaming.as_ref();
                let ready = |position: Vec3| {
                    streaming.is_none_or(|streaming| streaming.aabb_ready(position))
                };
                self.player
                    .update_guarded(&self.world, input, delta_seconds, &ready);
            }
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
            if slow3.saturating_duration_since(slow0).as_millis() > 5 {
                let ms =
                    |a: Instant, b: Instant| b.saturating_duration_since(a).as_secs_f64() * 1000.0;
                debug!(
                    stream = ms(slow0, slow1),
                    levels = ms(slow1, slow2),
                    upload = ms(slow2, slow3),
                    level_backlog = self.level_work.len(),
                    "slow phases"
                );
            }
        }

        let overlay = self.show_overlay.then(|| {
            format!(
                "FPS {:.0}  {:.2} MS",
                self.overlay_fps, self.overlay_frame_ms
            )
        });
        let frustum = {
            profiling::scope!("frustum");
            Frustum::from_view_projection(self.camera.view_projection())
        };
        let slow_render = Instant::now();
        if let Some(renderer) = &mut self.renderer {
            renderer.set_camera(&self.camera);
            renderer.set_sky(&self.sky.sample(), self.unlit);
            renderer.set_overlay(overlay.as_deref());
            if self.horizon_cull && self.horizon_at.elapsed() >= Duration::from_millis(250) {
                self.horizon = horizon::Horizon::build(self.camera.position(), &self.world);
                self.horizon_at = Instant::now();
            }
            self.stats.totals = renderer.render(&frustum, &self.horizon);
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
        metrics::geometry_bytes(self.stats.totals.geometry_bytes as f64);
        metrics::visible_geometry_bytes(self.stats.totals.visible_bytes as f64);
        metrics::rebuild_ms(self.stats.last_rebuild_ms);
        metrics::upload_ms(self.stats.last_upload_ms);
        metrics::mesh_jobs(self.stats.pending_jobs as f64);
        metrics::generated_columns(f64::from(self.stats.generated_columns));
        metrics::generation_ms(self.stats.generation_ms);
        profiling::finish_frame!();

        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    #[allow(clippy::cast_precision_loss)]
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
                geometry_mb = self.stats.totals.geometry_bytes as f64 / (1024.0 * 1024.0),
                visible_geometry_mb = self.stats.totals.visible_bytes as f64 / (1024.0 * 1024.0),
                rebuild_ms = self.stats.rebuild_ms,
                upload_ms = self.stats.upload_ms,
                generated_columns = self.stats.generated_columns,
                generation_ms = self.stats.generation_ms,
                player = ?self.player.position,
                on_ground = self.player.on_ground,
                "frame timing"
            );
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
                if code == KeyCode::KeyS && self.modifiers.control_key() {
                    self.save_world();
                } else if code == KeyCode::Space {
                    self.jump_requested = true;
                } else if code == KeyCode::Escape {
                    self.release_cursor();
                } else if code == KeyCode::KeyV {
                    self.toggle_vsync();
                } else if code == KeyCode::F3 {
                    self.toggle_overlay();
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
    fn apply_render_distance(&mut self) {
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
    fn stream_columns(&mut self, pool: &dyn MeshPool) {
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

    fn set_spawn(&mut self, position: Vec3) {
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
        let renderer =
            match pollster::block_on(Renderer::new(window.clone(), instance, &self.camera)) {
                Ok(renderer) => renderer,
                Err(error) => {
                    self.fail(RunError::Renderer(error), event_loop);
                    return;
                }
            };

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
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
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

    use super::*;
    use crate::generation::GenResult;
    use mesher::{Job, JobResult, MeshResult};

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
    fn stress_scene_is_an_eight_by_eight_grid_at_chunk_y_zero() {
        let world = stress_world();
        let chunks: Vec<_> = world.iter_chunks().map(|(chunk, _)| chunk).collect();

        assert_eq!(chunks.len(), 64);
        for chunk in chunks {
            let coordinates = chunk.coordinates();
            assert_eq!(coordinates.y, 0);
            assert!((0..STRESS_CHUNKS).contains(&coordinates.x));
            assert!((0..STRESS_CHUNKS).contains(&coordinates.z));
        }
    }

    #[test]
    fn stress_terrain_matches_the_formula_for_every_column() {
        let world = stress_world();
        let extent = STRESS_CHUNKS * CHUNK_SIZE;

        for x in 0..extent {
            for z in 0..extent {
                let expected = if x.div_euclid(CHUNK_SIZE) < STRESS_FLAT_CHUNKS {
                    2
                } else {
                    2 + (((x * 31) ^ (z * 17)) & 3)
                };
                assert_eq!(
                    world.block(IVec3::new(x, expected, z)),
                    Block::Grass,
                    "{x},{z}"
                );
                assert_eq!(world.block(IVec3::new(x, expected - 1, z)), Block::Dirt);
                assert_eq!(world.block(IVec3::new(x, expected + 1, z)), Block::Air);
            }
        }
    }

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

    #[test]
    fn unloading_saves_edited_chunks_and_drops_the_column() {
        let directory = temp_directory("unload");
        let chunk = ChunkCoord::new(0, 0, 0);
        let coordinate = IVec3::new(5, 0, 5);
        let mut world = World::default();
        world.set_block(coordinate, Block::Stone);
        let mut queue = MeshQueue::default();

        release_column(&mut world, &mut queue, &directory, IVec2::ZERO).unwrap();

        assert!(world.chunk(chunk).is_none());
        let saved = World::load_saved_chunk(&directory, chunk).unwrap().unwrap();
        assert_eq!(saved.block(coordinate), Block::Stone);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
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
    fn saved_chunks_override_generated_blocks() {
        let directory = temp_directory("apply-saved");
        let chunk = ChunkCoord::new(0, 0, 0);
        let coordinate = IVec3::new(5, 0, 5);
        let mut saved = World::default();
        saved.set_block(coordinate, Block::Stone);
        saved.save_chunks(&directory).unwrap();
        let mut generated = Chunk::default();
        generated.set_block(coordinate, Block::Grass);

        let mut world = World::default();
        insert_generated_chunk(&mut world, &directory, chunk, generated).unwrap();

        assert_eq!(world.block(coordinate), Block::Stone);
        assert!(world.is_modified(chunk));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn an_empty_saved_chunk_clears_generated_blocks() {
        let directory = temp_directory("apply-empty");
        let chunk = ChunkCoord::new(0, 0, 0);
        let coordinate = IVec3::new(5, 0, 5);
        let mut saved = World::default();
        saved.set_block(coordinate, Block::Stone);
        saved.save_chunks(&directory).unwrap();
        saved.set_block(coordinate, Block::Air);
        saved.save_chunks(&directory).unwrap();
        let mut generated = Chunk::default();
        generated.set_block(coordinate, Block::Grass);

        let mut world = World::default();
        insert_generated_chunk(&mut world, &directory, chunk, generated).unwrap();

        assert_eq!(world.block(coordinate), Block::Air);
        assert!(world.chunk(chunk).is_none());
        assert!(world.is_modified(chunk));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn generated_setup_applies_saved_edits_to_the_spawn_column() {
        let directory = temp_directory("setup-saved");
        let seed = 42;
        let generator = Generator::new(seed);
        let (spawn_x, spawn_z, _) = generator.spawn_column();
        let column =
            Streaming::column_at(Vec3::new(spawn_x as f32 + 0.5, 40.0, spawn_z as f32 + 0.5));
        let chunk = ChunkCoord::new(column.x, 0, column.y);
        let coordinate = chunk.world_origin() + IVec3::new(1, 0, 1);
        let mut saved = World::default();
        saved.set_block(coordinate, Block::Wood);
        saved.save_chunks(&directory).unwrap();

        let setup = generated_setup(seed, 8, &directory).unwrap();

        assert_eq!(setup.world.block(coordinate), Block::Wood);
        assert!(setup.world.is_modified(chunk));
        assert_eq!(setup.seed, Some(seed));
        assert!(setup.streaming.is_some());
        fs::remove_dir_all(directory).unwrap();
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
    fn stress_mode_ignores_the_world_directory() {
        let world_directory = temp_directory("stress-world");
        let chunks = world_directory.join("chunks");
        fs::create_dir_all(&chunks).unwrap();
        fs::write(chunks.join("chunk_bad.vchk"), []).unwrap();

        assert!(world_for(true, &world_directory, 12).is_ok());
        assert!(matches!(
            world_for(false, &world_directory, 12),
            Err(SaveError::MissingSeed)
        ));
        assert!(!world_directory.join("seed").exists());
        assert!(save_directory(true).is_none());
        assert_eq!(save_directory(false), Some(Path::new(WORLD_CHUNKS)));

        fs::remove_dir_all(world_directory).unwrap();
    }

    #[test]
    fn stress_mode_spawns_on_the_flat_half_facing_the_uneven_half() {
        let app = App::with_world(stress_world(), true);

        assert_eq!(app.player.position, Vec3::new(16.5, 3.0, 16.5));
        assert!((app.camera.yaw() - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
        assert_eq!(app.world.block(IVec3::new(16, 2, 16)), Block::Grass);
    }

    #[test]
    fn player_stands_on_the_stress_ground() {
        let world = stress_world();
        let mut player = Player::new(Vec3::new(16.5, 3.0, 16.5));
        let input = PlayerInput {
            forward: 0.0,
            right: 0.0,
            jump: false,
            yaw: std::f32::consts::FRAC_PI_4,
        };

        for _ in 0..240 {
            player.update(&world, input, 1.0 / 60.0);
        }

        assert!(player.on_ground);
        assert!(
            (player.position.y - 3.0).abs() < 1e-3,
            "{:?}",
            player.position
        );
    }

    #[test]
    fn the_stress_spawn_frustum_contains_the_nearby_chunks() {
        let app = App::with_world(stress_world(), true);
        let frustum = Frustum::from_view_projection(app.camera.view_projection());

        let inside = ChunkCoord::from_world(IVec3::new(16, 0, 16)).0.origin();
        assert!(frustum.intersects_aabb(inside, inside + Vec3::splat(32.0)));

        let behind = ChunkCoord::from_world(IVec3::new(16, 0, 240)).0.origin();
        assert!(!frustum.intersects_aabb(behind, behind + Vec3::splat(32.0)));
    }

    #[test]
    fn stress_scene_meshes_every_chunk_with_fewer_vertices_than_naive() {
        let world = stress_world();
        let mut greedy = 0;
        let mut naive = 0;

        for (chunk, _) in world.iter_chunks() {
            let snapshot = mesh::ChunkSnapshot::new(&world, chunk);
            let border = mesh::BorderBlocks::new(&world, chunk);
            let lod0 = mesh::build_mesh(&snapshot, &border, None, Lod::Lod0);
            let lod1 = mesh::build_mesh(&snapshot, &border, None, Lod::Lod1);

            assert_ne!(lod0.vertices.len(), 0);
            assert_ne!(lod1.vertices.len(), 0);
            greedy += lod0.vertices.len();
            naive += voxel::build_chunk_mesh(&world, chunk).len();
        }

        assert!(greedy < naive, "greedy {greedy} naive {naive}");
    }

    #[test]
    #[ignore = "benchmark; run with cargo test --release --lib bench_hot_paths -- --ignored --nocapture"]
    #[allow(clippy::cast_precision_loss)]
    fn bench_hot_paths() {
        let mut world = stress_world();
        let chunks: Vec<_> = world.iter_chunks().map(|(chunk, _)| chunk).collect();
        let chunk_count = chunks.len();

        let started = Instant::now();
        world.relight();
        let relight = started.elapsed();

        let started = Instant::now();
        let mut snapshots = Vec::with_capacity(chunk_count);
        for &chunk in &chunks {
            snapshots.push((
                mesh::ChunkSnapshot::new(&world, chunk),
                mesh::BorderBlocks::new(&world, chunk),
                world.light_field(chunk).cloned(),
            ));
        }
        let prepare = started.elapsed();

        let started = Instant::now();
        let lod0_vertices: usize = snapshots
            .iter()
            .map(|(snapshot, border, light)| {
                mesh::build_mesh(snapshot, border, light.as_deref(), Lod::Lod0)
                    .vertices
                    .len()
            })
            .sum();
        let lod0 = started.elapsed();

        let started = Instant::now();
        let lod1_vertices: usize = snapshots
            .iter()
            .map(|(snapshot, border, light)| {
                mesh::build_mesh(snapshot, border, light.as_deref(), Lod::Lod1)
                    .vertices
                    .len()
            })
            .sum();
        let lod1 = started.elapsed();

        let mut player = Player::new(Vec3::new(16.5, 3.0, 16.5));
        let input = PlayerInput {
            forward: 0.0,
            right: 1.0,
            jump: false,
            yaw: std::f32::consts::FRAC_PI_4,
        };
        let frames = 600_u32;
        let started = Instant::now();
        for _ in 0..frames {
            player.update(&world, input, 1.0 / 60.0);
        }
        let update = started.elapsed();

        let per_chunk = |elapsed: Duration| elapsed.as_secs_f64() * 1000.0 / chunk_count as f64;
        eprintln!(
            "chunks={chunk_count} prepare_ms/chunk={:.3} lod0_ms/chunk={:.3} lod1_ms/chunk={:.3} lod0_vertices={lod0_vertices} lod1_vertices={lod1_vertices} update_us/frame={:.1} relight_ms/chunk={:.3}",
            elapsed_ms(prepare, chunk_count),
            per_chunk(lod0),
            per_chunk(lod1),
            update.as_secs_f64() * 1e6 / f64::from(frames),
            per_chunk(relight),
        );
    }

    #[allow(clippy::cast_precision_loss)]
    fn elapsed_ms(elapsed: Duration, count: usize) -> f64 {
        elapsed.as_secs_f64() * 1000.0 / count as f64
    }
}

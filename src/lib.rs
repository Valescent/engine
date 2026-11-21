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
mod hierarchy;
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
mod section;
mod sky;
mod streaming;
mod voxel;
mod world;

#[cfg(test)]
mod bench;

use std::collections::{BTreeSet, HashMap, HashSet};
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
use hierarchy::{DEFAULT_DETAIL_CHUNKS, DEFAULT_PIXELS_PER_RADIAN, Hierarchy};
use mesh::Lod;
use mesher::{
    Job, MeshKind, MeshPool, MeshQueue, Priority, SectionJob, SectionResult, build_section,
    collect_finished, create_pool,
};
use player::{Player, PlayerInput};
use raycast::{RaycastHit, raycast};
pub use renderer::RendererError;
use renderer::{FrameStats, Renderer};
use section::{SectionBounds, SectionKey, overlapping_sections_for_chunk};
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
const MAX_SECTION_BUILDS_PER_FRAME: usize = 1;
/// Coarse section jobs kept in flight on the shared worker pool.
const MAX_SECTION_JOBS: usize = 11;
const INTERACTION_DISTANCE: f32 = 6.0;
const SPAWN_X: i32 = 32;
const SPAWN_Z: i32 = 32;
/// Mesh uploads collected per frame. `2` made distance-32 first-load settle in 33 s against 22 s at
/// `64`, so above this value the cap stops limiting; `8` stays above the worker rate while keeping
/// each frame's upload burst small.
const MAX_UPLOADS_PER_FRAME: usize = 8;
/// Consecutive upload failures tolerated for one chunk before it is left alone.
const MAX_UPLOAD_RETRIES: u8 = 8;

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

    let mut config = config::load(Path::new(CONFIG_FILE))?;
    let stress = stress_requested();
    let hierarchy = std::env::var("ENGINE_HIERARCHY").is_ok_and(|value| value == "1");
    let detail_chunks = std::env::var("ENGINE_DETAIL_CHUNKS")
        .ok()
        .and_then(|value| value.trim().parse::<i32>().ok())
        .unwrap_or(DEFAULT_DETAIL_CHUNKS)
        .clamp(4, 32);
    if hierarchy {
        config.render_distance_chunks = detail_chunks;
    }
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
    app.detail_chunks = detail_chunks;
    app.streaming = setup.streaming;
    app.seed = setup.seed;
    if let Some(directory) = app.save_directory.clone() {
        app.saved_chunks = World::saved_chunk_index(&directory)?;
    }
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
        insert_generated_chunk(&mut world, directory, None, chunk, blocks)?;
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
///
/// `saved` is the startup listing of chunk files. When it is present and does
/// not contain `chunk`, the generated blocks are used without probing the
/// filesystem.
fn insert_generated_chunk(
    world: &mut World,
    directory: &Path,
    saved: Option<&HashSet<ChunkCoord>>,
    chunk: ChunkCoord,
    generated: Chunk,
) -> Result<(), SaveError> {
    if saved.is_some_and(|saved| !saved.contains(&chunk)) {
        world.insert_chunk(chunk, generated);
        return Ok(());
    }
    match World::load_saved_chunk(directory, chunk)? {
        Some(saved) if saved.is_empty() => {
            debug!(?chunk, "applied an empty saved chunk");
        }
        Some(saved) => {
            debug!(?chunk, blocks = saved.len(), "applied a saved chunk");
            world.insert_chunk(chunk, saved);
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
    saved: &mut HashSet<ChunkCoord>,
    column: IVec2,
) -> Result<(), SaveError> {
    for y in 0..CHUNK_COLUMN {
        let chunk = ChunkCoord::new(column.x, y, column.y);
        if world.save_chunk(directory, chunk)? {
            saved.insert(chunk);
        }
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
        for (local, block) in data.iter() {
            if !block.is_solid() {
                continue;
            }
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
    /// Chunk coordinates with a file in `save_directory`, listed at startup.
    saved_chunks: HashSet<ChunkCoord>,
    level_work: BTreeSet<ChunkCoord>,
    level_player_chunk: ChunkCoord,
    /// Consecutive mesh upload failures per chunk, cleared on success.
    upload_failures: HashMap<ChunkCoord, u8>,
    /// Chunks whose light waits for an adjacent column that is still streaming in.
    deferred_light: BTreeSet<ChunkCoord>,
    /// Cached generation candidates, rebuilt when the player column or loaded set changes.
    wanted_columns: Vec<IVec2>,
    unload_columns: Vec<IVec2>,
    wanted_cursor: usize,
    wanted_player: Option<IVec2>,
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
    auto_walk: bool,
    auto_strafe: bool,
    hierarchy: bool,
    detail_chunks: i32,
    /// Parent-retained target, readiness, and build queue.
    sections: Hierarchy,
    section_cursor: usize,
    sections_uploaded: HashSet<SectionKey>,
    sections_in_flight: HashSet<SectionKey>,
    /// Job id expected for each in-flight section build.
    section_jobs: HashMap<SectionKey, u64>,
    next_section_job: u64,
    section_bounds: HashMap<SectionKey, SectionBounds>,
    hierarchy_at: Instant,
    hierarchy_cell: Option<IVec2>,
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
    last_stream_ms: f64,
    last_levels_ms: f64,
    last_render_ms: f64,
    pending_jobs: usize,
    generated_columns: u32,
    generation_ms: f64,
}

impl FrameAccumulator {
    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Column that owns `chunk`.
fn chunk_column(chunk: ChunkCoord) -> IVec2 {
    let coordinates = chunk.coordinates();
    IVec2::new(coordinates.x, coordinates.z)
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
        if let Some(yaw) = std::env::var("ENGINE_YAW")
            .ok()
            .and_then(|value| value.trim().parse::<f32>().ok())
        {
            camera.set_yaw(yaw.to_radians());
        }
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
            saved_chunks: HashSet::new(),
            level_work: world.iter_chunks().map(|(chunk, _)| chunk).collect(),
            level_player_chunk: ChunkCoord::from_world(spawn.floor().as_ivec3()).0,
            upload_failures: HashMap::new(),
            deferred_light: BTreeSet::new(),
            wanted_columns: Vec::new(),
            unload_columns: Vec::new(),
            wanted_cursor: 0,
            wanted_player: None,
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
            horizon_cull: !std::env::var("ENGINE_HORIZON_CULL").is_ok_and(|value| value == "0"),
            auto_walk: std::env::var("ENGINE_AUTO_WALK").is_ok_and(|value| value == "1"),
            auto_strafe: std::env::var("ENGINE_AUTO_STRAFE").is_ok_and(|value| value == "1"),
            hierarchy: std::env::var("ENGINE_HIERARCHY").is_ok_and(|value| value == "1"),
            detail_chunks: DEFAULT_DETAIL_CHUNKS,
            sections: Hierarchy::default(),
            section_cursor: 0,
            sections_uploaded: HashSet::new(),
            sections_in_flight: HashSet::new(),
            section_jobs: HashMap::new(),
            next_section_job: 0,
            section_bounds: HashMap::new(),
            hierarchy_at: Instant::now(),
            hierarchy_cell: None,
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

    /// Turns the section hierarchy off and drops its pending work.
    ///
    /// The fallback chunk renderer cannot draw sections, so a hierarchy that
    /// cannot upload would mark every section dirty forever.
    fn disable_hierarchy(&mut self, reason: &str) {
        warn!(%reason, "hierarchy disabled; rendering chunks only");
        self.hierarchy = false;
        self.sections = Hierarchy::default();
        self.section_cursor = 0;
        self.sections_uploaded.clear();
        self.sections_in_flight.clear();
        self.section_jobs.clear();
        self.section_bounds.clear();
        self.hierarchy_cell = None;
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
                self.deferred_light.remove(chunk);
                self.queue
                    .request_light(&*pool, &self.world, *chunk, Priority::Urgent);
            }
        }
        self.invalidate_sections(dirty);
        for chunk in dirty {
            if self.world.chunk(*chunk).is_none() {
                self.queue.remove(*chunk);
                self.upload_failures.remove(chunk);
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

    /// Marks the coarse sections overlapping an edit for a rebuild.
    ///
    /// The renderer keeps no section state after [`Renderer::remove_section`],
    /// so the next upload replaces the stale geometry instead of doubling it.
    /// Sections outside the current target stay marked but are not built until
    /// selection reaches them, so a far edit is harmless.
    fn invalidate_sections(&mut self, dirty: &DirtyChunks) {
        let mut keys = Vec::new();
        for chunk in dirty {
            for key in overlapping_sections_for_chunk(*chunk) {
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }
        if keys.is_empty() {
            return;
        }
        if let Some(renderer) = &mut self.renderer {
            for key in &keys {
                renderer.remove_section(*key);
            }
        }
        for key in &keys {
            self.sections_uploaded.remove(key);
            self.section_bounds.remove(key);
            self.section_jobs.remove(key);
            self.sections_in_flight.remove(key);
        }
        self.sections.mark_dirty(keys);
    }

    /// Invalidates the light and meshes of the eight columns around `column`.
    ///
    /// A column appearing or leaving changes its neighbours' sky exposure, so
    /// their light fields are stale even though their own blocks did not change.
    fn invalidate_neighbour_columns(&mut self, column: IVec2) {
        let neighbours = self.world.neighbour_column_chunks(column);
        self.queue.invalidate(&neighbours);
        self.level_work.extend(neighbours.iter().copied());
    }

    /// Whether the streamer may still generate this column for the player.
    fn column_is_wanted(&self, column: IVec2) -> bool {
        let Some(streaming) = &self.streaming else {
            return false;
        };
        let player_column = Streaming::column_at(self.player.position);
        let delta = (column - player_column).abs();
        delta.x.max(delta.y) <= streaming.render_distance()
    }

    /// Whether every adjacent column is loaded or will never load.
    ///
    /// Light treats a missing chunk as open sky. While an adjacent column is
    /// still streaming in, that is temporary, so computing light now produces a
    /// bright band that must be recomputed later. Columns outside the wanted
    /// area will never arrive, so their open sky is final.
    fn light_neighbourhood_stable(&self, column: IVec2) -> bool {
        let Some(streaming) = &self.streaming else {
            return true;
        };
        for dx in -1..=1 {
            for dz in -1..=1 {
                if dx == 0 && dz == 0 {
                    continue;
                }
                let neighbour = column + IVec2::new(dx, dz);
                if !streaming.is_loaded(neighbour) && self.column_is_wanted(neighbour) {
                    return false;
                }
            }
        }
        true
    }

    /// Moves deferred light work back into the level pass once it can be final.
    fn admit_deferred_light(&mut self) {
        if self.deferred_light.is_empty() {
            return;
        }
        let ready: Vec<ChunkCoord> = self
            .deferred_light
            .iter()
            .copied()
            .filter(|chunk| self.light_neighbourhood_stable(chunk_column(*chunk)))
            .collect();
        for chunk in &ready {
            self.deferred_light.remove(chunk);
        }
        self.level_work.extend(ready);
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
            self.admit_deferred_light();
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
                    if self.light_neighbourhood_stable(chunk_column(chunk)) {
                        self.queue
                            .request_light(pool, &self.world, chunk, Priority::Background);
                        retry = !self.queue.light_pending(chunk);
                    } else {
                        self.deferred_light.insert(chunk);
                    }
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

    fn insert_generated(
        &mut self,
        directory: &Path,
        chunk: ChunkCoord,
        blocks: Chunk,
    ) -> Result<(), SaveError> {
        insert_generated_chunk(
            &mut self.world,
            directory,
            Some(&self.saved_chunks),
            chunk,
            blocks,
        )
    }

    fn upload_finished_meshes(&mut self, pool: &dyn MeshPool) {
        profiling::scope!("mesh upload");
        let finished = collect_finished(&mut self.queue, pool, MAX_UPLOADS_PER_FRAME);
        let rebuild_ms = finished.worker_time.as_secs_f64() * 1000.0;
        self.stats.rebuild_ms += rebuild_ms;
        self.stats.last_rebuild_ms = rebuild_ms;

        for light in finished.lights {
            self.level_work.insert(light.chunk);
            self.world.set_light_field(light.chunk, light.light);
        }

        let chunk_directory = save_directory(self.stress);
        let had_generations = !finished.generations.is_empty();
        for generation in finished.generations {
            self.stats.generated_columns += 1;
            self.stats.generation_ms += generation.elapsed.as_secs_f64() * 1000.0;
            let mut loaded = true;
            for (chunk, blocks) in generation.chunks {
                let saved = if let Some(directory) = chunk_directory {
                    self.insert_generated(directory, chunk, blocks)
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
            if loaded {
                if let Some(streaming) = &mut self.streaming {
                    streaming.mark_loaded(generation.column);
                }
                self.invalidate_neighbour_columns(generation.column);
                self.mark_column_ready(generation.column);
            } else {
                // Retry this column from a rebuilt candidate list next frame.
                self.wanted_player = None;
            }
        }
        if had_generations {
            self.admit_deferred_light();
        }
        for result in &finished.meshes {
            self.mark_column_ready(chunk_column(result.chunk));
        }

        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let mut upload_ms = 0.0;
        for result in &finished.meshes {
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
                    if uploaded {
                        // Prefer the mesh the level pass wants; otherwise keep the
                        // LOD that just landed so the upload is not wasted.
                        let target = desired
                            .filter(|&lod| renderer.has_lod(result.chunk, lod))
                            .unwrap_or(result.lod);
                        renderer.select_active(result.chunk, target);
                        forget_inactive_lods(&mut self.queue, renderer, result.chunk);
                    }
                    uploaded
                }
                MeshKind::Water => {
                    renderer.upload_water_chunk(result.chunk, result.revision, &result.mesh)
                }
            };
            if uploaded {
                self.upload_failures.remove(&result.chunk);
                self.level_work.insert(result.chunk);
            } else {
                self.queue
                    .forget_built(result.chunk, result.lod, result.kind);
                let count = self.upload_failures.entry(result.chunk).or_insert(0);
                *count = count.saturating_add(1);
                let count = *count;
                if count <= MAX_UPLOAD_RETRIES {
                    self.level_work.insert(result.chunk);
                } else if count == MAX_UPLOAD_RETRIES + 1 {
                    warn!(
                        chunk = ?result.chunk,
                        count,
                        "mesh upload failed repeatedly; no longer retrying"
                    );
                }
            }
            upload_ms += started.elapsed().as_secs_f64() * 1000.0;
        }
        self.stats.upload_ms += upload_ms;
        self.stats.last_upload_ms = upload_ms;
        for result in &finished.sections {
            self.upload_section_result(result);
        }
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

    fn save_world(&mut self) {
        let Some(directory) = &self.save_directory else {
            debug!("stress scene is not saved");
            return;
        };
        match self.world.save_chunks(directory) {
            Ok(()) => {
                self.saved_chunks.extend(self.world.modified_chunks());
                self.world.clear_modified();
                debug!("world saved");
            }
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

    fn move_axes(&self) -> (f32, f32) {
        let forward = if self.auto_walk {
            1.0
        } else {
            axis(
                self.pressed.contains(&KeyCode::KeyW),
                self.pressed.contains(&KeyCode::KeyS),
            )
        };
        let right = if self.auto_strafe {
            1.0
        } else {
            axis(
                self.pressed.contains(&KeyCode::KeyD),
                self.pressed.contains(&KeyCode::KeyA),
            )
        };
        (forward, right)
    }

    #[allow(clippy::cast_precision_loss, clippy::too_many_lines)]
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

            let (delta_seconds, input) = {
                profiling::scope!("input");
                let (forward_axis, right_axis) = self.move_axes();
                (
                    elapsed.min(MAX_FRAME_DELTA).as_secs_f32(),
                    PlayerInput {
                        forward: forward_axis,
                        right: right_axis,
                        jump: std::mem::take(&mut self.jump_requested),
                        yaw: self.camera.yaw(),
                    },
                )
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
            {
                profiling::scope!("camera update");
                self.sky.advance(delta_seconds);
                self.camera.set_position(self.player.eye_position());
                self.update_highlight();
            }
        }

        let slow0 = Instant::now();
        if let Some(pool) = self.mesher.clone() {
            self.stream_columns(&*pool);
            let slow1 = Instant::now();
            self.request_levels(&*pool);
            let slow2 = Instant::now();
            self.upload_finished_meshes(&*pool);
            let slow3 = Instant::now();
            self.stats.last_stream_ms = slow1.duration_since(slow0).as_secs_f64() * 1000.0;
            self.stats.last_levels_ms = slow2.duration_since(slow1).as_secs_f64() * 1000.0;
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

        if self.hierarchy {
            let pool = self.mesher.clone();
            self.ensure_hierarchy(pool.as_deref());
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
        if self.horizon_cull && self.horizon_at.elapsed() >= Duration::from_millis(250) {
            self.rebuild_horizon();
        }
        let slow_render = Instant::now();
        if let Some(renderer) = &mut self.renderer {
            renderer.set_camera(&self.camera);
            renderer.set_sky(&self.sky.sample(), self.unlit);
            renderer.set_overlay(overlay.as_deref());
            self.stats.totals = renderer.render(&frustum, &self.horizon);
        }
        let render_ms = slow_render.elapsed().as_secs_f64() * 1000.0;
        self.stats.last_render_ms = render_ms;
        if render_ms > 5.0 {
            debug!(render_ms, "slow render");
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
        profiling::scope!("record frame");
        self.frame_accum += delta;
        self.frame_count += 1;
        if self.frame_accum >= FRAME_LOG_INTERVAL {
            let seconds = self.frame_accum.as_secs_f64();
            let frames = f64::from(self.frame_count);
            debug!(
                fps = frames / seconds,
                frame_ms = seconds * 1000.0 / frames,
                visible_chunks = self.stats.totals.visible_chunks,
                horizon_culled = self.stats.totals.horizon_culled,
                sections_in_flight = self.sections_in_flight.len(),
                total_chunks = self.stats.totals.total_chunks,
                visible_vertices = self.stats.totals.visible_vertices,
                draw_calls = self.stats.totals.draw_calls,
                geometry_mb = self.stats.totals.geometry_bytes as f64 / (1024.0 * 1024.0),
                visible_geometry_mb = self.stats.totals.visible_bytes as f64 / (1024.0 * 1024.0),
                allocated_geometry_mb =
                    self.stats.totals.allocated_bytes as f64 / (1024.0 * 1024.0),
                peak_geometry_mb =
                    self.stats.totals.peak_allocated_bytes as f64 / (1024.0 * 1024.0),
                rebuild_ms = self.stats.rebuild_ms,
                upload_ms = self.stats.upload_ms,
                stream_ms = self.stats.last_stream_ms,
                levels_ms = self.stats.last_levels_ms,
                render_ms = self.stats.last_render_ms,
                gpu_ms = self.stats.totals.gpu_ms,
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
        if self.hierarchy {
            self.camera.set_far_plane(9_216.0);
            self.camera.set_fog_end(9_200.0);
        } else {
            self.camera.set_far_plane(distance + 64.0);
            self.camera.set_fog_end(distance);
        }
    }

    /// Rebuilds the horizon from the near chunks and, with the hierarchy on,
    /// the uploaded sections' visible bounds.
    fn rebuild_horizon(&mut self) {
        let camera = self.camera.position();
        let built = if self.hierarchy {
            horizon::Horizon::build_with_sections(camera, &mut self.world, &self.section_bounds)
        } else {
            horizon::Horizon::build(camera, &mut self.world)
        };
        self.horizon = built;
        self.horizon_at = Instant::now();
    }

    /// Uploads one finished section and advances its hierarchy state.
    ///
    /// Returns whether the section became drawable. An upload that cannot get
    /// pool or slot space is marked dirty so it is rebuilt later.
    fn upload_section_result(&mut self, result: &SectionResult) -> bool {
        let key = result.key;
        if self.section_jobs.get(&key) != Some(&result.job) {
            return false;
        }
        self.section_jobs.remove(&key);
        self.sections_in_flight.remove(&key);
        if let Some(bounds) = result.bounds {
            self.section_bounds.insert(key, bounds);
        }
        let ready = if result.opaque.quads.is_empty() && result.water.quads.is_empty() {
            true
        } else {
            let Some(renderer) = &mut self.renderer else {
                return false;
            };
            let mut uploaded = true;
            if !result.opaque.quads.is_empty() {
                uploaded = renderer.upload_section(key, &result.opaque);
            }
            if uploaded && !result.water.quads.is_empty() {
                uploaded = renderer.upload_section_water(key, &result.water);
            }
            if uploaded {
                self.sections_uploaded.insert(key);
            }
            uploaded
        };
        if ready {
            self.sections.mark_ready(key);
            if self.sections_uploaded.len().is_multiple_of(100) {
                debug!(
                    sections = self.sections_uploaded.len(),
                    "hierarchy progress"
                );
            }
        } else {
            debug!(
                ?key,
                uploaded = self.sections_uploaded.len(),
                "section upload deferred: pool or slot limit"
            );
            self.sections_uploaded.remove(&key);
            self.sections.mark_dirty([key]);
        }
        ready
    }

    /// Selects, generates, and uploads hierarchy sections.
    ///
    /// The recursive parent-retained target is rebuilt when the player crosses
    /// a 64 m cell or a readiness change can swap a node for its in-world
    /// children. Uploads outside the target (plus a margin) are evicted, except
    /// for ancestor fallbacks and parents whose children are still building.
    /// With a worker pool, up to [`MAX_SECTION_JOBS`] generation/light/mesh jobs
    /// run in the background and their results are uploaded on the main thread.
    #[allow(clippy::cast_possible_truncation)]
    fn ensure_hierarchy(&mut self, pool: Option<&dyn MeshPool>) {
        profiling::scope!("hierarchy");
        if !self.hierarchy {
            return;
        }
        let Some(seed) = self.seed else {
            return;
        };

        let cell = IVec2::new(
            (self.player.position.x.floor() as i32).div_euclid(64),
            (self.player.position.z.floor() as i32).div_euclid(64),
        );
        let due = self.hierarchy_cell != Some(cell)
            || self.hierarchy_at.elapsed() >= Duration::from_millis(250);
        if due {
            self.hierarchy_cell = Some(cell);
            self.hierarchy_at = Instant::now();
            if self.sections.update_with_resident(
                self.player.position,
                self.detail_chunks,
                DEFAULT_PIXELS_PER_RADIAN,
                &self.sections_uploaded,
            ) {
                self.section_cursor = 0;
                self.evict_sections();
                if let Some(renderer) = &mut self.renderer {
                    renderer.set_section_visibility(self.sections.target());
                }
                debug!(
                    target = self.sections.target().len(),
                    queue = self.sections.queue().len(),
                    in_flight = self.sections_in_flight.len(),
                    detail = self.detail_chunks,
                    "hierarchy target"
                );
            }
        }

        if self.renderer.is_none() {
            return;
        }
        if let Some(pool) = pool {
            self.submit_section_jobs(pool, seed);
            return;
        }

        let mut built = 0;
        while built < MAX_SECTION_BUILDS_PER_FRAME {
            let Some(key) = self.sections.next_build(&mut self.section_cursor) else {
                break;
            };
            self.sections.mark_building(key);
            let edits =
                self.world
                    .section_edits(key, self.save_directory.as_deref(), &self.saved_chunks);
            let job = self.next_section_job;
            self.next_section_job = self.next_section_job.wrapping_add(1);
            let result = build_section(&SectionJob {
                key,
                job,
                seed,
                edits,
            });
            self.section_jobs.insert(key, job);
            self.sections_in_flight.insert(key);
            if self.upload_section_result(&result) {
                built += 1;
            } else {
                break;
            }
        }
    }

    /// Fills the worker pool with coarse section jobs.
    fn submit_section_jobs(&mut self, pool: &dyn MeshPool, seed: u64) {
        while self.sections_in_flight.len() < MAX_SECTION_JOBS
            && pool.can_submit(Priority::Background)
        {
            let Some(key) = self.sections.next_build(&mut self.section_cursor) else {
                break;
            };
            if self.sections_in_flight.contains(&key) {
                continue;
            }
            let edits =
                self.world
                    .section_edits(key, self.save_directory.as_deref(), &self.saved_chunks);
            let job = self.next_section_job;
            self.next_section_job = self.next_section_job.wrapping_add(1);
            let job = SectionJob {
                key,
                job,
                seed,
                edits,
            };
            let job_id = job.job;
            if pool.submit(Job::Section(job), Priority::Background) {
                self.sections.mark_building(key);
                self.sections_in_flight.insert(key);
                self.section_jobs.insert(key, job_id);
            } else {
                self.sections.mark_dirty([key]);
                break;
            }
        }
    }

    /// Removes uploaded sections that the current target no longer needs.
    fn evict_sections(&mut self) {
        let stale = self.sections.evictions(
            &self.sections_uploaded,
            self.player.position,
            self.detail_chunks,
        );
        if stale.is_empty() {
            return;
        }
        if let Some(renderer) = &mut self.renderer {
            for key in &stale {
                renderer.remove_section(*key);
            }
        }
        for key in &stale {
            self.sections_uploaded.remove(key);
            self.section_bounds.remove(key);
            self.section_jobs.remove(key);
            self.sections_in_flight.remove(key);
            self.sections.forget(*key);
        }
    }

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

    /// Marks a column ready once its last chunk is lit and meshed.
    fn mark_column_ready(&mut self, column: IVec2) {
        if self.column_ready(column)
            && let Some(streaming) = &mut self.streaming
        {
            streaming.mark_ready(column);
        }
    }

    /// Unloads distant columns and requests the nearest missing ones.
    ///
    /// Both the wanted and unloadable lists are rebuilt only when the player
    /// crosses a column, so a settled frame does not rescan the world.
    fn stream_columns(&mut self, pool: &dyn MeshPool) {
        profiling::scope!("stream columns");
        let Some(seed) = self.seed else {
            return;
        };
        let player_column = Streaming::column_at(self.player.position);

        if self.wanted_player != Some(player_column) {
            let forward = self.camera.forward();
            let forward = Vec2::new(forward.x, forward.z).normalize_or_zero();
            let (wanted, unload) = self
                .streaming
                .as_ref()
                .map(|streaming| {
                    (
                        streaming.wanted(self.player.position, forward),
                        streaming.unloadable(player_column),
                    )
                })
                .unwrap_or_default();
            self.wanted_columns = wanted;
            self.unload_columns = unload;
            self.wanted_cursor = 0;
            self.wanted_player = Some(player_column);
        }

        let mut failed_unload = false;
        for column in std::mem::take(&mut self.unload_columns) {
            if !self.unload_column(column) {
                self.unload_columns.push(column);
                failed_unload = true;
            }
        }
        if failed_unload {
            // Rebuild both lists next frame and retry the save.
            self.wanted_player = None;
        }

        let mut requested = 0;
        while requested < MAX_COLUMNS_PER_FRAME && self.wanted_cursor < self.wanted_columns.len() {
            let column = self.wanted_columns[self.wanted_cursor];
            self.wanted_cursor += 1;
            let loaded = self
                .streaming
                .as_ref()
                .is_some_and(|streaming| streaming.is_loaded(column));
            if loaded || self.queue.generation_pending(column) {
                continue;
            }
            self.queue
                .request_generation(pool, seed, column, Priority::Background);
            if self.queue.generation_pending(column) {
                requested += 1;
            } else {
                // The pool is behind; keep this candidate for the next frame.
                self.wanted_cursor -= 1;
                break;
            }
        }
    }

    /// Saves and drops one column that left the unload margin.
    ///
    /// Returns whether the column was released.
    fn unload_column(&mut self, column: IVec2) -> bool {
        let Some(directory) = self.save_directory.clone() else {
            return false;
        };
        self.invalidate_neighbour_columns(column);
        if let Err(error) = release_column(
            &mut self.world,
            &mut self.queue,
            &directory,
            &mut self.saved_chunks,
            column,
        ) {
            error!(%error, ?column, "failed to save an unloaded chunk");
            return false;
        }
        if let Some(renderer) = &mut self.renderer {
            for y in 0..CHUNK_COLUMN {
                renderer.remove_chunk(ChunkCoord::new(column.x, y, column.y));
            }
        }
        for y in 0..CHUNK_COLUMN {
            let chunk = ChunkCoord::new(column.x, y, column.y);
            self.level_work.remove(&chunk);
            self.deferred_light.remove(&chunk);
            self.upload_failures.remove(&chunk);
        }
        if let Some(streaming) = &mut self.streaming {
            streaming.mark_unloaded(column);
        }
        debug!(?column, "unloaded column");
        true
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
        if self.hierarchy && !renderer.supports_sections() {
            self.disable_hierarchy("quad pool render path unavailable");
        }
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

/// Every LOD that is not `active`; all three when nothing is active.
fn inactive_lods(active: Option<Lod>) -> Vec<Lod> {
    [Lod::Lod0, Lod::Lod1, Lod::Lod2]
        .into_iter()
        .filter(|lod| Some(*lod) != active)
        .collect()
}

/// Drops `built` entries for the LODs a selection just pruned.
///
/// `built` then means "resident and active": when the desired LOD changes back,
/// [`App::request_levels`] sees the missing entry and requests it again. An
/// empty upload has no resident LOD, so its `built` entry is kept; forgetting it
/// would make the level pass request the same empty mesh forever.
fn forget_inactive_lods(queue: &mut MeshQueue, renderer: &Renderer, chunk: ChunkCoord) {
    let Some(active) = renderer.active_lod(chunk) else {
        return;
    };
    for lod in inactive_lods(Some(active)) {
        queue.forget_built(chunk, lod, MeshKind::Opaque);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::generation::GenResult;
    use crate::mesh::MeshData;
    use crate::section::generate_section;
    use mesher::{Job, JobResult, MeshResult, SectionResult};

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

    #[test]
    fn spawn_height_ignores_water() {
        let mut world = World::default();
        world.set_block(IVec3::new(SPAWN_X, 0, SPAWN_Z), Block::Stone);
        world.set_block(IVec3::new(SPAWN_X, 1, SPAWN_Z), Block::Water);
        world.set_block(IVec3::new(SPAWN_X, 2, SPAWN_Z), Block::Water);

        let spawn = spawn_position(&world);

        assert_eq!(spawn.y, 1.5, "spawns on the solid ground, not the surface");
    }

    #[test]
    fn invalidating_a_column_marks_its_stored_neighbours() {
        let mut app = App::with_world(World::default(), false);
        let neighbour = ChunkCoord::new(0, 0, 0);
        app.world.set_block(IVec3::new(0, 0, 0), Block::Stone);
        app.level_work.clear();
        let revision = app.queue.revision(neighbour);

        app.invalidate_neighbour_columns(IVec2::new(1, 0));

        assert_eq!(app.queue.revision(neighbour), revision + 1);
        assert!(app.level_work.contains(&neighbour));
    }

    #[test]
    fn light_waits_for_a_streaming_neighbour_and_is_admitted_later() {
        let mut app = App::with_world(World::default(), false);
        let chunk = ChunkCoord::new(0, 0, 0);
        app.world.set_block(IVec3::new(0, 0, 0), Block::Stone);
        app.player = Player::new(Vec3::new(16.5, 0.0, 16.5));
        app.camera.set_position(app.player.eye_position());

        let mut streaming = Streaming::new(2);
        for dx in -1..=1 {
            for dz in -1..=1 {
                if (dx, dz) != (0, 0) && (dx, dz) != (1, 0) {
                    streaming.mark_loaded(IVec2::new(dx, dz));
                }
            }
        }
        app.streaming = Some(streaming);
        app.level_work.clear();
        app.deferred_light.insert(chunk);

        app.admit_deferred_light();

        assert!(app.deferred_light.contains(&chunk));
        assert!(!app.level_work.contains(&chunk));

        app.streaming
            .as_mut()
            .expect("streaming")
            .mark_loaded(IVec2::new(1, 0));
        app.admit_deferred_light();

        assert!(!app.deferred_light.contains(&chunk));
        assert!(app.level_work.contains(&chunk));
    }

    #[test]
    fn light_treats_columns_outside_the_render_distance_as_final() {
        let mut app = App::with_world(World::default(), false);
        app.player = Player::new(Vec3::new(16.5, 0.0, 16.5));
        app.camera.set_position(app.player.eye_position());

        let mut streaming = Streaming::new(1);
        for column in [
            IVec2::new(-2, -1),
            IVec2::new(-2, 1),
            IVec2::new(-1, -1),
            IVec2::new(-1, 0),
            IVec2::new(-1, 1),
            IVec2::new(0, -1),
            IVec2::new(0, 0),
            IVec2::new(0, 1),
        ] {
            streaming.mark_loaded(column);
        }
        app.streaming = Some(streaming);

        // (1, 0) is inside the render distance and still streaming in.
        assert!(!app.light_neighbourhood_stable(IVec2::new(0, 0)));
        // (-2, 0) is outside the render distance, so it will never arrive.
        assert!(app.light_neighbourhood_stable(IVec2::new(-1, 0)));
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
                Job::Section(job) => {
                    let generator = Generator::new(job.seed);
                    let section = generate_section(&generator, job.key, &job.edits);
                    let bounds = section.bounds();
                    let (opaque, water) = if section.is_empty() {
                        (MeshData::default(), MeshData::default())
                    } else {
                        let light = section.light();
                        (section.mesh(Some(&light)), section.water_mesh(Some(&light)))
                    };
                    self.results
                        .lock()
                        .unwrap()
                        .push(JobResult::Section(SectionResult {
                            key: job.key,
                            job: job.job,
                            opaque,
                            water,
                            bounds,
                            elapsed: Duration::ZERO,
                        }));
                }
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
    fn a_stale_section_result_is_dropped() {
        let mut app = App::with_world(World::default(), false);
        let key = SectionKey::new(1, IVec3::ZERO);
        app.section_jobs.insert(key, 7);
        app.sections_in_flight.insert(key);
        app.sections.mark_building(key);

        let stale = SectionResult {
            key,
            job: 6,
            opaque: MeshData::default(),
            water: MeshData::default(),
            bounds: None,
            elapsed: Duration::ZERO,
        };
        assert!(!app.upload_section_result(&stale));
        assert_ne!(app.sections.state(key), hierarchy::Readiness::Ready);
        assert!(app.sections_in_flight.contains(&key));

        let fresh = SectionResult { job: 7, ..stale };
        assert!(app.upload_section_result(&fresh));
        assert_eq!(app.sections.state(key), hierarchy::Readiness::Ready);
        assert!(!app.sections_in_flight.contains(&key));
    }

    #[test]
    fn invalidating_a_block_marks_the_overlapping_sections_dirty() {
        let mut app = App::with_world(World::default(), false);
        let world_position = IVec3::new(1, 127, 1);
        let chunk = ChunkCoord::from_world(world_position).0;
        let keys = overlapping_sections_for_chunk(chunk);
        let levels: HashSet<u8> = keys.iter().map(|key| key.level).collect();
        assert_eq!(levels, HashSet::from([1, 2, 3, 4]));
        for key in &keys {
            app.sections.mark_ready(*key);
            app.sections_uploaded.insert(*key);
        }

        let dirty = app.world.set_block(world_position, Block::Stone);
        app.invalidate_chunks(&dirty);

        assert!(dirty.contains(&chunk));
        for key in &keys {
            assert_eq!(
                app.sections.state(*key),
                hierarchy::Readiness::Requested,
                "{key:?} stayed ready"
            );
            assert!(
                !app.sections_uploaded.contains(key),
                "{key:?} is still drawable"
            );
        }
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
        let mut saved_chunks = HashSet::new();

        release_column(
            &mut world,
            &mut queue,
            &directory,
            &mut saved_chunks,
            IVec2::ZERO,
        )
        .unwrap();

        assert!(world.chunk(chunk).is_none());
        let saved = World::load_saved_chunk(&directory, chunk).unwrap().unwrap();
        assert_eq!(saved.block(coordinate), Block::Stone);
        assert!(saved_chunks.contains(&chunk));
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn saving_the_world_records_the_written_chunks() {
        let directory = temp_directory("save-index");
        let chunk = ChunkCoord::new(0, 0, 0);
        let coordinate = IVec3::new(1, 0, 1);
        let mut app = App::with_world(World::default(), false);
        app.save_directory = Some(directory.clone());
        app.world.set_block(coordinate, Block::Stone);

        app.save_world();

        assert!(app.saved_chunks.contains(&chunk));
        let saved = World::load_saved_chunk(&directory, chunk).unwrap().unwrap();
        assert_eq!(saved.block(coordinate), Block::Stone);
        assert!(!app.world.is_modified(chunk));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn saving_the_world_clears_the_modified_markers() {
        let directory = temp_directory("save-clears");
        let mut app = App::with_world(World::default(), false);
        app.save_directory = Some(directory.clone());
        let chunk = ChunkCoord::new(0, 0, 0);
        app.world.set_block(IVec3::new(5, 0, 5), Block::Stone);
        assert!(app.world.is_modified(chunk));

        app.save_world();

        assert!(!app.world.is_modified(chunk));
        assert!(directory.join("chunk_0_0_0.vchk").exists());
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
        insert_generated_chunk(&mut world, &directory, None, chunk, generated).unwrap();

        assert_eq!(world.block(coordinate), Block::Stone);
        assert!(!world.is_modified(chunk));
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
        insert_generated_chunk(&mut world, &directory, None, chunk, generated).unwrap();

        assert_eq!(world.block(coordinate), Block::Air);
        assert!(world.chunk(chunk).is_none());
        assert!(!world.is_modified(chunk));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn generated_chunks_use_the_saved_file_only_when_the_index_lists_it() {
        let directory = temp_directory("generated-index");
        let chunk = ChunkCoord::new(0, 0, 0);
        let coordinate = IVec3::new(1, 0, 1);
        let mut saved = World::default();
        saved.set_block(coordinate, Block::Wood);
        saved.save_chunks(&directory).unwrap();
        let index = World::saved_chunk_index(&directory).unwrap();

        let mut generated = Chunk::default();
        generated.set_block(coordinate, Block::Stone);
        let mut world = World::default();
        insert_generated_chunk(
            &mut world,
            &directory,
            Some(&index),
            chunk,
            generated.clone(),
        )
        .unwrap();
        assert_eq!(world.block(coordinate), Block::Wood);

        let mut world = World::default();
        insert_generated_chunk(
            &mut world,
            &directory,
            Some(&HashSet::new()),
            chunk,
            generated,
        )
        .unwrap();
        assert_eq!(world.block(coordinate), Block::Stone);
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
        assert!(!setup.world.is_modified(chunk));
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
    fn the_hierarchy_render_distance_covers_eight_kilometres() {
        let mut app = App::new(demo_world());
        app.hierarchy = true;
        app.config.render_distance_chunks = 12;

        app.apply_render_distance();

        assert_eq!(app.camera.far_plane(), 9_216.0);
        assert_eq!(app.camera.fog_end(), 9_200.0);
        assert!(app.camera.fog_end() > 8_192.0);
    }

    #[test]
    fn disabling_hierarchy_clears_pending_section_work() {
        let mut app = App::new(demo_world());
        app.hierarchy = true;
        let key = SectionKey::new(1, IVec3::ZERO);
        app.sections.mark_ready(key);
        app.sections_uploaded.insert(key);
        app.sections_in_flight.insert(key);
        app.section_jobs.insert(key, 7);
        app.section_bounds.insert(key, SectionBounds::default());
        app.section_cursor = 3;
        app.hierarchy_cell = Some(IVec2::ZERO);

        app.disable_hierarchy("test adapter");

        assert!(!app.hierarchy);
        assert_eq!(app.sections.target().len(), 0);
        assert_eq!(app.sections.queue().len(), 0);
        assert_eq!(app.sections.state(key), hierarchy::Readiness::Missing);
        assert!(app.sections_uploaded.is_empty());
        assert!(app.sections_in_flight.is_empty());
        assert!(app.section_jobs.is_empty());
        assert!(app.section_bounds.is_empty());
        assert_eq!(app.section_cursor, 0);
        assert!(app.hierarchy_cell.is_none());
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
    fn inactive_lods_exclude_only_the_active_level() {
        assert_eq!(inactive_lods(Some(Lod::Lod0)), vec![Lod::Lod1, Lod::Lod2]);
        assert_eq!(inactive_lods(Some(Lod::Lod1)), vec![Lod::Lod0, Lod::Lod2]);
        assert_eq!(inactive_lods(Some(Lod::Lod2)), vec![Lod::Lod0, Lod::Lod1]);
        assert_eq!(inactive_lods(None), vec![Lod::Lod0, Lod::Lod1, Lod::Lod2]);
    }

    #[test]
    fn a_pruned_lod_is_requested_again_when_it_is_desired() {
        let mut app = App::with_world(World::default(), false);
        let target = ChunkCoord::new(0, 0, 0);
        app.world.set_block(IVec3::new(0, 0, 0), Block::Stone);
        app.queue.mark_light_built(target);
        let pool = Arc::new(InlinePool::default());
        for lod in [Lod::Lod0, Lod::Lod1] {
            app.queue.request(
                &*pool,
                &app.world,
                target,
                lod,
                MeshKind::Opaque,
                Priority::Background,
            );
            let revision = app.queue.revision(target);
            assert!(app.queue.accept(&MeshResult {
                chunk: target,
                lod,
                kind: MeshKind::Opaque,
                revision,
                mesh: mesh::MeshData::default(),
                elapsed: Duration::ZERO,
            }));
        }
        assert!(app.queue.is_built(target, Lod::Lod0, MeshKind::Opaque));
        assert!(app.queue.is_built(target, Lod::Lod1, MeshKind::Opaque));

        // Lod1 becomes active, so the selection prunes Lod0 and its built entry.
        for lod in inactive_lods(Some(Lod::Lod1)) {
            app.queue.forget_built(target, lod, MeshKind::Opaque);
        }
        assert!(!app.queue.is_built(target, Lod::Lod0, MeshKind::Opaque));
        assert!(app.queue.is_built(target, Lod::Lod1, MeshKind::Opaque));

        // Walking back near makes Lod0 desired again; the level pass must ask.
        app.level_work.insert(target);
        app.request_levels(&*pool);
        assert!(app.queue.is_pending(target, Lod::Lod0, MeshKind::Opaque));
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

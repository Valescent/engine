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
mod light;
mod mesh;
mod mesher;
mod metrics;
mod overlay;
mod player;
mod raycast;
mod renderer;
mod sky;
mod streaming;
mod voxel;
mod world;

pub use terrain_core::{
    climate, content, curve, description, editlog, error, features, graph, hash, header, height,
    hydro, lattice, light2, lod, mesher2, metrics2, noise, padded, pipeline, residency, revisions,
    save, section, strata, surface, volume, world_save,
};

use std::collections::{BTreeMap, BTreeSet, HashSet};
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
use raycast::{RaycastHit, raycast, raycast_where};
pub use renderer::RendererError;
use renderer::{FrameStats, Renderer, TerrainSection};
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
const MAX_UPLOADS_PER_FRAME: usize = 2;
const STRESS_SCENE_ENV: &str = "ENGINE_STRESS_SCENE";
const STRESS_CHUNKS: i32 = 8;
const STRESS_FLAT_CHUNKS: i32 = 4;

/// Enables the static new-pipeline terrain patch. Unset keeps the app exactly
/// as before (`W14` renderer path, build-plan §6 item 3).
const TERRAIN_DEMO_ENV: &str = "TERRAIN_DEMO";
/// Directory for the v1 terrain demo's saved world (`A16`).
const TERRAIN_SAVE_DIR: &str = "world/terrain-demo";
/// Seed for the stress scene, which has no generated world seed.
const TERRAIN_DEMO_STRESS_SEED: u64 = 0x7E44_5EED;
/// Fine LOD radius in columns. Columns beyond this ring are drawn as LOD1.
const TERRAIN_DEMO_FINE_RADIUS: i32 = 2;
/// Fine meshes are kept one ring beyond the fine radius, so a column change
/// does not leave a gap while the new fine mesh builds.
const TERRAIN_DEMO_FINE_MARGIN: i32 = 1;
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
const TERRAIN_DEMO_UPLOAD_MS: u64 = 100;
/// Minimum interval between whole-ring coarse rebuilds, in milliseconds. LOD2
/// is rebuilt by the same job, so it shares this throttle.
const TERRAIN_DEMO_COARSE_MS: u64 = 250;

/// Sections per column as a `usize`, for array types (`K1.5`).
const COLUMN_SECTIONS: usize = description::SECTIONS_PER_COLUMN as usize;

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

/// Runs the engine until the window is closed.
///
/// # Errors
///
/// Returns an error if world loading, the event loop, window, or GPU
/// initialization fails.
#[allow(clippy::too_many_lines)]
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
    let (terrain_demo, terrain_ring, terrain_save, repair) = if terrain_demo_requested() {
        let seed = setup.seed.unwrap_or(TERRAIN_DEMO_STRESS_SEED);
        let spawn = setup.spawn.unwrap_or_else(|| Vec3::new(16.5, 3.0, 16.5));
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
        let player_column = Streaming::column_at(spawn);
        let world_hydro = pipeline::Terrain::new(seed)?.solve_world_hydro()?;
        let replay_terrain =
            pipeline::Terrain::new(seed)?.with_world_hydro(Arc::clone(&world_hydro));
        let view_radius = residency::demo_view_radius(config.render_distance_chunks);
        let mut demo = TerrainDemo::empty(seed, player_column, view_radius, world_hydro);
        demo.revision = last_saved;
        let save = match world.take() {
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
            &demo.pack,
            save.as_ref(),
            Some(&replay_terrain),
        );
        let read_only = replayed.read_only;
        let saved_count = replayed.sections;
        let applied_count = replayed.applied;
        demo.revision = replayed.revision;
        demo.read_only = read_only;
        let ring = RingStream::new(&demo, replayed.saved, worker_count().min(12));
        info!(
            seed = demo.seed,
            column = ?demo.origin_column,
            view_radius = demo.view_radius,
            fine_radius = demo.fine_radius,
            saved_sections = saved_count,
            replayed_edits = applied_count,
            read_only = demo.read_only,
            repair_needed = repair.is_some(),
            "TERRAIN_DEMO=1: column ring streaming starts"
        );
        (Some(demo), Some(ring), save, repair)
    } else {
        (None, None, None, None)
    };
    let mut app = App::with_world(setup.world, stress);
    app.terrain_demo = terrain_demo;
    app.terrain_ring = terrain_ring;
    app.terrain_save = terrain_save;
    app.repair_prompt = repair;
    if let Some(spawn) = setup.spawn {
        app.set_spawn(spawn);
    }
    if app.terrain_demo.is_some() {
        // The centre column is generated by the ring; the player is moved onto
        // its measured surface when the first build pass lands.
        app.camera.look(0.0, 0.0);
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

/// Returns whether the static new-pipeline terrain demo is requested.
fn terrain_demo_requested() -> bool {
    std::env::var(TERRAIN_DEMO_ENV).is_ok_and(|value| value == "1")
}

/// One meshed section of the demo patch.
#[derive(Clone)]
struct DemoSection {
    /// Section-aligned world block origin.
    origin: [i32; 3],
    /// World blocks per cell (`1` fine, `2` LOD1).
    scale: u32,
    mesh: mesher2::SectionMesh,
}

/// A corrupt section that must be repaired before saving can resume.
struct RepairPrompt {
    world: world_save::WorldSave,
    key: section::SectionKey,
}

/// The static new-pipeline terrain patch, built once before the window opens.
struct TerrainDemo {
    seed: u64,
    /// Column the patch is centred on, `Streaming` column coordinates.
    origin_column: IVec2,
    /// Chebyshev radius of the resident column ring.
    view_radius: i32,
    /// Chebyshev radius meshed at LOD0; beyond it, complete 2x2 groups are LOD1.
    fine_radius: i32,
    /// Every generated column, kept for collision and relighting.
    columns: BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    /// Per-column light, recomputed after an edit (`L1.2`).
    light: BTreeMap<(i32, i32), light2::ColumnLight>,
    /// LOD0 sections, keyed by the column they belong to.
    fine: BTreeMap<(i32, i32), Vec<DemoSection>>,
    /// LOD1 sections for every complete 2x2 region in the middle band.
    coarse: Vec<DemoSection>,
    /// LOD2 sections for every complete 4x4 region beyond the LOD1 band.
    far: Vec<DemoSection>,
    /// A deferred LOD2 rebuild is due after an edit. LOD2 is expensive, so it
    /// waits for the throttled whole-ring coarse job instead of rebuilding in
    /// the edit path.
    far_dirty: bool,
    /// Mesher and light material tables, resolved once from the pack.
    materials: mesher2::MaterialProps,
    /// The content pack, for resolving stable names on save (`A5.1`).
    pack: content::ContentPack,
    /// Generated baselines for sections that were loaded or edited (`J4.1`).
    baselines: BTreeMap<section::SectionKey, [u8; 32]>,
    /// Columns edited since their last queued save. Never evicted.
    unsaved: BTreeSet<(i32, i32)>,
    /// Next committed edit revision (`J1.2`).
    revision: u64,
    /// A world that loaded an unknown name is read-only (`A16`).
    read_only: bool,
    /// Air id, for edits.
    air: section::BlockId,
    /// `solid[id]` mirrors the content pack's collision flag.
    solid: Vec<bool>,
    /// Feet position on the patch's surface at the centre column. Valid once
    /// the centre column has light (`spawn_ready`).
    spawn: Vec3,
    /// Whether the centre column has been generated, lit, and measured.
    spawn_ready: bool,
    /// Active sections submitted to the renderer (fine and coarse by distance).
    sections: Vec<DemoSection>,
    /// Column the current LOD selection was made for.
    lod_column: (i32, i32),
    quads: usize,
    /// World block range covered by emitting sections, for the startup log.
    min_y: i32,
    max_y: i32,
    /// One solve shared by every worker's `Terrain` (`A7.1`).
    world_hydro: Arc<crate::hydro::WorldHydro>,
}

impl crate::player::BlockSource for TerrainDemo {
    fn solid_blocks_in(&self, min: IVec3, max: IVec3) -> Vec<IVec3> {
        let mut blocks = Vec::new();
        if min.cmpgt(max).any() {
            return blocks;
        }
        for x in min.x..=max.x {
            for y in min.y..=max.y {
                for z in min.z..=max.z {
                    let cell = IVec3::new(x, y, z);
                    if self.is_solid(cell) {
                        blocks.push(cell);
                    }
                }
            }
        }
        blocks
    }
}

impl TerrainDemo {
    /// An empty ring centred on `column`, ready to stream columns into.
    #[allow(clippy::cast_precision_loss)]
    fn empty(
        seed: u64,
        column: IVec2,
        view_radius: i32,
        world_hydro: Arc<crate::hydro::WorldHydro>,
    ) -> Self {
        let pack = content::v1_default();
        let solid = solid_table(&pack);
        let air = pack.id("core:air").unwrap_or(0);
        let section_blocks = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
        Self {
            seed,
            origin_column: column,
            view_radius,
            fine_radius: TERRAIN_DEMO_FINE_RADIUS,
            columns: BTreeMap::new(),
            light: BTreeMap::new(),
            fine: BTreeMap::new(),
            coarse: Vec::new(),
            far: Vec::new(),
            far_dirty: false,
            materials: demo_materials(),
            pack,
            baselines: BTreeMap::new(),
            unsaved: BTreeSet::new(),
            revision: 0,
            read_only: false,
            air,
            solid,
            spawn: Vec3::new(
                (column.x * section_blocks) as f32 + 16.5,
                64.0,
                (column.y * section_blocks) as f32 + 16.5,
            ),
            spawn_ready: false,
            sections: Vec::new(),
            lod_column: (column.x, column.y),
            quads: 0,
            min_y: 0,
            max_y: 0,
            world_hydro,
        }
    }

    /// The block at a world cell, or `None` outside the patch.
    fn block_at(&self, cell: IVec3) -> Option<section::BlockId> {
        if cell.y < 0 {
            return None;
        }
        let size = i32::try_from(description::SECTION_SIZE).ok()?;
        let column = (cell.x.div_euclid(size), cell.z.div_euclid(size));
        let stack = self.columns.get(&column)?;
        let section = stack.get(usize::try_from(cell.y.div_euclid(size)).ok()?)?;
        Some(section.get(
            u32::try_from(cell.x.rem_euclid(size)).ok()?,
            u32::try_from(cell.y.rem_euclid(size)).ok()?,
            u32::try_from(cell.z.rem_euclid(size)).ok()?,
        ))
    }

    /// Whether a cell is solid for collision and raycasts (`A5.2`).
    fn is_solid(&self, cell: IVec3) -> bool {
        self.block_at(cell)
            .is_some_and(|id| self.solid.get(usize::from(id)).copied().unwrap_or(false))
    }

    /// Whether a cell is inside the patch and empty.
    fn is_empty(&self, cell: IVec3) -> bool {
        matches!(self.block_at(cell), Some(id) if id == self.air)
    }

    /// Whether a column has light, which is the readiness gate for physics.
    fn column_ready(&self, column: (i32, i32)) -> bool {
        self.light.contains_key(&column)
    }

    /// Whether a player AABB at `position` overlaps only ready columns.
    #[allow(clippy::cast_possible_truncation)]
    fn aabb_ready(&self, position: Vec3) -> bool {
        let half = player::PLAYER_WIDTH / 2.0;
        let min = IVec2::new(
            (position.x - half).floor() as i32,
            (position.z - half).floor() as i32,
        );
        let max = IVec2::new(
            (position.x + half).floor() as i32,
            (position.z + half).floor() as i32,
        );
        let size = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
        for x in min.x.div_euclid(size)..=max.x.div_euclid(size) {
            for z in min.y.div_euclid(size)..=max.y.div_euclid(size) {
                if !self.column_ready((x, z)) {
                    return false;
                }
            }
        }
        true
    }

    /// The generated baseline for a section, computed once (`J4.1`).
    ///
    /// Callers compute this **before** mutating the section, or it would hash
    /// the edited blocks instead of the first generation.
    fn baseline_for(&mut self, key: section::SectionKey) -> [u8; 32] {
        if let Some(hash) = self.baselines.get(&key) {
            return *hash;
        }
        let hash = self
            .columns
            .get(&(key.column[0], key.column[1]))
            .and_then(|stack| stack.get(usize::from(key.section)))
            .map_or([0; 32], section::baseline_hash);
        self.baselines.insert(key, hash);
        hash
    }

    /// Inserts a generated column. Replacing a column also drops its light and
    /// fine mesh, so the next build pass recomputes them.
    fn insert_column(&mut self, column: (i32, i32), stack: [section::Section; COLUMN_SECTIONS]) {
        self.columns.insert(column, Arc::new(stack));
        self.light.remove(&column);
        self.fine.remove(&column);
    }

    /// Drops a column and everything derived from it.
    fn remove_column(&mut self, column: (i32, i32)) {
        self.columns.remove(&column);
        self.light.remove(&column);
        self.fine.remove(&column);
    }

    /// Installs computed light for the columns a build job returned.
    fn apply_light(&mut self, light: BTreeMap<(i32, i32), light2::ColumnLight>) {
        for (column, value) in light {
            if self.columns.contains_key(&column) {
                self.light.insert(column, value);
            }
        }
    }

    /// Installs fine-section replacements, keeping each column's list sorted.
    fn apply_patches(&mut self, patches: BTreeMap<(i32, i32), Vec<DemoPatch>>) {
        for (column, patches) in patches {
            if !self.columns.contains_key(&column) {
                continue;
            }
            let sections = self.fine.entry(column).or_default();
            for patch in patches {
                match patch.mesh {
                    Some(mesh) => {
                        let section = DemoSection {
                            origin: patch.origin,
                            scale: 1,
                            mesh,
                        };
                        match sections.binary_search_by_key(&patch.origin, |s| s.origin) {
                            Ok(index) => sections[index] = section,
                            Err(index) => sections.insert(index, section),
                        }
                    }
                    None => {
                        if let Ok(index) =
                            sections.binary_search_by_key(&patch.origin, |s| s.origin)
                        {
                            sections.remove(index);
                        }
                    }
                }
            }
        }
    }

    /// Measures the surface of the centre column and remembers the spawn.
    #[allow(clippy::cast_precision_loss)]
    fn settle_spawn(&mut self) {
        let column = (self.origin_column.x, self.origin_column.y);
        let Some(stack) = self.columns.get(&column).cloned() else {
            return;
        };
        let section_blocks = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
        let bx = self.origin_column.x * section_blocks + section_blocks / 2;
        let bz = self.origin_column.y * section_blocks + section_blocks / 2;
        let height = i32::try_from(description::WORLD_HEIGHT).unwrap_or(384);
        let top = (0..height).rev().find(|&y| {
            let sy = usize::try_from(y.div_euclid(section_blocks)).unwrap_or(0);
            let Some(section) = stack.get(sy) else {
                return false;
            };
            let id = section.get(
                u32::try_from(bx.rem_euclid(section_blocks)).unwrap_or(0),
                u32::try_from(y.rem_euclid(section_blocks)).unwrap_or(0),
                u32::try_from(bz.rem_euclid(section_blocks)).unwrap_or(0),
            );
            self.solid.get(usize::from(id)).copied().unwrap_or(false)
        });
        self.spawn = Vec3::new(
            bx as f32 + 0.5,
            top.map_or(description::SEA, |y| y + 1) as f32,
            bz as f32 + 0.5,
        );
        self.spawn_ready = true;
    }

    /// Layers saved sections over the resident columns and refreshes light,
    /// fine meshes, and coarse regions. Used after `world-repair`.
    fn apply_saved(&mut self, saved: &[section::SavedSection]) -> bool {
        let mut changed = false;
        for record in saved {
            let key = record.key;
            let Some(stack) = self.columns.get_mut(&(key.column[0], key.column[1])) else {
                continue;
            };
            let Some(slot) = Arc::make_mut(stack).get_mut(usize::from(key.section)) else {
                continue;
            };
            let generated = section::baseline_hash(slot);
            if generated != record.baseline_hash {
                warn!(
                    column = ?key.column,
                    section = key.section,
                    "saved terrain section does not match this generator; ignored"
                );
                continue;
            }
            if !record.unknown_names.is_empty() {
                self.read_only = true;
            }
            *slot = record.section.clone();
            self.baselines.insert(key, record.baseline_hash);
            changed = true;
        }
        if changed {
            self.light = demo_column_light(&self.columns);
            self.fine = demo_fine_all(&self.columns, &self.light, &self.materials);
            let (coarse, far) =
                demo_lod_sets(&self.columns, &self.materials, &self.pack, self.lod_column);
            self.coarse = coarse;
            self.far = far;
            self.far_dirty = false;
            self.select_lod(self.lod_column);
        }
        changed
    }

    /// Writes one block into the patch. Returns whether anything changed.
    ///
    /// Light and meshes are rebuilt separately: [`demo_rebuild`] does that off the
    /// main thread, so a click never stalls the frame.
    fn set_block(&mut self, cell: IVec3, id: section::BlockId) -> bool {
        if cell.y < 0 {
            return false;
        }
        let size = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
        let key = (cell.x.div_euclid(size), cell.z.div_euclid(size));
        let Some(stack) = self.columns.get_mut(&key) else {
            return false;
        };
        let Ok(sy) = usize::try_from(cell.y.div_euclid(size)) else {
            return false;
        };
        let stack = Arc::make_mut(stack);
        let Some(section) = stack.get_mut(sy) else {
            return false;
        };
        let (x, y, z) = (
            u32::try_from(cell.x.rem_euclid(size)).unwrap_or(0),
            u32::try_from(cell.y.rem_euclid(size)).unwrap_or(0),
            u32::try_from(cell.z.rem_euclid(size)).unwrap_or(0),
        );
        if section.get(x, y, z) == id {
            return false;
        }
        section.set(x, y, z, id);
        true
    }

    /// Installs a finished background rebuild.
    fn apply_rebuild(&mut self, rebuild: DemoRebuild) {
        self.light = rebuild.light;
        self.apply_patches(rebuild.patches);
        // Coarse regions are disposable: refresh the ones an edit can reach.
        // LOD2 reads 4x4 column groups and is too expensive for the edit path,
        // so it is rebuilt by the next throttled whole-ring job.
        self.refresh_coarse_around(&rebuild.columns);
        self.far_dirty = true;
        self.select_lod(self.lod_column);
    }

    /// Rebuilds every coarse region whose 2x2 column group touches `columns`.
    fn refresh_coarse_around(&mut self, columns: &BTreeSet<(i32, i32)>) {
        let mut regions = BTreeSet::new();
        for &(cx, cz) in columns {
            let bx = cx - cx.rem_euclid(2);
            let bz = cz - cz.rem_euclid(2);
            for dx in -1..=1_i32 {
                for dz in -1..=1_i32 {
                    regions.insert((bx + 2 * dx, bz + 2 * dz));
                }
            }
        }
        if regions.is_empty() {
            return;
        }
        let rebuilt = demo_coarse_regions(&self.columns, &self.materials, &self.pack, &regions);
        self.coarse
            .retain(|section| !regions.contains(&(section.origin[0] / 32, section.origin[2] / 32)));
        for (&region, sections) in &rebuilt {
            if regions.contains(&region) {
                self.coarse.extend(sections.iter().cloned());
            }
        }
        self.coarse.sort_by_key(|section| section.origin);
    }

    /// Chooses LOD0 near the player, LOD1 for the middle band, and LOD2 far.
    ///
    /// Fine meshes are kept one ring beyond the fine radius as a transition
    /// buffer. An LOD1 region is active only when all four of its columns are
    /// outside that buffer and it is not covered by an active LOD2 region, so
    /// the three bands never overlap. LOD2 regions are complete 4x4 column
    /// squares built entirely beyond [`TERRAIN_DEMO_LOD1_RADIUS`].
    fn select_lod(&mut self, player_column: (i32, i32)) {
        self.lod_column = player_column;
        let cutoff = self.fine_radius + TERRAIN_DEMO_FINE_MARGIN;
        let ring = |column: (i32, i32)| {
            (column.0 - player_column.0)
                .abs()
                .max((column.1 - player_column.1).abs())
        };
        let mut active: Vec<DemoSection> = self
            .fine
            .iter()
            .filter(|&(&column, _)| ring(column) <= cutoff)
            .flat_map(|(_, sections)| sections.iter().cloned())
            .collect();

        // Far regions still draw while the player walks toward them; the next
        // throttled rebuild reclassifies them. All 16 columns must be clear of
        // the fine buffer, which keeps LOD2 off the LOD0 transition.
        let far_regions: BTreeSet<(i32, i32)> = self
            .far
            .iter()
            .map(|section| (section.origin[0] / 32, section.origin[2] / 32))
            .filter(|&(x, z)| (0..4).all(|dx| (0..4).all(|dz| ring((x + dx, z + dz)) > cutoff)))
            .collect();
        for section in &self.far {
            if far_regions.contains(&(section.origin[0] / 32, section.origin[2] / 32)) {
                active.push(section.clone());
            }
        }

        for section in &self.coarse {
            let region = (section.origin[0] / 32, section.origin[2] / 32);
            let near = [
                region,
                (region.0 + 1, region.1),
                (region.0, region.1 + 1),
                (region.0 + 1, region.1 + 1),
            ]
            .into_iter()
            .any(|column| ring(column) <= cutoff);
            let far_base = (region.0.div_euclid(4) * 4, region.1.div_euclid(4) * 4);
            if !near && !far_regions.contains(&far_base) {
                active.push(section.clone());
            }
        }
        active.sort_by_key(|section| section.origin);
        self.sections = active;
        // Drop fine meshes that fell out of the transition buffer. Added
        // columns are rebuilt when the player returns.
        self.fine.retain(|&column, _| ring(column) <= cutoff);
        self.refresh_stats();
    }

    /// Recomputes the renderer-facing counters from the active section set.
    fn refresh_stats(&mut self) {
        self.quads = self
            .sections
            .iter()
            .map(|section| mesher2::quad_count(&section.mesh))
            .sum();
        let section_blocks = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
        self.min_y = self
            .sections
            .iter()
            .map(|section| section.origin[1])
            .min()
            .unwrap_or(i32::MAX);
        self.max_y = self
            .sections
            .iter()
            .map(|section| {
                section.origin[1] + section_blocks * i32::try_from(section.scale).unwrap_or(1)
            })
            .max()
            .unwrap_or(i32::MIN);
    }

    /// Approximate heap footprint of resident blocks, light, and meshes.
    fn footprint_bytes(&self) -> usize {
        let sections: usize = self
            .columns
            .values()
            .flat_map(|stack| stack.iter())
            .map(section_bytes)
            .sum();
        let light: usize = self
            .light
            .values()
            .map(light2::ColumnLight::packed_bytes)
            .sum();
        let meshes: usize = self
            .fine
            .values()
            .flatten()
            .chain(&self.coarse)
            .chain(&self.far)
            .map(mesh_bytes)
            .sum();
        sections + light + meshes
    }

    /// Synchronous rebuild, used by tests and by the startup path.
    #[cfg(test)]
    fn edit(&mut self, cell: IVec3, id: section::BlockId) -> bool {
        if !self.set_block(cell, id) {
            return false;
        }
        let edit = DemoEdit { cell };
        let rebuild = demo_rebuild(&self.columns, &self.light, &self.materials, &[edit]);
        self.apply_rebuild(rebuild);
        true
    }
}

/// `solid[id]` from the pack's collision flag (`A5.2`).
fn solid_table(pack: &content::ContentPack) -> Vec<bool> {
    let mut solid = vec![false; pack.len()];
    for record in pack.records() {
        if let Some(id) = pack.id(&record.name) {
            solid[id as usize] = record.collision;
        }
    }
    solid
}

/// Heap bytes held by one section's palette data.
fn section_bytes(section: &section::Section) -> usize {
    std::mem::size_of_val(section.words()) + section.palette().len() * 2
}

/// Heap bytes held by one mesh's quad lists.
fn mesh_bytes(section: &DemoSection) -> usize {
    mesher2::quad_count(&section.mesh) * std::mem::size_of::<mesher2::Quad>()
}

/// One block change fed to an incremental rebuild.
struct DemoEdit {
    cell: IVec3,
}

/// One section in a rebuild result; `mesh: None` removes it.
struct DemoPatch {
    origin: [i32; 3],
    mesh: Option<mesher2::SectionMesh>,
}

/// One finished background rebuild of the patch (`L3.3`).
struct DemoRebuild {
    light: BTreeMap<(i32, i32), light2::ColumnLight>,
    patches: BTreeMap<(i32, i32), Vec<DemoPatch>>,
    /// Columns the edits touched, for the coarse-region refresh.
    columns: BTreeSet<(i32, i32)>,
    light_time: std::time::Duration,
    mesh_time: std::time::Duration,
}

/// Repairs light and remeshes only what an edit can reach.
///
/// Light changes stay within `LIGHT_LEVELS` laterally, so only the 3x3 columns
/// around an edit are relit; within a column the edit's vertical span is
/// repaired by [`light2::ColumnLight::relight_column`]. A section is remeshed
/// when its light changed or when the edit touched its geometry.
#[allow(clippy::too_many_lines)]
fn demo_rebuild(
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
fn build_terrain_demo(
    seed: u64,
    column: IVec2,
    radius: i32,
) -> Result<TerrainDemo, error::TerrainError> {
    build_terrain_demo_saved(seed, column, radius, &[], &[], 0).map(|(demo, _)| demo)
}

/// What a load had to replay from the journal.
#[cfg(test)]
struct DemoReplay {
    /// Sections the replay changed, to be written before the header advances.
    keys: Vec<section::SectionKey>,
    /// Greatest journal revision now reflected in the patch.
    revision: u64,
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
fn build_terrain_demo_saved(
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
fn build_terrain_demo_extent(
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
    let (coarse, far) = demo_lod_sets(
        &demo.columns,
        &demo.materials,
        &demo.pack,
        (column.x, column.y),
    );
    demo.coarse = coarse;
    demo.far = far;
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

/// The mesher material table for the built-in v1 content pack.
///
/// Ids come from [`content::v1_default`], the same pack [`pipeline::Terrain::new`]
/// resolves, so sections and this table always agree. The pack has no texture
/// references yet, so [`demo_texture_layer`] maps names onto the renderer's
/// procedural array as a placeholder.
fn demo_materials() -> mesher2::MaterialProps {
    let pack = content::v1_default();
    let mut props = mesher2::MaterialProps::new();
    for record in pack.records() {
        let id = pack.id(&record.name).expect("every pack record has an id");
        props.set(
            id,
            mesher2::Material {
                solid: record.collision,
                opaque: record.opacity >= content::LIGHT_LEVELS,
                water: record.liquid,
                cross_quad: record.render_shape == content::RenderShape::CrossQuad,
                texture_top: demo_texture_layer(&record.textures.top),
                texture_side: demo_texture_layer(&record.textures.side),
                texture_bottom: demo_texture_layer(&record.textures.bottom),
                tint: 0,
                emissive: record.emission > 0,
            },
        );
    }
    props
}

/// Generates every column in an inclusive rectangle.
///
/// Test-only: the live demo generates one column per worker job.
#[cfg(test)]
#[allow(clippy::too_many_lines)]
fn demo_columns_extent(
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

/// The light scratch for one section, with the *real* neighbour light on the
/// horizontal border.
///
/// [`light2::ColumnLight::padded_section`] mirrors its own edge row, which is the
/// `I2.3` world-edge rule. At an internal section boundary that mirror reads the
/// solid edge cell, so a wall on `x = 0` or `x = 31` renders with sky 0. This
/// overlays the adjacent column's light, which is what the caller is expected to
/// do.
fn demo_section_light(
    lit: &BTreeMap<(i32, i32), light2::ColumnLight>,
    sx: i32,
    sz: i32,
    sy: usize,
) -> padded::PaddedLight {
    let Some(column) = lit.get(&(sx, sz)) else {
        return padded::PaddedLight::default();
    };
    let mut padded = column.padded_section(sy);
    let base_y = i32::try_from(sy).unwrap_or(0) * 32;
    let origin_x = sx * 32;
    let origin_z = sz * 32;
    for y in padded::LOCAL_MIN..=padded::LOCAL_MAX {
        let world_y = base_y + y;
        for z in padded::LOCAL_MIN..=padded::LOCAL_MAX {
            for x in padded::LOCAL_MIN..=padded::LOCAL_MAX {
                if x != padded::LOCAL_MIN
                    && x != padded::LOCAL_MAX
                    && z != padded::LOCAL_MIN
                    && z != padded::LOCAL_MAX
                {
                    continue;
                }
                let world_x = origin_x + x;
                let world_z = origin_z + z;
                let key = (world_x.div_euclid(32), world_z.div_euclid(32));
                if key == (sx, sz) {
                    continue;
                }
                let Some(neighbour) = lit.get(&key) else {
                    continue;
                };
                padded.set(
                    x,
                    y,
                    z,
                    neighbour.sky(world_x.rem_euclid(32), world_y, world_z.rem_euclid(32)),
                    neighbour.block(world_x.rem_euclid(32), world_y, world_z.rem_euclid(32)),
                );
            }
        }
    }
    padded
}

/// One `light2` job per column (`L1.2`), using the 8 horizontal neighbours.
fn demo_column_light(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
) -> BTreeMap<(i32, i32), light2::ColumnLight> {
    let table = demo_light_table();
    // One job per column, fanned out across the pool. Output is keyed, so the
    // order threads finish in cannot change the result (`P1.9`).
    let keys: Vec<(i32, i32)> = columns.keys().copied().collect();
    if keys.is_empty() {
        return BTreeMap::new();
    }
    let workers = worker_count().min(keys.len());
    let chunk = keys.len().div_ceil(workers);
    let mut lit = BTreeMap::new();
    std::thread::scope(|scope| {
        let table = &table;
        let handles: Vec<_> = keys
            .chunks(chunk)
            .map(|chunk| {
                scope.spawn(move || {
                    let mut out = Vec::with_capacity(chunk.len());
                    for &(sx, sz) in chunk {
                        let stack = &columns[&(sx, sz)];
                        let mut borders: [Option<&[section::Section; COLUMN_SECTIONS]>; 8] =
                            [None; 8];
                        let mut slot = 0;
                        for dz in -1..=1_i32 {
                            for dx in -1..=1_i32 {
                                if dx == 0 && dz == 0 {
                                    continue;
                                }
                                borders[slot] =
                                    columns.get(&(sx + dx, sz + dz)).map(|stack| &**stack);
                                slot += 1;
                            }
                        }
                        let blocks = light2::ColumnBlocks::new((**stack).clone(), &borders);
                        out.push(((sx, sz), light2::compute_column(&blocks, table)));
                    }
                    out
                })
            })
            .collect();
        for handle in handles {
            for (key, column) in handle.join().expect("a light job panicked") {
                lit.insert(key, column);
            }
        }
    });
    lit
}

/// Worker count for one fan-out: the available parallelism, at least one.
fn worker_count() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
}

/// Meshes LOD0 sections for every column (`M3.2`), keyed by column.
///
/// Every column gets an entry, even an empty one, so the streamer can tell a
/// meshed column from a column that has not been visited.
fn demo_fine_all(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    light: &BTreeMap<(i32, i32), light2::ColumnLight>,
    materials: &mesher2::MaterialProps,
) -> BTreeMap<(i32, i32), Vec<DemoSection>> {
    let tasks: Vec<(i32, i32)> = columns.keys().copied().collect();
    if tasks.is_empty() {
        return BTreeMap::new();
    }
    let workers = worker_count().min(tasks.len());
    let chunk = tasks.len().div_ceil(workers);
    let mut fine = BTreeMap::new();
    std::thread::scope(|scope| {
        let handles: Vec<_> = tasks
            .chunks(chunk)
            .map(|chunk| {
                scope.spawn(move || {
                    let mut out = Vec::with_capacity(chunk.len());
                    for &(sx, sz) in chunk {
                        out.push((sx, sz, demo_fine_column(columns, light, materials, sx, sz)));
                    }
                    out
                })
            })
            .collect();
        for handle in handles {
            for (sx, sz, sections) in handle.join().expect("a mesh job panicked") {
                fine.insert((sx, sz), sections);
            }
        }
    });
    fine
}

/// Meshes every LOD0 section of one column.
fn demo_fine_column(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    light: &BTreeMap<(i32, i32), light2::ColumnLight>,
    materials: &mesher2::MaterialProps,
    sx: i32,
    sz: i32,
) -> Vec<DemoSection> {
    let mut out = Vec::new();
    for sy in 0..COLUMN_SECTIONS {
        if let Some(section) = demo_section(columns, light, materials, sx, sz, sy) {
            out.push(section);
        }
    }
    out.sort_by_key(|section| section.origin);
    out
}

/// Derives and meshes every complete `2x2x2` coarse group (`N1.4`, `N3.2`).
#[cfg(test)]
fn demo_coarse(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    materials: &mesher2::MaterialProps,
    pack: &content::ContentPack,
) -> Vec<DemoSection> {
    let mut regions = BTreeSet::new();
    for &(x, z) in columns.keys() {
        if x.rem_euclid(2) == 0 && z.rem_euclid(2) == 0 {
            regions.insert((x, z));
        }
    }
    let mut out: Vec<DemoSection> = demo_coarse_regions(columns, materials, pack, &regions)
        .into_values()
        .flatten()
        .collect();
    out.sort_by_key(|section| section.origin);
    out
}

/// The LOD1 and LOD2 region bases selected for one player column.
type BandSets = (BTreeSet<(i32, i32)>, BTreeSet<(i32, i32)>);

/// The complete LOD1 and LOD2 region bases for a player column.
///
/// LOD2 owns a `4x4` column square only when all 16 columns are resident and
/// every one of them is beyond [`TERRAIN_DEMO_LOD1_RADIUS`]. LOD1 regions in
/// that square are dropped, so a later selection can never draw both levels
/// over the same columns.
fn coarse_bands(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    player: (i32, i32),
) -> BandSets {
    let ring = |column: (i32, i32)| (column.0 - player.0).abs().max((column.1 - player.1).abs());
    let mut far = BTreeSet::new();
    for &(x, z) in columns.keys() {
        if x.rem_euclid(4) != 0 || z.rem_euclid(4) != 0 {
            continue;
        }
        let complete = (0..4).all(|dx| (0..4).all(|dz| columns.contains_key(&(x + dx, z + dz))));
        let clear =
            (0..4).all(|dx| (0..4).all(|dz| ring((x + dx, z + dz)) > TERRAIN_DEMO_LOD1_RADIUS));
        if complete && clear {
            far.insert((x, z));
        }
    }

    let mut coarse = BTreeSet::new();
    for &(x, z) in columns.keys() {
        if x.rem_euclid(2) != 0 || z.rem_euclid(2) != 0 {
            continue;
        }
        if far.contains(&(x.div_euclid(4) * 4, z.div_euclid(4) * 4)) {
            continue;
        }
        if columns.contains_key(&(x + 1, z))
            && columns.contains_key(&(x, z + 1))
            && columns.contains_key(&(x + 1, z + 1))
        {
            coarse.insert((x, z));
        }
    }
    (coarse, far)
}

/// Builds the LOD1 and LOD2 section sets for `player`.
fn demo_lod_sets(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    materials: &mesher2::MaterialProps,
    pack: &content::ContentPack,
    player: (i32, i32),
) -> (Vec<DemoSection>, Vec<DemoSection>) {
    let (coarse_regions, far_regions) = coarse_bands(columns, player);
    let mut coarse: Vec<DemoSection> =
        demo_coarse_regions(columns, materials, pack, &coarse_regions)
            .into_values()
            .flatten()
            .collect();
    let mut far: Vec<DemoSection> = demo_far_regions(columns, materials, pack, &far_regions)
        .into_values()
        .flatten()
        .collect();
    coarse.sort_by_key(|section| section.origin);
    far.sort_by_key(|section| section.origin);
    (coarse, far)
}

/// Meshes the coarse sections of `regions`.
///
/// A coarse region is the 2x2 column group whose base is even; its sections are
/// `64` blocks tall, so six cover the column. Only regions with all four columns
/// produce sections. Every region in `regions` is rebuilt; the surrounding ring
/// is derived too, so a coarse mesh culls against a real neighbour when present
/// and falls back to air at the ring edge.
fn demo_coarse_regions(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    materials: &mesher2::MaterialProps,
    pack: &content::ContentPack,
    regions: &BTreeSet<(i32, i32)>,
) -> BTreeMap<(i32, i32), Vec<DemoSection>> {
    if regions.is_empty() {
        return BTreeMap::new();
    }
    let water = pack.id("core:water").unwrap_or(0);
    let scatter: Vec<section::BlockId> = pack
        .records()
        .iter()
        .filter(|record| record.render_shape == content::RenderShape::CrossQuad)
        .filter_map(|record| pack.id(&record.name))
        .collect();

    let mut expanded = BTreeSet::new();
    for &(x, z) in regions {
        for dx in -1..=1_i32 {
            for dz in -1..=1_i32 {
                expanded.insert((x + 2 * dx, z + 2 * dz));
            }
        }
    }

    let mut derived: BTreeMap<(i32, i32, i32), section::Section> = BTreeMap::new();
    for &(x, z) in &expanded {
        if !columns.contains_key(&(x + 1, z))
            || !columns.contains_key(&(x, z + 1))
            || !columns.contains_key(&(x + 1, z + 1))
            || !columns.contains_key(&(x, z))
        {
            continue;
        }
        for j in 0..(COLUMN_SECTIONS / 2) {
            let octants: [&section::Section; 8] = std::array::from_fn(|slot| {
                let dx = i32::try_from(slot & 1).unwrap_or(0);
                let dz = i32::try_from((slot >> 1) & 1).unwrap_or(0);
                let dy = (slot >> 2) & 1;
                &columns[&(x + dx, z + dz)][2 * j + dy]
            });
            if let Ok(coarse) = lod::downsample(&octants, water, &scatter) {
                derived.insert((x, i32::try_from(j).unwrap_or(0), z), coarse);
            }
        }
    }

    let air = section::Section::uniform(0);
    let mut out: BTreeMap<(i32, i32), Vec<DemoSection>> = BTreeMap::new();
    for &(x, z) in regions {
        let mut sections = Vec::new();
        for j in 0..(COLUMN_SECTIONS / 2) {
            let j = i32::try_from(j).unwrap_or(0);
            let Some(coarse) = derived.get(&(x, j, z)) else {
                continue;
            };
            // A missing coarse neighbour is drawn as air, so the coarse region's
            // boundary wall is emitted instead of culled against a section that is
            // not on screen. Skirts still continue the top face downward.
            let neighbours: [Option<&section::Section>; 26] =
                padded::NEIGHBOUR_ORDER.map(|(dx, dy, dz)| {
                    derived
                        .get(&(x + dx * 2, j + dy, z + dz * 2))
                        .or(Some(&air))
                });
            let blocks = padded::PaddedBlocks::from_sections(coarse, &neighbours);
            let mut mesh = mesher2::mesh_coarse(&blocks, materials, mesher2::MeshLod::Lod1, 0);
            mesher2::add_skirts(&mut mesh, mesher2::SkirtEdges::ALL, mesher2::MeshLod::Lod1);
            if mesher2::quad_count(&mesh) == 0 {
                continue;
            }
            sections.push(DemoSection {
                origin: [x * 32, j * 64, z * 32],
                scale: 2,
                mesh,
            });
        }
        sections.sort_by_key(|section| section.origin);
        out.insert((x, z), sections);
    }
    out
}

/// Derives and meshes the LOD2 sections of `regions` (`N1.4`, `N3.2`).
///
/// A LOD2 region is the `4x4` column square whose base is 4-aligned, and one
/// section covers `128` blocks, so three cover the column. Every section is
/// derived with [`lod::downsample4`] from 64 fine sections and meshed with
/// `mesh_coarse` at [`mesher2::MeshLod::Lod2`] (four blocks per cell), with a
/// skirt of two LOD2 cells (`8` blocks) on every side. As with LOD1, the
/// surrounding ring is derived too so a region culls against a real neighbour
/// when present and falls back to air at the ring edge.
#[allow(clippy::too_many_lines)]
fn demo_far_regions(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    materials: &mesher2::MaterialProps,
    pack: &content::ContentPack,
    regions: &BTreeSet<(i32, i32)>,
) -> BTreeMap<(i32, i32), Vec<DemoSection>> {
    if regions.is_empty() {
        return BTreeMap::new();
    }
    let water = pack.id("core:water").unwrap_or(0);
    let scatter: Vec<section::BlockId> = pack
        .records()
        .iter()
        .filter(|record| record.render_shape == content::RenderShape::CrossQuad)
        .filter_map(|record| pack.id(&record.name))
        .collect();

    let mut expanded = BTreeSet::new();
    for &(x, z) in regions {
        for dx in -1..=1_i32 {
            for dz in -1..=1_i32 {
                expanded.insert((x + 4 * dx, z + 4 * dz));
            }
        }
    }

    let mut derived: BTreeMap<(i32, i32, i32), section::Section> = BTreeMap::new();
    for &(x, z) in &expanded {
        let complete = (0..4).all(|dx| (0..4).all(|dz| columns.contains_key(&(x + dx, z + dz))));
        if !complete {
            continue;
        }
        for j in 0..(COLUMN_SECTIONS / 4) {
            let sections: [&section::Section; 64] = std::array::from_fn(|slot| {
                let dx = i32::try_from(slot & 3).unwrap_or(0);
                let dz = i32::try_from((slot >> 2) & 3).unwrap_or(0);
                let dy = (slot >> 4) & 3;
                &columns[&(x + dx, z + dz)][4 * j + dy]
            });
            if let Ok(coarse) = lod::downsample4(&sections, water, &scatter) {
                derived.insert((x, i32::try_from(j).unwrap_or(0), z), coarse);
            }
        }
    }

    let air = section::Section::uniform(0);
    let mut out: BTreeMap<(i32, i32), Vec<DemoSection>> = BTreeMap::new();
    for &(x, z) in regions {
        let mut sections = Vec::new();
        for j in 0..(COLUMN_SECTIONS / 4) {
            let j = i32::try_from(j).unwrap_or(0);
            let Some(coarse) = derived.get(&(x, j, z)) else {
                continue;
            };
            let neighbours: [Option<&section::Section>; 26] =
                padded::NEIGHBOUR_ORDER.map(|(dx, dy, dz)| {
                    derived
                        .get(&(x + dx * 4, j + dy, z + dz * 4))
                        .or(Some(&air))
                });
            let blocks = padded::PaddedBlocks::from_sections(coarse, &neighbours);
            let mut mesh = mesher2::mesh_coarse(&blocks, materials, mesher2::MeshLod::Lod2, 0);
            mesher2::add_skirts(&mut mesh, mesher2::SkirtEdges::ALL, mesher2::MeshLod::Lod2);
            if mesher2::quad_count(&mesh) == 0 {
                continue;
            }
            sections.push(DemoSection {
                origin: [x * 32, j * 128, z * 32],
                scale: 4,
                mesh,
            });
        }
        sections.sort_by_key(|section| section.origin);
        out.insert((x, z), sections);
    }
    out
}

/// Meshes one section, or `None` when it emits nothing.
///
/// A missing neighbour column contributes `None` to the 26-neighbour array,
/// which keeps `mesher2`'s `I2.3` world-edge mirror rule while a ring fills.
fn demo_section(
    columns: &BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>,
    light: &BTreeMap<(i32, i32), light2::ColumnLight>,
    materials: &mesher2::MaterialProps,
    sx: i32,
    sz: i32,
    sy: usize,
) -> Option<DemoSection> {
    let section = columns.get(&(sx, sz))?.get(sy)?;
    let neighbours: [Option<&section::Section>; 26] =
        padded::NEIGHBOUR_ORDER.map(|(dx, dy, dz)| {
            let ny = i32::try_from(sy).ok()? + dy;
            let neighbour = columns.get(&(sx + dx, sz + dz))?;
            neighbour.get(usize::try_from(ny).ok()?)
        });
    let light = demo_section_light(light, sx, sz, sy);
    let mesh = mesher2::mesh_from_neighbours(
        section,
        &neighbours,
        &light,
        materials,
        mesher2::MeshLod::Lod0,
    );
    if mesher2::quad_count(&mesh) == 0 {
        return None;
    }
    let section_blocks = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
    Some(DemoSection {
        origin: [
            sx * section_blocks,
            i32::try_from(sy).unwrap_or(0) * section_blocks,
            sz * section_blocks,
        ],
        scale: 1,
        mesh,
    })
}

/// The light table for the built-in v1 pack (`L1.1`, `L2.4`, `L2.1`).
///
/// `sky_atten` is the extra skylight loss (water is 1), `opacity` is the block-light
/// loss, and `emission` seeds block light. Ids come from the pack, so a record's
/// light properties are never hardcoded here.
fn demo_light_table() -> light2::LightTable {
    let pack = content::v1_default();
    let mut table = light2::LightTable::new(light2::LightProps::OPAQUE);
    for record in pack.records() {
        let Some(id) = pack.id(&record.name) else {
            continue;
        };
        table.set(
            id,
            light2::LightProps {
                sky_atten: record.sky_atten,
                block_opacity: record.opacity,
            },
        );
        if record.emission > 0 {
            table.set_emission(id, record.emission);
        }
    }
    table
}

/// Placeholder texture-array layer for a stable block name.
///
/// The v1 pack carries no texture refs (W22 note), so this is the only place the
/// demo maps names onto `renderer.rs`'s eleven procedural layers.
/// Maps a stable texture reference onto the renderer's procedural array.
///
/// The array is still placeholder art (`ADR-0007`, `ADR-0015`): a reference is
/// resolved by name, and an unknown name falls back to stone. Grass and wood
/// are the only blocks with three distinct faces today.
fn demo_texture_layer(name: &str) -> u16 {
    match name {
        "core:dirt" | "core:clay" | "core:grass_bottom" => 1,
        "core:grass_top" => 2,
        "core:grass_side" => 3,
        "core:wood_top" => 5,
        "core:wood_side" => 6,
        "core:leaves" | "core:plant" => 7,
        "core:lamp" => 8,
        "core:water" | "core:ice" => 9,
        "core:sand" | "core:snow" => 10,
        // Stone, bedrock, boulder, gravel, ores, and anything unknown.
        _ => 0,
    }
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

/// A column index to its generated sections. Shared with build jobs by `Arc`.
type ColumnMap = BTreeMap<(i32, i32), Arc<[section::Section; COLUMN_SECTIONS]>>;

/// Tables every ring job needs, cloned by `Arc`.
#[derive(Clone)]
struct RingAssets {
    materials: Arc<mesher2::MaterialProps>,
    light_table: Arc<light2::LightTable>,
    pack: Arc<content::ContentPack>,
}

impl RingAssets {
    fn from_demo(demo: &TerrainDemo) -> Self {
        Self {
            materials: Arc::new(demo.materials.clone()),
            light_table: Arc::new(demo_light_table()),
            pack: Arc::new(demo.pack.clone()),
        }
    }
}

/// One background column generation job.
struct GenerateJob {
    column: (i32, i32),
    /// Saved sections for this column, already replayed.
    saved: Vec<section::SavedSection>,
}

/// One background light/mesh job for a generated column.
struct BuildJob {
    centre: (i32, i32),
    /// Recompute light for the centre and every present neighbour.
    full: bool,
    /// Mesh the centre column at LOD0.
    detailed: bool,
    /// Columns within the build window, borrowed by `Arc`.
    columns: ColumnMap,
    /// Light the caller already had, for the section-diff test.
    previous_light: BTreeMap<(i32, i32), light2::ColumnLight>,
    /// Columns that currently have a fine mesh (or want one).
    fine_columns: BTreeSet<(i32, i32)>,
    materials: Arc<mesher2::MaterialProps>,
    light_table: Arc<light2::LightTable>,
}

/// One whole-ring coarse LOD rebuild.
struct CoarseJob {
    columns: ColumnMap,
    /// Player column the LOD1/LOD2 split was built for.
    player: (i32, i32),
    materials: Arc<mesher2::MaterialProps>,
    pack: Arc<content::ContentPack>,
}

/// A finished generated column.
struct GeneratedColumn {
    column: (i32, i32),
    stack: [section::Section; COLUMN_SECTIONS],
    applied: Vec<(section::SectionKey, [u8; 32])>,
    read_only: bool,
}

/// A finished light/mesh pass.
struct BuiltColumn {
    centre: (i32, i32),
    snapshot: ColumnMap,
    light: BTreeMap<(i32, i32), light2::ColumnLight>,
    /// Whole-column replacements (the centre when it was meshed).
    replace: BTreeMap<(i32, i32), Vec<DemoSection>>,
    /// Per-section replacements for neighbours whose border changed.
    patches: BTreeMap<(i32, i32), Vec<DemoPatch>>,
    light_ms: f64,
    mesh_ms: f64,
}

/// A finished coarse rebuild.
struct CoarseBuilt {
    snapshot: ColumnMap,
    player: (i32, i32),
    /// LOD1 sections for the middle band.
    coarse: Vec<DemoSection>,
    /// LOD2 sections for the far band.
    far: Vec<DemoSection>,
}

enum PoolJob {
    Generate(GenerateJob),
    Build(BuildJob),
    Coarse(CoarseJob),
}

enum PoolResult {
    Generated(
        (i32, i32),
        Box<Result<GeneratedColumn, error::TerrainError>>,
    ),
    Built(BuiltColumn),
    Coarse(CoarseBuilt),
}

/// A fixed fan-out of workers, one `Terrain` each (`F1.5`, `A7.1`).
struct ColumnPool {
    jobs: std::sync::mpsc::Sender<PoolJob>,
    results: std::sync::mpsc::Receiver<PoolResult>,
}

impl ColumnPool {
    fn new(seed: u64, world_hydro: &Arc<crate::hydro::WorldHydro>, workers: usize) -> Self {
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

    fn submit(&self, job: PoolJob) -> bool {
        self.jobs.send(job).is_ok()
    }

    fn try_recv(&self) -> Option<PoolResult> {
        self.results.try_recv().ok()
    }
}

/// Generates one column and layers saved sections over the generated baseline.
fn generate_column(
    terrain: Option<&pipeline::Terrain>,
    job: &GenerateJob,
) -> Result<GeneratedColumn, error::TerrainError> {
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
    })
}

/// The eight horizontal neighbours, in `ColumnView` order.
const COLUMN_NEIGHBOURS: [(i32, i32); 8] = [
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
fn build_column(job: BuildJob) -> BuiltColumn {
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
        snapshot: job.columns,
        light,
        replace,
        patches,
        light_ms,
        mesh_ms,
    }
}

/// Builds the coarse LOD sets for the whole resident ring.
fn build_coarse(job: CoarseJob) -> CoarseBuilt {
    let (coarse, far) = demo_lod_sets(&job.columns, &job.materials, &job.pack, job.player);
    debug!(
        player = ?job.player,
        coarse = coarse.len(),
        far = far.len(),
        columns = job.columns.len(),
        "terrain coarse built"
    );
    CoarseBuilt {
        snapshot: job.columns,
        player: job.player,
        coarse,
        far,
    }
}

/// Whether a section's block border toward `delta` differs from `a`'s mirror
/// edge, which is what a newly generated column can change.
fn border_section_changed(
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

/// Copies the columns in `centre +- margin` for a background job.
fn snapshot_around(columns: &ColumnMap, centre: (i32, i32), margin: i32) -> ColumnMap {
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

/// Whether the 5x5 window around `centre` still matches a job's snapshot.
fn neighbourhood_intact(columns: &ColumnMap, snapshot: &ColumnMap, centre: (i32, i32)) -> bool {
    for dx in -2..=2 {
        for dz in -2..=2 {
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
fn coarse_snapshot_intact(columns: &ColumnMap, snapshot: &ColumnMap) -> bool {
    snapshot
        .iter()
        .all(|(key, stack)| columns.get(key).is_some_and(|now| Arc::ptr_eq(now, stack)))
}

/// Streaming state for the column ring.
#[allow(clippy::struct_excessive_bools)]
struct RingStream {
    pool: ColumnPool,
    /// Every column with a generation or build job running.
    in_flight: BTreeSet<(i32, i32)>,
    /// Saved sections grouped by the column generation will need.
    saved: BTreeMap<(i32, i32), Vec<section::SavedSection>>,
    assets: RingAssets,
    /// Coarse LOD needs a rebuild after a column or edit change.
    coarse_dirty: bool,
    /// A coarse job is running.
    coarse_busy: bool,
    last_coarse: Instant,
    /// The renderer needs a terrain submission.
    upload_dirty: bool,
    last_upload: Instant,
    /// Whether the player has been placed on the centre column.
    spawned: bool,
    /// Logs one line when the ring first fills.
    announced_full: bool,
}

impl RingStream {
    fn new(demo: &TerrainDemo, saved: Vec<section::SavedSection>, workers: usize) -> Self {
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

/// The save key for a world cell (`J4.1`).
fn section_key(cell: IVec3) -> section::SectionKey {
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
fn queue_demo_edit(
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

/// The result of replaying recovered journal frames over the saved sections.
struct ReplayOutcome {
    saved: Vec<section::SavedSection>,
    revision: u64,
    read_only: bool,
    /// Saved section records the world holds.
    sections: usize,
    /// Journal edits this replay applied.
    applied: usize,
}

/// Replays recovered journal frames over saved sections before the ring opens.
///
/// Each frame is applied to the section record it covers, in revision order, so
/// a section file is ahead of the journal exactly like the normal edit path.
/// Frames for a column that has no saved record are left in the journal: the
/// header does not advance past them. Unknown block names make the world
/// read-only (`A16`).
#[allow(clippy::too_many_lines)]
fn replay_saved_sections(
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
struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    mesher: Option<Arc<dyn MeshPool>>,
    queue: MeshQueue,
    config: Config,
    streaming: Option<Streaming>,
    seed: Option<u64>,
    terrain_demo: Option<TerrainDemo>,
    /// Background writer for the v1 terrain world, when the demo has one.
    terrain_save: Option<world_save::SaveWorker>,
    /// Column-ring streaming state and workers.
    terrain_ring: Option<RingStream>,
    /// A corrupt section blocking saves until the player chooses an action.
    repair_prompt: Option<RepairPrompt>,
    /// In-flight background rebuild of the patch.
    demo_build: Option<std::sync::mpsc::Receiver<DemoRebuild>>,
    /// Edits waiting for the in-flight rebuild to finish, so clicks coalesce.
    demo_edits: Vec<DemoEdit>,
    save_directory: Option<PathBuf>,
    level_work: HashSet<ChunkCoord>,
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
    /// Last window-relative cursor position, for the repair prompt.
    cursor_position: (f64, f64),
    modifiers: ModifiersState,
    jump_requested: bool,
    highlighted_block: Option<IVec3>,
    focused: bool,
    occluded: bool,
    show_overlay: bool,
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

    fn request_initial_meshes(&mut self) {
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
    fn request_levels(&mut self, pool: &dyn MeshPool) {
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

    fn upload_finished_meshes(&mut self, pool: &dyn MeshPool) {
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
                !demo.light.contains_key(&key)
                    || (ring <= demo.fine_radius && !demo.fine.contains_key(&key))
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
                    columns: snapshot_around(&demo.columns, column, 2),
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
            let job = PoolJob::Coarse(CoarseJob {
                columns: demo.columns.clone(),
                player: player_column,
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
                    if neighbourhood_intact(&demo.columns, &built.snapshot, built.centre) {
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
                        demo.coarse = coarse.coarse;
                        demo.far = coarse.far;
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

    use super::*;
    use crate::generation::GenResult;
    use crate::player::BlockSource;
    use mesher::{Job, JobResult, MeshResult};

    #[test]
    fn demo_materials_map_each_face_from_the_pack() {
        let pack = content::v1_default();
        let props = demo_materials();
        let tiles = |name: &str| {
            let material = props.material(pack.id(name).expect("the pack holds the block"));
            (
                material.texture_top,
                material.texture_side,
                material.texture_bottom,
            )
        };

        assert_eq!(tiles("core:grass"), (2, 3, 1));
        assert_eq!(tiles("core:wood"), (5, 6, 5));
        assert_eq!(tiles("core:stone"), (0, 0, 0));
        assert_eq!(tiles("core:sand"), (10, 10, 10));
    }

    #[test]
    fn mesher_reads_an_identity_light_field() {
        // Decisive split: if a full-sky PaddedLight still yields sky 0, the
        // mesher indexes light wrongly; if it yields 15, the light field is bad.
        let demo = build_terrain_demo(0x5EED, IVec2::new(2, -3), 0).expect("the v1 terrain builds");
        let props = demo_materials();
        let light = padded::PaddedLight::new(15, 0);
        let mut seen = 0;
        for (&(sx, sz), stack) in &demo.columns {
            for (sy, section) in stack.iter().enumerate() {
                let sy_i32 = i32::try_from(sy).unwrap_or(0);
                let neighbours: [Option<&section::Section>; 26] =
                    padded::NEIGHBOUR_ORDER.map(|(dx, dy, dz)| {
                        let ny = i32::try_from(sy).ok()? + dy;
                        demo.columns
                            .get(&(sx + dx, sz + dz))?
                            .get(usize::try_from(ny).ok()?)
                    });
                let mesh = mesher2::mesh_from_neighbours(
                    section,
                    &neighbours,
                    &light,
                    &props,
                    mesher2::MeshLod::Lod0,
                );
                for quad in &mesh.opaque {
                    assert_eq!(quad.sky(), 15, "identity light must read as sky 15");
                    seen += 1;
                }
                let _ = sy_i32;
            }
        }
        assert!(seen > 0, "the fixture emits quads");
    }

    #[test]
    fn section_light_uses_the_neighbour_on_the_border() {
        let demo = build_terrain_demo(0x5EED, IVec2::new(2, -3), 1).expect("the v1 terrain builds");
        let lit = demo_column_light(&demo.columns);
        let mut checked = false;
        for &(sx, sz) in demo.columns.keys() {
            let Some(neighbour) = lit.get(&(sx + 1, sz)) else {
                continue;
            };
            let sy = 4_usize;
            let padded = demo_section_light(&lit, sx, sz, sy);
            let base = i32::try_from(sy).unwrap_or(0) * 32;
            for y in 0..32 {
                for z in 0..32 {
                    // Padded column 32 is the first cell of the +x neighbour.
                    assert_eq!(
                        padded.get(32, y, z).0,
                        neighbour.sky(0, base + y, z),
                        "border sky must come from the neighbour, not the mirror"
                    );
                }
            }
            checked = true;
            break;
        }
        assert!(checked, "the fixture has a column with a +x neighbour");
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn every_exposed_face_is_covered_once() {
        // `M1.1`: a greedy mesh must cover each exposed face exactly once. A miss
        // is a pinhole (sky through a wall); a double is coplanar z-fighting. The
        // grid is per face plane, because two walls at different `x` share a
        // `(y, z)`.
        const FACES: [(i32, i32, i32); 6] = [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ];
        let demo = build_terrain_demo(0x5EED, IVec2::new(2, -3), 0).expect("the v1 terrain builds");
        let props = demo_materials();
        let mut holes = 0_u32;
        let mut doubles = 0_u32;
        let mut samples: Vec<String> = Vec::new();
        for (&(sx, sz), stack) in &demo.columns {
            for (sy, _) in stack.iter().enumerate() {
                let sy_i32 = i32::try_from(sy).unwrap_or(0);
                let mut exposed = vec![false; 6 * 32 * 1024];
                for y in 0..32 {
                    for z in 0..32 {
                        for x in 0..32 {
                            let cell = IVec3::new(sx * 32 + x, sy_i32 * 32 + y, sz * 32 + z);
                            let id = demo.block_at(cell).unwrap_or(0);
                            if !props.material(id).opaque {
                                continue;
                            }
                            for (face, (dx, dy, dz)) in FACES.iter().enumerate() {
                                if face == 3 && cell.y == 0 {
                                    continue;
                                }
                                let neighbour = cell + IVec3::new(*dx, *dy, *dz);
                                let column =
                                    (neighbour.x.div_euclid(32), neighbour.z.div_euclid(32));
                                if !demo.columns.contains_key(&column) {
                                    continue;
                                }
                                if props.material(demo.block_at(neighbour).unwrap_or(0)).opaque {
                                    continue;
                                }
                                let (plane, index) = match face {
                                    0 | 1 => (x, z + y * 32),
                                    2 | 3 => (y, x + z * 32),
                                    _ => (z, x + y * 32),
                                };
                                let plane = usize::try_from(plane).unwrap_or(0);
                                let index = usize::try_from(index).unwrap_or(0);
                                exposed[(face * 32 + plane) * 1024 + index] = true;
                            }
                        }
                    }
                }
                let Some(section) = demo
                    .sections
                    .iter()
                    .find(|section| section.origin == [sx * 32, sy_i32 * 32, sz * 32])
                else {
                    let count = exposed.iter().filter(|seen| **seen).count();
                    if count > 0 {
                        eprintln!("missing section ({sx},{sy},{sz}) with {count} exposed");
                    }
                    holes += u32::try_from(count).unwrap_or(0);
                    continue;
                };
                let mut covered = vec![0_u16; 6 * 32 * 1024];
                for quad in &section.mesh.opaque {
                    let face = usize::from(quad.normal());
                    if face >= 6 {
                        continue;
                    }
                    let (ow, oh) = (u32::from(quad.w1()) + 1, u32::from(quad.h1()) + 1);
                    let (plane, ou, ov) = match face {
                        0 | 1 => (
                            u32::from(quad.x()),
                            u32::from(quad.z()),
                            u32::from(quad.y()),
                        ),
                        2 | 3 => (
                            u32::from(quad.y()),
                            u32::from(quad.x()),
                            u32::from(quad.z()),
                        ),
                        _ => (
                            u32::from(quad.z()),
                            u32::from(quad.x()),
                            u32::from(quad.y()),
                        ),
                    };
                    for v in ov..ov + oh {
                        for u in ou..ou + ow {
                            let index = usize::try_from(u + v * 32).unwrap_or(0);
                            if index < 1024 {
                                let plane_index = (face * 32 + usize::try_from(plane).unwrap_or(0))
                                    * 1024
                                    + index;
                                covered[plane_index] += 1;
                            }
                        }
                    }
                }
                for face in 0..6 {
                    for plane in 0..32 {
                        for index in 0..1024 {
                            let slot = (face * 32 + plane) * 1024 + index;
                            let want = exposed[slot];
                            let got = covered[slot];
                            if want && got == 0 {
                                holes += 1;
                                if samples.len() < 12 {
                                    samples.push(format!(
                                        "hole face {face} section ({sx},{sy},{sz}) plane {plane} index {index}"
                                    ));
                                }
                            } else if !want && got > 0 {
                                doubles += 1;
                                if samples.len() < 12 {
                                    samples.push(format!(
                                        "stray face {face} section ({sx},{sy},{sz}) plane {plane} index {index}"
                                    ));
                                }
                            } else if want && got > 1 {
                                doubles += 1;
                                if samples.len() < 12 {
                                    samples.push(format!(
                                        "overlap x{got} face {face} section ({sx},{sy},{sz}) plane {plane} index {index}"
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
        for sample in &samples {
            eprintln!("{sample}");
        }
        assert_eq!(holes, 0, "exposed faces with no quad");
        assert_eq!(doubles, 0, "faces covered more than once or never exposed");
    }
    #[test]
    fn demo_raycast_hits_the_spawn_surface() {
        let demo = build_terrain_demo(0x5EED, IVec2::new(2, -3), 0).expect("the v1 terrain builds");
        let origin = demo.spawn + Vec3::Y * 20.0;
        let hit = raycast_where(|cell| demo.is_solid(cell), origin, -Vec3::Y, 64.0)
            .expect("a downward ray hits the patch");
        #[allow(clippy::cast_possible_truncation)]
        let expected = demo.spawn.y.round() as i32 - 1;
        assert_eq!(hit.block.y, expected);
        assert_eq!(hit.normal, IVec3::Y);
    }

    #[test]
    fn lod_uses_coarse_regions_away_from_the_player() {
        let mut demo =
            build_terrain_demo(0x5EED, IVec2::new(2, -3), 2).expect("the v1 terrain builds");
        assert!(
            !demo.coarse.is_empty(),
            "a radius-2 patch has complete octant regions"
        );
        demo.fine_radius = 0;
        let fine: usize = demo.fine.values().map(Vec::len).sum();
        demo.select_lod((demo.origin_column.x, demo.origin_column.y));
        let near_fine: usize = demo
            .fine
            .iter()
            .filter(|(key, _)| {
                let (x, z) = **key;
                let (px, pz) = (demo.origin_column.x, demo.origin_column.y);
                (x - px).abs().max((z - pz).abs()) <= 1
            })
            .map(|(_, sections)| sections.len())
            .sum();
        assert_eq!(
            demo.sections.len(),
            near_fine,
            "the player's column and its fine ring are full detail"
        );

        demo.select_lod((demo.origin_column.x + 2, demo.origin_column.y + 2));
        assert!(
            demo.sections.len() < fine,
            "a region two columns away is drawn coarse: {} active vs {fine} fine",
            demo.sections.len()
        );

        let coarse: BTreeSet<[i32; 3]> = demo.coarse.iter().map(|section| section.origin).collect();
        let active_fine = demo
            .sections
            .iter()
            .filter(|section| !coarse.contains(&section.origin))
            .count();
        let active_coarse = demo.sections.len() - active_fine;
        assert!(active_coarse > 0, "at least one coarse section is active");
        assert!(active_fine > 0, "the player's region stays fine");

        let mut origins = BTreeSet::new();
        for section in &demo.sections {
            assert!(
                origins.insert(section.origin),
                "fine and coarse sections never overlap"
            );
        }
        assert!(
            demo.coarse
                .iter()
                .any(|section| !section.mesh.skirts.is_empty()),
            "coarse meshes carry skirts for the fine boundary"
        );
    }

    fn stub_section(origin: [i32; 3], scale: u32) -> DemoSection {
        DemoSection {
            origin,
            scale,
            mesh: mesher2::SectionMesh::default(),
        }
    }

    #[test]
    fn lod2_selects_the_far_band_without_overlapping_lod1() {
        let mut demo =
            build_terrain_demo(0x5EED, IVec2::new(0, 0), 0).expect("the v1 terrain builds");
        demo.origin_column = IVec2::ZERO;
        demo.fine.clear();
        demo.coarse = vec![
            stub_section([4 * 32, 0, 4 * 32], 2),
            stub_section([8 * 32, 0, 8 * 32], 2),
            stub_section([10 * 32, 0, 10 * 32], 2),
        ];
        demo.far = vec![
            stub_section([8 * 32, 0, 8 * 32], 4),
            stub_section([0, 0, 0], 4),
        ];
        demo.select_lod((0, 0));

        let origins: BTreeSet<[i32; 3]> = demo.sections.iter().map(|s| s.origin).collect();
        assert!(origins.contains(&[4 * 32, 0, 4 * 32]), "middle LOD1 region");
        assert!(
            origins.contains(&[8 * 32, 0, 8 * 32]),
            "far LOD2 region replaces its LOD1 copy"
        );
        assert!(
            !origins.contains(&[10 * 32, 0, 10 * 32]),
            "LOD1 inside an active LOD2 square is suppressed"
        );
        assert!(
            !origins.contains(&[0, 0, 0]),
            "LOD2 touching the fine buffer is suppressed"
        );
        assert_eq!(demo.sections.len(), 2);
        assert_eq!(
            demo.far
                .iter()
                .find(|s| s.origin == [8 * 32, 0, 8 * 32])
                .map(|s| s.scale),
            Some(4)
        );
    }

    #[test]
    fn lod2_regions_are_complete_and_skirted() {
        let mut columns = BTreeMap::new();
        for x in 0..4 {
            for z in 0..4 {
                let stack: [section::Section; COLUMN_SECTIONS] =
                    std::array::from_fn(|_| section::Section::uniform(1));
                columns.insert((x, z), Arc::new(stack));
            }
        }
        let regions = BTreeSet::from([(0, 0)]);
        let far = demo_far_regions(
            &columns,
            &demo_materials(),
            &content::v1_default(),
            &regions,
        );
        let sections = far.get(&(0, 0)).expect("the 4x4 region is complete");
        assert_eq!(sections.len(), 3, "one 128-block section per third");
        assert_eq!(
            sections.iter().map(|s| s.origin).collect::<Vec<_>>(),
            vec![[0, 0, 0], [0, 128, 0], [0, 256, 0]]
        );
        for section in sections {
            assert_eq!(section.scale, 4);
            assert!(mesher2::quad_count(&section.mesh) > 0);
            assert!(
                !section.mesh.skirts.is_empty(),
                "LOD2 sections are skirted on every side"
            );
        }
        assert!(
            demo_far_regions(
                &columns,
                &demo_materials(),
                &content::v1_default(),
                &BTreeSet::from([(4, 4)])
            )
            .get(&(4, 4))
            .is_none_or(Vec::is_empty),
            "a region with missing columns is not derived"
        );
    }

    #[test]
    fn coarse_bands_follow_a_smaller_ring() {
        let columns = |radius: i32| {
            let mut columns = BTreeMap::new();
            for x in -radius..=radius {
                for z in -radius..=radius {
                    let stack: [section::Section; COLUMN_SECTIONS] =
                        std::array::from_fn(|_| section::Section::uniform(0));
                    columns.insert((x, z), Arc::new(stack));
                }
            }
            columns
        };

        // Default 12: complete squares at base 8 and -12 are far.
        let (coarse12, far12) = coarse_bands(&columns(12), (0, 0));
        for base in [(8, 8), (8, -12), (-12, 8), (-12, -12)] {
            assert!(far12.contains(&base), "missing far base {base:?}");
        }
        assert!(!coarse12.is_empty(), "the middle band still owns regions");

        // Radius 8: no 4x4 square is both complete and beyond the LOD1 radius.
        let (coarse8, far8) = coarse_bands(&columns(8), (0, 0));
        assert!(far8.is_empty(), "LOD2 has no complete region at radius 8");
        assert!(!coarse8.is_empty(), "the ring falls back to LOD1");
    }

    #[test]
    fn the_demo_tile_is_lod_selected_at_spawn() {
        let (mut demo, replay) =
            build_terrain_demo_extent(0x5EED, IVec2::new(0, 0), 0, 3, 0, 3, &[], &[], 0)
                .expect("the one-tile demo builds");
        assert_eq!(replay.keys, Vec::<section::SectionKey>::new());
        assert_eq!(
            demo.columns.keys().min(),
            Some(&(0, 0)),
            "the tile is one 128-block generation unit"
        );
        assert_eq!(demo.columns.keys().max(), Some(&(3, 3)));
        let fine: usize = demo.fine.values().map(Vec::len).sum();
        demo.fine_radius = 0;
        demo.select_lod((0, 0));
        assert!(
            demo.sections.len() < fine,
            "the far corner is coarse at spawn: {} active vs {fine} fine",
            demo.sections.len(),
        );
    }

    #[test]
    fn saved_columns_replay_into_the_ring() {
        let mut saved = Vec::new();
        let pack = content::v1_default();
        let mut section = section::Section::uniform(0);
        let key = section::SectionKey {
            column: [0, 0],
            section: 0,
        };
        section.set(1, 1, 1, pack.id("core:stone").expect("stone"));
        saved.push(section::SavedSection {
            key,
            edited: true,
            baseline_hash: section::baseline_hash(&section::Section::uniform(0)),
            section,
            unknown_names: Vec::new(),
        });
        let replayed = replay_saved_sections(saved, &[], 0, &pack, None, None);
        assert_eq!(replayed.sections, 1);
        assert_eq!(replayed.revision, 0);
        assert!(!replayed.read_only);
        assert_eq!(
            replayed.saved[0].section.get(1, 1, 1),
            pack.id("core:stone").expect("stone")
        );
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn a_journal_frame_ahead_of_the_section_store_regenerates_it() {
        let seed = 0x5EED;
        let pack = content::v1_default();
        let terrain = pipeline::Terrain::new(seed).expect("the description compiles");
        let section = terrain.section(0, 0, 0).expect("the section generates");
        let mut cell = None;
        for y in 0..32 {
            for z in 0..32 {
                for x in 0..32 {
                    if section.get(x, y, z) != 0 {
                        cell = Some((x, y, z));
                        break;
                    }
                }
                if cell.is_some() {
                    break;
                }
            }
            if cell.is_some() {
                break;
            }
        }
        let (x, y, z) = cell.expect("the spawn section has a solid block");
        let old = pack
            .name(section.get(x, y, z))
            .expect("the block has a name");
        let record = editlog::Edit::record(
            [
                i32::try_from(x).unwrap_or(0),
                i32::try_from(y).unwrap_or(0),
                i32::try_from(z).unwrap_or(0),
            ],
            old,
            "core:air",
            1,
            0,
            0,
            0,
        )
        .expect("the edit records");
        let batch = editlog::EditBatch::new(1, 0, vec![record]);
        let replayed = replay_saved_sections(
            Vec::new(),
            std::slice::from_ref(&batch),
            0,
            &pack,
            None,
            Some(&terrain),
        );
        assert_eq!(replayed.applied, 1);
        assert_eq!(replayed.revision, 1);
        assert_eq!(replayed.sections, 1, "the missing section was regenerated");
        assert_eq!(replayed.saved[0].section.get(x, y, z), 0);
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
    fn demo_build_phase_split() {
        let started = Instant::now();
        let terrain = pipeline::Terrain::new(0x5EED).expect("the v1 description compiles");
        let terrain_ms = started.elapsed().as_secs_f64() * 1000.0;

        let tiled = Instant::now();
        terrain.tile(0, 0).expect("the tile builds");
        let tile_ms = tiled.elapsed().as_secs_f64() * 1000.0;

        let generated = Instant::now();
        let hydro = pipeline::Terrain::new(0x5EED)
            .expect("the v1 description compiles")
            .solve_world_hydro()
            .expect("the world hydrology solves");
        let columns: BTreeMap<_, _> = demo_columns_extent(0x5EED, 0, 3, 0, 3, &hydro)
            .expect("the tile generates")
            .into_iter()
            .map(|(key, stack)| (key, Arc::new(stack)))
            .collect();
        let generate_ms = generated.elapsed().as_secs_f64() * 1000.0;

        let lit = Instant::now();
        let light = demo_column_light(&columns);
        let light_ms = lit.elapsed().as_secs_f64() * 1000.0;

        let meshed = Instant::now();
        let materials = demo_materials();
        let fine = demo_fine_all(&columns, &light, &materials);
        let sections: usize = fine.values().map(Vec::len).sum();
        let quads: usize = fine
            .values()
            .flatten()
            .map(|section| mesher2::quad_count(&section.mesh))
            .sum();
        let mesh_ms = meshed.elapsed().as_secs_f64() * 1000.0;

        let coarse_started = Instant::now();
        let coarse = demo_coarse(&columns, &materials, &content::v1_default());
        let coarse_ms = coarse_started.elapsed().as_secs_f64() * 1000.0;

        eprintln!(
            "demo build: terrain={terrain_ms:.0}ms tile={tile_ms:.0}ms generate={generate_ms:.0}ms light={light_ms:.0}ms mesh={mesh_ms:.0}ms coarse={coarse_ms:.0}ms | {sections} fine, {} coarse, {quads} quads",
            coarse.len()
        );
        assert!(quads > 0);
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn demo_rebuild_cost_split() {
        let mut demo =
            build_terrain_demo(0x5EED, IVec2::new(2, -3), 1).expect("the v1 terrain builds");
        let cell = IVec3::new(
            demo.spawn.x.floor() as i32,
            demo.spawn.y.round() as i32 - 1,
            demo.spawn.z.floor() as i32,
        );
        let block = demo.air;
        assert!(demo.set_block(cell, block));
        let rebuild = demo_rebuild(
            &demo.columns,
            &demo.light,
            &demo.materials,
            &[DemoEdit { cell }],
        );
        eprintln!(
            "rebuild: workers={} light={:.1}ms mesh={:.1}ms patches={} ({} sections at startup)",
            worker_count(),
            rebuild.light_time.as_secs_f64() * 1000.0,
            rebuild.mesh_time.as_secs_f64() * 1000.0,
            rebuild.patches.len(),
            demo.sections.len()
        );
        assert!(!rebuild.patches.is_empty());
        assert!(
            rebuild.patches.len() <= 8,
            "an interior edit touches only its section and adjacent borders, not a 3x3x3 window: {} patches",
            rebuild.patches.len()
        );
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn incremental_rebuild_matches_a_full_rebuild() {
        let mut demo =
            build_terrain_demo(0x5EED, IVec2::new(2, -3), 1).expect("the v1 terrain builds");
        let center = IVec3::new(
            demo.spawn.x.floor() as i32,
            demo.spawn.y.round() as i32 - 1,
            demo.spawn.z.floor() as i32,
        );
        let above = center + IVec3::Y * 3;
        let border_x = demo.origin_column.x * 32 + 31;
        let border_z = demo.spawn.z.floor() as i32;
        let border = (0..=demo.spawn.y.round() as i32)
            .rev()
            .map(|y| IVec3::new(border_x, y, border_z))
            .find(|cell| demo.is_solid(*cell))
            .expect("the column edge has ground");
        let stone = content::v1_default()
            .id("core:stone")
            .expect("the v1 pack has stone");

        let edits = [
            (center, demo.air),
            (above, stone),
            (above, demo.air),
            (border, demo.air),
        ];
        for (index, (cell, block)) in edits.into_iter().enumerate() {
            assert!(
                demo.set_block(cell, block),
                "edit {index} changes the patch"
            );
            assert_incremental_matches_full(&mut demo, &[DemoEdit { cell }], index);
        }
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn coalesced_edits_match_a_full_rebuild() {
        let mut demo =
            build_terrain_demo(0x5EED, IVec2::new(2, -3), 1).expect("the v1 terrain builds");
        let center = IVec3::new(
            demo.spawn.x.floor() as i32,
            demo.spawn.y.round() as i32 - 1,
            demo.spawn.z.floor() as i32,
        );
        let neighbour_x = center.x + 32;
        let neighbour = (0..=center.y)
            .rev()
            .map(|y| IVec3::new(neighbour_x, y, center.z))
            .find(|cell| demo.is_solid(*cell))
            .expect("the next column has ground");
        let air = demo.air;
        assert!(demo.set_block(center, air));
        assert!(demo.set_block(neighbour, air));
        let edits = [DemoEdit { cell: center }, DemoEdit { cell: neighbour }];
        assert_incremental_matches_full(&mut demo, &edits, 0);
    }

    /// Rebuilds one batch and checks it against a full relight and remesh.
    fn assert_incremental_matches_full(demo: &mut TerrainDemo, edits: &[DemoEdit], label: usize) {
        let rebuild = demo_rebuild(&demo.columns, &demo.light, &demo.materials, edits);

        let expected_light = demo_column_light(&demo.columns);
        assert_eq!(
            rebuild.light, expected_light,
            "edit {label}: the incremental repair matches a full relight"
        );

        demo.apply_rebuild(rebuild);
        let expected: BTreeMap<[i32; 3], mesher2::SectionMesh> =
            demo_fine_all(&demo.columns, &expected_light, &demo.materials)
                .into_values()
                .flatten()
                .map(|section| (section.origin, section.mesh))
                .collect();
        let actual: BTreeMap<[i32; 3], mesher2::SectionMesh> = demo
            .sections
            .iter()
            .map(|section| (section.origin, section.mesh.clone()))
            .collect();
        assert_eq!(
            actual, expected,
            "edit {label}: the incremental mesh matches a full remesh"
        );
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn a_newer_journal_frame_is_replayed_over_the_saved_section() {
        let demo = build_terrain_demo(0x5EED, IVec2::new(2, -3), 0).expect("the v1 terrain builds");
        let cell = IVec3::new(
            demo.spawn.x.floor() as i32,
            demo.spawn.y.round() as i32 - 1,
            demo.spawn.z.floor() as i32,
        );
        let key = section_key(cell);
        let old = demo.block_at(cell).expect("the spawn block exists");
        let old_name = demo
            .pack
            .name(old)
            .expect("the block has a name")
            .to_owned();
        let new_name = demo.pack.name(demo.air).expect("air has a name").to_owned();
        let record =
            editlog::Edit::record([cell.x, cell.y, cell.z], old_name, new_name, 1, 0, 0, 0)
                .expect("the change is recorded");
        let batch = editlog::EditBatch::new(1, 0, vec![record]);

        let (loaded, replay) = build_terrain_demo_saved(
            demo.seed,
            demo.origin_column,
            0,
            &[],
            std::slice::from_ref(&batch),
            0,
        )
        .expect("the replayed patch builds");
        assert_eq!(loaded.block_at(cell), Some(demo.air));
        assert_eq!(replay.keys, vec![key]);
        assert_eq!(replay.revision, 1);
        assert_eq!(loaded.revision, 1);
        assert_ne!(
            loaded.baselines.get(&key).copied().unwrap_or([0; 32]),
            [0; 32],
            "the replay derives the generated baseline"
        );

        let (covered, replay) =
            build_terrain_demo_saved(demo.seed, demo.origin_column, 0, &[], &[batch], 1)
                .expect("the covered patch builds");
        assert_eq!(covered.block_at(cell), Some(old));
        assert_eq!(replay.keys.len(), 0);
        assert_eq!(replay.revision, 1);
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn saved_sections_override_generation_when_the_baseline_matches() {
        let mut demo =
            build_terrain_demo(0x5EED, IVec2::new(2, -3), 0).expect("the v1 terrain builds");
        let cell = IVec3::new(
            demo.spawn.x.floor() as i32,
            demo.spawn.y.round() as i32 - 1,
            demo.spawn.z.floor() as i32,
        );
        let key = section_key(cell);
        let baseline = demo.baseline_for(key);
        let original = demo.block_at(cell).expect("the spawn block exists");
        let air = demo.air;
        assert!(demo.set_block(cell, air));
        let section =
            demo.columns[&(key.column[0], key.column[1])][usize::from(key.section)].clone();
        let saved = section::SavedSection {
            key,
            edited: true,
            baseline_hash: baseline,
            section,
            unknown_names: Vec::new(),
        };

        let (loaded, replay) =
            build_terrain_demo_saved(demo.seed, demo.origin_column, 0, &[saved], &[], 0)
                .expect("the saved patch builds");
        assert_eq!(loaded.block_at(cell), Some(air));
        assert_eq!(replay.keys.len(), 0);
        assert_ne!(original, air, "the generated block was solid");
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn a_saved_section_with_another_baseline_is_ignored() {
        let mut demo =
            build_terrain_demo(0x5EED, IVec2::new(2, -3), 0).expect("the v1 terrain builds");
        let cell = IVec3::new(
            demo.spawn.x.floor() as i32,
            demo.spawn.y.round() as i32 - 1,
            demo.spawn.z.floor() as i32,
        );
        let key = section_key(cell);
        let original = demo.block_at(cell).expect("the spawn block exists");
        let air = demo.air;
        assert!(demo.set_block(cell, air));
        let section =
            demo.columns[&(key.column[0], key.column[1])][usize::from(key.section)].clone();
        let saved = section::SavedSection {
            key,
            edited: true,
            baseline_hash: [0; 32],
            section,
            unknown_names: Vec::new(),
        };

        let (loaded, replay) =
            build_terrain_demo_saved(demo.seed, demo.origin_column, 0, &[saved], &[], 0)
                .expect("the saved patch builds");
        assert_eq!(loaded.block_at(cell), Some(original));
        assert_eq!(replay.keys.len(), 0);
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn an_edit_queues_a_journal_batch_and_a_section_save() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "valescent-demo-save-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&directory);
        {
            let mut demo =
                build_terrain_demo(0x5EED, IVec2::new(2, -3), 0).expect("the v1 terrain builds");
            let world = world_save::WorldSave::open_or_create(&directory, demo.seed, "Demo test")
                .expect("the world opens");
            let save = world_save::SaveWorker::start(world, 8).expect("the worker starts");

            let cell = IVec3::new(
                demo.spawn.x.floor() as i32,
                demo.spawn.y.round() as i32 - 1,
                demo.spawn.z.floor() as i32,
            );
            let key = section_key(cell);
            let baseline = demo.baseline_for(key);
            let old = demo.block_at(cell).expect("the spawn block exists");
            let old_name = demo
                .pack
                .name(old)
                .expect("the block has a name")
                .to_owned();
            let new_name = demo.pack.name(demo.air).expect("air has a name").to_owned();
            assert!(demo.set_block(cell, demo.air));
            queue_demo_edit(&save, &mut demo, cell, key, baseline, old_name, new_name);
            let revision = demo.revision;
            save.shutdown().expect("the worker stops");

            let reopened =
                world_save::WorldSave::open(&directory, demo.seed).expect("the world reopens");
            assert_eq!(reopened.header.revision, revision);
            assert!(
                reopened.recovered().is_empty(),
                "the section save covers and trims the batch"
            );
            let sections = reopened.read_sections().expect("the sections read");
            assert_eq!(sections.len(), 1);
            assert_eq!(sections[0].key, key);
            let size = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
            assert_eq!(
                sections[0].section.get(
                    u32::try_from(cell.x.rem_euclid(size)).unwrap_or(0),
                    u32::try_from(cell.y.rem_euclid(size)).unwrap_or(0),
                    u32::try_from(cell.z.rem_euclid(size)).unwrap_or(0),
                ),
                demo.air
            );
        }
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn demo_edit_breaks_a_block_and_relights() {
        let mut demo =
            build_terrain_demo(0x5EED, IVec2::new(2, -3), 0).expect("the v1 terrain builds");
        let cell = IVec3::new(
            demo.spawn.x.floor() as i32,
            demo.spawn.y.round() as i32 - 1,
            demo.spawn.z.floor() as i32,
        );
        let air = demo.air;
        assert_ne!(demo.block_at(cell), Some(air), "the spawn block is solid");
        assert!(demo.edit(cell, air), "the break changes the patch");
        assert_eq!(demo.block_at(cell), Some(air));
        assert!(!demo.edit(cell, air), "a no-op edit reports no change");
        assert!(demo.quads > 0, "the patch still meshes after the edit");
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn terrain_demo_builds_packed_quads_for_one_column() {
        let started = Instant::now();
        let demo = build_terrain_demo(0x5EED, IVec2::new(2, -3), 0).expect("the v1 terrain builds");
        let elapsed = started.elapsed();
        eprintln!(
            "terrain demo: {} sections, {} quads, {elapsed:?}",
            demo.sections.len(),
            demo.quads
        );

        assert!(
            demo.sections
                .iter()
                .any(|section| !section.mesh.opaque.is_empty()),
            "at least one section emits opaque quads"
        );
        assert_eq!(demo.origin_column, IVec2::new(2, -3));

        // Collision is live: the block under the spawn is solid and the spawn
        // sits exactly one block above it.
        let feet = IVec3::new(
            demo.spawn.x.floor() as i32,
            demo.spawn.y.round() as i32 - 1,
            demo.spawn.z.floor() as i32,
        );
        let solid = demo.solid_blocks_in(feet, feet);
        assert_eq!(
            solid,
            vec![feet],
            "the spawn column stands on solid terrain"
        );
        for section in &demo.sections {
            for coordinate in section.origin {
                assert_eq!(
                    coordinate.rem_euclid(i32::try_from(description::SECTION_SIZE).unwrap()),
                    0,
                    "origins are section-aligned multiples of 32"
                );
            }
            for raw in section
                .mesh
                .opaque
                .iter()
                .chain(&section.mesh.cutout)
                .chain(&section.mesh.water)
            {
                let quad = renderer::decode_quad(raw.bits());
                assert!(quad.normal <= 7, "reserved quad normal code");
            }
        }
        // A gross-regression guard, not a benchmark: the suite runs tests in
        // parallel on shared cores, and one column also pays the first
        // whole-world hydrology solve on this thread.
        assert!(
            elapsed < Duration::from_secs(10),
            "one-column demo build took {elapsed:?}"
        );
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
            descend: false,
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
        let view = app.camera.render_view();
        let frustum = Frustum::from_view_projection(view.view_projection);

        let inside = ChunkCoord::from_world(IVec3::new(16, 0, 16)).0.origin();
        let inside = view.relative_point(inside);
        assert!(frustum.intersects_aabb(inside, inside + Vec3::splat(32.0)));

        let behind = ChunkCoord::from_world(IVec3::new(16, 0, 240)).0.origin();
        let behind = view.relative_point(behind);
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
            descend: false,
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

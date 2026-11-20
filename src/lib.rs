//! Valescent Engine: a voxel engine written in Rust.
//!
//! The binary in `src/main.rs` is a thin shell around this crate. Everything
//! the engine does lives here so it can be tested, benchmarked, and reused
//! without a window.

mod block;
mod camera;
pub mod climate;
mod config;
pub mod content;
pub mod curve;
pub mod description;
pub mod editlog;
pub mod error;
pub mod features;
mod frustum;
mod generation;
pub mod graph;
pub mod hash;
pub mod header;
mod headless;
pub mod height;
pub mod hydro;
pub mod lattice;
mod light;
pub mod light2;
pub mod lod;
mod mesh;
mod mesher;
pub mod mesher2;
mod metrics;
pub mod metrics2;
pub mod noise;
mod overlay;
pub mod padded;
pub mod pipeline;
mod player;
mod raycast;
mod renderer;
pub mod residency;
pub mod revisions;
pub mod save;
pub mod section;
mod sky;
pub mod strata;
mod streaming;
pub mod surface;
pub mod volume;
mod voxel;
mod world;
pub mod world_save;

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
/// Number of columns on one side of the demo's generation tile.
const TERRAIN_DEMO_TILE_COLUMNS: i32 = 4;
/// Seed for the stress scene, which has no generated world seed.
const TERRAIN_DEMO_STRESS_SEED: u64 = 0x7E44_5EED;

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
    let (terrain_demo, terrain_save, repair) = if terrain_demo_requested() {
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
                    } else {
                        warn!(%error, "terrain world unavailable; running without saves");
                    }
                    (Vec::new(), None, 0, Vec::new(), None)
                }
            };
        let player_column = Streaming::column_at(spawn);
        let base_x = player_column.x - player_column.x.rem_euclid(TERRAIN_DEMO_TILE_COLUMNS);
        let base_z = player_column.y - player_column.y.rem_euclid(TERRAIN_DEMO_TILE_COLUMNS);
        let (demo, replay) = build_terrain_demo_extent(
            seed,
            player_column,
            base_x,
            base_x + TERRAIN_DEMO_TILE_COLUMNS - 1,
            base_z,
            base_z + TERRAIN_DEMO_TILE_COLUMNS - 1,
            &saved,
            &recovered,
            last_saved,
        )?;
        let save = match world.take() {
            Some(mut world) => {
                if !demo.read_only {
                    for key in &replay.keys {
                        let section = demo
                            .columns
                            .get(&(key.column[0], key.column[1]))
                            .and_then(|stack| stack.get(usize::from(key.section)))
                            .cloned();
                        let Some(section) = section else {
                            continue;
                        };
                        let baseline = demo.baselines.get(key).copied().unwrap_or([0; 32]);
                        if let Err(error) = world.sections.write(*key, baseline, true, &section) {
                            warn!(%error, "replayed terrain section could not be saved");
                        }
                    }
                }
                if let Err(error) = world.advance_revision(replay.revision) {
                    warn!(%error, "terrain header revision could not be advanced");
                }
                match world_save::SaveWorker::start(world, 64) {
                    Ok(worker) => Some(worker),
                    Err(error) => {
                        warn!(%error, "terrain save worker unavailable; edits will not persist");
                        None
                    }
                }
            }
            None => None,
        };
        info!(
            seed = demo.seed,
            column = ?demo.origin_column,
            sections = demo.sections.len(),
            quads = demo.quads,
            min_y = demo.min_y,
            max_y = demo.max_y,
            saved_sections = saved.len(),
            replayed_sections = replay.keys.len(),
            read_only = demo.read_only,
            repair_needed = repair.is_some(),
            build_ms = demo.build_time.as_secs_f64() * 1000.0,
            "TERRAIN_DEMO=1: new-pipeline terrain patch ready"
        );
        (Some(demo), save, repair)
    } else {
        (None, None, None)
    };
    let mut app = App::with_world(setup.world, stress);
    app.terrain_demo = terrain_demo;
    app.terrain_save = terrain_save;
    app.repair_prompt = repair;
    if let Some(spawn) = setup.spawn {
        app.set_spawn(spawn);
    }
    if let Some(demo) = &app.terrain_demo {
        // Stand on the new terrain, facing level; the legacy world stays underneath.
        app.set_spawn(demo.spawn);
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
    /// Every generated column, kept for collision and relighting.
    columns: BTreeMap<(i32, i32), [section::Section; COLUMN_SECTIONS]>,
    /// Per-column light, recomputed after an edit (`L1.2`).
    light: BTreeMap<(i32, i32), light2::ColumnLight>,
    /// Mesher and light material tables, resolved once from the pack.
    materials: mesher2::MaterialProps,
    /// The content pack, for resolving stable names on save (`A5.1`).
    pack: content::ContentPack,
    /// Generated baselines for sections that were loaded or edited (`J4.1`).
    baselines: BTreeMap<section::SectionKey, [u8; 32]>,
    /// Next committed edit revision (`J1.2`).
    revision: u64,
    /// A world that loaded an unknown name is read-only (`A16`).
    read_only: bool,
    /// Air id, for edits.
    air: section::BlockId,
    /// `solid[id]` mirrors the content pack's collision flag.
    solid: Vec<bool>,
    /// Feet position on the patch's surface at the centre column.
    spawn: Vec3,
    /// Active sections submitted to the renderer (fine and coarse by distance).
    sections: Vec<DemoSection>,
    /// Full-detail sections for every generated column.
    fine_sections: Vec<DemoSection>,
    /// Coarse sections derived from complete 2x2x2 octant groups.
    coarse_sections: Vec<DemoSection>,
    /// Column the current LOD selection was made for.
    lod_column: (i32, i32),
    quads: usize,
    /// World block range covered by emitting sections, for the startup log.
    min_y: i32,
    max_y: i32,
    build_time: Duration,
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

    /// Layers saved sections over the generated patch and refreshes light and
    /// meshes. Used after a `world-repair` action or a late load.
    fn apply_saved(&mut self, saved: &[section::SavedSection]) -> bool {
        let mut changed = false;
        for record in saved {
            let key = record.key;
            let generated = self
                .columns
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
                self.read_only = true;
            }
            let Some(stack) = self.columns.get_mut(&(key.column[0], key.column[1])) else {
                continue;
            };
            let Some(slot) = stack.get_mut(usize::from(key.section)) else {
                continue;
            };
            *slot = record.section.clone();
            self.baselines.insert(key, record.baseline_hash);
            changed = true;
        }
        if changed {
            let light = demo_column_light(&self.columns);
            let (sections, _, _, _) = demo_mesh(&self.columns, &light, &self.materials);
            self.light = light;
            self.fine_sections = sections;
            self.coarse_sections = demo_coarse(&self.columns, &self.materials, &self.pack);
            self.select_lod(self.lod_column);
            self.quads = self
                .sections
                .iter()
                .map(|section| mesher2::quad_count(&section.mesh))
                .sum();
            let block = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
            self.min_y = self
                .sections
                .iter()
                .map(|section| section.origin[1])
                .min()
                .unwrap_or(i32::MAX);
            self.max_y = self
                .sections
                .iter()
                .map(|section| section.origin[1] + block)
                .max()
                .unwrap_or(i32::MIN);
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
        for patch in rebuild.patches {
            let origin = patch.origin;
            match patch.mesh {
                Some(mesh) => {
                    let section = DemoSection {
                        origin,
                        scale: 1,
                        mesh,
                    };
                    match self
                        .fine_sections
                        .binary_search_by_key(&origin, |section| section.origin)
                    {
                        Ok(index) => self.fine_sections[index] = section,
                        Err(index) => self.fine_sections.insert(index, section),
                    }
                }
                None => {
                    if let Ok(index) = self
                        .fine_sections
                        .binary_search_by_key(&origin, |section| section.origin)
                    {
                        self.fine_sections.remove(index);
                    }
                }
            }
        }
        self.coarse_sections = demo_coarse(&self.columns, &self.materials, &self.pack);
        self.select_lod(self.lod_column);
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
            .map(|section| section.origin[1] + section_blocks)
            .max()
            .unwrap_or(i32::MIN);
    }

    /// Chooses fine or coarse sections for each complete octant region.
    ///
    /// A region containing the player's fine column stays at full detail; every
    /// other complete region is drawn as one coarse section per 2x2x2 group.
    /// Columns outside any complete region are always fine.
    fn select_lod(&mut self, player_column: (i32, i32)) {
        self.lod_column = player_column;
        if self.coarse_sections.is_empty() {
            self.sections.clone_from(&self.fine_sections);
            return;
        }
        let mut far_regions = BTreeSet::new();
        for section in &self.coarse_sections {
            let x = section.origin[0] / 32;
            let z = section.origin[2] / 32;
            // Keep one fine column around the player so the resolution change
            // is never at the player's feet.
            let near = player_column.0 >= x - 1
                && player_column.0 <= x + 2
                && player_column.1 >= z - 1
                && player_column.1 <= z + 2;
            if !near {
                far_regions.insert((x, z));
            }
        }
        let far_fine: BTreeSet<(i32, i32)> = far_regions
            .iter()
            .flat_map(|&(x, z)| [(x, z), (x + 1, z), (x, z + 1), (x + 1, z + 1)])
            .collect();
        let mut active: Vec<DemoSection> = self
            .fine_sections
            .iter()
            .filter(|section| !far_fine.contains(&(section.origin[0] / 32, section.origin[2] / 32)))
            .cloned()
            .collect();
        active.extend(
            self.coarse_sections
                .iter()
                .filter(|section| {
                    far_regions.contains(&(section.origin[0] / 32, section.origin[2] / 32))
                })
                .cloned(),
        );
        active.sort_by_key(|section| section.origin);
        self.sections = active;
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
    patches: Vec<DemoPatch>,
    light_time: std::time::Duration,
    mesh_time: std::time::Duration,
}

/// Repairs light and remeshes only what an edit can reach.
///
/// Light changes stay within `LIGHT_LEVELS` laterally, so only the 3x3 columns
/// around an edit are relit; within a column the edit's vertical span is
/// repaired by [`light2::ColumnLight::relight_column`]. A section is remeshed
/// when its light changed or when the edit touched its geometry.
fn demo_rebuild(
    columns: &BTreeMap<(i32, i32), [section::Section; COLUMN_SECTIONS]>,
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
                borders[slot] = columns.get(&(sx + dx, sz + dz));
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
    let mut patches = Vec::with_capacity(queued.len());
    for &(sx, sy, sz) in &queued {
        let Ok(sy) = usize::try_from(sy) else {
            continue;
        };
        if sy >= COLUMN_SECTIONS || !columns.contains_key(&(sx, sz)) {
            continue;
        }
        let origin = [sx * size, i32::try_from(sy).unwrap_or(0) * size, sz * size];
        let mesh = demo_section(columns, &light, materials, sx, sz, sy).map(|section| section.mesh);
        patches.push(DemoPatch { origin, mesh });
    }
    DemoRebuild {
        light,
        patches,
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
/// A rectangle inside one 128-block generation tile costs one tile of work;
/// crossing a tile boundary costs four. The demo uses a 4x4 rectangle aligned
/// to a tile so the larger LOD patch still builds in one tile.
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
    let started = Instant::now();
    let terrain = pipeline::Terrain::new(seed)?;
    let materials = demo_materials();
    let pack = content::v1_default();
    let section_blocks = i32::try_from(description::SECTION_SIZE).expect("SECTION_SIZE fits i32");

    let mut columns = demo_columns_extent(seed, min_x, max_x, min_z, max_z)?;
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
    let column_light = demo_column_light(&columns);

    let (sections, quads, min_y, max_y) = demo_mesh(&columns, &column_light, &materials);

    debug_assert!(quads > 0, "the spawn patch emits no geometry");

    // `A5.2`: solidity comes from the pack, not from a hardcoded id list.
    let solid = {
        let mut solid = vec![false; pack.len()];
        for record in pack.records() {
            if let Some(id) = pack.id(&record.name) {
                solid[id as usize] = record.collision;
            }
        }
        solid
    };
    let air = pack.id("core:air").unwrap_or(0);

    // Stand on the patch surface at the centre column, not on the legacy world.
    let spawn = {
        let bx = column.x * section_blocks + section_blocks / 2;
        let bz = column.y * section_blocks + section_blocks / 2;
        let height = i32::try_from(description::WORLD_HEIGHT).unwrap_or(384);
        let top = (0..height).rev().find(|&y| {
            let column_key = (bx.div_euclid(section_blocks), bz.div_euclid(section_blocks));
            let Some(stack) = columns.get(&column_key) else {
                return false;
            };
            let Some(section) =
                stack.get(usize::try_from(y.div_euclid(section_blocks)).unwrap_or(0))
            else {
                return false;
            };
            let id = section.get(
                u32::try_from(bx.rem_euclid(section_blocks)).unwrap_or(0),
                u32::try_from(y.rem_euclid(section_blocks)).unwrap_or(0),
                u32::try_from(bz.rem_euclid(section_blocks)).unwrap_or(0),
            );
            solid.get(id as usize).copied().unwrap_or(false)
        });
        Vec3::new(
            bx as f32 + 0.5,
            top.map_or(description::SEA, |y| y + 1) as f32,
            bz as f32 + 0.5,
        )
    };

    let coarse_sections = demo_coarse(&columns, &materials, &pack);
    let mut demo = TerrainDemo {
        seed,
        origin_column: column,
        columns,
        light: column_light,
        materials,
        pack,
        baselines,
        revision: replay_revision,
        read_only,
        air,
        solid,
        spawn,
        sections: Vec::new(),
        fine_sections: sections,
        coarse_sections,
        lod_column: (column.x, column.y),
        quads,
        min_y,
        max_y,
        build_time: started.elapsed(),
    };
    demo.select_lod((column.x, column.y));
    demo.quads = demo
        .sections
        .iter()
        .map(|section| mesher2::quad_count(&section.mesh))
        .sum();
    let block = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
    demo.min_y = demo
        .sections
        .iter()
        .map(|section| section.origin[1])
        .min()
        .unwrap_or(i32::MAX);
    demo.max_y = demo
        .sections
        .iter()
        .map(|section| section.origin[1] + block)
        .max()
        .unwrap_or(i32::MIN);
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
/// Each worker builds its own synthesizer because `Terrain` caches behind
/// `RefCell`. A field tile is ~18 ms, so duplicating it per worker is far
/// cheaper than serialising 3.5 s of per-column work.
fn demo_columns_extent(
    seed: u64,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
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
                scope.spawn(move || -> Result<Vec<_>, error::TerrainError> {
                    let terrain = pipeline::Terrain::new(seed)?;
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
    columns: &BTreeMap<(i32, i32), [section::Section; COLUMN_SECTIONS]>,
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
                                borders[slot] = columns.get(&(sx + dx, sz + dz));
                                slot += 1;
                            }
                        }
                        let blocks = light2::ColumnBlocks::new(stack.clone(), &borders);
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

/// Meshes every section of the patch (`M3.2`), returning the emitting sections
/// and their world `y` extent. Used at startup and after an edit.
fn demo_mesh(
    columns: &BTreeMap<(i32, i32), [section::Section; COLUMN_SECTIONS]>,
    light: &BTreeMap<(i32, i32), light2::ColumnLight>,
    materials: &mesher2::MaterialProps,
) -> (Vec<DemoSection>, usize, i32, i32) {
    let mut tasks = Vec::with_capacity(columns.len() * COLUMN_SECTIONS);
    for &(sx, sz) in columns.keys() {
        for sy in 0..COLUMN_SECTIONS {
            tasks.push((sx, sz, sy));
        }
    }
    if tasks.is_empty() {
        return (Vec::new(), 0, i32::MAX, i32::MIN);
    }
    let workers = worker_count().min(tasks.len());
    let chunk = tasks.len().div_ceil(workers);
    let mut sections = Vec::new();
    std::thread::scope(|scope| {
        let handles: Vec<_> = tasks
            .chunks(chunk)
            .map(|chunk| {
                scope.spawn(move || {
                    let mut out = Vec::new();
                    for &(sx, sz, sy) in chunk {
                        if let Some(section) = demo_section(columns, light, materials, sx, sz, sy) {
                            out.push(section);
                        }
                    }
                    out
                })
            })
            .collect();
        for handle in handles {
            sections.extend(handle.join().expect("a mesh job panicked"));
        }
    });
    // Sorting by origin keeps the section list deterministic regardless of which
    // worker finished first (`P1.9`).
    sections.sort_by_key(|section| section.origin);
    let section_blocks = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
    let quads = sections
        .iter()
        .map(|section| mesher2::quad_count(&section.mesh))
        .sum();
    let min_y = sections
        .iter()
        .map(|section| section.origin[1])
        .min()
        .unwrap_or(i32::MAX);
    let max_y = sections
        .iter()
        .map(|section| section.origin[1] + section_blocks)
        .max()
        .unwrap_or(i32::MIN);
    (sections, quads, min_y, max_y)
}

/// Derives and meshes every complete `2x2x2` coarse group (`N1.4`, `N3.2`).
///
/// A coarse section exists only where all eight fine octants are present, so a
/// patch edge strip stays fine. Skirts go on every coarse side: the coarse mesh
/// may sit beside a fine neighbour at the selection boundary.
fn demo_coarse(
    columns: &BTreeMap<(i32, i32), [section::Section; COLUMN_SECTIONS]>,
    materials: &mesher2::MaterialProps,
    pack: &content::ContentPack,
) -> Vec<DemoSection> {
    let Some((&(base_x, base_z), _)) = columns.iter().next() else {
        return Vec::new();
    };
    let water = pack.id("core:water").unwrap_or(0);
    let scatter: Vec<section::BlockId> = pack
        .records()
        .iter()
        .filter(|record| record.render_shape == content::RenderShape::CrossQuad)
        .filter_map(|record| pack.id(&record.name))
        .collect();

    let mut derived: BTreeMap<(i32, i32, i32), section::Section> = BTreeMap::new();
    for &(x, z) in columns.keys() {
        if (x - base_x).rem_euclid(2) != 0 || (z - base_z).rem_euclid(2) != 0 {
            continue;
        }
        if !columns.contains_key(&(x + 1, z))
            || !columns.contains_key(&(x, z + 1))
            || !columns.contains_key(&(x + 1, z + 1))
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

    let mut out = Vec::new();
    let air = section::Section::uniform(0);
    for (&(x, j, z), coarse) in &derived {
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
        out.push(DemoSection {
            origin: [x * 32, j * 64, z * 32],
            scale: 2,
            mesh,
        });
    }
    out.sort_by_key(|section| section.origin);
    out
}

/// Meshes one section, or `None` when it emits nothing.
fn demo_section(
    columns: &BTreeMap<(i32, i32), [section::Section; COLUMN_SECTIONS]>,
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

/// The repair action a shortcut key selects.
fn repair_action(code: KeyCode) -> Option<save::RepairAction> {
    match code {
        KeyCode::KeyR => Some(save::RepairAction::RestoreBackup),
        KeyCode::KeyQ => Some(save::RepairAction::Quarantine),
        KeyCode::KeyS => Some(save::RepairAction::Salvage),
        _ => None,
    }
}

/// The base column of a demo patch, or `(0, 0)` when it has no columns.
fn demo_tile(demo: &TerrainDemo) -> (i32, i32) {
    demo.columns.keys().next().map_or((0, 0), |&(x, z)| (x, z))
}

/// Tiles to prefetch, in priority order, for a player at `local` in `base`.
///
/// Two columns of lead time keep a crossing covered while one tile builds at a
/// time. When both axes are near an edge, the diagonal follows the two
/// orthogonal neighbours so a corner crossing is also cached.
fn prefetch_candidates(base: (i32, i32), local: (i32, i32), tile: i32) -> Vec<(i32, i32)> {
    let left = local.0 <= 1;
    let right = local.0 >= tile - 2;
    let up = local.1 <= 1;
    let down = local.1 >= tile - 2;
    let mut candidates = Vec::new();
    if left {
        candidates.push((base.0 - tile, base.1));
    }
    if right {
        candidates.push((base.0 + tile, base.1));
    }
    if up {
        candidates.push((base.0, base.1 - tile));
    }
    if down {
        candidates.push((base.0, base.1 + tile));
    }
    let x = if left {
        Some(-tile)
    } else if right {
        Some(tile)
    } else {
        None
    };
    let z = if up {
        Some(-tile)
    } else if down {
        Some(tile)
    } else {
        None
    };
    if let (Some(dx), Some(dz)) = (x, z) {
        candidates.push((base.0 + dx, base.1 + dz));
    }
    candidates
}

/// What the tile streamer should do this frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StreamPlan {
    /// A cached tile to activate now.
    swap: Option<(i32, i32)>,
    /// A tile to generate now.
    build: Option<(i32, i32)>,
    /// Whether the player has ground data this frame.
    ready: bool,
}

/// Decides tile swaps, urgent builds, and prefetches for one frame.
fn plan_stream(
    active: (i32, i32),
    player_column: (i32, i32),
    cached: &BTreeSet<(i32, i32)>,
    pending: Option<(i32, i32)>,
    tile: i32,
) -> StreamPlan {
    let player_base = (
        player_column.0 - player_column.0.rem_euclid(tile),
        player_column.1 - player_column.1.rem_euclid(tile),
    );
    if player_base != active {
        if cached.contains(&player_base) {
            return StreamPlan {
                swap: Some(player_base),
                build: None,
                ready: true,
            };
        }
        return StreamPlan {
            swap: None,
            build: (pending != Some(player_base)).then_some(player_base),
            ready: false,
        };
    }
    if pending.is_some() {
        return StreamPlan {
            swap: None,
            build: None,
            ready: true,
        };
    }
    let local = (player_column.0 - active.0, player_column.1 - active.1);
    let build = prefetch_candidates(active, local, tile)
        .into_iter()
        .find(|candidate| {
            *candidate != active && !cached.contains(candidate) && pending != Some(*candidate)
        });
    StreamPlan {
        swap: None,
        build,
        ready: true,
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
    /// A tile being generated for the demo streamer.
    terrain_stream: Option<std::sync::mpsc::Receiver<Result<TerrainDemo, error::TerrainError>>>,
    /// Tile base column the in-flight stream build targets.
    terrain_stream_base: Option<(i32, i32)>,
    /// Ready neighbouring tiles, keyed by their base column.
    terrain_cache: BTreeMap<(i32, i32), TerrainDemo>,
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
            terrain_stream: None,
            terrain_stream_base: None,
            terrain_cache: BTreeMap::new(),
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
        if let (false, Some(save), Some(old_name), Some(new_name)) = (
            demo.read_only,
            self.terrain_save.as_ref(),
            old_name,
            new_name,
        ) {
            queue_demo_edit(save, demo, cell, key, baseline, old_name, new_name);
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
        if let Some(demo) = &mut self.terrain_demo {
            if demo.lod_column != column {
                demo.select_lod(column);
            }
        }
        let (Some(demo), Some(renderer)) = (self.terrain_demo.as_ref(), self.renderer.as_mut())
        else {
            return;
        };
        if let Err(error) = upload_terrain_demo(renderer, demo) {
            error!(%error, "terrain upload failed");
        }
    }

    /// Advances the demo streamer: receives a finished tile, swaps when the
    /// player is inside it, and prefetches the tile beyond a near edge.
    fn poll_stream(&mut self) -> bool {
        if let Some(receiver) = &self.terrain_stream {
            match receiver.try_recv() {
                Ok(Ok(demo)) => {
                    let base = demo_tile(&demo);
                    debug!(?base, "terrain stream tile ready");
                    self.terrain_cache.insert(base, demo);
                    self.terrain_stream = None;
                    self.terrain_stream_base = None;
                }
                Ok(Err(error)) => {
                    warn!(%error, "terrain stream tile failed");
                    self.terrain_stream = None;
                    self.terrain_stream_base = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.terrain_stream = None;
                    self.terrain_stream_base = None;
                }
            }
        }

        let Some(active) = self.terrain_demo.as_ref() else {
            return true;
        };
        let active_base = demo_tile(active);
        let player = Streaming::column_at(self.player.position);
        let player_column = (player.x, player.y);
        let tile = TERRAIN_DEMO_TILE_COLUMNS;
        let cached: BTreeSet<(i32, i32)> = self.terrain_cache.keys().copied().collect();
        let plan = plan_stream(
            active_base,
            player_column,
            &cached,
            self.terrain_stream_base,
            tile,
        );
        let mut ready = plan.ready;
        if let Some(base) = plan.swap {
            if let Some(demo) = self.terrain_cache.remove(&base) {
                if let Some(old) = self.terrain_demo.take() {
                    self.terrain_cache.insert(active_base, old);
                }
                self.terrain_demo = Some(demo);
                self.upload_demo();
                debug!(?base, "terrain stream tile swapped");
                ready = true;
            }
        }
        if let Some(base) = plan.build {
            if self.terrain_stream_base != Some(base) {
                self.terrain_stream = None;
                self.terrain_stream_base = None;
                self.start_stream_build(base);
            }
        }

        let active_base = self.terrain_demo.as_ref().map_or(active_base, demo_tile);
        while self.terrain_cache.len() > 3 {
            let farthest = self
                .terrain_cache
                .keys()
                .max_by_key(|(x, z)| (x - active_base.0).abs() + (z - active_base.1).abs())
                .copied();
            let Some(farthest) = farthest else {
                break;
            };
            self.terrain_cache.remove(&farthest);
        }
        ready
    }

    /// Generates one 4x4 tile in the background, applying its saved sections.
    fn start_stream_build(&mut self, base: (i32, i32)) {
        let seed = self
            .terrain_demo
            .as_ref()
            .map_or(TERRAIN_DEMO_STRESS_SEED, |demo| demo.seed);
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let store = save::SectionStore::new(TERRAIN_SAVE_DIR, content::v1_default());
            let saved = match store.read_all() {
                Ok(sections) => sections
                    .into_iter()
                    .filter(|record| {
                        (base.0..base.0 + TERRAIN_DEMO_TILE_COLUMNS).contains(&record.key.column[0])
                            && (base.1..base.1 + TERRAIN_DEMO_TILE_COLUMNS)
                                .contains(&record.key.column[1])
                    })
                    .collect(),
                Err(error) => {
                    warn!(%error, "saved tile sections could not be read");
                    Vec::new()
                }
            };
            let spawn = IVec2::new(
                base.0 + TERRAIN_DEMO_TILE_COLUMNS / 2,
                base.1 + TERRAIN_DEMO_TILE_COLUMNS / 2,
            );
            let result = build_terrain_demo_extent(
                seed,
                spawn,
                base.0,
                base.0 + TERRAIN_DEMO_TILE_COLUMNS - 1,
                base.1,
                base.1 + TERRAIN_DEMO_TILE_COLUMNS - 1,
                &saved,
                &[],
                0,
            )
            .map(|(demo, _)| demo);
            let _ = sender.send(result);
        });
        self.terrain_stream = Some(receiver);
        self.terrain_stream_base = Some(base);
        debug!(?base, "terrain stream tile build started");
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
        let ready =
            |position: Vec3| streaming.is_none_or(|streaming| streaming.aabb_ready(position));
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
            let stream_ready = self.poll_stream();
            let delta_seconds = elapsed.min(MAX_FRAME_DELTA).as_secs_f32();
            if stream_ready {
                self.step_player(delta_seconds);
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
        let frustum = {
            profiling::scope!("frustum");
            Frustum::from_view_projection(self.camera.view_projection())
        };
        let slow_render = Instant::now();
        if let Some(renderer) = &mut self.renderer {
            renderer.set_camera(&self.camera);
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
            !demo.coarse_sections.is_empty(),
            "a radius-2 patch has complete octant regions"
        );
        let fine = demo.fine_sections.len();
        demo.select_lod((demo.origin_column.x, demo.origin_column.y));
        assert_eq!(
            demo.sections.len(),
            fine,
            "the player's region and its fine margin stay full detail"
        );

        demo.select_lod((demo.origin_column.x + 2, demo.origin_column.y + 2));
        assert!(
            demo.sections.len() < fine,
            "a region two columns away is drawn coarse: {} active vs {fine} fine",
            demo.sections.len()
        );

        let coarse: BTreeSet<[i32; 3]> = demo
            .coarse_sections
            .iter()
            .map(|section| section.origin)
            .collect();
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
            demo.coarse_sections
                .iter()
                .any(|section| !section.mesh.skirts.is_empty()),
            "coarse meshes carry skirts for the fine boundary"
        );
    }

    #[test]
    fn the_demo_tile_is_lod_selected_at_spawn() {
        let (demo, replay) =
            build_terrain_demo_extent(0x5EED, IVec2::new(0, 0), 0, 3, 0, 3, &[], &[], 0)
                .expect("the one-tile demo builds");
        assert!(replay.keys.is_empty());
        assert_eq!(
            demo.columns.keys().min(),
            Some(&(0, 0)),
            "the tile is one 128-block generation unit"
        );
        assert_eq!(demo.columns.keys().max(), Some(&(3, 3)));
        assert!(
            demo.sections.len() < demo.fine_sections.len(),
            "the far corner is coarse at spawn: {} active vs {} fine",
            demo.sections.len(),
            demo.fine_sections.len()
        );
    }

    #[test]
    fn prefetch_candidates_lead_the_player_by_two_columns() {
        assert_eq!(
            prefetch_candidates((0, 0), (0, 0), 4),
            vec![(-4, 0), (0, -4), (-4, -4)]
        );
        assert_eq!(
            prefetch_candidates((0, 0), (2, 2), 4),
            vec![(4, 0), (0, 4), (4, 4)]
        );
        assert_eq!(
            prefetch_candidates((0, 0), (1, 3), 4),
            vec![(-4, 0), (0, 4), (-4, 4)]
        );
    }

    #[test]
    fn stream_plan_swaps_cached_tiles_and_prefetches_early() {
        let tile = 4;
        let active = (0, 0);
        let none = BTreeSet::new();
        assert_eq!(
            plan_stream(active, (1, 1), &none, None, tile),
            StreamPlan {
                swap: None,
                build: Some((-4, 0)),
                ready: true,
            }
        );
        assert_eq!(
            plan_stream(active, (2, 2), &none, None, tile),
            StreamPlan {
                swap: None,
                build: Some((4, 0)),
                ready: true,
            }
        );

        let cached = BTreeSet::from([(4, 0)]);
        assert_eq!(
            plan_stream(active, (4, 2), &cached, None, tile),
            StreamPlan {
                swap: Some((4, 0)),
                build: None,
                ready: true,
            }
        );
        assert_eq!(
            plan_stream(active, (4, 2), &none, None, tile),
            StreamPlan {
                swap: None,
                build: Some((4, 0)),
                ready: false,
            }
        );
        assert_eq!(
            plan_stream(active, (4, 2), &none, Some((4, 0)), tile),
            StreamPlan {
                swap: None,
                build: None,
                ready: false,
            }
        );
        assert_eq!(
            plan_stream(active, (1, 1), &none, Some((4, 0)), tile),
            StreamPlan {
                swap: None,
                build: None,
                ready: true,
            }
        );
    }

    fn tiny_demo(base: (i32, i32)) -> TerrainDemo {
        let pack = content::v1_default();
        let mut columns = BTreeMap::new();
        columns.insert(base, std::array::from_fn(|_| section::Section::uniform(0)));
        let mut solid = vec![false; pack.len()];
        solid[0] = false;
        TerrainDemo {
            seed: 1,
            origin_column: IVec2::new(base.0, base.1),
            columns,
            light: BTreeMap::new(),
            materials: demo_materials(),
            pack,
            baselines: BTreeMap::new(),
            revision: 0,
            read_only: false,
            air: 0,
            solid,
            spawn: Vec3::ZERO,
            sections: Vec::new(),
            fine_sections: Vec::new(),
            coarse_sections: Vec::new(),
            lod_column: base,
            quads: 0,
            min_y: 0,
            max_y: 0,
            build_time: Duration::ZERO,
        }
    }

    #[test]
    fn a_cached_tile_swaps_under_the_player_and_keeps_the_old_one() {
        let mut app = App::with_world(World::default(), true);
        app.terrain_demo = Some(tiny_demo((0, 0)));
        app.terrain_cache.insert((-4, 0), tiny_demo((-4, 0)));
        app.player.position = Vec3::new(-1.0, 64.0, 16.0);

        assert!(app.poll_stream(), "the cached tile has ground");
        assert_eq!(
            app.terrain_demo.as_ref().map(demo_tile),
            Some((-4, 0)),
            "the cached tile is active"
        );
        assert!(
            app.terrain_cache.contains_key(&(0, 0)),
            "the old tile stays cached for a swap back"
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
        let columns = demo_columns_extent(0x5EED, 0, 3, 0, 3).expect("the tile generates");
        let generate_ms = generated.elapsed().as_secs_f64() * 1000.0;

        let lit = Instant::now();
        let light = demo_column_light(&columns);
        let light_ms = lit.elapsed().as_secs_f64() * 1000.0;

        let meshed = Instant::now();
        let materials = demo_materials();
        let (sections, quads, _, _) = demo_mesh(&columns, &light, &materials);
        let mesh_ms = meshed.elapsed().as_secs_f64() * 1000.0;

        let coarse_started = Instant::now();
        let coarse = demo_coarse(&columns, &materials, &content::v1_default());
        let coarse_ms = coarse_started.elapsed().as_secs_f64() * 1000.0;

        eprintln!(
            "demo build: terrain={terrain_ms:.0}ms tile={tile_ms:.0}ms generate={generate_ms:.0}ms light={light_ms:.0}ms mesh={mesh_ms:.0}ms coarse={coarse_ms:.0}ms | {} fine, {} coarse, {quads} quads",
            sections.len(),
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
        let (expected_sections, _, _, _) =
            demo_mesh(&demo.columns, &expected_light, &demo.materials);
        let expected: BTreeMap<[i32; 3], mesher2::SectionMesh> = expected_sections
            .into_iter()
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
        assert!(
            elapsed < Duration::from_secs(2),
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

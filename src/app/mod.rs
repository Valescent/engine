//! The `winit` application: `App` state and construction, the window and
//! renderer lifecycle, and the error boundary. Per-frame work lives in
//! [`frame`]; key and mouse handling lives in [`input`].

mod frame;
mod game;
mod hierarchy;
mod input;
mod ui;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::{IVec2, IVec3, Vec3};
use thiserror::Error;
use tracing::{debug, error, info, warn};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, ModifiersState};
use winit::window::{Window, WindowId};

use crate::block::Block;
use crate::camera::Camera;
use crate::config::{Config, ConfigError};
use crate::content::ContentPack;
use crate::demo::DemoRuntime;
use crate::game::{GameState, ItemRegistry, RecipeBook};
use crate::hierarchy::{DEFAULT_DETAIL_CHUNKS, Hierarchy};
use crate::horizon;
use crate::mesh::Lod;
use crate::mesher::{MeshKind, MeshPool, MeshQueue, Priority, create_pool};
use crate::player::Player;
use crate::renderer::{Renderer, RendererError};
use crate::save;
use crate::section::{SectionBounds, SectionKey, overlapping_sections_for_chunk};
use crate::sky::Sky;
use crate::streaming::Streaming;
use crate::ui::UiState;
use crate::world::{ChunkCoord, DirtyChunks, SaveError, World};
use crate::world_io::{no_vsync_requested, save_directory};

use self::frame::{FrameAccumulator, upload_terrain_demo};

const WINDOW_TITLE: &str = concat!("Valescent Engine ", env!("CARGO_PKG_VERSION"));
const SPAWN_X: i32 = 32;
const SPAWN_Z: i32 = 32;

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

#[allow(clippy::struct_excessive_bools)]
pub(crate) struct App {
    pub(crate) window: Option<Arc<Window>>,
    pub(crate) renderer: Option<Renderer>,
    pub(crate) mesher: Option<Arc<dyn MeshPool>>,
    pub(crate) queue: MeshQueue,
    pub(crate) config: Config,
    pub(crate) streaming: Option<Streaming>,
    pub(crate) seed: Option<u64>,
    /// The v1 terrain demo: patch, ring, saves, repair, and pending rebuild.
    pub(crate) demo: Option<DemoRuntime>,
    pub(crate) save_directory: Option<PathBuf>,
    /// Chunk coordinates with a file in `save_directory`, listed at startup.
    pub(crate) saved_chunks: HashSet<ChunkCoord>,
    pub(crate) level_work: BTreeSet<ChunkCoord>,
    pub(crate) level_player_chunk: ChunkCoord,
    /// Cached generation candidates, rebuilt when the player column or loaded set changes.
    pub(crate) wanted_columns: Vec<IVec2>,
    pub(crate) unload_columns: Vec<IVec2>,
    pub(crate) wanted_cursor: usize,
    pub(crate) wanted_player: Option<IVec2>,
    /// Chunks whose light waits for an adjacent column that is still streaming in.
    pub(crate) deferred_light: BTreeSet<ChunkCoord>,
    /// Consecutive mesh upload failures per chunk, cleared on success.
    pub(crate) upload_failures: HashMap<ChunkCoord, u8>,
    pub(crate) hierarchy: bool,
    pub(crate) detail_chunks: i32,
    /// Parent-retained target, readiness, and build queue.
    pub(crate) sections: Hierarchy,
    pub(crate) section_cursor: usize,
    pub(crate) sections_uploaded: HashSet<SectionKey>,
    pub(crate) sections_in_flight: HashSet<SectionKey>,
    /// Job id expected for each in-flight section build.
    pub(crate) section_jobs: HashMap<SectionKey, u64>,
    pub(crate) next_section_job: u64,
    pub(crate) section_bounds: HashMap<SectionKey, SectionBounds>,
    pub(crate) hierarchy_at: Instant,
    pub(crate) hierarchy_cell: Option<IVec2>,
    pub(crate) horizon_cull: bool,
    pub(crate) horizon: horizon::Horizon,
    pub(crate) horizon_at: Instant,
    pub(crate) stats: FrameAccumulator,
    pub(crate) stress: bool,
    pub(crate) world: World,
    pub(crate) player: Player,
    pub(crate) camera: Camera,
    pub(crate) sky: Sky,
    pub(crate) unlit: bool,
    pub(crate) selected_block: Block,
    /// The built-in content pack, used to resolve block names and hardness.
    pub(crate) pack: ContentPack,
    /// Stable item ids for the built-in pack.
    pub(crate) items: ItemRegistry,
    /// The compiled Phase 1 recipes, known from the start.
    pub(crate) recipes: RecipeBook,
    /// Inventory, break progress, drops, and the fixed tick.
    pub(crate) game: GameState,
    pub(crate) break_held: bool,
    /// egui state shared by the winit adapter and the renderer upload.
    pub(crate) egui_ctx: egui::Context,
    pub(crate) egui_state: Option<egui_winit::State>,
    pub(crate) ui: UiState,
    /// Whether the run restored a player save, which suppresses the demo spawn
    /// override.
    pub(crate) has_player_save: bool,
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

        let pack = crate::content::v1_default();
        let items = ItemRegistry::builtin(&pack);
        let recipes = RecipeBook::builtin(&items).expect("the built-in recipe table is valid");
        let game = GameState::new();
        let egui_ctx = egui::Context::default();
        egui_ctx.set_visuals(egui::Visuals::dark());
        let mut ui = UiState::new();
        if std::env::var("ENGINE_UI_WORKBENCH").is_ok_and(|value| value == "1") {
            ui.open_workbench(0);
        } else if std::env::var("ENGINE_UI_INVENTORY").is_ok_and(|value| value == "1") {
            ui.open_inventory(0);
        }

        Self {
            window: None,
            renderer: None,
            mesher: None,
            queue,
            config: Config::default(),
            streaming: None,
            seed: None,
            demo: None,
            save_directory: save_directory(stress).map(Path::to_path_buf),
            saved_chunks: HashSet::new(),
            level_work: world.iter_chunks().map(|(chunk, _)| chunk).collect(),
            level_player_chunk: ChunkCoord::from_world(spawn.floor().as_ivec3()).0,
            wanted_columns: Vec::new(),
            unload_columns: Vec::new(),
            wanted_cursor: 0,
            wanted_player: None,
            deferred_light: BTreeSet::new(),
            upload_failures: HashMap::new(),
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
            horizon_cull: !std::env::var("ENGINE_HORIZON_CULL").is_ok_and(|value| value == "0"),
            horizon: horizon::Horizon::disabled(),
            horizon_at: Instant::now(),
            stats: FrameAccumulator::default(),
            stress,
            world,
            player,
            camera,
            sky: Sky::new(),
            unlit: false,
            selected_block: Block::Stone,
            pack,
            items,
            recipes,
            game,
            break_held: false,
            egui_ctx,
            egui_state: None,
            ui,
            has_player_save: false,
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
        self.invalidate_sections(dirty);

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

    /// Marks the coarse sections overlapping an edit for a rebuild.
    ///
    /// Sections outside the current target stay marked but are not built until
    /// they re-enter it, so an edit never forces off-target work.
    fn invalidate_sections(&mut self, dirty: &DirtyChunks) {
        if !self.hierarchy {
            return;
        }
        let mut keys = Vec::new();
        for chunk in dirty {
            keys.extend(overlapping_sections_for_chunk(*chunk));
        }
        for key in &keys {
            if let Some(renderer) = &mut self.renderer {
                renderer.remove_section(*key);
            }
            self.sections_uploaded.remove(key);
            self.section_bounds.remove(key);
            self.section_jobs.remove(key);
            self.sections_in_flight.remove(key);
        }
        self.sections.mark_dirty(keys);
    }

    /// Turns the section hierarchy off and drops its pending work.
    ///
    /// The fallback chunk renderer cannot draw sections, so a hierarchy that
    /// cannot upload must fall back instead of queueing work forever.
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

    fn save_world(&mut self, final_save: bool) {
        if let Some(demo) = &mut self.demo {
            if final_save {
                demo.shutdown();
            } else {
                demo.flush();
            }
        }
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
}

impl ApplicationHandler for App {
    #[allow(clippy::cast_possible_truncation)]
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
        self.egui_state = Some(egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            None,
            None,
        ));

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
        let upload = match &self.demo {
            Some(demo) => upload_terrain_demo(&mut renderer, demo),
            None => Ok(()),
        };
        if let Err(error) = upload {
            self.fail(RunError::Renderer(error), event_loop);
            return;
        }
        if self.hierarchy && !renderer.supports_sections() {
            self.disable_hierarchy("quad pool render path unavailable");
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
        if self.handle_ui_event(&event) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                self.stow_ui_items();
                self.save_world(true);
                self.save_game();
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
                if self
                    .demo
                    .as_ref()
                    .is_some_and(|demo| demo.repair_key().is_some())
                {
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
                    self.begin_break();
                } else {
                    self.capture_cursor();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => {
                self.end_break();
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::frustum::Frustum;
    use crate::light;
    use crate::mesher::{Job, JobResult};
    use crate::world_io::stress_world;

    use super::*;

    pub(super) fn temp_directory(name: &str) -> std::path::PathBuf {
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
    pub(super) fn demo_world() -> World {
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
    pub(super) struct InlinePool {
        pub(super) light_jobs: AtomicUsize,
        pub(super) mesh_jobs: AtomicUsize,
        pub(super) results: Mutex<Vec<JobResult>>,
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
                Job::Generate(_) | Job::Section(_) => {}
            }
            true
        }

        fn try_recv(&self) -> Option<JobResult> {
            self.results.lock().unwrap().pop()
        }
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

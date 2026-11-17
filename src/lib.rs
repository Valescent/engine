//! Valescent Engine: a voxel engine written in Rust.
//!
//! The binary in `src/main.rs` is a thin shell around this crate. Everything
//! the engine does lives here so it can be tested, benchmarked, and reused
//! without a window.

mod block;
mod camera;
mod frustum;
mod mesh;
mod mesher;
mod metrics;
mod overlay;
mod player;
mod raycast;
mod renderer;
mod voxel;
mod world;

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::{IVec3, Vec3};
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
use frustum::Frustum;
use mesh::Lod;
use mesher::{MeshPool, MeshQueue, Priority, WorkerPool, collect_finished};
use player::{Player, PlayerInput};
use raycast::{RaycastHit, raycast};
pub use renderer::RendererError;
use renderer::{FrameStats, Renderer};
use voxel::build_highlight_mesh;
use world::{CHUNK_SIZE, DirtyChunks, World};
pub use world::{ChunkCoord, SaveError};

const WINDOW_TITLE: &str = concat!("Valescent Engine ", env!("CARGO_PKG_VERSION"));
const MAX_FRAME_DELTA: Duration = Duration::from_millis(100);
const FRAME_LOG_INTERVAL: Duration = Duration::from_secs(1);
const WORLD_CHUNKS: &str = "world/chunks";
const INTERACTION_DISTANCE: f32 = 6.0;
const SPAWN_X: i32 = 32;
const SPAWN_Z: i32 = 32;
const MAX_UPLOADS_PER_FRAME: usize = 2;
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

    let stress = stress_requested();
    let world = world_for(stress, Path::new(WORLD_CHUNKS))?;
    let event_loop = EventLoop::new()?;
    let mut app = App::with_world(world, stress);
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

/// Loads the stress scene or the saved (or demo) world.
///
/// Stress mode never touches `directory`.
fn world_for(stress: bool, directory: &Path) -> Result<World, SaveError> {
    if stress {
        Ok(stress_world())
    } else if directory.exists() {
        World::load_chunks(directory)
    } else {
        Ok(demo_world())
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
        for &local in data.blocks.keys() {
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
    stats: FrameAccumulator,
    stress: bool,
    world: World,
    player: Player,
    camera: Camera,
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

        Self {
            window: None,
            renderer: None,
            mesher: None,
            queue: MeshQueue::default(),
            stats: FrameAccumulator::default(),
            stress,
            world,
            player,
            camera,
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

    /// Queues an urgent full-detail rebuild and a background coarse rebuild.
    fn invalidate_chunks(&mut self, dirty: &DirtyChunks) {
        let started = Instant::now();
        self.queue.invalidate(dirty);

        if let Some(pool) = self.mesher.clone() {
            for chunk in dirty {
                if self.world.chunk(*chunk).is_some() {
                    self.queue
                        .request(&*pool, &self.world, *chunk, Lod::Lod0, Priority::Urgent);
                    self.queue.request(
                        &*pool,
                        &self.world,
                        *chunk,
                        Lod::Lod1,
                        Priority::Background,
                    );
                } else {
                    self.queue.remove(*chunk);
                    if let Some(renderer) = &mut self.renderer {
                        renderer.remove_chunk(*chunk);
                    }
                }
            }
        }

        if let Some(window) = &self.window {
            window.request_redraw();
        }

        debug!(
            dirty_chunks = dirty.len(),
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
            self.queue
                .request(&*pool, &self.world, chunk, Lod::Lod0, Priority::Urgent);
        }
    }

    /// Recalculates every chunk's LOD and requests any missing mesh.
    fn request_levels(&mut self, pool: &dyn MeshPool) {
        profiling::scope!("mesh request");
        let feet = self.player.position.floor().as_ivec3();
        let player_chunk = ChunkCoord::from_world(feet).0;

        for (chunk, _) in self.world.iter_chunks() {
            let lod = self.queue.desired_lod(chunk, player_chunk);
            let revision = self.queue.revision(chunk);
            let resident = self
                .renderer
                .as_ref()
                .is_some_and(|renderer| renderer.has_current_mesh(chunk, lod, revision));
            if !resident {
                self.queue
                    .request(pool, &self.world, chunk, lod, Priority::Background);
            }
        }
        self.stats.pending_jobs = self.queue.pending_len();
    }

    fn upload_finished_meshes(&mut self, pool: &dyn MeshPool) {
        profiling::scope!("mesh upload");
        let (results, rebuild) = collect_finished(&mut self.queue, pool, MAX_UPLOADS_PER_FRAME);
        let rebuild_ms = rebuild.as_secs_f64() * 1000.0;
        self.stats.rebuild_ms += rebuild_ms;
        self.stats.last_rebuild_ms = rebuild_ms;

        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let mut upload_ms = 0.0;
        for result in &results {
            let started = Instant::now();
            renderer.upload_chunk(result.chunk, result.lod, result.revision, &result.mesh);
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
        let Some(directory) = save_directory(self.stress) else {
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
                self.player.update(&self.world, input, delta_seconds);
            }
            self.camera.set_position(self.player.eye_position());
            self.update_highlight();
        }

        if let Some(pool) = self.mesher.clone() {
            self.request_levels(&*pool);
            self.upload_finished_meshes(&*pool);
            self.select_active_levels();
        }

        let overlay = self.show_overlay.then(|| {
            format!(
                "FPS {:.0}  {:.2} MS",
                self.overlay_fps, self.overlay_frame_ms
            )
        });
        let frustum = Frustum::from_view_projection(self.camera.view_projection());
        if let Some(renderer) = &mut self.renderer {
            renderer.set_camera(&self.camera);
            renderer.set_overlay(overlay.as_deref());
            self.stats.totals = renderer.render(&frustum);
        }

        metrics::frame_ms(elapsed.as_secs_f64() * 1000.0);
        metrics::visible_chunks(f64::from(self.stats.totals.visible_chunks));
        metrics::draw_calls(f64::from(self.stats.totals.draw_calls));
        metrics::visible_vertices(self.stats.totals.visible_vertices as f64);
        metrics::rebuild_ms(self.stats.last_rebuild_ms);
        metrics::upload_ms(self.stats.last_upload_ms);
        metrics::mesh_jobs(self.stats.pending_jobs as f64);
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
        self.mesher = Some(Arc::new(WorkerPool::new()));
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

    use super::*;

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
                .any(|(_, chunk)| { chunk.blocks.values().any(|block| *block == Block::Stone) })
        );
        assert!(
            world
                .iter_chunks()
                .any(|(_, chunk)| { chunk.blocks.values().any(|block| *block == Block::Wood) })
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

    #[test]
    fn stress_mode_ignores_the_saved_world_directory() {
        let directory = std::env::temp_dir().join(format!(
            "valescent-stress-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("chunk_bad.vchk"), []).unwrap();

        assert!(world_for(true, &directory).is_ok());
        assert!(world_for(false, &directory).is_err());
        assert!(save_directory(true).is_none());
        assert_eq!(save_directory(false), Some(Path::new(WORLD_CHUNKS)));

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
            let lod0 = mesh::build_mesh(&snapshot, &border, Lod::Lod0);
            let lod1 = mesh::build_mesh(&snapshot, &border, Lod::Lod1);

            assert_ne!(lod0.len(), 0);
            assert_ne!(lod1.len(), 0);
            greedy += lod0.len();
            naive += voxel::build_chunk_mesh(&world, chunk).len();
        }

        assert!(greedy < naive, "greedy {greedy} naive {naive}");
    }

    #[test]
    #[ignore = "benchmark; run with cargo test --release --lib bench_hot_paths -- --ignored --nocapture"]
    #[allow(clippy::cast_precision_loss)]
    fn bench_hot_paths() {
        let world = stress_world();
        let chunks: Vec<_> = world.iter_chunks().map(|(chunk, _)| chunk).collect();
        let chunk_count = chunks.len();

        let started = Instant::now();
        let mut snapshots = Vec::with_capacity(chunk_count);
        for &chunk in &chunks {
            snapshots.push((
                mesh::ChunkSnapshot::new(&world, chunk),
                mesh::BorderBlocks::new(&world, chunk),
            ));
        }
        let prepare = started.elapsed();

        let started = Instant::now();
        let lod0_vertices: usize = snapshots
            .iter()
            .map(|(snapshot, border)| mesh::build_mesh(snapshot, border, Lod::Lod0).len())
            .sum();
        let lod0 = started.elapsed();

        let started = Instant::now();
        let lod1_vertices: usize = snapshots
            .iter()
            .map(|(snapshot, border)| mesh::build_mesh(snapshot, border, Lod::Lod1).len())
            .sum();
        let lod1 = started.elapsed();

        let mut player = Player::new(Vec3::new(16.5, 3.0, 16.5));
        let input = PlayerInput {
            forward: 0.0,
            right: 0.0,
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
            "chunks={chunk_count} prepare_ms/chunk={:.3} lod0_ms/chunk={:.3} lod1_ms/chunk={:.3} lod0_vertices={lod0_vertices} lod1_vertices={lod1_vertices} update_us/frame={:.1}",
            elapsed_ms(prepare, chunk_count),
            per_chunk(lod0),
            per_chunk(lod1),
            update.as_secs_f64() * 1e6 / f64::from(frames),
        );
    }

    #[allow(clippy::cast_precision_loss)]
    fn elapsed_ms(elapsed: Duration, count: usize) -> f64 {
        elapsed.as_secs_f64() * 1000.0 / count as f64
    }
}

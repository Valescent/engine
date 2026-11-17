//! Valescent Engine: a voxel engine written in Rust.
//!
//! The binary in `src/main.rs` is a thin shell around this crate. Everything
//! the engine does lives here so it can be tested, benchmarked, and reused
//! without a window.

mod block;
mod camera;
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
use player::{Player, PlayerInput};
use raycast::{RaycastHit, raycast};
use renderer::Renderer;
pub use renderer::RendererError;
use voxel::{build_chunk_mesh, build_highlight_mesh};
pub use world::{ChunkCoord, SaveError};
use world::{DirtyChunks, World};

const WINDOW_TITLE: &str = concat!("Valescent Engine ", env!("CARGO_PKG_VERSION"));
const MAX_FRAME_DELTA: Duration = Duration::from_millis(100);
const FRAME_LOG_INTERVAL: Duration = Duration::from_secs(1);
const WORLD_CHUNKS: &str = "world/chunks";
const INTERACTION_DISTANCE: f32 = 6.0;
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
}

/// Runs the engine until the window is closed.
///
/// # Errors
///
/// Returns an error if world loading, the event loop, window, or GPU
/// initialization fails.
pub fn run() -> Result<(), RunError> {
    init_tracing();

    let world = load_world()?;
    let event_loop = EventLoop::new()?;
    let mut app = App::new(world);
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

fn load_world() -> Result<World, SaveError> {
    let directory = Path::new(WORLD_CHUNKS);
    if directory.exists() {
        World::load_chunks(directory)
    } else {
        Ok(demo_world())
    }
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

impl App {
    fn new(world: World) -> Self {
        let player = Player::new(spawn_position(&world));
        let mut camera = Camera::new(1.0);
        camera.set_position(player.eye_position());
        debug!(spawn = ?player.position, "player spawned");

        Self {
            window: None,
            renderer: None,
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

    fn rebuild_chunks(&mut self, dirty: &DirtyChunks) {
        let started = Instant::now();
        if let Some(renderer) = &mut self.renderer {
            for chunk in dirty {
                let mesh = build_chunk_mesh(&self.world, *chunk);
                renderer.upload_chunk(*chunk, &mesh);
            }
        }
        let elapsed = started.elapsed();
        debug!(
            dirty_chunks = dirty.len(),
            vertices = self.renderer.as_ref().map_or(0, Renderer::total_vertices),
            rebuild_ms = elapsed.as_secs_f64() * 1000.0,
            "rebuilt chunks"
        );
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
        self.rebuild_chunks(&dirty);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
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
        self.rebuild_chunks(&dirty);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn save_world(&self) {
        match self.world.save_chunks(Path::new(WORLD_CHUNKS)) {
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

    fn redraw(&mut self) {
        if self.occluded {
            return;
        }

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
            self.player.update(&self.world, input, delta_seconds);
            self.camera.set_position(self.player.eye_position());
            self.update_highlight();
        }

        let overlay = self.show_overlay.then(|| {
            format!(
                "FPS {:.0}  {:.2} MS",
                self.overlay_fps, self.overlay_frame_ms
            )
        });
        if let Some(renderer) = &mut self.renderer {
            renderer.set_camera(&self.camera);
            renderer.set_overlay(overlay.as_deref());
            renderer.render();
        }
        if self.focused
            && let Some(window) = &self.window
        {
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
                player = ?self.player.position,
                on_ground = self.player.on_ground,
                "frame timing"
            );
            self.overlay_fps = frames / seconds;
            self.overlay_frame_ms = seconds * 1000.0 / frames;
            self.frame_accum = Duration::ZERO;
            self.frame_count = 0;
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
        let renderer = match pollster::block_on(Renderer::new(
            window.clone(),
            instance,
            &self.world,
            &self.camera,
        )) {
            Ok(renderer) => renderer,
            Err(error) => {
                self.fail(RunError::Renderer(error), event_loop);
                return;
            }
        };

        info!(width = size.width, height = size.height, "window ready");
        self.window = Some(window.clone());
        self.renderer = Some(renderer);
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
}

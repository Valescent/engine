//! Valescent Engine: a voxel engine written in Rust.
//!
//! The binary in `src/main.rs` is a thin shell around this crate. Everything
//! the engine does lives here so it can be tested, benchmarked, and reused
//! without a window.

mod camera;
mod renderer;
mod voxel;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::IVec3;
use thiserror::Error;
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use camera::Camera;
use renderer::Renderer;
pub use renderer::RendererError;
use voxel::build_mesh;

const WINDOW_TITLE: &str = concat!("Valescent Engine ", env!("CARGO_PKG_VERSION"));
const TOGGLE_TARGET: IVec3 = IVec3::new(1, 1, 1);
const MAX_FRAME_DELTA: Duration = Duration::from_millis(100);
const FRAME_LOG_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Error)]
pub enum RunError {
    #[error(transparent)]
    EventLoop(#[from] winit::error::EventLoopError),
    #[error(transparent)]
    Window(#[from] winit::error::OsError),
    #[error(transparent)]
    Renderer(#[from] RendererError),
}

/// Runs the engine until the window is closed.
///
/// # Errors
///
/// Returns an error if the event loop, window, or GPU initialization fails.
pub fn run() -> Result<(), RunError> {
    init_tracing();

    let event_loop = EventLoop::new()?;
    let mut app = App::new();
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

struct App {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    voxels: HashSet<IVec3>,
    camera: Camera,
    error: Option<RunError>,
    pressed: HashSet<KeyCode>,
    mouse_captured: bool,
    focused: bool,
    occluded: bool,
    last_frame: Instant,
    frame_accum: Duration,
    frame_count: u32,
}

impl App {
    fn new() -> Self {
        Self {
            window: None,
            renderer: None,
            voxels: initial_voxels(),
            camera: Camera::new(1.0),
            error: None,
            pressed: HashSet::new(),
            mouse_captured: false,
            focused: true,
            occluded: false,
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

    fn toggle_voxel(&mut self) {
        if !self.voxels.remove(&TOGGLE_TARGET) {
            self.voxels.insert(TOGGLE_TARGET);
        }

        let mesh = build_mesh(&self.voxels);
        if let Some(renderer) = &mut self.renderer {
            renderer.upload_mesh(&mesh);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        debug!(voxels = self.voxels.len(), "toggled voxel");
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
            self.camera.move_by(forward_axis, right_axis, delta_seconds);
        }

        if let Some(renderer) = &mut self.renderer {
            renderer.set_camera(&self.camera);
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
                "frame timing"
            );
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
                if code == KeyCode::Space {
                    self.toggle_voxel();
                } else if code == KeyCode::Escape {
                    self.release_cursor();
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
        let mesh = build_mesh(&self.voxels);
        let renderer = match pollster::block_on(Renderer::new(
            window.clone(),
            instance,
            &mesh,
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
                self.release_cursor();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => self.resize(size),
            WindowEvent::KeyboardInput { event, .. } => self.handle_key(&event),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => self.capture_cursor(),
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

fn initial_voxels() -> HashSet<IVec3> {
    [
        IVec3::new(0, 0, 0),
        IVec3::new(1, 0, 0),
        IVec3::new(2, 0, 0),
        IVec3::new(0, 0, 1),
        IVec3::new(1, 0, 1),
        IVec3::new(2, 0, 1),
        IVec3::new(0, 0, 2),
        IVec3::new(1, 0, 2),
        IVec3::new(2, 0, 2),
        IVec3::new(1, 1, 1),
    ]
    .into_iter()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_look_requires_capture_and_focus() {
        let mut app = App::new();
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
}

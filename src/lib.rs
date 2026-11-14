//! Valescent Engine: a voxel engine written in Rust.
//!
//! The binary in `src/main.rs` is a thin shell around this crate. Everything
//! the engine does lives here so it can be tested, benchmarked, and reused
//! without a window.

mod renderer;
mod voxel;

use std::collections::HashSet;
use std::sync::Arc;

use glam::camera::rh::{proj::directx, view::look_at_mat4};
use glam::{IVec3, Mat4, Vec3};
use thiserror::Error;
use tracing::{debug, error, info};
use tracing_subscriber::EnvFilter;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use renderer::Renderer;
pub use renderer::RendererError;
use voxel::build_mesh;

const WINDOW_TITLE: &str = concat!("Valescent Engine ", env!("CARGO_PKG_VERSION"));
const TOGGLE_TARGET: IVec3 = IVec3::new(1, 1, 1);

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
}

impl App {
    fn new() -> Self {
        Self {
            window: None,
            renderer: None,
            voxels: initial_voxels(),
            camera: Camera::new(1.0),
            error: None,
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
        window.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.resize(size),
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed
                    && !event.repeat
                    && event.logical_key == Key::Named(NamedKey::Space)
                {
                    self.toggle_voxel();
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.render();
                }
            }
            _ => {}
        }
    }
}

impl App {
    fn resize(&mut self, size: PhysicalSize<u32>) {
        self.camera.set_aspect(size.width, size.height);
        if let Some(renderer) = &mut self.renderer {
            renderer.resize(size, &self.camera);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Camera {
    position: Vec3,
    target: Vec3,
    up: Vec3,
    fov_y_radians: f32,
    z_near: f32,
    z_far: f32,
    aspect: f32,
}

impl Camera {
    fn new(aspect: f32) -> Self {
        Self {
            position: Vec3::new(6.0, 4.5, 7.0),
            target: Vec3::new(1.0, 0.5, 1.0),
            up: Vec3::Y,
            fov_y_radians: std::f32::consts::FRAC_PI_3,
            z_near: 0.1,
            z_far: 100.0,
            aspect,
        }
    }

    fn set_aspect(&mut self, width: u32, height: u32) {
        self.aspect = aspect_ratio(width, height);
    }

    fn view_projection(&self) -> Mat4 {
        directx::perspective(self.fov_y_radians, self.aspect, self.z_near, self.z_far)
            * look_at_mat4(self.position, self.target, self.up)
    }
}

#[allow(clippy::cast_precision_loss)]
fn aspect_ratio(width: u32, height: u32) -> f32 {
    width.max(1) as f32 / height.max(1) as f32
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

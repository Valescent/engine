//! Minimal `wgpu` renderer for chunk meshes, textures, and the block outline.

use std::collections::BTreeMap;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use thiserror::Error;
use tracing::{error, info, warn};
use wgpu::util::DeviceExt;
use winit::dpi::PhysicalSize;
use winit::window::Window;

use crate::block::{TEXTURE_LAYERS, TEXTURE_SIZE};
use crate::camera::Camera;
use crate::frustum::Frustum;
use crate::horizon::Horizon;
use crate::mesh::{Lod, MeshData};
use crate::overlay::{OverlayVertex, build_overlay_mesh};
use crate::quad_renderer::{POOL_BYTES, QuadRenderer};
use crate::sky::SkySample;
use crate::voxel::{HighlightVertex, Vertex};
use crate::world::{CHUNK_SIZE, ChunkCoord};

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.45,
    g: 0.65,
    b: 0.90,
    a: 1.0,
};

/// At most one pool-full warning per second; the rest are counted and folded
/// into the next warning.
const POOL_WARNING_INTERVAL: Duration = Duration::from_secs(1);

/// Per-frame uniform: camera, sky colours, sun, and fog settings.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct FrameUniform {
    view_projection: [f32; 16],
    sky_colour: [f32; 4],
    ambient_colour: [f32; 4],
    sun_colour: [f32; 4],
    sun_direction: [f32; 4],
    /// Unlit flag, camera far plane, time of day, fog end distance.
    settings: [f32; 4],
    camera_position: [f32; 4],
}

impl Default for FrameUniform {
    fn default() -> Self {
        Self {
            view_projection: [0.0; 16],
            sky_colour: [0.45, 0.65, 0.90, 1.0],
            ambient_colour: [0.65, 0.70, 0.75, 0.0],
            sun_colour: [1.00, 0.95, 0.85, 0.0],
            sun_direction: [0.0, 1.0, 0.0, 0.0],
            settings: [0.0, 100.0, 0.25, 100.0],
            camera_position: [0.0; 4],
        }
    }
}

#[derive(Debug, Error)]
pub enum RendererError {
    #[error("failed to create the window surface: {0}")]
    CreateSurface(#[from] wgpu::CreateSurfaceError),
    #[error("failed to find a compatible GPU adapter: {0}")]
    RequestAdapter(#[from] wgpu::RequestAdapterError),
    #[error("failed to create the GPU device: {0}")]
    RequestDevice(#[from] wgpu::RequestDeviceError),
    #[error("the surface is not supported by the selected adapter")]
    UnsupportedSurface,
    #[error("GPU validation failed: {0}")]
    Validation(String),
}

struct GpuMesh {
    vertex_buffer: wgpu::Buffer,
    /// Per-chunk buffer only when the mesh exceeds a `u16` index; otherwise the
    /// renderer's shared quad pattern is used.
    index_buffer: Option<wgpu::Buffer>,
    index_format: wgpu::IndexFormat,
    index_count: u32,
    bytes: u64,
}

struct GpuChunk {
    _origin_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    lod0: Option<(u64, GpuMesh)>,
    lod1: Option<(u64, GpuMesh)>,
    lod2: Option<(u64, GpuMesh)>,
    water: Option<(u64, GpuMesh)>,
    active: Option<Lod>,
}

impl GpuChunk {
    fn mesh(&self, lod: Lod) -> Option<&GpuMesh> {
        let slot = match lod {
            Lod::Lod0 => &self.lod0,
            Lod::Lod1 => &self.lod1,
            Lod::Lod2 => &self.lod2,
        };
        slot.as_ref().map(|(_, mesh)| mesh)
    }

    fn slot(&mut self, lod: Lod) -> &mut Option<(u64, GpuMesh)> {
        match lod {
            Lod::Lod0 => &mut self.lod0,
            Lod::Lod1 => &mut self.lod1,
            Lod::Lod2 => &mut self.lod2,
        }
    }

    /// Stores `mesh` in `lod`, returning the mesh it replaced.
    fn replace_mesh(&mut self, lod: Lod, revision: u64, mesh: GpuMesh) -> Option<GpuMesh> {
        self.slot(lod)
            .replace((revision, mesh))
            .map(|(_, replaced)| replaced)
    }

    /// Clears `lod`, returning the mesh it held.
    fn take_mesh(&mut self, lod: Lod) -> Option<GpuMesh> {
        self.slot(lod).take().map(|(_, mesh)| mesh)
    }

    fn active_mesh(&self) -> Option<&GpuMesh> {
        self.active.and_then(|lod| self.mesh(lod))
    }

    fn is_empty(&self) -> bool {
        self.lod0.is_none() && self.lod1.is_none() && self.lod2.is_none() && self.water.is_none()
    }

    fn meshes(&self) -> impl Iterator<Item = &GpuMesh> {
        [
            self.lod0.as_ref(),
            self.lod1.as_ref(),
            self.lod2.as_ref(),
            self.water.as_ref(),
        ]
        .into_iter()
        .flatten()
        .map(|(_, mesh)| mesh)
    }
}

/// Counts for one drawn frame.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FrameStats {
    pub(crate) total_chunks: u32,
    pub(crate) visible_chunks: u32,
    pub(crate) visible_vertices: u64,
    pub(crate) draw_calls: u32,
    /// Vertex and index bytes resident for every built LOD and water mesh.
    pub(crate) geometry_bytes: u64,
    /// Vertex and index bytes reached by this frame's draws.
    pub(crate) visible_bytes: u64,
    /// Quad-pool records currently reserved, including page rounding.
    pub(crate) allocated_bytes: u64,
    /// Session high-water mark of `allocated_bytes`.
    pub(crate) peak_allocated_bytes: u64,
    /// Latest resolved GPU time for the scene pass, in milliseconds.
    pub(crate) gpu_ms: f64,
}

pub(crate) struct Renderer {
    window: Arc<Window>,
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    water_pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    frame: FrameUniform,
    clear_colour: wgpu::Color,
    camera_bind_group: wgpu::BindGroup,
    chunk_bind_group_layout: wgpu::BindGroupLayout,
    chunks: BTreeMap<ChunkCoord, GpuChunk>,
    /// Six `u16` indices per quad, shared by every chunk that fits a `u16` index.
    shared_index_buffer: wgpu::Buffer,
    /// Sum of `GpuMesh::bytes` over every resident slot plus the shared index
    /// buffer, maintained on mutation.
    geometry_bytes: u64,
    /// The ADR-0015 quad-pool path, when the adapter supports it.
    quad: Option<QuadRenderer>,
    frame_index: u64,
    /// Last time a pool-full warning was emitted.
    last_pool_warning: Option<Instant>,
    /// Pool-full messages folded into the next warning.
    suppressed_pool_warnings: u32,
    present_modes: Vec<wgpu::PresentMode>,
    vsync: bool,
    _block_texture: wgpu::Texture,
    texture_bind_group: wgpu::BindGroup,
    highlight_pipeline: wgpu::RenderPipeline,
    highlight_buffer: wgpu::Buffer,
    highlight_count: u32,
    overlay_pipeline: wgpu::RenderPipeline,
    overlay_buffer: wgpu::Buffer,
    overlay_buffer_size: u64,
    overlay_count: u32,
    depth_texture: wgpu::Texture,
    depth_view: wgpu::TextureView,
    profiler: Option<wgpu_profiler::GpuProfiler>,
    gpu_ms: f64,
    capture: Option<Capture>,
}

/// One pending frame readback, armed by `ENGINE_CAPTURE`.
struct Capture {
    path: PathBuf,
    delay: Duration,
    started: Instant,
    buffer: wgpu::Buffer,
    bytes_per_row: u32,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    done: bool,
}

impl Capture {
    fn new(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> Option<Self> {
        let path = PathBuf::from(std::env::var("ENGINE_CAPTURE").ok()?);
        if !config.usage.contains(wgpu::TextureUsages::COPY_SRC) {
            warn!("surface cannot be copied; frame capture disabled");
            return None;
        }
        let delay = std::env::var("ENGINE_CAPTURE_DELAY")
            .ok()
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(20.0)
            .max(0.0);
        let bytes_per_row = (config.width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
            * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame capture"),
            size: u64::from(bytes_per_row) * u64::from(config.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        info!(
            path = %path.display(),
            delay_secs = delay,
            "frame capture armed"
        );
        Some(Self {
            path,
            delay: Duration::from_secs_f64(delay),
            started: Instant::now(),
            buffer,
            bytes_per_row,
            width: config.width,
            height: config.height,
            format: config.format,
            done: false,
        })
    }

    fn due(&self) -> bool {
        !self.done && self.started.elapsed() >= self.delay
    }
}

impl Renderer {
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn new(
        window: Arc<Window>,
        instance: wgpu::Instance,
        camera: &Camera,
    ) -> Result<Self, RendererError> {
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        let surface = instance.create_surface(window.clone())?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await?;

        let adapter_info = adapter.get_info();
        info!(
            adapter = %adapter_info.name,
            backend = ?adapter_info.backend,
            "selected GPU adapter"
        );

        let adapter_limits = adapter.limits();
        let downlevel = adapter.get_downlevel_capabilities().flags;
        let legacy_forced = std::env::var("ENGINE_LEGACY_RENDER").is_ok_and(|value| value == "1");
        let quad_supported = !legacy_forced
            && adapter
                .features()
                .contains(wgpu::Features::INDIRECT_FIRST_INSTANCE)
            && downlevel.contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION)
            && downlevel.contains(wgpu::DownlevelFlags::VERTEX_STORAGE)
            && adapter_limits.max_storage_buffers_per_shader_stage >= 2
            && adapter_limits.max_storage_buffer_binding_size >= POOL_BYTES
            && adapter_limits.max_buffer_size >= POOL_BYTES;
        if quad_supported {
            info!("quad pool render path enabled");
        } else {
            warn!("quad pool render path unavailable; using per-chunk meshes");
        }

        let mut required_features = timer_features(&adapter);
        if quad_supported {
            required_features |= wgpu::Features::INDIRECT_FIRST_INSTANCE;
        }
        let mut required_limits =
            wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter_limits);
        if quad_supported {
            required_limits.max_storage_buffers_per_shader_stage = 2;
            required_limits.max_storage_buffer_binding_size = POOL_BYTES;
            required_limits.max_buffer_size = required_limits.max_buffer_size.max(POOL_BYTES);
        }

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("engine device"),
                required_features,
                required_limits,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await?;

        let profiler = if timer_features(&adapter).contains(wgpu::Features::TIMESTAMP_QUERY) {
            #[cfg(feature = "tracy")]
            let created = wgpu_profiler::GpuProfiler::new_with_tracy_client(
                wgpu_profiler::GpuProfilerSettings::default(),
                adapter.get_info().backend,
                &device,
                &queue,
            );
            #[cfg(not(feature = "tracy"))]
            let created = wgpu_profiler::GpuProfiler::new(
                &device,
                wgpu_profiler::GpuProfilerSettings::default(),
            );
            match created {
                Ok(profiler) => Some(profiler),
                Err(error) => {
                    warn!(%error, "GPU timing disabled");
                    None
                }
            }
        } else {
            warn!("no timestamp-query support; GPU timing disabled");
            None
        };

        let config = surface
            .get_default_config(&adapter, width, height)
            .ok_or(RendererError::UnsupportedSurface)?;
        let caps = surface.get_capabilities(&adapter);
        let present_modes = caps.present_modes;
        let mut config = config;
        config.present_mode = wgpu::PresentMode::Fifo;
        if caps.usages.contains(wgpu::TextureUsages::COPY_SRC) {
            config.usage |= wgpu::TextureUsages::COPY_SRC;
        }
        surface.configure(&device, &config);
        let capture = Capture::new(&device, &config);

        let frame = FrameUniform {
            view_projection: camera.view_projection().to_cols_array(),
            settings: [0.0, camera.far_plane(), 0.25, camera.fog_end()],
            ..FrameUniform::default()
        };
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame uniform"),
            size: size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&uniform_buffer, 0, bytemuck::bytes_of(&frame));

        let camera_bind_group_layout = create_camera_bind_group_layout(&device);
        let camera_bind_group =
            create_camera_bind_group(&device, &camera_bind_group_layout, &uniform_buffer);
        let chunk_bind_group_layout = create_chunk_bind_group_layout(&device);
        let texture_bind_group_layout = create_texture_bind_group_layout(&device);
        let (block_texture, texture_bind_group) =
            create_block_texture(&device, &queue, &texture_bind_group_layout);
        let quad = if quad_supported {
            QuadRenderer::new(
                &device,
                &queue,
                &config,
                &camera_bind_group_layout,
                &texture_bind_group_layout,
            )
        } else {
            None
        };
        let pipeline = create_pipeline(
            &device,
            &config,
            &camera_bind_group_layout,
            &chunk_bind_group_layout,
            &texture_bind_group_layout,
            "voxel pipeline",
            "fs_main",
            None,
            true,
            wgpu::CompareFunction::Less,
        )?;
        let water_pipeline = create_pipeline(
            &device,
            &config,
            &camera_bind_group_layout,
            &chunk_bind_group_layout,
            &texture_bind_group_layout,
            "water pipeline",
            "fs_water",
            Some(wgpu::BlendState::ALPHA_BLENDING),
            false,
            wgpu::CompareFunction::LessEqual,
        )?;
        let highlight_pipeline =
            create_highlight_pipeline(&device, &config, &camera_bind_group_layout)?;
        let (depth_texture, depth_view) = create_depth_texture(&device, &config);
        let (highlight_buffer, highlight_count) = create_highlight_buffer(&device, &[]);
        let overlay_pipeline = create_overlay_pipeline(&device, &config)?;
        let (overlay_buffer, overlay_buffer_size) = create_overlay_buffer(&device);

        let shared_index_buffer = create_shared_index_buffer(&device);
        let shared_index_bytes = shared_index_buffer.size();

        let renderer = Self {
            window,
            instance,
            surface,
            device,
            queue,
            config,
            pipeline,
            water_pipeline,
            uniform_buffer,
            frame,
            clear_colour: CLEAR_COLOR,
            camera_bind_group,
            chunk_bind_group_layout,
            chunks: BTreeMap::new(),
            shared_index_buffer,
            geometry_bytes: shared_index_bytes,
            quad,
            frame_index: 0,
            last_pool_warning: None,
            suppressed_pool_warnings: 0,
            present_modes,
            vsync: true,
            _block_texture: block_texture,
            texture_bind_group,
            highlight_pipeline,
            highlight_buffer,
            highlight_count,
            overlay_pipeline,
            overlay_buffer,
            overlay_buffer_size,
            overlay_count: 0,
            depth_texture,
            depth_view,
            profiler,
            gpu_ms: 0.0,
            capture,
        };

        info!(
            width = renderer.config.width,
            height = renderer.config.height,
            "renderer ready"
        );

        Ok(renderer)
    }

    pub(crate) fn resize(&mut self, size: PhysicalSize<u32>, camera: &Camera) {
        let width = size.width.max(1);
        let height = size.height.max(1);
        if width == self.config.width && height == self.config.height {
            return;
        }

        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        let (depth_texture, depth_view) = create_depth_texture(&self.device, &self.config);
        self.depth_texture = depth_texture;
        self.depth_view = depth_view;
        self.set_camera(camera);
    }

    pub(crate) fn set_camera(&mut self, camera: &Camera) {
        self.frame.view_projection = camera.view_projection().to_cols_array();
        self.frame.settings[1] = camera.far_plane();
        self.frame.settings[3] = camera.fog_end();
        let position = camera.position();
        self.frame.camera_position = [position.x, position.y, position.z, 0.0];
        self.write_frame();
    }

    /// Writes sky colours, sun direction, the unlit flag, and the clear colour.
    pub(crate) fn set_sky(&mut self, sample: &SkySample, unlit: bool) {
        self.frame.sky_colour = [sample.sky[0], sample.sky[1], sample.sky[2], 1.0];
        self.frame.ambient_colour = [sample.ambient[0], sample.ambient[1], sample.ambient[2], 0.0];
        self.frame.sun_colour = [sample.sun[0], sample.sun[1], sample.sun[2], 0.0];
        self.frame.sun_direction = [
            sample.sun_direction[0],
            sample.sun_direction[1],
            sample.sun_direction[2],
            0.0,
        ];
        self.frame.settings[0] = if unlit { 1.0 } else { 0.0 };
        self.clear_colour = wgpu::Color {
            r: f64::from(sample.sky[0]),
            g: f64::from(sample.sky[1]),
            b: f64::from(sample.sky[2]),
            a: 1.0,
        };
        self.write_frame();
    }

    fn write_frame(&mut self) {
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&self.frame));
    }

    /// Emits at most one pool-full warning per second.
    ///
    /// Every suppressed message is counted and reported with the next warning.
    fn warn_pool_full(&mut self, chunk: ChunkCoord, quads: usize, water: bool) {
        let now = Instant::now();
        if self
            .last_pool_warning
            .is_some_and(|last| now.saturating_duration_since(last) < POOL_WARNING_INTERVAL)
        {
            self.suppressed_pool_warnings = self.suppressed_pool_warnings.saturating_add(1);
            return;
        }
        let suppressed = std::mem::take(&mut self.suppressed_pool_warnings);
        self.last_pool_warning = Some(now);
        warn!(?chunk, quads, water, suppressed, "quad pool full; retrying");
    }

    /// The LOD the renderer currently draws for a chunk, if any.
    pub(crate) fn active_lod(&self, chunk: ChunkCoord) -> Option<Lod> {
        if let Some(quad) = &self.quad {
            return quad.active_lod(chunk);
        }
        self.chunks
            .get(&chunk)
            .and_then(|gpu_chunk| gpu_chunk.active)
    }

    /// Whether the renderer has the given LOD resident for a chunk.
    pub(crate) fn has_lod(&self, chunk: ChunkCoord, lod: Lod) -> bool {
        if let Some(quad) = &self.quad {
            return quad.has_lod(chunk, lod);
        }
        self.chunks
            .get(&chunk)
            .is_some_and(|gpu_chunk| gpu_chunk.mesh(lod).is_some())
    }

    pub(crate) fn upload_chunk(
        &mut self,
        chunk: ChunkCoord,
        lod: Lod,
        revision: u64,
        mesh: &MeshData,
    ) -> bool {
        let frame = self.frame_index;
        let uploaded = self
            .quad
            .as_mut()
            .map(|quad| quad.upload(&self.queue, chunk, lod, mesh, frame));
        if let Some(uploaded) = uploaded {
            if !uploaded {
                self.warn_pool_full(chunk, mesh.quads.len(), false);
            }
            return uploaded;
        }

        if mesh.is_empty() {
            let mut removed = 0;
            if let Some(gpu_chunk) = self.chunks.get_mut(&chunk) {
                removed = gpu_chunk.take_mesh(lod).map_or(0, |mesh| mesh.bytes);
                if gpu_chunk.active == Some(lod) {
                    gpu_chunk.active = None;
                }
                if gpu_chunk.is_empty() {
                    self.chunks.remove(&chunk);
                }
            }
            self.geometry_bytes -= removed;
            return true;
        }

        let (vertex_buffer, _) = create_vertex_buffer(&self.device, &mesh.vertices);
        let (index_buffer, index_format, index_count) = create_index_buffer(&self.device, mesh);
        let gpu_mesh = GpuMesh {
            bytes: vertex_buffer.size() + index_buffer.as_ref().map_or(0, wgpu::Buffer::size),
            vertex_buffer,
            index_buffer,
            index_format,
            index_count,
        };
        let bytes = gpu_mesh.bytes;
        let replaced = {
            let gpu_chunk = self.chunk_entry(chunk);
            let replaced = gpu_chunk.replace_mesh(lod, revision, gpu_mesh);
            if gpu_chunk.active.is_none() {
                gpu_chunk.active = Some(lod);
            }
            replaced
        };
        self.geometry_bytes += bytes;
        if let Some(replaced) = replaced {
            self.geometry_bytes -= replaced.bytes;
        }
        true
    }

    fn chunk_entry(&mut self, chunk: ChunkCoord) -> &mut GpuChunk {
        let device = &self.device;
        let layout = &self.chunk_bind_group_layout;
        self.chunks.entry(chunk).or_insert_with(|| {
            let origin = chunk.origin();
            let origin_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("chunk origin"),
                contents: bytemuck::cast_slice(&[origin.x, origin.y, origin.z, 0.0]),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            GpuChunk {
                bind_group: create_chunk_bind_group(device, layout, &origin_buffer),
                _origin_buffer: origin_buffer,
                lod0: None,
                lod1: None,
                lod2: None,
                water: None,
                active: None,
            }
        })
    }

    /// Makes `lod` the drawn mesh when that mesh is resident.
    ///
    /// Once the active mesh is resident, stale LOD slots are dropped so LOD
    /// transitions do not retain the previous meshes. Water is never pruned
    /// here, and the active slot always survives.
    pub(crate) fn select_active(&mut self, chunk: ChunkCoord, lod: Lod) {
        if let Some(quad) = self.quad.as_mut() {
            let frame = self.frame_index;
            quad.select_active(&self.queue, chunk, lod, frame);
            return;
        }
        let mut pruned = 0;
        if let Some(gpu_chunk) = self.chunks.get_mut(&chunk)
            && gpu_chunk.mesh(lod).is_some()
        {
            gpu_chunk.active = Some(lod);
            for stale in [Lod::Lod0, Lod::Lod1, Lod::Lod2] {
                if stale != lod {
                    pruned += gpu_chunk.take_mesh(stale).map_or(0, |mesh| mesh.bytes);
                }
            }
        }
        self.geometry_bytes -= pruned;
    }

    /// Uploads or clears a chunk's transparent water mesh.
    pub(crate) fn upload_water_chunk(
        &mut self,
        chunk: ChunkCoord,
        revision: u64,
        mesh: &MeshData,
    ) -> bool {
        let frame = self.frame_index;
        let uploaded = self
            .quad
            .as_mut()
            .map(|quad| quad.upload_water(&self.queue, chunk, mesh, frame));
        if let Some(uploaded) = uploaded {
            if !uploaded {
                self.warn_pool_full(chunk, mesh.quads.len(), true);
            }
            return uploaded;
        }
        if mesh.is_empty() {
            let mut removed = 0;
            if let Some(gpu_chunk) = self.chunks.get_mut(&chunk) {
                removed = gpu_chunk.water.take().map_or(0, |(_, mesh)| mesh.bytes);
                if gpu_chunk.is_empty() {
                    self.chunks.remove(&chunk);
                }
            }
            self.geometry_bytes -= removed;
            return true;
        }

        let (vertex_buffer, _) = create_vertex_buffer(&self.device, &mesh.vertices);
        let (index_buffer, index_format, index_count) = create_index_buffer(&self.device, mesh);
        let gpu_mesh = GpuMesh {
            bytes: vertex_buffer.size() + index_buffer.as_ref().map_or(0, wgpu::Buffer::size),
            vertex_buffer,
            index_buffer,
            index_format,
            index_count,
        };
        let bytes = gpu_mesh.bytes;
        let replaced = self.chunk_entry(chunk).water.replace((revision, gpu_mesh));
        self.geometry_bytes += bytes;
        if let Some((_, replaced)) = replaced {
            self.geometry_bytes -= replaced.bytes;
        }
        true
    }

    pub(crate) fn remove_chunk(&mut self, chunk: ChunkCoord) {
        if let Some(quad) = self.quad.as_mut() {
            let frame = self.frame_index;
            quad.remove(chunk, frame);
        }
        if let Some(gpu_chunk) = self.chunks.remove(&chunk) {
            self.geometry_bytes -= gpu_chunk.meshes().map(|mesh| mesh.bytes).sum::<u64>();
        }
    }

    pub(crate) fn total_vertices(&self) -> u64 {
        if let Some(quad) = &self.quad {
            return quad.total_vertices();
        }
        self.chunks
            .values()
            .filter_map(GpuChunk::active_mesh)
            .map(|mesh| u64::from(mesh.index_count))
            .sum()
    }

    pub(crate) fn toggle_vsync(&mut self) -> bool {
        self.vsync = !self.vsync;
        let mode = preferred_present_mode(self.vsync, &self.present_modes);
        self.vsync = mode == wgpu::PresentMode::Fifo;
        self.config.present_mode = mode;
        self.surface.configure(&self.device, &self.config);
        self.window.request_redraw();
        self.vsync
    }

    pub(crate) fn set_highlight(&mut self, mesh: &[HighlightVertex]) {
        let (highlight_buffer, highlight_count) = create_highlight_buffer(&self.device, mesh);
        self.highlight_buffer = highlight_buffer;
        self.highlight_count = highlight_count;
    }

    pub(crate) fn set_overlay(&mut self, text: Option<&str>) {
        let Some(text) = text.filter(|text| !text.is_empty()) else {
            self.overlay_count = 0;
            return;
        };
        let mesh = build_overlay_mesh(text, self.config.width, self.config.height);
        let count = u32::try_from(mesh.len()).expect("overlay vertex count fits in u32");
        let bytes = bytemuck::cast_slice(&mesh);
        let byte_count = u64::try_from(bytes.len()).expect("overlay byte size fits in u64");
        if byte_count > self.overlay_buffer_size {
            self.overlay_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("overlay vertices"),
                size: byte_count,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.overlay_buffer_size = byte_count;
        }
        self.queue.write_buffer(&self.overlay_buffer, 0, bytes);
        self.overlay_count = count;
    }

    #[allow(clippy::cast_precision_loss, clippy::too_many_lines)]
    pub(crate) fn render(&mut self, frustum: &Frustum, horizon: &Horizon) -> FrameStats {
        self.frame_index += 1;
        if let Some(quad) = self.quad.as_mut() {
            quad.begin_frame(self.frame_index);
        }
        let mut stats = FrameStats {
            gpu_ms: self.gpu_ms,
            ..FrameStats::default()
        };
        let frame = {
            profiling::scope!("acquire");
            match self.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(frame) => frame,
                wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                    return stats;
                }
                wgpu::CurrentSurfaceTexture::Suboptimal(texture) => {
                    drop(texture);
                    self.surface.configure(&self.device, &self.config);
                    self.window.request_redraw();
                    return stats;
                }
                wgpu::CurrentSurfaceTexture::Outdated => {
                    self.surface.configure(&self.device, &self.config);
                    self.window.request_redraw();
                    return stats;
                }
                wgpu::CurrentSurfaceTexture::Lost => {
                    self.recreate_surface();
                    self.window.request_redraw();
                    return stats;
                }
                wgpu::CurrentSurfaceTexture::Validation => {
                    error!("surface acquisition hit a validation error");
                    return stats;
                }
            }
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame encoder"),
            });

        let mut captured = false;
        {
            profiling::scope!("encode");
            let attachments = [Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(self.clear_colour),
                    store: wgpu::StoreOp::Store,
                },
            })];
            let mut quad = self.quad.take();
            let descriptor = self.scene_pass_descriptor(&attachments);

            if let Some(profiler) = &self.profiler {
                let mut scope = profiler.scope("world", &mut encoder);
                let mut pass = scope.scoped_render_pass("terrain", descriptor);
                self.draw_scene(quad.as_mut(), &mut pass, frustum, horizon, &mut stats);
                drop(pass);
            } else {
                let mut pass = encoder.begin_render_pass(&descriptor);
                self.draw_scene(quad.as_mut(), &mut pass, frustum, horizon, &mut stats);
            }

            self.quad = quad;

            if let Some(profiler) = self.profiler.as_mut() {
                profiler.resolve_queries(&mut encoder);
            }

            if let Some(capture) = &mut self.capture
                && capture.due()
            {
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &frame.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &capture.buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(capture.bytes_per_row),
                            rows_per_image: Some(capture.height),
                        },
                    },
                    wgpu::Extent3d {
                        width: capture.width,
                        height: capture.height,
                        depth_or_array_layers: 1,
                    },
                );
                captured = true;
            }
        }

        {
            profiling::scope!("submit");
            self.queue.submit(Some(encoder.finish()));
            self.window.pre_present_notify();
        }
        if captured {
            self.finish_capture();
        }
        {
            profiling::scope!("present");
            self.queue.present(frame);
        }

        profiling::scope!("profiler end");
        if let Some(profiler) = self.profiler.as_mut() {
            if let Err(error) = profiler.end_frame() {
                warn!(%error, "GPU profiler frame was not resolved");
            }
            let period = self.queue.get_timestamp_period();
            if let Some(queries) = profiler.process_finished_frame(period) {
                let gpu_ms = queries
                    .iter()
                    .filter_map(|query| query.time.as_ref())
                    .map(|time| time.end - time.start)
                    .sum::<f64>()
                    * 1000.0;
                self.gpu_ms = gpu_ms;
                crate::metrics::gpu_ms(gpu_ms);
            }
        }

        stats
    }

    fn finish_capture(&mut self) {
        let Some(capture) = &mut self.capture else {
            return;
        };
        let slice = capture.buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let result = slice.get_mapped_range().map(|data| {
            write_bmp(
                &capture.path,
                capture.width,
                capture.height,
                capture.bytes_per_row,
                capture.format,
                &data,
            )
        });
        capture.buffer.unmap();
        capture.done = true;
        match result {
            Ok(Ok(())) => info!(path = %capture.path.display(), "captured frame"),
            Ok(Err(error)) => {
                warn!(%error, path = %capture.path.display(), "failed to write frame capture");
            }
            Err(error) => {
                warn!(%error, path = %capture.path.display(), "failed to map frame capture");
            }
        }
    }

    fn scene_pass_descriptor<'a>(
        &'a self,
        color_attachments: &'a [Option<wgpu::RenderPassColorAttachment<'a>>],
    ) -> wgpu::RenderPassDescriptor<'a> {
        wgpu::RenderPassDescriptor {
            label: Some("voxel pass"),
            color_attachments,
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        }
    }

    #[allow(clippy::cast_precision_loss)]
    fn draw_scene(
        &self,
        quad: Option<&mut QuadRenderer>,
        pass: &mut wgpu::RenderPass<'_>,
        frustum: &Frustum,
        horizon: &Horizon,
        stats: &mut FrameStats,
    ) {
        if let Some(quad) = quad {
            quad.draw(
                &self.queue,
                pass,
                &self.camera_bind_group,
                &self.texture_bind_group,
                frustum,
                horizon,
                stats,
            );
            return;
        }

        stats.geometry_bytes = self.geometry_bytes;

        // One traversal: count every resident chunk and test each chunk's AABB
        // once, recording the opaque and water meshes that survive culling.
        let mut opaque = Vec::with_capacity(self.chunks.len());
        let mut water = Vec::with_capacity(self.chunks.len());
        for (chunk, gpu_chunk) in &self.chunks {
            stats.total_chunks += 1;
            let min = chunk.origin();
            if !frustum.intersects_aabb(min, min + Vec3::splat(CHUNK_SIZE as f32)) {
                continue;
            }
            if let Some(mesh) = gpu_chunk.active_mesh() {
                opaque.push((gpu_chunk, mesh));
            }
            if let Some((_, mesh)) = &gpu_chunk.water {
                water.push((gpu_chunk, mesh));
            }
        }

        if !opaque.is_empty() {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            pass.set_bind_group(2, &self.texture_bind_group, &[]);
            for (gpu_chunk, mesh) in opaque {
                pass.set_bind_group(1, &gpu_chunk.bind_group, &[]);
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(
                    mesh.index_buffer
                        .as_ref()
                        .unwrap_or(&self.shared_index_buffer)
                        .slice(..),
                    mesh.index_format,
                );
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                stats.visible_chunks += 1;
                stats.draw_calls += 1;
                stats.visible_vertices += u64::from(mesh.index_count);
                stats.visible_bytes += mesh.bytes;
            }
        }

        // Transparent water after all opaque geometry; depth writes stay off.
        pass.set_pipeline(&self.water_pipeline);
        pass.set_bind_group(0, &self.camera_bind_group, &[]);
        pass.set_bind_group(2, &self.texture_bind_group, &[]);
        for (gpu_chunk, mesh) in water {
            if gpu_chunk.active_mesh().is_none() {
                stats.visible_chunks += 1;
            }
            pass.set_bind_group(1, &gpu_chunk.bind_group, &[]);
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(
                mesh.index_buffer
                    .as_ref()
                    .unwrap_or(&self.shared_index_buffer)
                    .slice(..),
                mesh.index_format,
            );
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            stats.draw_calls += 1;
            stats.visible_vertices += u64::from(mesh.index_count);
            stats.visible_bytes += mesh.bytes;
        }

        if self.highlight_count > 0 {
            pass.set_pipeline(&self.highlight_pipeline);
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            pass.set_vertex_buffer(0, self.highlight_buffer.slice(..));
            pass.draw(0..self.highlight_count, 0..1);
        }

        if self.overlay_count > 0 {
            pass.set_pipeline(&self.overlay_pipeline);
            pass.set_vertex_buffer(0, self.overlay_buffer.slice(..));
            pass.draw(0..self.overlay_count, 0..1);
        }
    }

    fn recreate_surface(&mut self) {
        match self.instance.create_surface(self.window.clone()) {
            Ok(surface) => {
                self.surface = surface;
                self.surface.configure(&self.device, &self.config);
            }
            Err(error) => error!(%error, "failed to recreate the window surface"),
        }
    }
}

/// Writes one BGRA/RGBA readback as a 24-bit BMP.
#[allow(clippy::cast_possible_truncation)]
fn write_bmp(
    path: &Path,
    width: u32,
    height: u32,
    bytes_per_row: u32,
    format: wgpu::TextureFormat,
    data: &[u8],
) -> std::io::Result<()> {
    let row_bytes = width.saturating_mul(3);
    let padding = (4 - row_bytes % 4) % 4;
    let image_size = (row_bytes + padding).saturating_mul(height);
    let file_size = 54 + image_size;
    let mut out = Vec::with_capacity(usize::try_from(file_size).unwrap_or(0));
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&file_size.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54_u32.to_le_bytes());
    out.extend_from_slice(&40_u32.to_le_bytes());
    out.extend_from_slice(&i32::try_from(width).unwrap_or(i32::MAX).to_le_bytes());
    out.extend_from_slice(&i32::try_from(height).unwrap_or(i32::MAX).to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&24_u16.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&image_size.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&0_u32.to_le_bytes());
    for y in (0..height).rev() {
        let row = &data[y as usize * bytes_per_row as usize..];
        for x in 0..width {
            let i = x as usize * 4;
            let (blue, green, red) = match format {
                wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => {
                    (row[i + 2], row[i + 1], row[i])
                }
                _ => (row[i], row[i + 1], row[i + 2]),
            };
            out.extend_from_slice(&[blue, green, red]);
        }
        out.extend(std::iter::repeat_n(0, padding as usize));
    }
    std::fs::write(path, out)
}

/// Features the GPU profiler needs, intersected with what the adapter supports.
fn timer_features(adapter: &wgpu::Adapter) -> wgpu::Features {
    let requested = wgpu_profiler::GpuProfiler::ALL_WGPU_TIMER_FEATURES & adapter.features();
    if requested.contains(wgpu::Features::TIMESTAMP_QUERY) {
        requested
    } else {
        wgpu::Features::empty()
    }
}

fn preferred_present_mode(vsync: bool, supported: &[wgpu::PresentMode]) -> wgpu::PresentMode {
    if vsync {
        wgpu::PresentMode::Fifo
    } else if supported.contains(&wgpu::PresentMode::Immediate) {
        wgpu::PresentMode::Immediate
    } else if supported.contains(&wgpu::PresentMode::Mailbox) {
        wgpu::PresentMode::Mailbox
    } else {
        wgpu::PresentMode::Fifo
    }
}

fn create_camera_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("camera bind group layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(160),
            },
            count: None,
        }],
    })
}

fn create_camera_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform_buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("camera bind group"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform_buffer.as_entire_binding(),
        }],
    })
}

fn create_chunk_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("chunk bind group layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: wgpu::BufferSize::new(16),
            },
            count: None,
        }],
    })
}

fn create_chunk_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    origin_buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("chunk bind group"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: origin_buffer.as_entire_binding(),
        }],
    })
}

fn create_texture_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("block texture bind group layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

fn create_block_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
) -> (wgpu::Texture, wgpu::BindGroup) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("block texture array"),
        size: wgpu::Extent3d {
            width: TEXTURE_SIZE,
            height: TEXTURE_SIZE,
            depth_or_array_layers: TEXTURE_LAYERS,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let data = block_texture_data();
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(TEXTURE_SIZE * 4),
            rows_per_image: Some(TEXTURE_SIZE),
        },
        wgpu::Extent3d {
            width: TEXTURE_SIZE,
            height: TEXTURE_SIZE,
            depth_or_array_layers: TEXTURE_LAYERS,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("block texture sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("block texture bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    (texture, bind_group)
}

fn block_texture_data() -> Vec<u8> {
    let mut data = Vec::with_capacity(
        usize::try_from(TEXTURE_SIZE * TEXTURE_SIZE * 4 * TEXTURE_LAYERS)
            .expect("texture data size fits in usize"),
    );
    for layer in 0..TEXTURE_LAYERS {
        for y in 0..TEXTURE_SIZE {
            for x in 0..TEXTURE_SIZE {
                data.extend_from_slice(&tile_pixel(layer, x, y));
            }
        }
    }
    data
}

fn tile_pixel(layer: u32, x: u32, y: u32) -> [u8; 4] {
    let noise = tile_noise(layer, x, y);
    let variation = i32::try_from(noise % 25).expect("variation fits in i32") - 12;
    let colour = match layer {
        0 => shade([122, 122, 128], variation),
        2 => shade([82, 142, 66], variation),
        3 if y < 4 => shade([82, 142, 66], variation),
        1 | 3 | 4 => shade([116, 82, 58], variation),
        5 => {
            let dx = i32::try_from(x).expect("tile x fits in i32") - 8;
            let dy = i32::try_from(y).expect("tile y fits in i32") - 8;
            let ring = (dx * dx + dy * dy) / 8 % 2;
            if ring == 0 {
                shade([150, 112, 70], variation)
            } else {
                shade([118, 86, 53], variation)
            }
        }
        6 if x.is_multiple_of(5) => shade([108, 78, 48], variation),
        6 => shade([150, 112, 70], variation),
        8 if x.is_multiple_of(4) || y.is_multiple_of(4) => shade([122, 96, 48], variation),
        8 => shade([238, 198, 108], variation),
        9 if noise.is_multiple_of(11) => shade([64, 132, 196], variation),
        9 => shade([46, 104, 172], variation),
        10 => shade([214, 196, 138], variation),
        _ => {
            if noise.is_multiple_of(7) {
                shade([42, 88, 42], variation)
            } else {
                shade([62, 118, 54], variation)
            }
        }
    };
    [colour[0], colour[1], colour[2], 255]
}

fn shade(base: [i32; 3], variation: i32) -> [u8; 3] {
    base.map(|channel| {
        u8::try_from((channel + variation).clamp(0, 255)).expect("colour channel in range")
    })
}

fn tile_noise(layer: u32, x: u32, y: u32) -> u32 {
    let mut value = layer
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(x.wrapping_mul(0x85EB_CA6B))
        .wrapping_add(y.wrapping_mul(0xC2B2_AE35));
    value ^= value >> 16;
    value = value.wrapping_mul(0x7FEB_352D);
    value ^= value >> 15;
    value
}

#[allow(clippy::too_many_arguments)]
fn create_pipeline(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
    camera_bind_group_layout: &wgpu::BindGroupLayout,
    chunk_bind_group_layout: &wgpu::BindGroupLayout,
    texture_bind_group_layout: &wgpu::BindGroupLayout,
    label: &str,
    fragment_entry: &str,
    blend: Option<wgpu::BlendState>,
    depth_write: bool,
    depth_compare: wgpu::CompareFunction,
) -> Result<wgpu::RenderPipeline, RendererError> {
    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let shader = device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("voxel pipeline layout"),
        bind_group_layouts: &[
            Some(camera_bind_group_layout),
            Some(chunk_bind_group_layout),
            Some(texture_bind_group_layout),
        ],
        immediate_size: 0,
    });
    let vertex_layout = wgpu::VertexBufferLayout {
        array_stride: u64::try_from(size_of::<Vertex>()).expect("vertex size fits in u64"),
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Uint8x4,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Uint8x4,
                offset: 4,
                shader_location: 1,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Uint32,
                offset: 8,
                shader_location: 2,
            },
        ],
    };
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(vertex_layout)],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some(fragment_entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: config.format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(depth_write),
            depth_compare: Some(depth_compare),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    if let Some(error) = pollster::block_on(error_scope.pop()) {
        return Err(RendererError::Validation(error.to_string()));
    }
    Ok(pipeline)
}

fn create_highlight_pipeline(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
    camera_bind_group_layout: &wgpu::BindGroupLayout,
) -> Result<wgpu::RenderPipeline, RendererError> {
    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let shader = device.create_shader_module(wgpu::include_wgsl!("highlight.wgsl"));
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("highlight pipeline layout"),
        bind_group_layouts: &[Some(camera_bind_group_layout)],
        immediate_size: 0,
    });
    let vertex_layout = wgpu::VertexBufferLayout {
        array_stride: u64::try_from(size_of::<HighlightVertex>())
            .expect("highlight vertex size fits in u64"),
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        }],
    };
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("highlight pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(vertex_layout)],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(config.format.into())],
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::LineList,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    if let Some(error) = pollster::block_on(error_scope.pop()) {
        return Err(RendererError::Validation(error.to_string()));
    }
    Ok(pipeline)
}

fn create_depth_texture(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth texture"),
        size: wgpu::Extent3d {
            width: config.width,
            height: config.height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// Quads addressable with a `u16` index buffer (four vertices per quad).
const SHARED_QUADS: u32 = 65_536 / 4;

/// Shared index pattern: six `u16` indices per quad, matching [`MeshData`].
fn shared_index_data(quads: u32) -> Vec<u16> {
    let mut data = Vec::with_capacity(quads as usize * 6);
    for quad in 0..quads {
        let base = u16::try_from(quad * 4).expect("shared index fits in u16");
        data.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    data
}

fn create_shared_index_buffer(device: &wgpu::Device) -> wgpu::Buffer {
    let indices = shared_index_data(SHARED_QUADS);
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("shared quad indices"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    })
}

fn create_index_buffer(
    device: &wgpu::Device,
    mesh: &MeshData,
) -> (Option<wgpu::Buffer>, wgpu::IndexFormat, u32) {
    let count = u32::try_from(mesh.indices.len()).expect("index count fits in u32");
    if u16::try_from(mesh.vertices.len()).is_ok() {
        return (None, wgpu::IndexFormat::Uint16, count);
    }
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("chunk indices"),
        contents: bytemuck::cast_slice(&mesh.indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    (Some(buffer), wgpu::IndexFormat::Uint32, count)
}

fn create_vertex_buffer(device: &wgpu::Device, mesh: &[Vertex]) -> (wgpu::Buffer, u32) {
    let vertex_count = u32::try_from(mesh.len()).expect("mesh vertex count fits in u32");
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("chunk vertices"),
        contents: bytemuck::cast_slice(mesh),
        usage: wgpu::BufferUsages::VERTEX,
    });
    (buffer, vertex_count)
}

fn create_highlight_buffer(device: &wgpu::Device, mesh: &[HighlightVertex]) -> (wgpu::Buffer, u32) {
    let vertex_count = u32::try_from(mesh.len()).expect("highlight vertex count fits in u32");
    let empty = [HighlightVertex { position: [0.0; 3] }];
    let contents = if mesh.is_empty() {
        bytemuck::cast_slice(&empty)
    } else {
        bytemuck::cast_slice(mesh)
    };
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("highlight vertices"),
        contents,
        usage: wgpu::BufferUsages::VERTEX,
    });
    (buffer, vertex_count)
}

fn create_overlay_pipeline(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
) -> Result<wgpu::RenderPipeline, RendererError> {
    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let shader = device.create_shader_module(wgpu::include_wgsl!("overlay.wgsl"));
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("overlay pipeline layout"),
        bind_group_layouts: &[],
        immediate_size: 0,
    });
    let vertex_layout = wgpu::VertexBufferLayout {
        array_stride: u64::try_from(size_of::<OverlayVertex>())
            .expect("overlay vertex size fits in u64"),
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 8,
                shader_location: 1,
            },
        ],
    };
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("overlay pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[Some(vertex_layout)],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: config.format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });

    if let Some(error) = pollster::block_on(error_scope.pop()) {
        return Err(RendererError::Validation(error.to_string()));
    }
    Ok(pipeline)
}

fn create_overlay_buffer(device: &wgpu::Device) -> (wgpu::Buffer, u64) {
    const OVERLAY_BUFFER_SIZE: u64 = 128 * 1024;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("overlay vertices"),
        size: OVERLAY_BUFFER_SIZE,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    (buffer, OVERLAY_BUFFER_SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bmp_rows_are_bottom_up_bgr_with_padding() {
        let directory = std::env::temp_dir().join(format!("engine-bmp-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("capture.bmp");
        let data = [10, 20, 30, 255, 40, 50, 60, 255];
        write_bmp(&path, 2, 1, 8, wgpu::TextureFormat::Bgra8UnormSrgb, &data).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..2], b"BM");
        assert_eq!(bytes.len(), 54 + 6 + 2);
        assert_eq!(&bytes[18..22], &2_i32.to_le_bytes());
        assert_eq!(&bytes[22..26], &1_i32.to_le_bytes());
        assert_eq!(&bytes[54..60], &[10, 20, 30, 40, 50, 60]);
        assert_eq!(&bytes[60..62], &[0, 0]);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn shared_index_pattern_matches_the_mesher() {
        let data = shared_index_data(3);
        assert_eq!(data.len(), 18);
        assert_eq!(&data[..6], &[0, 1, 2, 0, 2, 3]);
        assert_eq!(&data[6..12], &[4, 5, 6, 4, 6, 7]);
        assert_eq!(data.iter().max(), Some(&11));
        assert_eq!(
            shared_index_data(SHARED_QUADS).len(),
            SHARED_QUADS as usize * 6
        );
    }

    #[test]
    fn vsync_prefers_fifo_and_tearing_falls_back() {
        assert_eq!(
            preferred_present_mode(true, &[wgpu::PresentMode::Fifo]),
            wgpu::PresentMode::Fifo
        );
        assert_eq!(
            preferred_present_mode(
                false,
                &[wgpu::PresentMode::Fifo, wgpu::PresentMode::Immediate]
            ),
            wgpu::PresentMode::Immediate
        );
        assert_eq!(
            preferred_present_mode(
                false,
                &[wgpu::PresentMode::Fifo, wgpu::PresentMode::Mailbox]
            ),
            wgpu::PresentMode::Mailbox
        );
        assert_eq!(
            preferred_present_mode(false, &[wgpu::PresentMode::Fifo]),
            wgpu::PresentMode::Fifo
        );
    }

    #[test]
    fn texture_data_has_one_rgba_tile_per_layer() {
        let data = block_texture_data();
        let tile_bytes = usize::try_from(TEXTURE_SIZE * TEXTURE_SIZE * 4).unwrap();
        assert_eq!(
            data.len(),
            tile_bytes * usize::try_from(TEXTURE_LAYERS).unwrap()
        );
    }

    #[test]
    fn texture_data_is_deterministic_and_layers_differ() {
        let first = block_texture_data();
        let second = block_texture_data();
        let tile_bytes = usize::try_from(TEXTURE_SIZE * TEXTURE_SIZE * 4).unwrap();

        assert_eq!(first, second);
        assert_ne!(first[..tile_bytes], second[tile_bytes..tile_bytes * 2]);
    }
}

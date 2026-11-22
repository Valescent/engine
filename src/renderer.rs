//! Minimal `wgpu` renderer for chunk meshes, textures, and the block outline.
//!
//! [`TerrainDraw`] is a second, opt-in path for `mesher2`'s packed `u64` quad
//! records. It vertex-pulls from a read-only storage buffer, so it needs no
//! index buffer and no per-vertex data, and it leaves the legacy [`MeshData`]
//! path untouched.
//!
//! The implementation is split into [`chunk`] (the legacy per-chunk meshes and
//! their origin uniforms), [`frame`] (the frame uniform, camera bind group,
//! depth attachment, present mode, and stats), [`origin`] (absolute origin
//! uniforms and the camera-relative bookkeeping), [`pipeline`] (shader modules,
//! bind group layouts, and pipelines), [`terrain`] (the packed-quad section
//! path), and [`texture`] (the procedural block texture array). Every item is
//! re-exported here, so `renderer::<item>` paths are unchanged.

mod capture;
mod chunk;
mod frame;
mod items;
mod origin;
mod pipeline;
mod pool;
mod terrain;
mod texture;

use std::collections::BTreeMap;
use std::mem::size_of;
use std::sync::Arc;
use std::time::Instant;

use glam::{IVec3, Vec3};
use thiserror::Error;
use tracing::{debug, error, info, warn};
use winit::dpi::PhysicalSize;
use winit::window::Window;

use crate::camera::{Camera, RenderView, relative_origin};
use crate::frustum::Frustum;
use crate::horizon::Horizon;
use crate::overlay::build_overlay_mesh;
use crate::quad_renderer::{CAPACITY_QUADS, HIERARCHY_CAPACITY_QUADS, QuadRenderer, pool_bytes};
use crate::sky::SkySample;
use crate::voxel::HighlightVertex;
use crate::world::{CHUNK_SIZE, ChunkCoord};

pub(crate) use frame::FrameStats;
#[allow(
    unused_imports,
    reason = "kept as the renderer::OriginChange path for callers and tests"
)]
pub(crate) use origin::OriginChange;
#[allow(
    unused_imports,
    reason = "kept as the renderer::<item> paths for callers and tests"
)]
pub(crate) use terrain::{
    DecodedQuad, DuplicateOrigin, TerrainDraw, TerrainSection, UploadPlan, decode_quad, plan_upload,
};

use self::capture::{Capture, write_bmp};
use self::chunk::{GpuChunk, create_shared_index_buffer};
use self::frame::{
    CLEAR_COLOR, FrameUniform, create_camera_bind_group, create_depth_texture,
    create_highlight_buffer, create_overlay_buffer, device_limits, preferred_present_mode,
    timer_features,
};
use self::items::ItemPass;
use self::origin::{origin_change, wave_anchor};
use self::pipeline::{
    create_camera_bind_group_layout, create_chunk_bind_group_layout, create_highlight_pipeline,
    create_overlay_pipeline, create_pipeline, create_texture_bind_group_layout,
};
use self::pool::{PoolCapabilities, pool_supported};
use self::terrain::section_emits;
use self::texture::create_block_texture;

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
    #[error("terrain section origin {0:?} appears more than once")]
    DuplicateTerrainSection([i32; 3]),
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
    camera_bind_group_layout: wgpu::BindGroupLayout,
    chunk_bind_group_layout: wgpu::BindGroupLayout,
    textures_layout: wgpu::BindGroupLayout,
    chunks: BTreeMap<ChunkCoord, GpuChunk>,
    /// Six `u16` indices per quad, shared by every chunk that fits a `u16` index.
    shared_index_buffer: wgpu::Buffer,
    /// Sum of `GpuMesh::bytes` over every resident slot plus the shared index
    /// buffer, maintained on mutation.
    geometry_bytes: u64,
    /// The ADR-0017 quad-pool path, when the adapter supports it.
    quad: Option<QuadRenderer>,
    frame_index: u64,
    /// Last time a pool-full warning was emitted.
    last_pool_warning: Option<Instant>,
    /// Pool-full messages folded into the next warning.
    suppressed_pool_warnings: u32,
    terrain: Option<TerrainDraw>,
    /// Sections created or rewritten since the last [`Renderer::render`].
    pending_terrain_uploads: u32,
    present_modes: Vec<wgpu::PresentMode>,
    vsync: bool,
    _block_texture: wgpu::Texture,
    texture_bind_group: wgpu::BindGroup,
    highlight_pipeline: wgpu::RenderPipeline,
    highlight_buffer: wgpu::Buffer,
    highlight_count: u32,
    /// Break progress in `0..=1`, passed to the highlight shader through the
    /// spare `camera_position.w` lane.
    highlight_progress: f32,
    /// World-space highlight vertices, rebased whenever the render origin moves.
    highlight_world: Vec<HighlightVertex>,
    /// Dropped-item placeholder cubes.
    items: ItemPass,
    /// World block subtracted from every submitted origin this frame.
    render_origin: IVec3,
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
        let capacity_quads = if std::env::var("ENGINE_HIERARCHY").is_ok_and(|value| value == "1") {
            HIERARCHY_CAPACITY_QUADS
        } else {
            CAPACITY_QUADS
        };
        let quad_pool_bytes = pool_bytes(capacity_quads);
        let quad_supported = pool_supported(PoolCapabilities {
            legacy_forced,
            indirect_first_instance: adapter
                .features()
                .contains(wgpu::Features::INDIRECT_FIRST_INSTANCE),
            indirect_execution: downlevel.contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION),
            vertex_storage: downlevel.contains(wgpu::DownlevelFlags::VERTEX_STORAGE),
            storage_buffers_per_stage: adapter_limits.max_storage_buffers_per_shader_stage,
            storage_buffer_binding_size: adapter_limits.max_storage_buffer_binding_size,
            max_buffer_size: adapter_limits.max_buffer_size,
            pool_bytes: quad_pool_bytes,
        });
        if quad_supported {
            info!("quad pool render path enabled");
        } else {
            warn!("quad pool render path unavailable; using per-chunk meshes");
        }

        let mut required_features = timer_features(&adapter);
        if quad_supported {
            required_features |= wgpu::Features::INDIRECT_FIRST_INSTANCE;
        }
        let mut required_limits = device_limits(&adapter);
        if quad_supported {
            required_limits.max_storage_buffers_per_shader_stage =
                required_limits.max_storage_buffers_per_shader_stage.max(2);
            required_limits.max_storage_buffer_binding_size = required_limits
                .max_storage_buffer_binding_size
                .max(quad_pool_bytes);
            required_limits.max_buffer_size = required_limits.max_buffer_size.max(quad_pool_bytes);
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

        let view = camera.render_view();
        let frame = FrameUniform {
            view_projection: view.view_projection.to_cols_array(),
            settings: [0.0, camera.far_plane(), 0.25, camera.fog_end()],
            camera_position: [
                view.camera_position.x,
                view.camera_position.y,
                view.camera_position.z,
                0.0,
            ],
            render_origin: [view.origin.x, view.origin.y, view.origin.z, 0],
            wave_anchor: wave_anchor(view.origin),
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
                capacity_quads,
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
        let items = ItemPass::new(&device, &config, &camera_bind_group_layout)?;
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
            camera_bind_group_layout,
            chunk_bind_group_layout,
            textures_layout: texture_bind_group_layout,
            chunks: BTreeMap::new(),
            shared_index_buffer,
            geometry_bytes: shared_index_bytes,
            quad,
            frame_index: 0,
            last_pool_warning: None,
            suppressed_pool_warnings: 0,
            terrain: None,
            pending_terrain_uploads: 0,
            present_modes,
            vsync: true,
            _block_texture: block_texture,
            texture_bind_group,
            highlight_pipeline,
            highlight_buffer,
            highlight_count,
            highlight_progress: 0.0,
            highlight_world: Vec::new(),
            items,
            render_origin: view.origin,
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
        if let Some(capture) = &mut self.capture {
            capture.resize(&self.device, &self.config);
        }
        let (depth_texture, depth_view) = create_depth_texture(&self.device, &self.config);
        self.depth_texture = depth_texture;
        self.depth_view = depth_view;
        self.set_camera(&camera.render_view());
    }

    /// Applies one frame's render origin and camera-relative transform.
    ///
    /// Both draw paths and the highlight use the same origin. Origin uniforms
    /// store absolute world-block integers, so moving the origin rewrites no
    /// chunk or section buffer at all; only the small relative highlight buffer
    /// is re-uploaded.
    pub(crate) fn set_camera(&mut self, view: &RenderView) {
        self.frame.view_projection = view.view_projection.to_cols_array();
        self.frame.settings[1] = view.far_plane;
        self.frame.settings[3] = view.fog_end;
        let camera_position = view.camera_position;
        self.frame.camera_position = [
            camera_position.x,
            camera_position.y,
            camera_position.z,
            self.highlight_progress,
        ];
        self.frame.render_origin = [view.origin.x, view.origin.y, view.origin.z, 0];
        self.frame.wave_anchor = wave_anchor(view.origin);
        if view.origin != self.render_origin {
            let sections = self.terrain.as_ref().map_or(0, TerrainDraw::resident_count);
            let change =
                origin_change(self.render_origin, view.origin, self.chunks.len(), sections);
            self.render_origin = view.origin;
            self.upload_highlight();
            debug!(
                from = ?change.from,
                to = ?change.to,
                origin_buffer_writes = change.origin_buffer_writes,
                avoided_origin_writes = change.avoided_origin_writes,
                resident_chunks = self.chunks.len(),
                resident_sections = sections,
                highlight_vertices = self.highlight_world.len(),
                "render origin moved"
            );
        }
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
        self.highlight_world.clear();
        self.highlight_world.extend_from_slice(mesh);
        self.upload_highlight();
    }

    /// Sets break progress in `0..=1` for the highlight colour.
    pub(crate) fn set_highlight_progress(&mut self, progress: f32) {
        self.highlight_progress = progress.clamp(0.0, 1.0);
        self.frame.camera_position[3] = self.highlight_progress;
        self.write_frame();
    }

    /// Replaces the dropped-item cubes for this frame.
    pub(crate) fn set_items(&mut self, items: &[(Vec3, &str)]) {
        self.items
            .set_items(&self.device, &self.queue, self.render_origin, items);
    }

    /// Uploads the world-space highlight mesh in the frame's relative space.
    fn upload_highlight(&mut self) {
        let origin = self.render_origin.as_vec3();
        let relative: Vec<HighlightVertex> = self
            .highlight_world
            .iter()
            .map(|vertex| HighlightVertex {
                position: [
                    vertex.position[0] - origin.x,
                    vertex.position[1] - origin.y,
                    vertex.position[2] - origin.z,
                ],
            })
            .collect();
        let (highlight_buffer, highlight_count) = create_highlight_buffer(&self.device, &relative);
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

    /// Enables the packed-quad terrain path, creating its pipelines on first
    /// use. The legacy chunk-mesh path keeps drawing alongside it.
    #[allow(
        dead_code,
        reason = "opt-in API: the lead wires it in; the demo must not change"
    )]
    pub(crate) fn enable_terrain(&mut self) -> Result<&mut TerrainDraw, RendererError> {
        if self.terrain.is_none() {
            let terrain = TerrainDraw::new(
                &self.device,
                &self.queue,
                self.config.format,
                &self.camera_bind_group_layout,
                &self.textures_layout,
            )?;
            self.terrain = Some(terrain);
        }
        Ok(self
            .terrain
            .as_mut()
            .expect("terrain draw was created above"))
    }

    /// Replaces the complete list of packed-quad sections to draw.
    ///
    /// The call is a diff against the resident sections: only origins that are
    /// new or whose content changed allocate and upload. Build one
    /// [`TerrainSection`] per section and submit them all at once; unchanged
    /// sections cost no GPU work. Returns the number of sections uploaded in
    /// the next [`FrameStats::terrain_sections_uploaded`] count.
    #[allow(
        dead_code,
        reason = "opt-in API: the lead wires it in; the demo must not change"
    )]
    pub(crate) fn submit_terrain(
        &mut self,
        sections: &[TerrainSection<'_>],
    ) -> Result<(), RendererError> {
        let uploaded = self.enable_terrain()?.submit(sections)?;
        self.pending_terrain_uploads = self.pending_terrain_uploads.saturating_add(uploaded);
        Ok(())
    }

    /// Creates or rewrites exactly one section, keyed by its world origin.
    ///
    /// Every other resident section keeps its buffers untouched. This is the
    /// one-block-edit path: one call, one section.
    #[allow(
        dead_code,
        reason = "opt-in API: the lead wires it in; the demo must not change"
    )]
    pub(crate) fn update_terrain_section(
        &mut self,
        section: TerrainSection<'_>,
    ) -> Result<(), RendererError> {
        let uploaded = u32::from(section_emits(&section));
        self.enable_terrain()?.update(&section)?;
        self.pending_terrain_uploads = self.pending_terrain_uploads.saturating_add(uploaded);
        Ok(())
    }

    /// Removes every packed-quad section without tearing down the pipelines.
    #[allow(
        dead_code,
        reason = "opt-in API: the lead wires it in; the demo must not change"
    )]
    pub(crate) fn clear_terrain(&mut self) {
        if let Some(terrain) = &mut self.terrain
            && let Err(error) = terrain.submit(&[])
        {
            error!(%error, "failed to clear terrain sections");
        }
    }

    #[allow(clippy::cast_precision_loss, clippy::too_many_lines)]
    pub(crate) fn render(&mut self, frustum: &Frustum, horizon: &Horizon) -> FrameStats {
        self.frame_index += 1;
        if let Some(quad) = self.quad.as_mut() {
            quad.begin_frame(self.frame_index);
        }
        let mut stats = FrameStats {
            gpu_ms: self.gpu_ms,
            terrain_sections_uploaded: std::mem::take(&mut self.pending_terrain_uploads),
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

    #[allow(clippy::cast_precision_loss, clippy::too_many_lines)]
    fn draw_scene(
        &self,
        quad: Option<&mut QuadRenderer>,
        pass: &mut wgpu::RenderPass<'_>,
        frustum: &Frustum,
        horizon: &Horizon,
        stats: &mut FrameStats,
    ) {
        let render_origin = self.render_origin;
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
        } else {
            stats.geometry_bytes = self.geometry_bytes;

            // One traversal: count every resident chunk and test each chunk's AABB
            // once, recording the opaque and water meshes that survive culling.
            let mut opaque = Vec::with_capacity(self.chunks.len());
            let mut water = Vec::with_capacity(self.chunks.len());
            for (chunk, gpu_chunk) in &self.chunks {
                stats.total_chunks += 1;
                let min = relative_origin(chunk.world_origin().to_array(), render_origin);
                if !frustum.intersects_aabb(min, min + Vec3::splat(CHUNK_SIZE as f32)) {
                    continue;
                }
                if horizon.occludes(*chunk) {
                    stats.horizon_culled += 1;
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

            // Packed-quad opaque and cutout terrain, still before any water.
            if let Some(terrain) = &self.terrain {
                terrain.draw_opaque(
                    pass,
                    frustum,
                    render_origin,
                    &self.camera_bind_group,
                    &self.texture_bind_group,
                    stats,
                );
            }

            // Transparent water after all opaque geometry; depth writes stay off.
            if !water.is_empty() {
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
            }

            // Packed-quad water, last of the terrain passes: blended, no depth
            // writes, and no culling.
            if let Some(terrain) = &self.terrain {
                terrain.draw_water(
                    pass,
                    frustum,
                    render_origin,
                    &self.camera_bind_group,
                    &self.texture_bind_group,
                    stats,
                );
            }
        }

        self.items.draw(pass, &self.camera_bind_group);

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
                if let Some(capture) = &mut self.capture {
                    capture.resize(&self.device, &self.config);
                }
            }
            Err(error) => error!(%error, "failed to recreate the window surface"),
        }
    }
}

#[cfg(test)]
mod tests;

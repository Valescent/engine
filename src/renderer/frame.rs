//! The per-frame uniform, camera bind group, depth attachment, present-mode
//! selection, and frame statistics.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::voxel::HighlightVertex;

use super::pipeline::DEPTH_FORMAT;
use super::terrain::TERRAIN_STORAGE_BYTES;

pub(super) const CLEAR_COLOR: wgpu::Color = wgpu::Color {
    r: 0.45,
    g: 0.65,
    b: 0.90,
    a: 1.0,
};

/// Per-frame uniform: camera, sky colours, sun, and fog settings.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(super) struct FrameUniform {
    pub(super) view_projection: [f32; 16],
    pub(super) sky_colour: [f32; 4],
    pub(super) ambient_colour: [f32; 4],
    pub(super) sun_colour: [f32; 4],
    pub(super) sun_direction: [f32; 4],
    /// Unlit flag, camera far plane, time of day, fog end distance.
    pub(super) settings: [f32; 4],
    /// Camera position relative to the frame's render origin, for fog distance.
    pub(super) camera_position: [f32; 4],
    /// The frame's absolute world-block render origin.
    ///
    /// Section and chunk origin uniforms carry absolute integer origins too;
    /// every vertex shader subtracts this field in integer space before the
    /// small difference is converted to `f32`.
    pub(super) render_origin: [i32; 4],
    /// The legacy water wave phase of `render_origin`, wrapped to `[0, TAU)`.
    ///
    /// `x` anchors `sin(phase + 0.7 x + 0.5 z)` and `y` anchors
    /// `cos(phase + 0.6 x - 0.4 z)`. Wrapping the phase in `f64` keeps the wave
    /// pattern world-anchored without casting an absolute world coordinate to
    /// `f32`.
    pub(super) wave_anchor: [f32; 4],
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
            render_origin: [0; 4],
            wave_anchor: [0.0; 4],
        }
    }
}

/// Counts for one drawn frame.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FrameStats {
    pub(crate) total_chunks: u32,
    pub(crate) visible_chunks: u32,
    /// Slots skipped by the horizon pass this frame.
    pub(crate) horizon_culled: u32,
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
    /// Packed-quad sections created or rewritten since the previous frame.
    ///
    /// A full `submit_terrain` diffs against the resident set, so a one-block
    /// edit reports one here rather than one per section in the world.
    #[allow(dead_code, reason = "read by the frame capture")]
    pub(crate) terrain_sections_uploaded: u32,
}

/// Device limits: the WebGL2-parity floor, plus one storage buffer for the
/// packed-quad terrain path.
///
/// `downlevel_webgl2_defaults` requests no storage buffers at all, which would
/// reject `terrain_draw.wgsl`'s record buffer. One buffer of at most
/// [`TERRAIN_STORAGE_BYTES`] is enough for the spec's per-section budget, and
/// asking for the adapter's own maximum would be wasteful.
pub(super) fn device_limits(adapter: &wgpu::Adapter) -> wgpu::Limits {
    let adapter_limits = adapter.limits();
    let storage_buffers = adapter_limits.max_storage_buffers_per_shader_stage;
    let storage_bytes = adapter_limits.max_storage_buffer_binding_size;
    let mut limits = wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter_limits);
    limits.max_storage_buffers_per_shader_stage = limits
        .max_storage_buffers_per_shader_stage
        .max(storage_buffers.min(1));
    limits.max_storage_buffer_binding_size = limits
        .max_storage_buffer_binding_size
        .max(storage_bytes.min(TERRAIN_STORAGE_BYTES));
    limits
}

/// Features the GPU profiler needs, intersected with what the adapter supports.
#[cfg(feature = "tracy")]
pub(super) fn timer_features(adapter: &wgpu::Adapter) -> wgpu::Features {
    let requested = wgpu_profiler::GpuProfiler::ALL_WGPU_TIMER_FEATURES & adapter.features();
    if requested.contains(wgpu::Features::TIMESTAMP_QUERY) {
        requested
    } else {
        wgpu::Features::empty()
    }
}

#[cfg(not(feature = "tracy"))]
pub(super) fn timer_features(_adapter: &wgpu::Adapter) -> wgpu::Features {
    wgpu::Features::empty()
}

pub(super) fn preferred_present_mode(
    vsync: bool,
    supported: &[wgpu::PresentMode],
) -> wgpu::PresentMode {
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

pub(super) fn create_camera_bind_group(
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

pub(super) fn create_depth_texture(
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

pub(super) fn create_highlight_buffer(
    device: &wgpu::Device,
    mesh: &[HighlightVertex],
) -> (wgpu::Buffer, u32) {
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

pub(super) fn create_overlay_buffer(device: &wgpu::Device) -> (wgpu::Buffer, u64) {
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
    use std::mem::size_of;

    use super::*;

    #[test]
    fn frame_uniform_matches_the_shader_layout() {
        // 64-byte matrix, six vec4s, the integer render origin, and the wave
        // anchor: the camera bind group layout advertises 192 bytes.
        assert_eq!(size_of::<FrameUniform>(), 192);
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
}

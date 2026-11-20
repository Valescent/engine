//! Minimal `wgpu` renderer for chunk meshes, textures, and the block outline.
//!
//! [`TerrainDraw`] is a second, opt-in path for `mesher2`'s packed `u64` quad
//! records. It vertex-pulls from a read-only storage buffer, so it needs no
//! index buffer and no per-vertex data, and it leaves the legacy [`MeshData`]
//! path untouched.

use std::collections::BTreeMap;
use std::mem::size_of;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use thiserror::Error;
#[cfg(feature = "tracy")]
use tracing::warn;
use tracing::{error, info};
use wgpu::util::DeviceExt;
use winit::dpi::PhysicalSize;
use winit::window::Window;

use crate::block::{TEXTURE_LAYERS, TEXTURE_SIZE};
use crate::camera::Camera;
use crate::frustum::Frustum;
use crate::mesh::{Lod, MeshData};
use crate::mesher2::Quad;
use crate::overlay::{OverlayVertex, build_overlay_mesh};
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

/// Section-local extent of one terrain quad, in blocks.
const SECTION_EXTENT: f32 = 32.0;

/// Bytes requested for one section's storage-buffer binding. The spec budget
/// is 25,000 quads per section, which is 200 KB of records.
const TERRAIN_STORAGE_BYTES: u64 = 1 << 20;

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
    #[error("terrain section origin {0:?} appears more than once")]
    DuplicateTerrainSection([i32; 3]),
}

struct GpuMesh {
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_format: wgpu::IndexFormat,
    index_count: u32,
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

    fn active_mesh(&self) -> Option<&GpuMesh> {
        self.active.and_then(|lod| self.mesh(lod))
    }

    fn is_empty(&self) -> bool {
        self.lod0.is_none() && self.lod1.is_none() && self.lod2.is_none() && self.water.is_none()
    }
}

/// Counts for one drawn frame.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FrameStats {
    pub(crate) total_chunks: u32,
    pub(crate) visible_chunks: u32,
    pub(crate) visible_vertices: u64,
    pub(crate) draw_calls: u32,
    /// Packed-quad sections created or rewritten since the previous frame.
    ///
    /// A full `submit_terrain` diffs against the resident set, so a one-block
    /// edit reports one here rather than one per section in the world.
    #[allow(dead_code, reason = "read by the frame capture")]
    pub(crate) terrain_sections_uploaded: u32,
}

/// One section of packed `mesher2` records for [`TerrainDraw`].
///
/// `origin` is the section's world-space block origin (its section coordinate
/// times 32). All slices are raw [`Quad`] records and may be passed straight
/// from a `mesher2::SectionMesh`.
///
/// The origin is the identity: [`Renderer::submit_terrain`] diffs by origin and
/// fingerprints the three ranges, so rebuilding an unchanged section does not
/// re-upload it. A caller that already tracks a revision can use
/// [`Renderer::update_terrain_section`] for a single origin instead.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TerrainSection<'a> {
    /// World block origin of the section's local `(0, 0, 0)` cell.
    pub(crate) origin: [i32; 3],
    /// World blocks per cell: `1` fine, `2` LOD1, `4` LOD2.
    pub(crate) scale: u32,
    /// Opaque full-cube records, from `mesher2::SectionMesh::opaque`.
    pub(crate) opaque: &'a [Quad],
    /// Cutout records, from `mesher2::SectionMesh::cutout`. Drawn in the opaque
    /// pass by the same alpha-tested pipeline, including cross quads.
    pub(crate) cutout: &'a [Quad],
    /// Water records, from `mesher2::SectionMesh::water`.
    pub(crate) water: &'a [Quad],
}

/// The decoded fields of one packed quad record (spec 12.3).
///
/// Field widths and bit offsets match [`crate::mesher2::Quad`] exactly; the AO byte is
/// unpacked into corner order so tests can read it without shifting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DecodedQuad {
    /// Section-local quad origin along `x`, `0..=31`.
    pub(crate) x: u8,
    /// Section-local quad origin along `y`, `0..=31`.
    pub(crate) y: u8,
    /// Section-local quad origin along `z`, `0..=31`.
    pub(crate) z: u8,
    /// Width minus one along the face tangent, `0..=31`.
    pub(crate) w1: u8,
    /// Height minus one along the face tangent, `0..=31`.
    pub(crate) h1: u8,
    /// Face direction `0..=5`, or a cross-quad diagonal `6..=7`.
    pub(crate) normal: u8,
    /// Four 2-bit AO corners, low corner first.
    pub(crate) ao: [u8; 4],
    /// Sky-light nibble of the air-side cell.
    pub(crate) sky: u8,
    /// Block-light nibble of the air-side cell.
    pub(crate) blk: u8,
    /// Texture-array layer.
    pub(crate) tile: u16,
    /// Biome tint index.
    pub(crate) tint: u8,
}

/// Decodes one packed `u64` quad record without a GPU.
///
/// This is the CPU side of the layout the vertex shader reads in
/// `terrain_draw.wgsl`; the tests use it to pin the bit offsets.
pub(crate) fn decode_quad(raw: u64) -> DecodedQuad {
    let five =
        |shift: u32| u8::try_from((raw >> shift) & 0x1F).expect("a 5-bit quad field fits in u8");
    let four =
        |shift: u32| u8::try_from((raw >> shift) & 0x0F).expect("a 4-bit quad field fits in u8");
    let packed_ao = u8::try_from((raw >> 28) & 0xFF).expect("the AO byte fits in u8");
    DecodedQuad {
        x: five(0),
        y: five(5),
        z: five(10),
        w1: five(15),
        h1: five(20),
        normal: u8::try_from((raw >> 25) & 0x07).expect("a 3-bit quad field fits in u8"),
        ao: [
            packed_ao & 0x3,
            (packed_ao >> 2) & 0x3,
            (packed_ao >> 4) & 0x3,
            (packed_ao >> 6) & 0x3,
        ],
        sky: four(36),
        blk: four(40),
        tile: u16::try_from((raw >> 44) & 0xFFFF).expect("a 16-bit quad field fits in u16"),
        tint: four(60),
    }
}

/// The packed-quad terrain path: vertex pulling over `mesher2` records.
///
/// Each section keeps its own origin uniform and one storage buffer per range,
/// so a draw needs no vertex buffer, no index buffer, and no per-quad data.
/// Sections are keyed by world origin, so a submit is a diff: only new or
/// changed origins allocate and upload, and unchanged origins keep their
/// buffers. The path is created only when [`Renderer::enable_terrain`] is
/// called, which keeps the legacy mesh demo untouched.
#[derive(Debug)]
pub(crate) struct TerrainDraw {
    device: wgpu::Device,
    queue: wgpu::Queue,
    section_layout: wgpu::BindGroupLayout,
    opaque_pipeline: wgpu::RenderPipeline,
    water_pipeline: wgpu::RenderPipeline,
    sections: BTreeMap<[i32; 3], GpuSection>,
    /// Draw order of the resident origins. Kept beside the map so a full
    /// submit draws in the caller's order, exactly like the old `Vec`.
    order: Vec<[i32; 3]>,
}

#[derive(Debug)]
struct GpuSection {
    /// The revision that produced the resident buffers.
    revision: u64,
    origin: Vec3,
    /// World blocks per cell, mirrored into the origin uniform.
    scale: u32,
    origin_buffer: wgpu::Buffer,
    opaque: Option<GpuQuadRange>,
    cutout: Option<GpuQuadRange>,
    water: Option<GpuQuadRange>,
}

#[derive(Debug)]
struct GpuQuadRange {
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    vertices: u32,
    /// Allocated bytes. A shrinking range keeps this buffer and rewrites only
    /// the live prefix, so an edit never grows a buffer unnecessarily.
    capacity: u64,
}

/// The change from the resident section set to an incoming one.
#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct UploadPlan {
    /// Origins to create or rewrite, sorted by origin.
    pub(crate) uploads: Vec<([i32; 3], u64)>,
    /// Resident origins with no incoming section, sorted by origin.
    pub(crate) removals: Vec<[i32; 3]>,
}

/// An incoming section list that names one origin more than once.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("terrain section origin {origin:?} appears more than once")]
pub(crate) struct DuplicateOrigin {
    pub(crate) origin: [i32; 3],
}

/// Smallest storage-buffer allocation for one quad range.
const MIN_QUAD_BUFFER_BYTES: u64 = 256;

/// Diffs the resident revisions against an incoming section list.
///
/// `revision` is a cheap identity for a section's contents; equal revisions
/// mean "leave those GPU resources alone". The function is pure, so the diff
/// rules are unit-tested without a `wgpu` adapter.
pub(crate) fn plan_upload(
    current: &BTreeMap<[i32; 3], u64>,
    incoming: &[([i32; 3], u64)],
) -> Result<UploadPlan, DuplicateOrigin> {
    let mut next = BTreeMap::new();
    for &(origin, revision) in incoming {
        if next.insert(origin, revision).is_some() {
            return Err(DuplicateOrigin { origin });
        }
    }
    let uploads = next
        .iter()
        .filter(|&(origin, revision)| current.get(origin) != Some(revision))
        .map(|(&origin, &revision)| (origin, revision))
        .collect();
    let removals = current
        .keys()
        .filter(|origin| !next.contains_key(*origin))
        .copied()
        .collect();
    Ok(UploadPlan { uploads, removals })
}

impl TerrainDraw {
    fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        camera_layout: &wgpu::BindGroupLayout,
        texture_layout: &wgpu::BindGroupLayout,
    ) -> Result<Self, RendererError> {
        let section_layout = create_terrain_section_layout(device);
        let opaque_pipeline = create_terrain_pipeline(
            device,
            format,
            camera_layout,
            &section_layout,
            texture_layout,
            "terrain opaque pipeline",
            "fs_main",
            None,
            true,
            wgpu::CompareFunction::Less,
        )?;
        let water_pipeline = create_terrain_pipeline(
            device,
            format,
            camera_layout,
            &section_layout,
            texture_layout,
            "terrain water pipeline",
            "fs_water",
            Some(wgpu::BlendState::ALPHA_BLENDING),
            false,
            wgpu::CompareFunction::LessEqual,
        )?;
        Ok(Self {
            device: device.clone(),
            queue: queue.clone(),
            section_layout,
            opaque_pipeline,
            water_pipeline,
            sections: BTreeMap::new(),
            order: Vec::new(),
        })
    }

    /// Replaces the complete section list, uploading only changed origins.
    ///
    /// Sections with no geometry are treated as absent. Returns how many
    /// sections were created or rewritten; unchanged origins cost no buffer
    /// work at all.
    pub(crate) fn submit(&mut self, sections: &[TerrainSection<'_>]) -> Result<u32, RendererError> {
        let mut by_origin: BTreeMap<[i32; 3], &TerrainSection<'_>> = BTreeMap::new();
        let mut incoming = Vec::with_capacity(sections.len());
        for section in sections {
            if by_origin.insert(section.origin, section).is_some() {
                return Err(RendererError::DuplicateTerrainSection(section.origin));
            }
            if section_emits(section) {
                incoming.push((section.origin, section_revision(section)));
            }
        }

        let current: BTreeMap<[i32; 3], u64> = self
            .sections
            .iter()
            .map(|(&origin, section)| (origin, section.revision))
            .collect();
        let plan = plan_upload(&current, &incoming)
            .map_err(|error| RendererError::DuplicateTerrainSection(error.origin))?;

        // Reject every oversized range before touching the resident set, so a
        // bad submit cannot leave a half-applied diff behind.
        for section in by_origin.values() {
            validate_section(section)?;
        }

        for origin in plan.removals {
            self.sections.remove(&origin);
        }
        let mut uploaded = 0_u32;
        for (origin, revision) in plan.uploads {
            let section = by_origin
                .get(&origin)
                .expect("every planned upload came from the incoming list");
            self.apply(section, revision)?;
            uploaded = uploaded.saturating_add(1);
        }

        // Rebuild the draw order from the caller's list, matching the old
        // `Vec` order exactly while skipping non-emitting sections.
        self.order = incoming
            .into_iter()
            .map(|(origin, _revision)| origin)
            .collect();
        Ok(uploaded)
    }

    /// Creates or rewrites exactly one origin, leaving every other section
    /// untouched. An empty section removes that origin's resources.
    pub(crate) fn update(&mut self, section: &TerrainSection<'_>) -> Result<(), RendererError> {
        let revision = section_revision(section);
        self.apply(section, revision)?;
        let origin = section.origin;
        if section_emits(section) {
            if !self.order.contains(&origin) {
                self.order.push(origin);
            }
        } else {
            self.order.retain(|resident| *resident != origin);
        }
        Ok(())
    }

    fn apply(&mut self, section: &TerrainSection<'_>, revision: u64) -> Result<(), RendererError> {
        if !section_emits(section) {
            self.sections.remove(&section.origin);
            return Ok(());
        }
        validate_section(section)?;
        debug_assert_normals(section);

        let device = &self.device;
        let queue = &self.queue;
        let layout = &self.section_layout;
        if let Some(resident) = self.sections.get_mut(&section.origin) {
            resident.revision = revision;
            if resident.scale != section.scale {
                resident.scale = section.scale;
                resident.origin_buffer = create_terrain_origin_buffer(
                    device,
                    resident.origin,
                    scale_factor(section.scale),
                );
            }
            resident.opaque = update_quad_range(
                device,
                queue,
                layout,
                &resident.origin_buffer,
                resident.opaque.take(),
                section.opaque,
            )?;
            resident.cutout = update_quad_range(
                device,
                queue,
                layout,
                &resident.origin_buffer,
                resident.cutout.take(),
                section.cutout,
            )?;
            resident.water = update_quad_range(
                device,
                queue,
                layout,
                &resident.origin_buffer,
                resident.water.take(),
                section.water,
            )?;
            return Ok(());
        }

        let origin = section_origin(section.origin);
        let origin_buffer =
            create_terrain_origin_buffer(device, origin, scale_factor(section.scale));
        let opaque = create_quad_range(device, queue, layout, &origin_buffer, section.opaque)?;
        let cutout = create_quad_range(device, queue, layout, &origin_buffer, section.cutout)?;
        let water = create_quad_range(device, queue, layout, &origin_buffer, section.water)?;
        self.sections.insert(
            section.origin,
            GpuSection {
                revision,
                origin,
                scale: section.scale,
                origin_buffer,
                opaque,
                cutout,
                water,
            },
        );
        Ok(())
    }

    fn draw_opaque(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        frustum: &Frustum,
        camera: &wgpu::BindGroup,
        texture: &wgpu::BindGroup,
        stats: &mut FrameStats,
    ) {
        pass.set_pipeline(&self.opaque_pipeline);
        pass.set_bind_group(0, camera, &[]);
        pass.set_bind_group(2, texture, &[]);
        for origin in &self.order {
            let Some(section) = self.sections.get(origin) else {
                continue;
            };
            if !section_visible(frustum, section.origin, scale_factor(section.scale)) {
                continue;
            }
            // Opaque and cutout share the pipeline; `fs_main` alpha tests.
            for range in [&section.opaque, &section.cutout].into_iter().flatten() {
                pass.set_bind_group(1, &range.bind_group, &[]);
                pass.draw(0..range.vertices, 0..1);
                stats.draw_calls += 1;
                stats.visible_vertices += u64::from(range.vertices);
            }
        }
    }

    fn draw_water(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        frustum: &Frustum,
        camera: &wgpu::BindGroup,
        texture: &wgpu::BindGroup,
        stats: &mut FrameStats,
    ) {
        pass.set_pipeline(&self.water_pipeline);
        pass.set_bind_group(0, camera, &[]);
        pass.set_bind_group(2, texture, &[]);
        for origin in &self.order {
            let Some(section) = self.sections.get(origin) else {
                continue;
            };
            let Some(range) = &section.water else {
                continue;
            };
            if !section_visible(frustum, section.origin, scale_factor(section.scale)) {
                continue;
            }
            pass.set_bind_group(1, &range.bind_group, &[]);
            pass.draw(0..range.vertices, 0..1);
            stats.draw_calls += 1;
            stats.visible_vertices += u64::from(range.vertices);
        }
    }
}

/// Whether a section has any range to draw.
fn section_emits(section: &TerrainSection<'_>) -> bool {
    !(section.opaque.is_empty() && section.cutout.is_empty() && section.water.is_empty())
}

/// Checks every range against the storage budget before a buffer is touched.
fn validate_section(section: &TerrainSection<'_>) -> Result<(), RendererError> {
    if !matches!(section.scale, 1 | 2 | 4) {
        return Err(RendererError::Validation(format!(
            "terrain section scale {} is not 1, 2, or 4",
            section.scale
        )));
    }
    for range in [section.opaque, section.cutout, section.water] {
        let _ = quad_bytes(range)?;
    }
    Ok(())
}

fn debug_assert_normals(section: &TerrainSection<'_>) {
    debug_assert!(
        section
            .opaque
            .iter()
            .chain(section.cutout)
            .chain(section.water)
            .all(|quad| decode_quad(quad.bits()).normal <= 7),
        "quad normal codes above 7 are reserved"
    );
}

/// A cheap content identity for one section.
///
/// `TerrainSection` borrows raw slices, so the renderer cannot ask the caller
/// for a revision without changing the literal at every call site. FNV-1a over
/// the packed records and the three range lengths is deterministic and costs a
/// fraction of the GPU work it avoids. It is an in-memory key only; it never
/// feeds terrain output (spec 2.2).
fn section_revision(section: &TerrainSection<'_>) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    hash ^= u64::from(section.scale);
    hash = hash.wrapping_mul(PRIME);
    for range in [section.opaque, section.cutout, section.water] {
        hash ^= u64::try_from(range.len()).expect("a slice length fits in u64");
        hash = hash.wrapping_mul(PRIME);
        for quad in range {
            hash ^= quad.bits();
            hash = hash.wrapping_mul(PRIME);
        }
    }
    hash
}

fn section_visible(frustum: &Frustum, origin: Vec3, scale: f32) -> bool {
    frustum.intersects_aabb(origin, origin + Vec3::splat(SECTION_EXTENT * scale))
}

#[allow(clippy::cast_precision_loss)]
fn section_origin(origin: [i32; 3]) -> Vec3 {
    // World space, exactly like the legacy chunk origin uniform: `Camera` has
    // no floating origin yet, so its view-projection is world-space and both
    // draw paths must use the same frame. Section origins are small integers,
    // so the `f32` conversion is exact.
    Vec3::new(origin[0] as f32, origin[1] as f32, origin[2] as f32)
}

#[allow(
    clippy::cast_precision_loss,
    reason = "scale is 1, 2, or 4 and exact in f32"
)]
fn scale_factor(scale: u32) -> f32 {
    scale as f32
}

fn create_terrain_origin_buffer(device: &wgpu::Device, origin: Vec3, scale: f32) -> wgpu::Buffer {
    let data = [origin.x, origin.y, origin.z, scale];
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("terrain section origin"),
        contents: bytemuck::cast_slice(&data),
        usage: wgpu::BufferUsages::UNIFORM,
    })
}

fn create_quad_range(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    origin: &wgpu::Buffer,
    quads: &[Quad],
) -> Result<Option<GpuQuadRange>, RendererError> {
    let Some(bytes) = quad_bytes(quads)? else {
        return Ok(None);
    };
    let capacity = allocation_bytes(bytes);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("terrain quads"),
        size: capacity,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    write_quads(queue, &buffer, quads);
    let bind_group = create_quad_bind_group(device, layout, origin, &buffer);
    Ok(Some(GpuQuadRange {
        buffer,
        bind_group,
        vertices: quad_vertices(quads),
        capacity,
    }))
}

/// Rewrites one range in place when it still fits, otherwise replaces its
/// buffer. Shrinking always reuses the existing capacity.
fn update_quad_range(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    origin: &wgpu::Buffer,
    existing: Option<GpuQuadRange>,
    quads: &[Quad],
) -> Result<Option<GpuQuadRange>, RendererError> {
    let Some(bytes) = quad_bytes(quads)? else {
        return Ok(None);
    };
    let Some(mut range) = existing else {
        return create_quad_range(device, queue, layout, origin, quads);
    };
    if bytes > range.capacity {
        return create_quad_range(device, queue, layout, origin, quads);
    }
    write_quads(queue, &range.buffer, quads);
    range.vertices = quad_vertices(quads);
    Ok(Some(range))
}

/// The byte size of a range, or `None` when it is empty.
///
/// The spec's section budget is 25,000 quads, well under the 1 MiB storage
/// binding this renderer requests; a larger range is rejected before it can
/// produce an invalid bind group.
fn quad_bytes(quads: &[Quad]) -> Result<Option<u64>, RendererError> {
    if quads.is_empty() {
        return Ok(None);
    }
    let len = u64::try_from(quads.len())
        .map_err(|_| RendererError::Validation("section quad count exceeds u64".into()))?;
    let stride = u64::try_from(size_of::<Quad>())
        .map_err(|_| RendererError::Validation("quad record size exceeds u64".into()))?;
    let bytes = len
        .checked_mul(stride)
        .ok_or_else(|| RendererError::Validation("section quad byte size overflows".into()))?;
    if bytes > TERRAIN_STORAGE_BYTES {
        return Err(RendererError::Validation(format!(
            "section range needs {bytes} bytes; the storage budget is {TERRAIN_STORAGE_BYTES}"
        )));
    }
    Ok(Some(bytes))
}

/// Rounds up to a power of two so growth doubles instead of reallocating on
/// every quad, and shrinking never touches the allocation.
fn allocation_bytes(needed: u64) -> u64 {
    needed
        .max(MIN_QUAD_BUFFER_BYTES)
        .next_power_of_two()
        .min(TERRAIN_STORAGE_BYTES)
}

fn write_quads(queue: &wgpu::Queue, buffer: &wgpu::Buffer, quads: &[Quad]) {
    // WGSL reads the records as `array<u32>`: the two little-endian halves of
    // every `u64`.
    let words: Vec<u64> = quads.iter().map(|quad| quad.bits()).collect();
    queue.write_buffer(buffer, 0, bytemuck::cast_slice(&words));
}

fn quad_vertices(quads: &[Quad]) -> u32 {
    u32::try_from(quads.len())
        .ok()
        .and_then(|count| count.checked_mul(6))
        .expect("a section's quad count fits in one draw")
}

fn create_quad_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    origin: &wgpu::Buffer,
    quads: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("terrain section bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: origin.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: quads.as_entire_binding(),
            },
        ],
    })
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
    overlay_pipeline: wgpu::RenderPipeline,
    overlay_buffer: wgpu::Buffer,
    overlay_buffer_size: u64,
    overlay_count: u32,
    depth_texture: wgpu::Texture,
    depth_view: wgpu::TextureView,
    #[cfg(feature = "tracy")]
    profiler: Option<wgpu_profiler::GpuProfiler>,
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

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("engine device"),
                required_features: timer_features(&adapter),
                required_limits: device_limits(&adapter),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await?;

        #[cfg(feature = "tracy")]
        let profiler = if timer_features(&adapter).contains(wgpu::Features::TIMESTAMP_QUERY) {
            match wgpu_profiler::GpuProfiler::new_with_tracy_client(
                wgpu_profiler::GpuProfilerSettings::default(),
                adapter.get_info().backend,
                &device,
                &queue,
            ) {
                Ok(profiler) => Some(profiler),
                Err(error) => {
                    warn!(%error, "GPU profiling disabled");
                    None
                }
            }
        } else {
            warn!("no timestamp-query support; profiling the CPU only");
            None
        };

        let config = surface
            .get_default_config(&adapter, width, height)
            .ok_or(RendererError::UnsupportedSurface)?;
        let present_modes = surface.get_capabilities(&adapter).present_modes;
        let mut config = config;
        config.present_mode = wgpu::PresentMode::Fifo;
        surface.configure(&device, &config);

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
            terrain: None,
            pending_terrain_uploads: 0,
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
            #[cfg(feature = "tracy")]
            profiler,
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

    pub(crate) fn upload_chunk(
        &mut self,
        chunk: ChunkCoord,
        lod: Lod,
        revision: u64,
        mesh: &MeshData,
    ) {
        if mesh.is_empty() {
            if let Some(gpu_chunk) = self.chunks.get_mut(&chunk) {
                *gpu_chunk.slot(lod) = None;
                if gpu_chunk.active == Some(lod) {
                    gpu_chunk.active = None;
                }
                if gpu_chunk.is_empty() {
                    self.chunks.remove(&chunk);
                }
            }
            return;
        }

        let (vertex_buffer, _) = create_vertex_buffer(&self.device, &mesh.vertices);
        let (index_buffer, index_format, index_count) = create_index_buffer(&self.device, mesh);
        let gpu_chunk = self.chunk_entry(chunk);
        *gpu_chunk.slot(lod) = Some((
            revision,
            GpuMesh {
                vertex_buffer,
                index_buffer,
                index_format,
                index_count,
            },
        ));
        if gpu_chunk.active.is_none() {
            gpu_chunk.active = Some(lod);
        }
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
    pub(crate) fn select_active(&mut self, chunk: ChunkCoord, lod: Lod) {
        if let Some(gpu_chunk) = self.chunks.get_mut(&chunk)
            && gpu_chunk.mesh(lod).is_some()
        {
            gpu_chunk.active = Some(lod);
        }
    }

    /// Uploads or clears a chunk's transparent water mesh.
    pub(crate) fn upload_water_chunk(&mut self, chunk: ChunkCoord, revision: u64, mesh: &MeshData) {
        if mesh.is_empty() {
            if let Some(gpu_chunk) = self.chunks.get_mut(&chunk) {
                gpu_chunk.water = None;
                if gpu_chunk.is_empty() {
                    self.chunks.remove(&chunk);
                }
            }
            return;
        }

        let (vertex_buffer, _) = create_vertex_buffer(&self.device, &mesh.vertices);
        let (index_buffer, index_format, index_count) = create_index_buffer(&self.device, mesh);
        let gpu_chunk = self.chunk_entry(chunk);
        gpu_chunk.water = Some((
            revision,
            GpuMesh {
                vertex_buffer,
                index_buffer,
                index_format,
                index_count,
            },
        ));
    }

    pub(crate) fn remove_chunk(&mut self, chunk: ChunkCoord) {
        self.chunks.remove(&chunk);
    }

    pub(crate) fn total_vertices(&self) -> u64 {
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
    pub(crate) fn render(&mut self, frustum: &Frustum) -> FrameStats {
        let mut stats = FrameStats {
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
            let descriptor = self.scene_pass_descriptor(&attachments);

            #[cfg(feature = "tracy")]
            if let Some(profiler) = &self.profiler {
                let mut scope = profiler.scope("world", &mut encoder);
                let mut pass = scope.scoped_render_pass("terrain", descriptor);
                self.draw_scene(&mut pass, frustum, &mut stats);
                drop(pass);
            } else {
                let mut pass = encoder.begin_render_pass(&descriptor);
                self.draw_scene(&mut pass, frustum, &mut stats);
            }

            #[cfg(not(feature = "tracy"))]
            {
                let mut pass = encoder.begin_render_pass(&descriptor);
                self.draw_scene(&mut pass, frustum, &mut stats);
            }

            #[cfg(feature = "tracy")]
            if let Some(profiler) = self.profiler.as_mut() {
                profiler.resolve_queries(&mut encoder);
            }
        }

        {
            profiling::scope!("submit");
            self.queue.submit(Some(encoder.finish()));
            self.window.pre_present_notify();
        }
        {
            profiling::scope!("present");
            self.queue.present(frame);
        }

        #[cfg(feature = "tracy")]
        {
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
                    crate::metrics::gpu_ms(gpu_ms);
                }
            }
        }

        stats
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
        pass: &mut wgpu::RenderPass<'_>,
        frustum: &Frustum,
        stats: &mut FrameStats,
    ) {
        if !self.chunks.is_empty() {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            pass.set_bind_group(2, &self.texture_bind_group, &[]);
            for (chunk, gpu_chunk) in &self.chunks {
                stats.total_chunks += 1;
                let Some(mesh) = gpu_chunk.active_mesh() else {
                    continue;
                };
                let min = chunk.origin();
                if !frustum.intersects_aabb(min, min + Vec3::splat(CHUNK_SIZE as f32)) {
                    continue;
                }
                pass.set_bind_group(1, &gpu_chunk.bind_group, &[]);
                pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
                pass.set_index_buffer(mesh.index_buffer.slice(..), mesh.index_format);
                pass.draw_indexed(0..mesh.index_count, 0, 0..1);
                stats.visible_chunks += 1;
                stats.draw_calls += 1;
                stats.visible_vertices += u64::from(mesh.index_count);
            }
        }

        // Packed-quad opaque and cutout terrain, still before any water.
        if let Some(terrain) = &self.terrain {
            terrain.draw_opaque(
                pass,
                frustum,
                &self.camera_bind_group,
                &self.texture_bind_group,
                stats,
            );
        }

        // Transparent water after all opaque geometry; depth writes stay off.
        pass.set_pipeline(&self.water_pipeline);
        pass.set_bind_group(0, &self.camera_bind_group, &[]);
        pass.set_bind_group(2, &self.texture_bind_group, &[]);
        for (chunk, gpu_chunk) in &self.chunks {
            let Some((_, mesh)) = &gpu_chunk.water else {
                continue;
            };
            let min = chunk.origin();
            if !frustum.intersects_aabb(min, min + Vec3::splat(CHUNK_SIZE as f32)) {
                continue;
            }
            if gpu_chunk.active_mesh().is_none() {
                stats.visible_chunks += 1;
            }
            pass.set_bind_group(1, &gpu_chunk.bind_group, &[]);
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(mesh.index_buffer.slice(..), mesh.index_format);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            stats.draw_calls += 1;
            stats.visible_vertices += u64::from(mesh.index_count);
        }

        // Packed-quad water, last of the terrain passes: blended, no depth
        // writes, and no culling.
        if let Some(terrain) = &self.terrain {
            terrain.draw_water(
                pass,
                frustum,
                &self.camera_bind_group,
                &self.texture_bind_group,
                stats,
            );
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

/// Device limits: the WebGL2-parity floor, plus one storage buffer for the
/// packed-quad terrain path.
///
/// `downlevel_webgl2_defaults` requests no storage buffers at all, which would
/// reject `terrain_draw.wgsl`'s record buffer. One buffer of at most
/// [`TERRAIN_STORAGE_BYTES`] is enough for the spec's per-section budget, and
/// asking for the adapter's own maximum would be wasteful.
fn device_limits(adapter: &wgpu::Adapter) -> wgpu::Limits {
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
fn timer_features(adapter: &wgpu::Adapter) -> wgpu::Features {
    let requested = wgpu_profiler::GpuProfiler::ALL_WGPU_TIMER_FEATURES & adapter.features();
    if requested.contains(wgpu::Features::TIMESTAMP_QUERY) {
        requested
    } else {
        wgpu::Features::empty()
    }
}

#[cfg(not(feature = "tracy"))]
fn timer_features(_adapter: &wgpu::Adapter) -> wgpu::Features {
    wgpu::Features::empty()
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

fn create_terrain_section_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("terrain section bind group layout"),
        entries: &[
            // The section origin, relative to the same world space as the
            // legacy chunk origin uniform.
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(16),
                },
                count: None,
            },
            // The packed `u64` records of one range, as 32-bit words.
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    })
}

/// Creates one packed-quad pipeline.
///
/// Culling stays off: cross quads are single-sided planes that must be visible
/// from both sides, and the opaque records are already back-face culled by the
/// mesher's winding.
#[allow(clippy::too_many_arguments)]
fn create_terrain_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    camera_layout: &wgpu::BindGroupLayout,
    section_layout: &wgpu::BindGroupLayout,
    texture_layout: &wgpu::BindGroupLayout,
    label: &str,
    fragment_entry: &str,
    blend: Option<wgpu::BlendState>,
    depth_write: bool,
    depth_compare: wgpu::CompareFunction,
) -> Result<wgpu::RenderPipeline, RendererError> {
    let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let shader = device.create_shader_module(wgpu::include_wgsl!("terrain_draw.wgsl"));
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("terrain pipeline layout"),
        bind_group_layouts: &[
            Some(camera_layout),
            Some(section_layout),
            Some(texture_layout),
        ],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            // Vertex pulling: no vertex buffer, no per-vertex attributes.
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some(fragment_entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
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

fn create_index_buffer(
    device: &wgpu::Device,
    mesh: &MeshData,
) -> (wgpu::Buffer, wgpu::IndexFormat, u32) {
    let count = u32::try_from(mesh.indices.len()).expect("index count fits in u32");
    if u16::try_from(mesh.vertices.len()).is_ok() {
        let indices: Vec<u16> = mesh
            .indices
            .iter()
            .map(|index| u16::try_from(*index).expect("index fits in u16"))
            .collect();
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("chunk indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        return (buffer, wgpu::IndexFormat::Uint16, count);
    }
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("chunk indices"),
        contents: bytemuck::cast_slice(&mesh.indices),
        usage: wgpu::BufferUsages::INDEX,
    });
    (buffer, wgpu::IndexFormat::Uint32, count)
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
    use crate::mesher2::Quad;

    /// The CPU mirror of `terrain_draw.wgsl`'s `face_duv`: `(du, dv)`
    /// multipliers of the quad extents for one corner slot, in the face's
    /// tangent order.
    fn face_duv(normal: u8, slot: u32) -> [f32; 2] {
        let mut duv = match slot {
            1 => [0.0, 1.0],
            2 => [1.0, 1.0],
            3 => [1.0, 0.0],
            _ => [0.0, 0.0],
        };
        match normal {
            1 | 4 => duv[0] = 1.0 - duv[0],
            3 => duv[1] = 1.0 - duv[1],
            _ => {}
        }
        duv
    }

    /// The CPU mirror of `terrain_draw.wgsl`'s `corner_geometry`.
    fn corner_position(normal: u8, slot: u32, base: [f32; 3], width: f32, height: f32) -> [f32; 3] {
        let [x, y, z] = base;
        if normal >= 6 {
            // Cross quads: a vertical diagonal through the cell.
            let (start, end) = if normal == 6 {
                ([x, z], [x + 1.0, z + 1.0])
            } else {
                ([x + 1.0, z], [x, z + 1.0])
            };
            return match slot {
                0 => [start[0], y, start[1]],
                1 => [start[0], y + 1.0, start[1]],
                2 => [end[0], y + 1.0, end[1]],
                _ => [end[0], y, end[1]],
            };
        }
        let duv = face_duv(normal, slot);
        let along_u = duv[0] * width;
        let along_v = duv[1] * height;
        match normal {
            0 => [x + 1.0, y + along_v, z + along_u],
            1 => [x, y + along_v, z + along_u],
            2 => [x + along_u, y + 1.0, z + along_v],
            3 => [x + along_u, y, z + along_v],
            4 => [x + along_u, y + along_v, z + 1.0],
            _ => [x + along_u, y + along_v, z],
        }
    }

    /// The six section-local positions of one quad, after the AO flip.
    fn quad_positions(quad: DecodedQuad) -> [[f32; 3]; 6] {
        let flip = quad.normal < 6 && (quad.ao[0] + quad.ao[2]) > (quad.ao[1] + quad.ao[3]);
        let order: [u32; 6] = if flip {
            [1, 2, 3, 1, 3, 0]
        } else {
            [0, 1, 2, 0, 2, 3]
        };
        let base = [f32::from(quad.x), f32::from(quad.y), f32::from(quad.z)];
        let width = f32::from(quad.w1) + 1.0;
        let height = f32::from(quad.h1) + 1.0;
        order.map(|slot| corner_position(quad.normal, slot, base, width, height))
    }

    #[test]
    fn decode_quad_reads_every_field_for_every_normal() {
        for normal in 0..=5 {
            let raw = Quad::encode(3, 7, 11, 9, 17, normal, 0xE4, 13, 5, 777, 9).bits();
            let quad = decode_quad(raw);

            assert_eq!((quad.x, quad.y, quad.z), (3, 7, 11), "normal {normal}");
            assert_eq!((quad.w1, quad.h1), (9, 17), "normal {normal}");
            assert_eq!(quad.normal, normal);
            assert_eq!(quad.ao, [0, 1, 2, 3], "normal {normal}");
            assert_eq!((quad.sky, quad.blk), (13, 5), "normal {normal}");
            assert_eq!(quad.tile, 777, "normal {normal}");
            assert_eq!(quad.tint, 9, "normal {normal}");
        }
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn plus_y_one_block_quad_has_the_expected_six_corners() {
        let quad = decode_quad(Quad::encode(2, 3, 4, 0, 0, 2, 0xE4, 15, 0, 2, 0).bits());

        assert_eq!(
            quad_positions(quad),
            [
                [2.0, 4.0, 4.0],
                [2.0, 4.0, 5.0],
                [3.0, 4.0, 5.0],
                [2.0, 4.0, 4.0],
                [3.0, 4.0, 5.0],
                [3.0, 4.0, 4.0],
            ]
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn ao_anisotropy_flips_the_diagonal() {
        // Corner 0 and corner 2 are fully open; the other pair is fully dark.
        let quad = decode_quad(Quad::encode(0, 0, 0, 0, 0, 2, 0x33, 15, 0, 0, 0).bits());
        assert_eq!(quad.ao, [3, 0, 3, 0]);

        let positions = quad_positions(quad);
        assert_eq!(
            positions,
            [
                [0.0, 1.0, 1.0],
                [1.0, 1.0, 1.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 1.0],
                [1.0, 1.0, 0.0],
                [0.0, 1.0, 0.0],
            ]
        );
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn cross_quad_decodes_with_zero_extents() {
        let quad = decode_quad(Quad::encode(5, 6, 7, 0, 0, 6, 0, 4, 2, 12, 3).bits());

        assert_eq!(quad.normal, 6);
        assert_eq!((quad.w1, quad.h1), (0, 0));
        assert_eq!((quad.x, quad.y, quad.z), (5, 6, 7));
        assert_eq!((quad.sky, quad.blk), (4, 2));

        // A cross quad still covers one vertical cell, so its corners span 1x1.
        let positions = quad_positions(quad);
        assert_eq!(positions[0], [5.0, 6.0, 7.0]);
        assert_eq!(positions[2], [6.0, 7.0, 8.0]);
    }

    #[test]
    fn terrain_shader_parses_and_validates() {
        let module = wgpu::naga::front::wgsl::parse_str(include_str!("terrain_draw.wgsl"))
            .expect("terrain_draw.wgsl parses");
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("terrain_draw.wgsl validates without optional capabilities");
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

    fn revisions(entries: &[([i32; 3], u64)]) -> BTreeMap<[i32; 3], u64> {
        entries.iter().copied().collect()
    }

    #[test]
    fn plan_upload_adds_a_new_origin() {
        let current = revisions(&[([0, 0, 0], 1)]);
        let incoming = [([0, 0, 0], 1), ([32, 0, 0], 7)];

        let plan = plan_upload(&current, &incoming).expect("origins are unique");

        assert_eq!(plan.uploads, vec![([32, 0, 0], 7)]);
        assert_eq!(plan.removals, Vec::<[i32; 3]>::new());
    }

    #[test]
    fn plan_upload_rewrites_a_changed_revision() {
        let current = revisions(&[([0, 0, 0], 1), ([32, 0, 0], 4)]);
        let incoming = [([0, 0, 0], 1), ([32, 0, 0], 5)];

        let plan = plan_upload(&current, &incoming).expect("origins are unique");

        assert_eq!(plan.uploads, vec![([32, 0, 0], 5)]);
        assert_eq!(plan.removals, Vec::<[i32; 3]>::new());
    }

    #[test]
    fn plan_upload_skips_unchanged_revisions() {
        let current = revisions(&[([0, 0, 0], 1), ([32, 0, 0], 4)]);
        let incoming = [([0, 0, 0], 1), ([32, 0, 0], 4)];

        let plan = plan_upload(&current, &incoming).expect("origins are unique");

        assert_eq!(plan.uploads, Vec::<([i32; 3], u64)>::new());
        assert_eq!(plan.removals, Vec::<[i32; 3]>::new());
    }

    #[test]
    fn plan_upload_drops_a_removed_origin() {
        let current = revisions(&[([0, 0, 0], 1), ([32, 0, 0], 4)]);
        let incoming = [([0, 0, 0], 1)];

        let plan = plan_upload(&current, &incoming).expect("origins are unique");

        assert_eq!(plan.uploads, Vec::<([i32; 3], u64)>::new());
        assert_eq!(plan.removals, vec![[32, 0, 0]]);
    }

    #[test]
    fn plan_upload_rejects_duplicate_origins() {
        let current = BTreeMap::new();
        let incoming = [([1, 2, 3], 1), ([1, 2, 3], 2)];

        let error = plan_upload(&current, &incoming).expect_err("duplicates are rejected");

        assert_eq!(error.origin, [1, 2, 3]);
    }

    #[test]
    fn section_revision_tracks_content_and_length() {
        let quad = Quad::encode(0, 0, 0, 0, 0, 2, 0, 15, 0, 1, 0);
        let darker = Quad::encode(0, 0, 0, 0, 0, 2, 0, 14, 0, 1, 0);
        let one = [quad];
        let same = [quad];
        let two = [quad, quad];
        let changed = [darker];

        let section = TerrainSection {
            origin: [0; 3],
            scale: 1,
            opaque: &one,
            cutout: &[],
            water: &[],
        };
        let identical = TerrainSection {
            origin: [0; 3],
            scale: 1,
            opaque: &same,
            cutout: &[],
            water: &[],
        };
        let longer = TerrainSection {
            origin: [0; 3],
            scale: 1,
            opaque: &two,
            cutout: &[],
            water: &[],
        };
        let edited = TerrainSection {
            origin: [0; 3],
            scale: 1,
            opaque: &changed,
            cutout: &[],
            water: &[],
        };

        assert_eq!(section_revision(&section), section_revision(&identical));
        assert_ne!(section_revision(&section), section_revision(&longer));
        assert_ne!(section_revision(&section), section_revision(&edited));
    }
}

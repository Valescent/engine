//! The packed-quad terrain path: vertex pulling over `mesher2` records.
//!
//! Each section keeps its own origin uniform and one storage buffer per range,
//! so a draw needs no vertex buffer, no index buffer, and no per-quad data.
//! The shader pulls every vertex from the storage buffer, and each origin is
//! uploaded once as absolute world blocks.

use std::collections::BTreeMap;
use std::mem::size_of;

use glam::{IVec3, Vec3};
use thiserror::Error;
use tracing::debug;

use crate::camera::relative_origin;
use crate::frustum::Frustum;
use crate::mesher2::Quad;

use super::FrameStats;
#[allow(unused_imports, reason = "named only by the intra-doc links below")]
use super::Renderer;
use super::RendererError;
use super::origin::create_origin_buffer;
use super::pipeline::{create_terrain_pipeline, create_terrain_section_layout};

/// Section-local extent of one terrain quad, in blocks.
const SECTION_EXTENT: f32 = 32.0;

/// Bytes requested for one section's storage-buffer binding. The spec budget
/// is 25,000 quads per section, which is 200 KB of records.
pub(super) const TERRAIN_STORAGE_BYTES: u64 = 1 << 20;

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
/// buffers. The origin uniform holds the section's **absolute** world-block
/// origin; `terrain_draw.wgsl` subtracts the frame's render origin in integer
/// space, so crossing a block never rewrites a resident origin. The path is
/// created only when [`Renderer::enable_terrain`] is called, which keeps the
/// legacy mesh demo untouched.
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
    /// Absolute world block origin, kept as integers for culling.
    world_origin: [i32; 3],
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
    pub(super) fn new(
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
        debug!(
            resident = self.sections.len(),
            uploaded, "terrain sections submitted"
        );
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
                resident.origin_buffer = create_origin_buffer(
                    device,
                    "terrain section origin",
                    resident.world_origin,
                    section_scale(section.scale),
                );
                // The scale lives in the origin uniform, so every resident
                // range must bind the new buffer.
                let origin_buffer = &resident.origin_buffer;
                for range in [
                    &mut resident.opaque,
                    &mut resident.cutout,
                    &mut resident.water,
                ]
                .into_iter()
                .flatten()
                {
                    range.bind_group =
                        create_quad_bind_group(device, layout, origin_buffer, &range.buffer);
                }
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

        let world_origin = section.origin;
        let origin_buffer = create_origin_buffer(
            device,
            "terrain section origin",
            world_origin,
            section_scale(section.scale),
        );
        let opaque = create_quad_range(device, queue, layout, &origin_buffer, section.opaque)?;
        let cutout = create_quad_range(device, queue, layout, &origin_buffer, section.cutout)?;
        let water = create_quad_range(device, queue, layout, &origin_buffer, section.water)?;
        self.sections.insert(
            section.origin,
            GpuSection {
                revision,
                world_origin,
                scale: section.scale,
                origin_buffer,
                opaque,
                cutout,
                water,
            },
        );
        Ok(())
    }

    /// Number of resident emitting sections.
    pub(super) fn resident_count(&self) -> usize {
        self.sections.len()
    }

    pub(super) fn draw_opaque(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        frustum: &Frustum,
        render_origin: IVec3,
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
            if !section_visible(frustum, section.world_origin, section.scale, render_origin) {
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

    pub(super) fn draw_water(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        frustum: &Frustum,
        render_origin: IVec3,
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
            if !section_visible(frustum, section.world_origin, section.scale, render_origin) {
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
pub(super) fn section_emits(section: &TerrainSection<'_>) -> bool {
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

fn section_visible(
    frustum: &Frustum,
    world_origin: [i32; 3],
    scale: u32,
    render_origin: IVec3,
) -> bool {
    let origin = relative_origin(world_origin, render_origin);
    frustum.intersects_aabb(
        origin,
        origin + Vec3::splat(SECTION_EXTENT * scale_factor(scale)),
    )
}

#[allow(
    clippy::cast_precision_loss,
    reason = "scale is 1, 2, or 4 and exact in f32"
)]
fn scale_factor(scale: u32) -> f32 {
    scale as f32
}

/// The `w` lane of a section origin uniform: its LOD scale as an integer.
fn section_scale(scale: u32) -> i32 {
    i32::try_from(scale).expect("a terrain section scale is 1, 2, or 4")
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

#[cfg(test)]
mod tests {
    use super::*;
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

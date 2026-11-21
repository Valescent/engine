//! GPU side of ADR-0015: the flat quad pool, chunk table, and indirect draws.
//!
//! The allocator is [`crate::pool::QuadPool`]; this module owns the storage
//! buffers, the vertex-pulling pipelines, and the per-frame argument list.

use std::collections::{HashMap, HashSet};
use std::mem::size_of;

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use tracing::error;

use crate::frustum::Frustum;
use crate::horizon::Horizon;
use crate::mesh::{Lod, MeshData};
use crate::pool::{QuadPool, QuadRange};
use crate::quad::{ChunkInfo, QuadRecord};
use crate::renderer::FrameStats;
use crate::section::SectionKey;
use crate::world::{CHUNK_SIZE, ChunkCoord};

/// A pool slot is either an L0 chunk or a coarse section.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum NodeKey {
    Chunk(ChunkCoord),
    Section(SectionKey),
}

impl NodeKey {
    fn origin(self) -> Vec3 {
        match self {
            Self::Chunk(chunk) => chunk.origin(),
            Self::Section(section) => section.origin().as_vec3(),
        }
    }

    #[allow(clippy::cast_precision_loss)]
    fn extent(self) -> f32 {
        match self {
            Self::Chunk(_) => CHUNK_SIZE as f32,
            Self::Section(section) => section.extent() as f32,
        }
    }
}

/// Pool budget: 3.2M quads at 12 bytes each, 38.4 MB.
pub(crate) const CAPACITY_QUADS: u32 = 3_200_000;
/// Larger pool for the section hierarchy, which covers 8 km.
pub(crate) const HIERARCHY_CAPACITY_QUADS: u32 = 8_000_000;

/// Bytes reserved for `capacity_quads` quad records.
pub(crate) fn pool_bytes(capacity_quads: u32) -> u64 {
    u64::from(capacity_quads) * size_of::<QuadRecord>() as u64
}

/// Slots for the largest world: `(2 * (32 + 2) + 1)^2` columns of four chunks,
/// rounded up with a small margin.
const SLOT_COUNT: u32 = 20_480;
const INFO_BYTES: u64 = SLOT_COUNT as u64 * size_of::<ChunkInfo>() as u64;

/// Largest mesh drawn through the shared `u16` index pattern.
///
/// A larger mesh keeps the six-vertex non-indexed path. Real chunks are around
/// two thousand quads; the cap only guards pathological edits.
const MAX_INDEXED_QUADS: u32 = 16_383;
/// Shared `[0,1,2,0,2,3]` pattern, one group per possible quad.
const INDEX_BYTES: u64 = MAX_INDEXED_QUADS as u64 * 6 * size_of::<u16>() as u64;

/// Indexed indirect arguments.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct DrawIndexedArgs {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}

/// One frame may write four lists: opaque and water, each indexed or plain.
const ARGS_BYTES: u64 = SLOT_COUNT as u64 * 4 * size_of::<DrawIndexedArgs>() as u64;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

const _: () = assert!(ARGS_BYTES >= SLOT_COUNT as u64 * 4 * size_of::<DrawIndexedArgs>() as u64);

/// Non-indexed indirect arguments.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct DrawArgs {
    vertex_count: u32,
    instance_count: u32,
    first_vertex: u32,
    first_instance: u32,
}

struct Slot {
    index: u32,
    key: NodeKey,
    /// Sections stay hidden while a retained parent is drawn.
    visible: bool,
    info: ChunkInfo,
    opaque: [Option<QuadRange>; 3],
    opaque_indexed: [bool; 3],
    active: Option<Lod>,
    water: Option<QuadRange>,
    water_indexed: bool,
    /// Index into [`QuadRenderer::active_slots`] while the slot has geometry.
    active_position: Option<usize>,
}

fn lod_index(lod: Lod) -> usize {
    match lod {
        Lod::Lod0 => 0,
        Lod::Lod1 => 1,
        Lod::Lod2 => 2,
    }
}

/// Six index values for one quad, in corner space.
fn index_pattern(base: u16) -> [u16; 6] {
    [base, base + 1, base + 2, base, base + 2, base + 3]
}

impl Slot {
    /// Counts the quads the slot actually draws: the active opaque range plus water.
    fn drawn_quads(&self) -> u64 {
        let opaque = self
            .active
            .and_then(|lod| self.opaque[lod_index(lod)])
            .map_or(0, |range| u64::from(range.quads));
        let water = self.water.map_or(0, |range| u64::from(range.quads));
        opaque + water
    }

    /// Whether the slot holds any resident geometry at all.
    fn has_geometry(&self) -> bool {
        self.opaque.iter().flatten().any(|range| range.quads > 0)
            || self.water.is_some_and(|range| range.quads > 0)
    }
}

/// Adds `position` to the active list when it holds geometry, or drops it.
fn refresh_active(slots: &mut [Slot], active_slots: &mut Vec<usize>, position: usize) {
    match (
        slots[position].has_geometry(),
        slots[position].active_position,
    ) {
        (true, None) => {
            slots[position].active_position = Some(active_slots.len());
            active_slots.push(position);
        }
        (false, Some(_)) => remove_active(slots, active_slots, position),
        _ => {}
    }
}

/// Removes `position` from the active list, keeping the swapped-in entry's link.
fn remove_active(slots: &mut [Slot], active_slots: &mut Vec<usize>, position: usize) {
    let Some(active) = slots[position].active_position.take() else {
        return;
    };
    active_slots.swap_remove(active);
    if active < active_slots.len() {
        let moved = active_slots[active];
        slots[moved].active_position = Some(active);
    }
}

/// Bytes held by one hierarchy's pool, chunk table, indirect arguments, and index pattern.
pub(crate) fn geometry_bytes(capacity_quads: u32) -> u64 {
    pool_bytes(capacity_quads) + INFO_BYTES + ARGS_BYTES + INDEX_BYTES
}

/// The quad-pool render path. Present only when the adapter supports it.
pub(crate) struct QuadRenderer {
    pool_buffer: wgpu::Buffer,
    info_buffer: wgpu::Buffer,
    args_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    index_buffer: wgpu::Buffer,
    opaque_pipeline: wgpu::RenderPipeline,
    water_pipeline: wgpu::RenderPipeline,
    opaque_indexed_pipeline: wgpu::RenderPipeline,
    water_indexed_pipeline: wgpu::RenderPipeline,
    pool: QuadPool,
    /// Bytes reserved by this renderer's pool, table, arguments, and index.
    geometry_bytes: u64,
    /// Active slots in draw order; `slot_positions` maps a node to its index.
    slots: Vec<Slot>,
    /// Positions in `slots` that hold geometry, so a draw skips empty slots.
    active_slots: Vec<usize>,
    slot_positions: HashMap<NodeKey, usize>,
    free_slots: Vec<u32>,
    next_slot: u32,
    deferred_slots: Vec<(u64, u32)>,
    /// Reused per-frame indirect argument lists for the two passes.
    opaque_args: Vec<DrawArgs>,
    water_args: Vec<DrawArgs>,
    opaque_indexed_args: Vec<DrawIndexedArgs>,
    water_indexed_args: Vec<DrawIndexedArgs>,
    /// Highest allocated-quad count seen, for pool sizing.
    peak_allocated_quads: u32,
}

impl QuadRenderer {
    /// Builds the pool path, or returns `None` when its shader or pipelines fail.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        config: &wgpu::SurfaceConfiguration,
        camera_layout: &wgpu::BindGroupLayout,
        texture_layout: &wgpu::BindGroupLayout,
        capacity_quads: u32,
    ) -> Option<Self> {
        let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);

        let quad_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("quad pool layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<ChunkInfo>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(size_of::<QuadRecord>() as u64),
                    },
                    count: None,
                },
            ],
        });

        let pool_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad pool"),
            size: pool_bytes(capacity_quads),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let info_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad chunk info"),
            size: INFO_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let args_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad indirect args"),
            size: ARGS_BYTES,
            usage: wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("quad pool bind group"),
            layout: &quad_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: info_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: pool_buffer.as_entire_binding(),
                },
            ],
        });

        let shader = device.create_shader_module(wgpu::include_wgsl!("quad.wgsl"));
        let mut indices = Vec::with_capacity((MAX_INDEXED_QUADS as usize) * 6);
        for quad in 0..MAX_INDEXED_QUADS {
            let base = u16::try_from(quad * 4).expect("index pattern fits");
            indices.extend_from_slice(&index_pattern(base));
        }
        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad index pattern"),
            size: INDEX_BYTES,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&index_buffer, 0, bytemuck::cast_slice(&indices));

        let opaque_pipeline = create_pipeline(
            device,
            config,
            camera_layout,
            &quad_layout,
            texture_layout,
            &shader,
            "quad opaque pipeline",
            "vs_quad",
            "fs_main",
            None,
            true,
            wgpu::CompareFunction::Less,
        );
        let water_pipeline = create_pipeline(
            device,
            config,
            camera_layout,
            &quad_layout,
            texture_layout,
            &shader,
            "quad water pipeline",
            "vs_water_quad",
            "fs_water",
            Some(wgpu::BlendState::ALPHA_BLENDING),
            false,
            wgpu::CompareFunction::LessEqual,
        );
        let opaque_indexed_pipeline = create_pipeline(
            device,
            config,
            camera_layout,
            &quad_layout,
            texture_layout,
            &shader,
            "quad opaque indexed pipeline",
            "vs_quad_indexed",
            "fs_main",
            None,
            true,
            wgpu::CompareFunction::Less,
        );
        let water_indexed_pipeline = create_pipeline(
            device,
            config,
            camera_layout,
            &quad_layout,
            texture_layout,
            &shader,
            "quad water indexed pipeline",
            "vs_water_quad_indexed",
            "fs_water",
            Some(wgpu::BlendState::ALPHA_BLENDING),
            false,
            wgpu::CompareFunction::LessEqual,
        );

        if let Some(error) = pollster::block_on(error_scope.pop()) {
            error!(%error, "quad pool pipeline disabled");
            return None;
        }

        Some(Self {
            pool_buffer,
            info_buffer,
            args_buffer,
            index_buffer,
            bind_group,
            opaque_pipeline,
            water_pipeline,
            opaque_indexed_pipeline,
            water_indexed_pipeline,
            pool: QuadPool::new(capacity_quads),
            geometry_bytes: geometry_bytes(capacity_quads),
            slots: Vec::new(),
            active_slots: Vec::new(),
            slot_positions: HashMap::new(),
            free_slots: Vec::new(),
            next_slot: 0,
            deferred_slots: Vec::new(),
            opaque_args: Vec::new(),
            water_args: Vec::new(),
            opaque_indexed_args: Vec::new(),
            water_indexed_args: Vec::new(),
            peak_allocated_quads: 0,
        })
    }

    /// Uploads an opaque mesh. Returns false when the pool or slot table is full.
    pub(crate) fn upload(
        &mut self,
        queue: &wgpu::Queue,
        chunk: ChunkCoord,
        lod: Lod,
        mesh: &MeshData,
        frame: u64,
    ) -> bool {
        if mesh.quads.is_empty() {
            self.clear_opaque(queue, chunk, lod, frame);
            return true;
        }
        let Ok(quads) = u32::try_from(mesh.quads.len()) else {
            return false;
        };
        // Allocate the pool range first so a slot is never reserved for a failed
        // upload; release the range if the slot table is full.
        let Some(range) = self.pool.allocate(quads) else {
            return false;
        };
        let Some(slot_index) = self.slot_for(NodeKey::Chunk(chunk)) else {
            self.pool.free(range, frame);
            return false;
        };
        queue.write_buffer(
            &self.pool_buffer,
            u64::from(range.first_quad) * size_of::<QuadRecord>() as u64,
            bytemuck::cast_slice(&mesh.quads),
        );

        let origin = chunk.origin();
        let position = *self
            .slot_positions
            .get(&NodeKey::Chunk(chunk))
            .expect("slot exists");
        let slot = &mut self.slots[position];
        let index = lod_index(lod);
        if let Some(old) = slot.opaque[index].replace(range) {
            self.pool.free(old, frame);
        }
        slot.opaque_indexed[index] = quads <= MAX_INDEXED_QUADS;
        if slot.active.is_none() {
            slot.active = Some(lod);
        }
        if slot.active == Some(lod) {
            slot.info.origin = [origin.x, origin.y, origin.z, lod.scale()];
            slot.info.opaque_first = range.first_quad;
            slot.info.opaque_quads = range.quads;
        }
        let info = slot.info;
        self.write_info(queue, slot_index, &info);
        refresh_active(&mut self.slots, &mut self.active_slots, position);
        true
    }

    /// Makes `lod` the drawn opaque mesh when that LOD has been uploaded.
    pub(crate) fn select_active(
        &mut self,
        queue: &wgpu::Queue,
        chunk: ChunkCoord,
        lod: Lod,
        frame: u64,
    ) {
        let Some(&position) = self.slot_positions.get(&NodeKey::Chunk(chunk)) else {
            return;
        };
        let slot = &mut self.slots[position];
        let index = lod_index(lod);
        let Some(range) = slot.opaque[index] else {
            return;
        };
        slot.active = Some(lod);
        slot.info.origin[3] = lod.scale();
        slot.info.opaque_first = range.first_quad;
        slot.info.opaque_quads = range.quads;
        for (other, slot_range) in slot.opaque.iter_mut().enumerate() {
            if other != index
                && let Some(stale) = slot_range.take()
            {
                self.pool.free(stale, frame);
            }
        }
        let info = slot.info;
        let slot_index = slot.index;
        self.write_info(queue, slot_index, &info);
        refresh_active(&mut self.slots, &mut self.active_slots, position);
    }

    /// Uploads a water mesh. Returns false when the pool or slot table is full.
    pub(crate) fn upload_water(
        &mut self,
        queue: &wgpu::Queue,
        chunk: ChunkCoord,
        mesh: &MeshData,
        frame: u64,
    ) -> bool {
        if mesh.quads.is_empty() {
            self.clear_water(queue, chunk, frame);
            return true;
        }
        let Ok(quads) = u32::try_from(mesh.quads.len()) else {
            return false;
        };
        // Allocate first; a failed slot allocation releases the range instead of
        // leaving an empty slot behind.
        let Some(range) = self.pool.allocate(quads) else {
            return false;
        };
        let Some(slot_index) = self.slot_for(NodeKey::Chunk(chunk)) else {
            self.pool.free(range, frame);
            return false;
        };
        queue.write_buffer(
            &self.pool_buffer,
            u64::from(range.first_quad) * size_of::<QuadRecord>() as u64,
            bytemuck::cast_slice(&mesh.quads),
        );

        let origin = chunk.origin();
        let position = *self
            .slot_positions
            .get(&NodeKey::Chunk(chunk))
            .expect("slot exists");
        let slot = &mut self.slots[position];
        if let Some(old) = slot.water.replace(range) {
            self.pool.free(old, frame);
        }
        slot.water_indexed = quads <= MAX_INDEXED_QUADS;
        slot.info.water_origin = [origin.x, origin.y, origin.z, 1.0];
        slot.info.water_first = range.first_quad;
        slot.info.water_quads = range.quads;
        let info = slot.info;
        self.write_info(queue, slot_index, &info);
        refresh_active(&mut self.slots, &mut self.active_slots, position);
        true
    }

    /// Uploads a coarse section mesh. Returns false when allocation fails.
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn upload_section(
        &mut self,
        queue: &wgpu::Queue,
        section: SectionKey,
        mesh: &MeshData,
        frame: u64,
    ) -> bool {
        if mesh.quads.is_empty() {
            self.remove_section(section, frame);
            return true;
        }
        let Ok(quads) = u32::try_from(mesh.quads.len()) else {
            return false;
        };
        let Some(range) = self.pool.allocate(quads) else {
            return false;
        };
        let key = NodeKey::Section(section);
        let Some(slot_index) = self.slot_for(key) else {
            self.pool.free(range, frame);
            return false;
        };
        queue.write_buffer(
            &self.pool_buffer,
            u64::from(range.first_quad) * size_of::<QuadRecord>() as u64,
            bytemuck::cast_slice(&mesh.quads),
        );

        let origin = section.origin().as_vec3();
        let position = *self.slot_positions.get(&key).expect("slot exists");
        let slot = &mut self.slots[position];
        if let Some(old) = slot.opaque[0].replace(range) {
            self.pool.free(old, frame);
        }
        slot.opaque_indexed[0] = quads <= MAX_INDEXED_QUADS;
        slot.active = Some(Lod::Lod0);
        slot.info.origin = [origin.x, origin.y, origin.z, section.scale() as f32];
        slot.info.opaque_first = range.first_quad;
        slot.info.opaque_quads = range.quads;
        let info = slot.info;
        self.write_info(queue, slot_index, &info);
        refresh_active(&mut self.slots, &mut self.active_slots, position);
        true
    }

    /// Uploads a coarse section's water mesh.
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn upload_section_water(
        &mut self,
        queue: &wgpu::Queue,
        section: SectionKey,
        mesh: &MeshData,
        frame: u64,
    ) -> bool {
        if mesh.quads.is_empty() {
            return true;
        }
        let Ok(quads) = u32::try_from(mesh.quads.len()) else {
            return false;
        };
        let Some(range) = self.pool.allocate(quads) else {
            return false;
        };
        let key = NodeKey::Section(section);
        let Some(slot_index) = self.slot_for(key) else {
            self.pool.free(range, frame);
            return false;
        };
        queue.write_buffer(
            &self.pool_buffer,
            u64::from(range.first_quad) * size_of::<QuadRecord>() as u64,
            bytemuck::cast_slice(&mesh.quads),
        );

        let origin = section.origin().as_vec3();
        let position = *self.slot_positions.get(&key).expect("slot exists");
        let slot = &mut self.slots[position];
        if let Some(old) = slot.water.replace(range) {
            self.pool.free(old, frame);
        }
        slot.water_indexed = quads <= MAX_INDEXED_QUADS;
        slot.info.water_origin = [origin.x, origin.y, origin.z, section.scale() as f32];
        slot.info.water_first = range.first_quad;
        slot.info.water_quads = range.quads;
        let info = slot.info;
        self.write_info(queue, slot_index, &info);
        refresh_active(&mut self.slots, &mut self.active_slots, position);
        true
    }

    /// Sets which sections are drawn; chunks are always visible.
    pub(crate) fn set_section_visibility(&mut self, target: &[SectionKey]) {
        let visible: HashSet<SectionKey> = target.iter().copied().collect();
        for slot in &mut self.slots {
            slot.visible = match slot.key {
                NodeKey::Chunk(_) => true,
                NodeKey::Section(key) => visible.contains(&key),
            };
        }
    }

    /// The LOD currently drawn for a chunk, if any.
    pub(crate) fn active_lod(&self, chunk: ChunkCoord) -> Option<Lod> {
        self.slot(NodeKey::Chunk(chunk))
            .and_then(|slot| slot.active)
    }

    /// Whether a chunk has the given LOD resident in the pool.
    pub(crate) fn has_lod(&self, chunk: ChunkCoord, lod: Lod) -> bool {
        self.slot(NodeKey::Chunk(chunk))
            .is_some_and(|slot| slot.opaque[lod_index(lod)].is_some())
    }

    /// Six vertices per drawn opaque and water quad.
    pub(crate) fn total_vertices(&self) -> u64 {
        6 * self
            .active_slots
            .iter()
            .map(|&position| self.slots[position].drawn_quads())
            .sum::<u64>()
    }

    /// Releases a chunk's ranges and slot after it leaves the world.
    pub(crate) fn remove(&mut self, chunk: ChunkCoord, frame: u64) {
        self.remove_node(NodeKey::Chunk(chunk), frame);
    }

    /// Releases a section's ranges and slot when it unloads.
    pub(crate) fn remove_section(&mut self, section: SectionKey, frame: u64) {
        self.remove_node(NodeKey::Section(section), frame);
    }

    fn remove_node(&mut self, key: NodeKey, frame: u64) {
        let Some(position) = self.slot_positions.remove(&key) else {
            return;
        };
        remove_active(&mut self.slots, &mut self.active_slots, position);
        let mut slot = self.slots.swap_remove(position);
        if position < self.slots.len() {
            self.slot_positions
                .insert(self.slots[position].key, position);
            if let Some(active) = self.slots[position].active_position {
                self.active_slots[active] = position;
            }
        }
        for range in slot.opaque.iter_mut().filter_map(Option::take) {
            self.pool.free(range, frame);
        }
        if let Some(range) = slot.water.take() {
            self.pool.free(range, frame);
        }
        self.deferred_slots.push((frame, slot.index));
    }

    /// Retires deferred frees and slot reuse for the current frame.
    pub(crate) fn begin_frame(&mut self, frame: u64) {
        self.pool.retire(frame);
        let mut index = 0;
        while index < self.deferred_slots.len() {
            if self.deferred_slots[index].0 + 2 <= frame {
                let (_, slot) = self.deferred_slots.swap_remove(index);
                self.free_slots.push(slot);
            } else {
                index += 1;
            }
        }
    }

    /// Culls, builds the indirect lists, and records both batches.
    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        clippy::cast_precision_loss
    )]
    pub(crate) fn draw(
        &mut self,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'_>,
        camera: &wgpu::BindGroup,
        textures: &wgpu::BindGroup,
        frustum: &Frustum,
        horizon: &Horizon,
        stats: &mut FrameStats,
    ) {
        self.opaque_args.clear();
        self.water_args.clear();
        self.opaque_indexed_args.clear();
        self.water_indexed_args.clear();
        let mut visible_quads = 0_u64;
        let mut visible_chunks = 0_u32;

        for active in 0..self.active_slots.len() {
            let position = self.active_slots[active];
            let slot = &self.slots[position];
            if !slot.visible {
                continue;
            }
            let key = slot.key;
            let min = key.origin();
            let extent = key.extent();
            if !frustum.intersects_aabb(min, min + Vec3::splat(extent)) {
                continue;
            }
            let occluded = match key {
                NodeKey::Chunk(chunk) => horizon.occludes(chunk),
                NodeKey::Section(section) => horizon.occludes_section(section),
            };
            if occluded {
                stats.horizon_culled += 1;
                continue;
            }
            let mut visible = false;
            if let Some(active) = slot.active
                && let Some(range) = slot.opaque[lod_index(active)].filter(|range| range.quads > 0)
            {
                if slot.opaque_indexed[lod_index(active)] {
                    self.opaque_indexed_args.push(DrawIndexedArgs {
                        index_count: 6 * range.quads,
                        instance_count: 1,
                        first_index: 0,
                        base_vertex: i32::try_from(u64::from(range.first_quad) * 4)
                            .expect("base vertex fits"),
                        first_instance: slot.index,
                    });
                } else {
                    self.opaque_args.push(DrawArgs {
                        vertex_count: 6 * range.quads,
                        instance_count: 1,
                        first_vertex: 0,
                        first_instance: slot.index,
                    });
                }
                visible_quads += u64::from(range.quads);
                visible = true;
            }
            if let Some(range) = slot.water.filter(|range| range.quads > 0) {
                if slot.water_indexed {
                    self.water_indexed_args.push(DrawIndexedArgs {
                        index_count: 6 * range.quads,
                        instance_count: 1,
                        first_index: 0,
                        base_vertex: i32::try_from(u64::from(range.first_quad) * 4)
                            .expect("base vertex fits"),
                        first_instance: slot.index,
                    });
                } else {
                    self.water_args.push(DrawArgs {
                        vertex_count: 6 * range.quads,
                        instance_count: 1,
                        first_vertex: 0,
                        first_instance: slot.index,
                    });
                }
                visible_quads += u64::from(range.quads);
                visible = true;
            }
            if visible {
                visible_chunks += 1;
            }
        }

        let opaque_indexed_offset = 0_u64;
        let opaque_offset = opaque_indexed_offset
            + (self.opaque_indexed_args.len() * size_of::<DrawIndexedArgs>()) as u64;
        let water_indexed_offset =
            opaque_offset + (self.opaque_args.len() * size_of::<DrawArgs>()) as u64;
        let water_offset = water_indexed_offset
            + (self.water_indexed_args.len() * size_of::<DrawIndexedArgs>()) as u64;

        if !self.opaque_indexed_args.is_empty() {
            queue.write_buffer(
                &self.args_buffer,
                opaque_indexed_offset,
                bytemuck::cast_slice(&self.opaque_indexed_args),
            );
        }
        if !self.opaque_args.is_empty() {
            queue.write_buffer(
                &self.args_buffer,
                opaque_offset,
                bytemuck::cast_slice(&self.opaque_args),
            );
        }
        if !self.water_indexed_args.is_empty() {
            queue.write_buffer(
                &self.args_buffer,
                water_indexed_offset,
                bytemuck::cast_slice(&self.water_indexed_args),
            );
        }
        if !self.water_args.is_empty() {
            queue.write_buffer(
                &self.args_buffer,
                water_offset,
                bytemuck::cast_slice(&self.water_args),
            );
        }

        stats.visible_chunks += visible_chunks;
        stats.visible_vertices += visible_quads * 6;
        stats.geometry_bytes = self.geometry_bytes;
        stats.visible_bytes = visible_quads * size_of::<QuadRecord>() as u64;
        let allocated_quads = self.pool.allocated_quads();
        self.peak_allocated_quads = self.peak_allocated_quads.max(allocated_quads);
        let record_bytes = size_of::<QuadRecord>() as u64;
        stats.allocated_bytes = u64::from(allocated_quads) * record_bytes;
        stats.peak_allocated_bytes = u64::from(self.peak_allocated_quads) * record_bytes;
        stats.total_chunks = u32::try_from(self.active_slots.len()).unwrap_or(u32::MAX);

        let mut index_bound = false;
        if !self.opaque_indexed_args.is_empty() {
            pass.set_pipeline(&self.opaque_indexed_pipeline);
            pass.set_bind_group(0, camera, &[]);
            pass.set_bind_group(1, &self.bind_group, &[]);
            pass.set_bind_group(2, textures, &[]);
            pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
            index_bound = true;
            pass.multi_draw_indexed_indirect(
                &self.args_buffer,
                opaque_indexed_offset,
                u32::try_from(self.opaque_indexed_args.len()).unwrap(),
            );
            stats.draw_calls += 1;
        }
        if !self.opaque_args.is_empty() {
            pass.set_pipeline(&self.opaque_pipeline);
            pass.set_bind_group(0, camera, &[]);
            pass.set_bind_group(1, &self.bind_group, &[]);
            pass.set_bind_group(2, textures, &[]);
            pass.multi_draw_indirect(
                &self.args_buffer,
                opaque_offset,
                u32::try_from(self.opaque_args.len()).unwrap(),
            );
            stats.draw_calls += 1;
        }

        if !self.water_indexed_args.is_empty() {
            pass.set_pipeline(&self.water_indexed_pipeline);
            pass.set_bind_group(0, camera, &[]);
            pass.set_bind_group(1, &self.bind_group, &[]);
            pass.set_bind_group(2, textures, &[]);
            if !index_bound {
                pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
            }
            pass.multi_draw_indexed_indirect(
                &self.args_buffer,
                water_indexed_offset,
                u32::try_from(self.water_indexed_args.len()).unwrap(),
            );
            stats.draw_calls += 1;
        }
        if !self.water_args.is_empty() {
            pass.set_pipeline(&self.water_pipeline);
            pass.set_bind_group(0, camera, &[]);
            pass.set_bind_group(1, &self.bind_group, &[]);
            pass.set_bind_group(2, textures, &[]);
            pass.multi_draw_indirect(
                &self.args_buffer,
                water_offset,
                u32::try_from(self.water_args.len()).unwrap(),
            );
            stats.draw_calls += 1;
        }
    }

    /// Looks up an active slot.
    fn slot(&self, key: NodeKey) -> Option<&Slot> {
        self.slot_positions
            .get(&key)
            .map(|&position| &self.slots[position])
    }

    fn slot_for(&mut self, key: NodeKey) -> Option<u32> {
        if let Some(&position) = self.slot_positions.get(&key) {
            return Some(self.slots[position].index);
        }
        let index = if let Some(index) = self.free_slots.pop() {
            index
        } else {
            if self.next_slot >= SLOT_COUNT {
                return None;
            }
            let index = self.next_slot;
            self.next_slot += 1;
            index
        };
        self.slot_positions.insert(key, self.slots.len());
        self.slots.push(Slot {
            index,
            key,
            visible: matches!(key, NodeKey::Chunk(_)),
            info: ChunkInfo::default(),
            opaque: [None; 3],
            opaque_indexed: [false; 3],
            active: None,
            water: None,
            water_indexed: false,
            active_position: None,
        });
        Some(index)
    }

    fn write_info(&self, queue: &wgpu::Queue, index: u32, info: &ChunkInfo) {
        queue.write_buffer(
            &self.info_buffer,
            u64::from(index) * size_of::<ChunkInfo>() as u64,
            bytemuck::bytes_of(info),
        );
    }

    fn clear_opaque(&mut self, queue: &wgpu::Queue, chunk: ChunkCoord, lod: Lod, frame: u64) {
        let Some(&position) = self.slot_positions.get(&NodeKey::Chunk(chunk)) else {
            return;
        };
        let slot = &mut self.slots[position];
        let index = lod_index(lod);
        if let Some(range) = slot.opaque[index].take() {
            self.pool.free(range, frame);
        }
        slot.opaque_indexed[index] = false;
        if slot.active == Some(lod) {
            slot.active = [Lod::Lod0, Lod::Lod1, Lod::Lod2]
                .into_iter()
                .find(|candidate| slot.opaque[lod_index(*candidate)].is_some());
            match slot
                .active
                .and_then(|active| slot.opaque[lod_index(active)])
            {
                Some(range) => {
                    slot.info.origin[3] = slot.active.map_or(1.0, Lod::scale);
                    slot.info.opaque_first = range.first_quad;
                    slot.info.opaque_quads = range.quads;
                }
                None => slot.info.opaque_quads = 0,
            }
        }
        let info = slot.info;
        let slot_index = slot.index;
        self.write_info(queue, slot_index, &info);
        refresh_active(&mut self.slots, &mut self.active_slots, position);
    }

    fn clear_water(&mut self, queue: &wgpu::Queue, chunk: ChunkCoord, frame: u64) {
        let Some(&position) = self.slot_positions.get(&NodeKey::Chunk(chunk)) else {
            return;
        };
        let slot = &mut self.slots[position];
        if let Some(range) = slot.water.take() {
            self.pool.free(range, frame);
        }
        slot.water_indexed = false;
        slot.info.water_quads = 0;
        let info = slot.info;
        let index = slot.index;
        self.write_info(queue, index, &info);
        refresh_active(&mut self.slots, &mut self.active_slots, position);
    }
}

#[allow(clippy::too_many_arguments)]
fn create_pipeline(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
    camera_layout: &wgpu::BindGroupLayout,
    quad_layout: &wgpu::BindGroupLayout,
    texture_layout: &wgpu::BindGroupLayout,
    shader: &wgpu::ShaderModule,
    label: &str,
    vertex_entry: &str,
    fragment_entry: &str,
    blend: Option<wgpu::BlendState>,
    depth_write: bool,
    depth_compare: wgpu::CompareFunction,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("quad pipeline layout"),
        bind_group_layouts: &[Some(camera_layout), Some(quad_layout), Some(texture_layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vertex_entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MAX_RENDER_DISTANCE;
    use crate::streaming::UNLOAD_MARGIN;
    use crate::world::CHUNK_COLUMN;

    #[test]
    fn section_nodes_use_their_own_origin_and_extent() {
        let key = NodeKey::Section(SectionKey::new(1, glam::IVec3::new(1, 0, -1)));
        assert_eq!(key.extent(), 64.0);
        assert_eq!(key.origin(), Vec3::new(64.0, 0.0, -64.0));
    }

    #[test]
    fn index_pattern_replays_the_quad_corner_order() {
        let corners = [0_u16, 1, 2, 0, 2, 3];
        for quad in 0..MAX_INDEXED_QUADS {
            let base = u16::try_from(quad * 4).unwrap();
            let pattern = index_pattern(base);
            assert_eq!(pattern, corners.map(|corner| base + corner));
            for (vertex, corner) in pattern.into_iter().zip(corners) {
                assert_eq!(u32::from(vertex) / 4, quad);
                assert_eq!(u32::from(vertex) % 4, u32::from(corner));
            }
        }
    }

    #[test]
    fn indirect_args_cover_four_batches() {
        let batch = u64::from(SLOT_COUNT) * size_of::<DrawIndexedArgs>() as u64;
        assert_eq!(ARGS_BYTES, 4 * batch);
        assert!(size_of::<DrawIndexedArgs>() >= size_of::<DrawArgs>());
    }

    #[test]
    fn slot_table_covers_the_largest_render_distance() {
        let radius = MAX_RENDER_DISTANCE + UNLOAD_MARGIN;
        let columns = (2 * radius + 1).pow(2);
        let chunks = u32::try_from(columns * CHUNK_COLUMN).unwrap();
        assert!(
            chunks <= SLOT_COUNT,
            "{chunks} chunks exceed {SLOT_COUNT} slots"
        );
    }

    #[test]
    fn slots_count_only_resident_geometry() {
        let empty = Slot {
            index: 0,
            key: NodeKey::Chunk(ChunkCoord::new(0, 0, 0)),
            visible: true,
            info: ChunkInfo::default(),
            opaque: [None; 3],
            opaque_indexed: [false; 3],
            active: None,
            water: None,
            water_indexed: false,
            active_position: None,
        };
        assert!(!empty.has_geometry());
        assert_eq!(empty.drawn_quads(), 0);

        let range = QuadRange {
            first_quad: 0,
            quads: 3,
            pages: 1,
        };
        let opaque = Slot {
            opaque: [None, Some(range), None],
            active: Some(Lod::Lod1),
            ..empty
        };
        assert!(opaque.has_geometry());
        assert_eq!(opaque.drawn_quads(), 3);

        let inactive = Slot {
            active: None,
            ..opaque
        };
        assert!(inactive.has_geometry());
        assert_eq!(inactive.drawn_quads(), 0);
    }

    fn empty_slot(index: u32, chunk: ChunkCoord) -> Slot {
        Slot {
            index,
            key: NodeKey::Chunk(chunk),
            visible: true,
            info: ChunkInfo::default(),
            opaque: [None; 3],
            opaque_indexed: [false; 3],
            active: None,
            water: None,
            water_indexed: false,
            active_position: None,
        }
    }

    #[test]
    fn active_slots_track_geometry_through_upload_and_removal() {
        let range = QuadRange {
            first_quad: 0,
            quads: 3,
            pages: 1,
        };
        let mut slots = vec![
            empty_slot(0, ChunkCoord::new(0, 0, 0)),
            empty_slot(1, ChunkCoord::new(1, 0, 0)),
        ];
        let mut active_slots = Vec::new();

        refresh_active(&mut slots, &mut active_slots, 0);
        assert_eq!(active_slots, Vec::<usize>::new());

        // An upload adds one entry, and refreshing again does not duplicate it.
        slots[0].opaque[0] = Some(range);
        slots[0].active = Some(Lod::Lod0);
        refresh_active(&mut slots, &mut active_slots, 0);
        assert_eq!(active_slots, vec![0]);
        refresh_active(&mut slots, &mut active_slots, 0);
        assert_eq!(active_slots, vec![0]);

        slots[1].water = Some(range);
        refresh_active(&mut slots, &mut active_slots, 1);
        assert_eq!(active_slots, vec![0, 1]);

        // The active list totals the same drawn quads as a full slot scan.
        let all = slots.iter().map(Slot::drawn_quads).sum::<u64>();
        let listed = active_slots
            .iter()
            .map(|&position| slots[position].drawn_quads())
            .sum::<u64>();
        assert_eq!(all, listed);

        // Removal drops the entry and keeps the swapped-in slot's link correct.
        slots[0].opaque[0] = None;
        slots[0].active = None;
        remove_active(&mut slots, &mut active_slots, 0);
        assert_eq!(active_slots, vec![1]);
        assert_eq!(slots[1].active_position, Some(0));
    }
}

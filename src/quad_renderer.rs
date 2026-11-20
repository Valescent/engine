//! GPU side of ADR-0015: the flat quad pool, chunk table, and indirect draws.
//!
//! The allocator is [`crate::pool::QuadPool`]; this module owns the storage
//! buffers, the vertex-pulling pipelines, and the per-frame argument list.

use std::collections::HashMap;
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
use crate::world::{CHUNK_SIZE, ChunkCoord};

/// Pool budget: 3.2M quads at 12 bytes each, 38.4 MB.
pub(crate) const CAPACITY_QUADS: u32 = 3_200_000;
/// Bytes reserved for the quad pool.
pub(crate) const POOL_BYTES: u64 = CAPACITY_QUADS as u64 * size_of::<QuadRecord>() as u64;

const SLOT_COUNT: u32 = 16_384;
const INFO_BYTES: u64 = SLOT_COUNT as u64 * size_of::<ChunkInfo>() as u64;
const ARGS_BYTES: u64 = SLOT_COUNT as u64 * size_of::<DrawArgs>() as u64;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

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
    info: ChunkInfo,
    opaque: [Option<QuadRange>; 3],
    active: Option<Lod>,
    water: Option<QuadRange>,
}

fn lod_index(lod: Lod) -> usize {
    match lod {
        Lod::Lod0 => 0,
        Lod::Lod1 => 1,
        Lod::Lod2 => 2,
    }
}

/// Bytes held by the pool, chunk table, and indirect arguments.
pub(crate) const GEOMETRY_BYTES: u64 = POOL_BYTES + INFO_BYTES + ARGS_BYTES;

/// The quad-pool render path. Present only when the adapter supports it.
pub(crate) struct QuadRenderer {
    pool_buffer: wgpu::Buffer,
    info_buffer: wgpu::Buffer,
    args_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    opaque_pipeline: wgpu::RenderPipeline,
    water_pipeline: wgpu::RenderPipeline,
    pool: QuadPool,
    slots: HashMap<ChunkCoord, Slot>,
    free_slots: Vec<u32>,
    next_slot: u32,
    deferred_slots: Vec<(u64, u32)>,
}

impl QuadRenderer {
    /// Builds the pool path, or returns `None` when its shader or pipelines fail.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn new(
        device: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
        camera_layout: &wgpu::BindGroupLayout,
        texture_layout: &wgpu::BindGroupLayout,
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
            size: POOL_BYTES,
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

        if let Some(error) = pollster::block_on(error_scope.pop()) {
            error!(%error, "quad pool pipeline disabled");
            return None;
        }

        Some(Self {
            pool_buffer,
            info_buffer,
            args_buffer,
            bind_group,
            opaque_pipeline,
            water_pipeline,
            pool: QuadPool::new(CAPACITY_QUADS),
            slots: HashMap::new(),
            free_slots: Vec::new(),
            next_slot: 0,
            deferred_slots: Vec::new(),
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
        let Some(slot_index) = self.slot_for(chunk) else {
            return false;
        };
        let Ok(quads) = u32::try_from(mesh.quads.len()) else {
            return false;
        };
        let Some(range) = self.pool.allocate(quads) else {
            return false;
        };
        queue.write_buffer(
            &self.pool_buffer,
            u64::from(range.first_quad) * size_of::<QuadRecord>() as u64,
            bytemuck::cast_slice(&mesh.quads),
        );

        let origin = chunk.origin();
        let slot = self.slots.get_mut(&chunk).expect("slot exists");
        let index = lod_index(lod);
        if let Some(old) = slot.opaque[index].replace(range) {
            self.pool.free(old, frame);
        }
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
        let Some(slot) = self.slots.get_mut(&chunk) else {
            return;
        };
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
        let Some(slot_index) = self.slot_for(chunk) else {
            return false;
        };
        let Ok(quads) = u32::try_from(mesh.quads.len()) else {
            return false;
        };
        let Some(range) = self.pool.allocate(quads) else {
            return false;
        };
        queue.write_buffer(
            &self.pool_buffer,
            u64::from(range.first_quad) * size_of::<QuadRecord>() as u64,
            bytemuck::cast_slice(&mesh.quads),
        );

        let origin = chunk.origin();
        let slot = self.slots.get_mut(&chunk).expect("slot exists");
        if let Some(old) = slot.water.replace(range) {
            self.pool.free(old, frame);
        }
        slot.info.water_origin = [origin.x, origin.y, origin.z, 1.0];
        slot.info.water_first = range.first_quad;
        slot.info.water_quads = range.quads;
        let info = slot.info;
        self.write_info(queue, slot_index, &info);
        true
    }

    /// Releases a chunk's ranges and slot after it leaves the world.
    pub(crate) fn remove(&mut self, chunk: ChunkCoord, frame: u64) {
        if let Some(mut slot) = self.slots.remove(&chunk) {
            for range in slot.opaque.iter_mut().filter_map(Option::take) {
                self.pool.free(range, frame);
            }
            if let Some(range) = slot.water.take() {
                self.pool.free(range, frame);
            }
            self.deferred_slots.push((frame, slot.index));
        }
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
        let mut opaque = Vec::with_capacity(self.slots.len());
        let mut water = Vec::new();
        let mut visible_quads = 0_u64;
        let mut visible_chunks = 0_u32;

        for (chunk, slot) in &self.slots {
            let min = chunk.origin();
            if !frustum.intersects_aabb(min, min + Vec3::splat(CHUNK_SIZE as f32)) {
                continue;
            }
            if horizon.occludes(*chunk) {
                continue;
            }
            let mut visible = false;
            if let Some(active) = slot.active
                && let Some(range) = slot.opaque[lod_index(active)].filter(|range| range.quads > 0)
            {
                opaque.push(DrawArgs {
                    vertex_count: 6 * range.quads,
                    instance_count: 1,
                    first_vertex: 0,
                    first_instance: slot.index,
                });
                visible_quads += u64::from(range.quads);
                visible = true;
            }
            if let Some(range) = slot.water.filter(|range| range.quads > 0) {
                water.push(DrawArgs {
                    vertex_count: 6 * range.quads,
                    instance_count: 1,
                    first_vertex: 0,
                    first_instance: slot.index,
                });
                visible_quads += u64::from(range.quads);
                visible = true;
            }
            if visible {
                visible_chunks += 1;
            }
        }

        if !opaque.is_empty() {
            queue.write_buffer(&self.args_buffer, 0, bytemuck::cast_slice(&opaque));
        }
        if !water.is_empty() {
            let offset = u64::try_from(opaque.len()).unwrap() * size_of::<DrawArgs>() as u64;
            queue.write_buffer(&self.args_buffer, offset, bytemuck::cast_slice(&water));
        }

        stats.visible_chunks += visible_chunks;
        stats.visible_vertices += visible_quads * 6;
        stats.geometry_bytes = GEOMETRY_BYTES;
        stats.visible_bytes = visible_quads * size_of::<QuadRecord>() as u64;
        stats.total_chunks = u32::try_from(self.slots.len()).unwrap_or(u32::MAX);

        pass.set_pipeline(&self.opaque_pipeline);
        pass.set_bind_group(0, camera, &[]);
        pass.set_bind_group(1, &self.bind_group, &[]);
        pass.set_bind_group(2, textures, &[]);
        if !opaque.is_empty() {
            pass.multi_draw_indirect(&self.args_buffer, 0, u32::try_from(opaque.len()).unwrap());
            stats.draw_calls += 1;
        }

        if !water.is_empty() {
            pass.set_pipeline(&self.water_pipeline);
            pass.set_bind_group(0, camera, &[]);
            pass.set_bind_group(1, &self.bind_group, &[]);
            pass.set_bind_group(2, textures, &[]);
            let offset = u64::try_from(opaque.len()).unwrap() * size_of::<DrawArgs>() as u64;
            pass.multi_draw_indirect(
                &self.args_buffer,
                offset,
                u32::try_from(water.len()).unwrap(),
            );
            stats.draw_calls += 1;
        }
    }

    fn slot_for(&mut self, chunk: ChunkCoord) -> Option<u32> {
        if let Some(slot) = self.slots.get(&chunk) {
            return Some(slot.index);
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
        self.slots.insert(
            chunk,
            Slot {
                index,
                info: ChunkInfo::default(),
                opaque: [None; 3],
                active: None,
                water: None,
            },
        );
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
        let Some(slot) = self.slots.get_mut(&chunk) else {
            return;
        };
        let index = lod_index(lod);
        if let Some(range) = slot.opaque[index].take() {
            self.pool.free(range, frame);
        }
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
    }

    fn clear_water(&mut self, queue: &wgpu::Queue, chunk: ChunkCoord, frame: u64) {
        let Some(slot) = self.slots.get_mut(&chunk) else {
            return;
        };
        if let Some(range) = slot.water.take() {
            self.pool.free(range, frame);
        }
        slot.info.water_quads = 0;
        let info = slot.info;
        let index = slot.index;
        self.write_info(queue, index, &info);
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

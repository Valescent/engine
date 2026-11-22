//! A tiny instanced cube pass for dropped items.
//!
//! Items are flat-coloured placeholder cubes until the art pipeline lands.
//! The pass rebuilds a small vertex buffer from world positions each frame,
//! subtracting the render origin so it shares the camera uniform.

use std::mem::size_of;

use bytemuck::{Pod, Zeroable};
use glam::{IVec3, Vec3};

use super::RendererError;
use super::pipeline::DEPTH_FORMAT;

/// Vertices per cube (six faces, two triangles each).
const VERTICES_PER_ITEM: usize = 36;
/// Initial item capacity.
const INITIAL_ITEMS: usize = 64;
/// Cube edge length in blocks.
const ITEM_SIZE: f32 = 0.3;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct ItemVertex {
    position: [f32; 3],
    color: [f32; 3],
}

/// The item draw pass.
pub(super) struct ItemPass {
    pipeline: wgpu::RenderPipeline,
    buffer: wgpu::Buffer,
    capacity: usize,
    count: u32,
}

impl ItemPass {
    pub(super) fn new(
        device: &wgpu::Device,
        config: &wgpu::SurfaceConfiguration,
        camera_bind_group_layout: &wgpu::BindGroupLayout,
    ) -> Result<Self, RendererError> {
        let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::include_wgsl!("../items.wgsl"));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("item pipeline layout"),
            bind_group_layouts: &[Some(camera_bind_group_layout)],
            immediate_size: 0,
        });
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: u64::try_from(size_of::<ItemVertex>())
                .expect("item vertex size fits in u64"),
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: 12,
                    shader_location: 1,
                },
            ],
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("item pipeline"),
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
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
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

        let capacity = INITIAL_ITEMS;
        let buffer = create_buffer(device, capacity);
        Ok(Self {
            pipeline,
            buffer,
            capacity,
            count: 0,
        })
    }

    /// Replaces the item buffer for this frame.
    pub(super) fn set_items(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        origin: IVec3,
        items: &[(Vec3, &str)],
    ) {
        if items.len() > self.capacity {
            self.capacity = items.len().next_power_of_two();
            self.buffer = create_buffer(device, self.capacity);
        }
        let mut vertices = Vec::with_capacity(items.len() * VERTICES_PER_ITEM);
        let origin = origin.as_vec3();
        for &(position, name) in items {
            push_cube(&mut vertices, position - origin, item_color(name));
        }
        self.count = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
        if !vertices.is_empty() {
            queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(&vertices));
        }
    }

    /// Draws the current item buffer.
    pub(super) fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        camera_bind_group: &wgpu::BindGroup,
    ) {
        if self.count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, camera_bind_group, &[]);
        pass.set_vertex_buffer(0, self.buffer.slice(..));
        pass.draw(0..self.count, 0..1);
    }
}

fn create_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    let size = u64::try_from(capacity * VERTICES_PER_ITEM * size_of::<ItemVertex>())
        .expect("item buffer size fits in u64");
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("item vertices"),
        size,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Cube faces in counter-clockwise order, as corner indices.
const FACES: [[usize; 4]; 6] = [
    [0, 3, 2, 1],
    [4, 5, 6, 7],
    [0, 4, 7, 3],
    [1, 2, 6, 5],
    [0, 1, 5, 4],
    [3, 7, 6, 2],
];

fn push_cube(vertices: &mut Vec<ItemVertex>, centre: Vec3, color: [f32; 3]) {
    let half = ITEM_SIZE / 2.0;
    let corners = [
        centre + Vec3::new(-half, -half, -half),
        centre + Vec3::new(half, -half, -half),
        centre + Vec3::new(half, -half, half),
        centre + Vec3::new(-half, -half, half),
        centre + Vec3::new(-half, half, -half),
        centre + Vec3::new(half, half, -half),
        centre + Vec3::new(half, half, half),
        centre + Vec3::new(-half, half, half),
    ];
    for face in FACES {
        let a = corners[face[0]];
        let b = corners[face[1]];
        let c = corners[face[2]];
        let d = corners[face[3]];
        for position in [a, b, c, a, c, d] {
            vertices.push(ItemVertex {
                position: position.to_array(),
                color,
            });
        }
    }
}

/// Placeholder colour for an item's dropped cube.
fn item_color(name: &str) -> [f32; 3] {
    match name {
        "core:dirt" | "core:clay" => [0.45, 0.31, 0.20],
        "core:grass" => [0.30, 0.62, 0.24],
        "core:leaves" | "core:plant" => [0.20, 0.52, 0.20],
        "core:wood" | "core:planks" | "core:workbench" => [0.55, 0.40, 0.22],
        "core:sand" => [0.85, 0.80, 0.55],
        "core:snow" | "core:ice" => [0.85, 0.92, 0.98],
        "core:water" => [0.20, 0.42, 0.75],
        "core:lamp" => [1.0, 0.85, 0.40],
        "core:ore_copper" => [0.55, 0.45, 0.30],
        "core:ore_iron" => [0.70, 0.62, 0.55],
        "core:stick" => [0.62, 0.45, 0.25],
        "core:apple" => [0.82, 0.15, 0.15],
        name if name.contains("pickaxe") => [0.62, 0.50, 0.30],
        name if name.contains("axe") => [0.55, 0.45, 0.28],
        name if name.contains("shovel") => [0.48, 0.40, 0.26],
        name if name.contains("sword") => [0.72, 0.68, 0.55],
        _ => [0.62, 0.62, 0.62],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cube_has_the_expected_vertex_count() {
        let mut vertices = Vec::new();
        push_cube(&mut vertices, Vec3::ZERO, [1.0; 3]);

        assert_eq!(vertices.len(), VERTICES_PER_ITEM);
    }

    #[test]
    fn known_items_have_distinct_colours() {
        assert_ne!(item_color("core:grass"), item_color("core:stone"));
        assert_eq!(item_color("core:stone"), [0.62, 0.62, 0.62]);
    }
}

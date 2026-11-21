//! The legacy per-chunk mesh path: LOD meshes, water meshes, and their origin
//! uniforms.

use wgpu::util::DeviceExt;

use crate::mesh::{Lod, MeshData};
use crate::voxel::Vertex;

pub(super) struct GpuMesh {
    pub(super) vertex_buffer: wgpu::Buffer,
    pub(super) index_buffer: wgpu::Buffer,
    pub(super) index_format: wgpu::IndexFormat,
    pub(super) index_count: u32,
}

pub(super) struct GpuChunk {
    /// Absolute world-block chunk origin, uploaded once and never rewritten.
    /// Kept for the chunk bind group's lifetime.
    pub(super) _origin_buffer: wgpu::Buffer,
    pub(super) bind_group: wgpu::BindGroup,
    pub(super) lod0: Option<(u64, GpuMesh)>,
    pub(super) lod1: Option<(u64, GpuMesh)>,
    pub(super) lod2: Option<(u64, GpuMesh)>,
    pub(super) water: Option<(u64, GpuMesh)>,
    pub(super) active: Option<Lod>,
}

impl GpuChunk {
    pub(super) fn mesh(&self, lod: Lod) -> Option<&GpuMesh> {
        let slot = match lod {
            Lod::Lod0 => &self.lod0,
            Lod::Lod1 => &self.lod1,
            Lod::Lod2 => &self.lod2,
        };
        slot.as_ref().map(|(_, mesh)| mesh)
    }

    pub(super) fn slot(&mut self, lod: Lod) -> &mut Option<(u64, GpuMesh)> {
        match lod {
            Lod::Lod0 => &mut self.lod0,
            Lod::Lod1 => &mut self.lod1,
            Lod::Lod2 => &mut self.lod2,
        }
    }

    pub(super) fn active_mesh(&self) -> Option<&GpuMesh> {
        self.active.and_then(|lod| self.mesh(lod))
    }

    pub(super) fn is_empty(&self) -> bool {
        self.lod0.is_none() && self.lod1.is_none() && self.lod2.is_none() && self.water.is_none()
    }
}

pub(super) fn create_index_buffer(
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

pub(super) fn create_vertex_buffer(device: &wgpu::Device, mesh: &[Vertex]) -> (wgpu::Buffer, u32) {
    let vertex_count = u32::try_from(mesh.len()).expect("mesh vertex count fits in u32");
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("chunk vertices"),
        contents: bytemuck::cast_slice(mesh),
        usage: wgpu::BufferUsages::VERTEX,
    });
    (buffer, vertex_count)
}

//! The legacy per-chunk mesh path: LOD meshes, water meshes, and their origin
//! uniforms.

use wgpu::util::DeviceExt;

use crate::mesh::{Lod, MeshData};
use crate::voxel::Vertex;

pub(super) struct GpuMesh {
    pub(super) vertex_buffer: wgpu::Buffer,
    /// Per-chunk buffer only when the mesh exceeds a `u16` index; otherwise the
    /// renderer's shared quad pattern is used.
    pub(super) index_buffer: Option<wgpu::Buffer>,
    pub(super) index_format: wgpu::IndexFormat,
    pub(super) index_count: u32,
    pub(super) bytes: u64,
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

    /// Stores `mesh` in `lod`, returning the mesh it replaced.
    pub(super) fn replace_mesh(
        &mut self,
        lod: Lod,
        revision: u64,
        mesh: GpuMesh,
    ) -> Option<GpuMesh> {
        self.slot(lod)
            .replace((revision, mesh))
            .map(|(_, replaced)| replaced)
    }

    /// Clears `lod`, returning the mesh it held.
    pub(super) fn take_mesh(&mut self, lod: Lod) -> Option<GpuMesh> {
        self.slot(lod).take().map(|(_, mesh)| mesh)
    }

    pub(super) fn active_mesh(&self) -> Option<&GpuMesh> {
        self.active.and_then(|lod| self.mesh(lod))
    }

    pub(super) fn is_empty(&self) -> bool {
        self.lod0.is_none() && self.lod1.is_none() && self.lod2.is_none() && self.water.is_none()
    }

    pub(super) fn meshes(&self) -> impl Iterator<Item = &GpuMesh> {
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

/// Quads in the shared index pattern; `u16` indices cap it at 16,384 quads.
pub(super) const SHARED_QUADS: u32 = 65_536 / 4;

pub(super) fn shared_index_data(quads: u32) -> Vec<u16> {
    let mut data = Vec::with_capacity(quads as usize * 6);
    for quad in 0..quads {
        let base = u16::try_from(quad * 4).expect("shared index fits in u16");
        data.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    data
}

pub(super) fn create_shared_index_buffer(device: &wgpu::Device) -> wgpu::Buffer {
    let indices = shared_index_data(SHARED_QUADS);
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("shared quad indices"),
        contents: bytemuck::cast_slice(&indices),
        usage: wgpu::BufferUsages::INDEX,
    })
}

pub(super) fn create_index_buffer(
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

pub(super) fn create_vertex_buffer(device: &wgpu::Device, mesh: &[Vertex]) -> (wgpu::Buffer, u32) {
    let vertex_count = u32::try_from(mesh.len()).expect("mesh vertex count fits in u32");
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("chunk vertices"),
        contents: bytemuck::cast_slice(mesh),
        usage: wgpu::BufferUsages::VERTEX,
    });
    (buffer, vertex_count)
}

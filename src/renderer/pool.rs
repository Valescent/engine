//! Quad-pool capability gates and the pool/fallback upload paths.

use std::time::{Duration, Instant};

use tracing::warn;

use super::Renderer;
use super::chunk::{GpuChunk, GpuMesh, create_index_buffer, create_vertex_buffer};
use super::origin::create_origin_buffer;
use super::pipeline::create_chunk_bind_group;
use crate::mesh::{Lod, MeshData};
use crate::section::SectionKey;
use crate::world::ChunkCoord;

/// At most one pool-full warning per second; the rest are counted and folded
/// into the next warning.
const POOL_WARNING_INTERVAL: Duration = Duration::from_secs(1);

/// Adapter properties that decide whether the quad-pool section path can run.
#[allow(clippy::struct_excessive_bools)] // One field per independent adapter gate.
#[derive(Clone, Copy, Debug)]
pub(super) struct PoolCapabilities {
    pub(super) legacy_forced: bool,
    pub(super) indirect_first_instance: bool,
    pub(super) indirect_execution: bool,
    pub(super) vertex_storage: bool,
    pub(super) storage_buffers_per_stage: u32,
    pub(super) storage_buffer_binding_size: u64,
    pub(super) max_buffer_size: u64,
    pub(super) pool_bytes: u64,
}

/// Whether the adapter can host the quad pool and its section draws.
pub(super) fn pool_supported(capabilities: PoolCapabilities) -> bool {
    !capabilities.legacy_forced
        && capabilities.indirect_first_instance
        && capabilities.indirect_execution
        && capabilities.vertex_storage
        && capabilities.storage_buffers_per_stage >= 2
        && capabilities.storage_buffer_binding_size >= capabilities.pool_bytes
        && capabilities.max_buffer_size >= capabilities.pool_bytes
}

impl Renderer {
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

    /// Whether the quad-pool path can render hierarchy sections.
    ///
    /// When this is false, section uploads would fail forever and the fallback
    /// chunk renderer does not draw sections, so the app disables hierarchy.
    pub(crate) fn supports_sections(&self) -> bool {
        self.quad.is_some()
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
            let world_origin = chunk.world_origin().to_array();
            let origin_buffer = create_origin_buffer(device, "chunk origin", world_origin, 0);
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

    pub(crate) fn upload_section(&mut self, section: SectionKey, mesh: &MeshData) -> bool {
        let frame = self.frame_index;
        self.quad
            .as_mut()
            .is_some_and(|quad| quad.upload_section(&self.queue, section, mesh, frame))
    }

    pub(crate) fn upload_section_water(&mut self, section: SectionKey, mesh: &MeshData) -> bool {
        let frame = self.frame_index;
        self.quad
            .as_mut()
            .is_some_and(|quad| quad.upload_section_water(&self.queue, section, mesh, frame))
    }

    #[allow(dead_code)] // Streaming uses this when section eviction lands.
    pub(crate) fn set_section_visibility(&mut self, target: &[SectionKey]) {
        if let Some(quad) = self.quad.as_mut() {
            quad.set_section_visibility(target);
        }
    }

    pub(crate) fn remove_section(&mut self, section: SectionKey) {
        if let Some(quad) = self.quad.as_mut() {
            quad.remove_section(section, self.frame_index);
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
}

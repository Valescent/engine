//! Absolute origin uniforms and the camera-relative bookkeeping.
//!
//! Both draw paths upload each resident's absolute world-block origin once and
//! let the vertex shaders subtract the frame origin in integer space. Crossing a
//! block rewrites no resident buffer; only the frame uniform and the small
//! relative highlight buffer move.

use glam::IVec3;
use wgpu::util::DeviceExt;

/// What moving the frame origin costs now that origin uniforms are static.
///
/// Section and chunk origin uniforms hold absolute world-block integers and
/// are uploaded once per resident. Only the relative-space highlight buffer is
/// rebuilt, so `origin_buffer_writes` is always zero; `avoided_origin_writes`
/// records the per-origin `Queue::write_buffer` calls the previous relative
/// convention made on every whole-block crossing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct OriginChange {
    /// Frame origin before the move.
    pub(crate) from: [i32; 3],
    /// Frame origin after the move.
    pub(crate) to: [i32; 3],
    /// Resident origin uniforms rewritten by the move. Always zero.
    pub(crate) origin_buffer_writes: usize,
    /// Resident chunk plus section origins the move no longer rewrites.
    pub(crate) avoided_origin_writes: usize,
}

pub(super) fn origin_change(
    from: IVec3,
    to: IVec3,
    resident_chunks: usize,
    resident_sections: usize,
) -> OriginChange {
    OriginChange {
        from: from.to_array(),
        to: to.to_array(),
        origin_buffer_writes: 0,
        avoided_origin_writes: resident_chunks.saturating_add(resident_sections),
    }
}

/// Wraps the world-space water-wave phase of `origin` into `[0, TAU)`.
///
/// The legacy water shader evaluates `sin(phase + 0.7 x + 0.5 z)` and
/// `cos(phase + 0.6 x - 0.4 z)`. Shipping the phase of the frame origin,
/// computed in `f64` and wrapped modulo `TAU`, lets the shader work only from
/// relative coordinates: wrapping is invisible to `sin`/`cos`, and the pattern
/// stays anchored to world x/z as the camera crosses a block. The result is
/// below `TAU`, so it is exact enough in `f32` even when the world coordinate
/// is not.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a value in [0, TAU) loses no visible precision in f32"
)]
pub(super) fn wave_anchor(origin: IVec3) -> [f32; 4] {
    let x = f64::from(origin.x);
    let z = f64::from(origin.z);
    let along_x = (0.7 * x + 0.5 * z).rem_euclid(std::f64::consts::TAU);
    let along_z = (0.6 * x - 0.4 * z).rem_euclid(std::f64::consts::TAU);
    [along_x as f32, along_z as f32, 0.0, 0.0]
}

/// Usage for every per-origin uniform.
///
/// Deliberately no `COPY_DST`: the absolute world-space origin is uploaded once
/// with the resident entry and can never be rewritten by
/// `Queue::write_buffer`, so a camera crossing does no origin work at all.
const ORIGIN_BUFFER_USAGE: wgpu::BufferUsages = wgpu::BufferUsages::UNIFORM;

/// The four `i32` lanes of one origin uniform.
fn origin_packet(world_origin: [i32; 3], extra: i32) -> [i32; 4] {
    [world_origin[0], world_origin[1], world_origin[2], extra]
}

/// Uploads one origin uniform: absolute world blocks in `xyz`, `extra` in `w`.
pub(super) fn create_origin_buffer(
    device: &wgpu::Device,
    label: &str,
    world_origin: [i32; 3],
    extra: i32,
) -> wgpu::Buffer {
    let data = origin_packet(world_origin, extra);
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: bytemuck::cast_slice(&data),
        usage: ORIGIN_BUFFER_USAGE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_packet_keeps_absolute_world_blocks_exact() {
        let world = [1_048_579, 64, -1_048_581];

        assert_eq!(origin_packet(world, 2), [1_048_579, 64, -1_048_581, 2]);
        // The frame origin is not part of the packet: the same resident always
        // uploads the same bytes, no matter where the camera is.
        assert_eq!(origin_packet(world, 2), origin_packet(world, 2));
    }

    /// Origin uniforms are `UNIFORM` only: `Queue::write_buffer` requires
    /// `COPY_DST`, so no camera crossing can rewrite them.
    #[test]
    fn origin_buffers_are_static() {
        assert!(ORIGIN_BUFFER_USAGE.contains(wgpu::BufferUsages::UNIFORM));
        assert!(!ORIGIN_BUFFER_USAGE.contains(wgpu::BufferUsages::COPY_DST));
    }

    #[test]
    fn whole_block_crossing_writes_no_origin_buffers() {
        // Whatever is resident, moving the frame origin rewrites nothing. The
        // 1,089 + 64 case is the brief's ring-column count plus the legacy
        // stress scene's 64 chunks; the second is the measured full demo ring
        // (372 emitting sections + 1,388 legacy chunks), where the old rebase
        // made one `write_buffer` call per origin on every crossing.
        for (chunks, sections, avoided) in [(64, 1_089, 1_153), (1_388, 372, 1_760), (0, 0, 0)] {
            let change =
                origin_change(IVec3::new(0, 64, 0), IVec3::new(1, 64, 0), chunks, sections);

            assert_eq!(change.origin_buffer_writes, 0);
            assert_eq!(change.avoided_origin_writes, avoided);
        }
    }
}

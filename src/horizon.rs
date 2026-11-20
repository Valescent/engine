//! Conservative horizon occlusion culling from chunk tops.
//!
//! A chunk is culled only when terrain nearer than it rises above the ray to its
//! top across **every** azimuth bin its footprint covers. That is deliberately
//! conservative: an empty bin, or a clear direction through the chunk, keeps it.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap
)]

use std::collections::HashMap;

use glam::Vec3;

use crate::world::{CHUNK_SIZE, ChunkCoord, World};

const BINS: usize = 128;
const TAU: f32 = std::f32::consts::TAU;
/// Do not cull chunks nearer than this; their angular size and parallax are large.
const MIN_DISTANCE: f32 = 8.0 * CHUNK_SIZE as f32;
/// Nearer terrain must be at least this much closer to count.
const DISTANCE_MARGIN: f32 = CHUNK_SIZE as f32;
/// Safety margin on the elevation comparison, in radians.
const ANGLE_MARGIN: f32 = 0.03;

#[derive(Debug, Default)]
pub(crate) struct Horizon {
    camera: Vec3,
    /// Per bin, `(distance, prefix maximum elevation)` sorted by distance.
    bins: Vec<Vec<(f32, f32)>>,
    /// World-space top face of each chunk that has solid blocks.
    tops: HashMap<ChunkCoord, f32>,
}

impl Horizon {
    /// A horizon that never culls, for the disabled path.
    pub(crate) fn disabled() -> Self {
        Self::default()
    }

    /// Builds the horizon for one camera position.
    pub(crate) fn build(camera: Vec3, world: &World) -> Self {
        let mut bins: Vec<Vec<(f32, f32)>> = (0..BINS).map(|_| Vec::new()).collect();
        let mut tops = HashMap::new();
        for (chunk, data) in world.iter_chunks() {
            if data.top() < 0 {
                continue;
            }
            let origin = chunk.origin();
            let top = origin.y + data.top() as f32 + 1.0;
            tops.insert(chunk, top);
            let Some((distance, azimuth)) = sample(camera, origin) else {
                continue;
            };
            let elevation = (top - camera.y).atan2(distance.max(1.0));
            let center = bin_of(azimuth);
            let half_angle = (f32::from(CHUNK_SIZE as u16) * 0.75).atan2(distance.max(1.0));
            let span = (half_angle / TAU * BINS as f32).ceil() as isize;
            if span >= BINS as isize / 2 {
                for bin in &mut bins {
                    bin.push((distance, elevation));
                }
            } else {
                for offset in -span..=span {
                    let bin = (center as isize + offset).rem_euclid(BINS as isize) as usize;
                    bins[bin].push((distance, elevation));
                }
            }
        }

        for bin in &mut bins {
            bin.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut maximum = f32::NEG_INFINITY;
            for entry in bin {
                maximum = maximum.max(entry.1);
                entry.1 = maximum;
            }
        }

        Self { camera, bins, tops }
    }

    /// Returns whether nearer terrain hides this chunk's top.
    pub(crate) fn occludes(&self, chunk: ChunkCoord) -> bool {
        if self.bins.is_empty() {
            return false;
        }
        let Some(&top) = self.tops.get(&chunk) else {
            return false;
        };
        let origin = chunk.origin();
        let Some((distance, azimuth)) = sample(self.camera, origin) else {
            return false;
        };
        if distance < MIN_DISTANCE {
            return false;
        }
        let elevation = (top - self.camera.y).atan2(distance.max(1.0));
        let half_angle = (f32::from(CHUNK_SIZE as u16) * 0.75).atan2(distance);
        let limit = distance - DISTANCE_MARGIN;
        let mut horizon = f32::INFINITY;

        let center = bin_of(azimuth);
        let span = (half_angle / TAU * BINS as f32).ceil() as isize;
        if span >= BINS as isize / 2 {
            return false;
        }
        for offset in -span..=span {
            let bin = (center as isize + offset).rem_euclid(BINS as isize) as usize;
            let entries = &self.bins[bin];
            let count = entries.partition_point(|(d, _)| *d < limit);
            if count == 0 {
                return false;
            }
            horizon = horizon.min(entries[count - 1].1);
        }

        elevation + ANGLE_MARGIN < horizon
    }
}

/// Nearest horizontal distance and direction from the camera to a chunk.
fn sample(camera: Vec3, origin: Vec3) -> Option<(f32, f32)> {
    let min = glam::Vec2::new(origin.x, origin.z);
    let max = min + glam::Vec2::splat(CHUNK_SIZE as f32);
    let camera_xz = glam::Vec2::new(camera.x, camera.z);
    let nearest = camera_xz.clamp(min, max);
    let delta = nearest - camera_xz;
    let distance = delta.length();
    if distance < 1.0 {
        return None;
    }
    Some((distance, delta.y.atan2(delta.x)))
}

fn bin_of(azimuth: f32) -> usize {
    let normalized = (azimuth + std::f32::consts::PI) / TAU;
    (normalized * BINS as f32).floor().rem_euclid(BINS as f32) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use glam::IVec3;

    fn wall(world: &mut World) {
        for z in -2..=2 {
            world.set_block(IVec3::new(64, 40, z * 32), Block::Stone);
        }
    }

    #[test]
    fn nearer_terrain_hides_a_lower_chunk() {
        let mut world = World::default();
        wall(&mut world);
        world.set_block(IVec3::new(320, 2, 0), Block::Stone);
        let horizon = Horizon::build(Vec3::new(0.0, 10.0, 0.0), &world);

        assert!(horizon.occludes(ChunkCoord::new(10, 0, 0)));
    }

    #[test]
    fn an_empty_direction_keeps_the_chunk() {
        let mut world = World::default();
        world.set_block(IVec3::new(320, 2, 0), Block::Stone);
        let horizon = Horizon::build(Vec3::new(0.0, 10.0, 0.0), &world);

        assert!(!horizon.occludes(ChunkCoord::new(10, 0, 0)));
    }

    #[test]
    fn near_chunks_are_never_culled() {
        let mut world = World::default();
        world.set_block(IVec3::new(0, 40, 0), Block::Stone);
        world.set_block(IVec3::new(96, 2, 0), Block::Stone);
        let horizon = Horizon::build(Vec3::new(0.0, 10.0, 0.0), &world);

        assert!(!horizon.occludes(ChunkCoord::new(3, 0, 0)));
    }

    #[test]
    fn disabled_never_culls() {
        let horizon = Horizon::disabled();
        assert!(!horizon.occludes(ChunkCoord::new(0, 0, 0)));
    }
}

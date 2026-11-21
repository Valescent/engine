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

use std::collections::{HashMap, HashSet};

use glam::{Vec2, Vec3};

use crate::section::{SectionBounds, SectionKey};
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
    /// Conservative occluder and candidate tops for uploaded sections.
    sections: HashMap<SectionKey, SectionBounds>,
    /// Chunks hidden by the final bins, computed once per build.
    occluded_chunks: HashSet<ChunkCoord>,
    /// Sections hidden by the final bins, computed once per build.
    occluded_sections: HashSet<SectionKey>,
}

impl Horizon {
    /// A horizon that never culls, for the disabled path.
    pub(crate) fn disabled() -> Self {
        Self::default()
    }

    /// Builds the horizon for one camera position.
    pub(crate) fn build(camera: Vec3, world: &mut World) -> Self {
        let mut horizon = Self::empty(camera);
        horizon.add_chunks(world);
        horizon.finish();
        horizon.cache_chunk_occlusion();
        horizon
    }

    /// Builds the horizon from the near chunks and the hierarchy's sections.
    pub(crate) fn build_with_sections(
        camera: Vec3,
        world: &mut World,
        sections: &HashMap<SectionKey, SectionBounds>,
    ) -> Self {
        let mut horizon = Self::empty(camera);
        horizon.add_chunks(world);
        horizon.sections.clone_from(sections);
        for (key, bounds) in sections {
            horizon.add_section(*key, *bounds);
        }
        horizon.finish();
        horizon.cache_chunk_occlusion();
        horizon.cache_section_occlusion();
        horizon
    }

    fn empty(camera: Vec3) -> Self {
        Self {
            camera,
            bins: (0..BINS).map(|_| Vec::new()).collect(),
            tops: HashMap::new(),
            sections: HashMap::new(),
            occluded_chunks: HashSet::new(),
            occluded_sections: HashSet::new(),
        }
    }

    fn add_chunks(&mut self, world: &mut World) {
        world.refresh_terrain_min();
        for (chunk, data) in world.iter_chunks() {
            let visible = data.terrain_top().max(data.water_top());
            if visible < 0 {
                continue;
            }
            let origin = chunk.origin();
            let top = origin.y + visible as f32 + 1.0;
            self.tops.insert(chunk, top);

            let occluder = data.terrain_min();
            if occluder < 0 {
                continue;
            }
            let occluder_top = origin.y + occluder as f32 + 1.0;
            let Some((distance, azimuth)) = sample(self.camera, origin) else {
                continue;
            };
            let elevation = (occluder_top - self.camera.y).atan2(distance.max(1.0));
            let half_angle = (f32::from(CHUNK_SIZE as u16) * 0.5).atan2(distance.max(1.0));
            push_occluder(&mut self.bins, distance, azimuth, elevation, half_angle);
        }
    }

    fn add_section(&mut self, key: SectionKey, bounds: SectionBounds) {
        if !bounds.min_top.is_finite() {
            return;
        }
        let Some((nearest, farthest, azimuth, half_angle)) = section_frame(self.camera, key) else {
            return;
        };
        if nearest < MIN_DISTANCE {
            return;
        }
        let elevation = (bounds.min_top - self.camera.y).atan2(farthest.max(1.0));
        push_occluder(&mut self.bins, farthest, azimuth, elevation, half_angle);
    }

    fn finish(&mut self) {
        for bin in &mut self.bins {
            bin.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut maximum = f32::NEG_INFINITY;
            for entry in bin {
                maximum = maximum.max(entry.1);
                entry.1 = maximum;
            }
        }
    }

    /// Caches the chunks hidden by the finished bins.
    fn cache_chunk_occlusion(&mut self) {
        let occluded: Vec<ChunkCoord> = self
            .tops
            .keys()
            .copied()
            .filter(|&chunk| self.chunk_occluded(chunk))
            .collect();
        self.occluded_chunks.extend(occluded);
    }

    /// Caches the sections hidden by the finished bins.
    fn cache_section_occlusion(&mut self) {
        let occluded: Vec<SectionKey> = self
            .sections
            .keys()
            .copied()
            .filter(|&key| self.section_occluded(key))
            .collect();
        self.occluded_sections.extend(occluded);
    }

    /// Returns whether nearer terrain hides this chunk's top.
    pub(crate) fn occludes(&self, chunk: ChunkCoord) -> bool {
        self.occluded_chunks.contains(&chunk)
    }

    /// The per-build test behind [`Self::occludes`].
    fn chunk_occluded(&self, chunk: ChunkCoord) -> bool {
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

    /// Returns whether nearer terrain hides this section's highest surface.
    ///
    /// The candidate uses its nearest point and highest top, so it is culled
    /// only when even that top is below the horizon across the whole footprint.
    pub(crate) fn occludes_section(&self, key: SectionKey) -> bool {
        self.occluded_sections.contains(&key)
    }

    /// The per-build test behind [`Self::occludes_section`].
    fn section_occluded(&self, key: SectionKey) -> bool {
        if self.bins.is_empty() {
            return false;
        }
        let Some(bounds) = self.sections.get(&key) else {
            return false;
        };
        if !bounds.max_top.is_finite() {
            return false;
        }
        let Some((nearest, _, azimuth, half_angle)) = section_frame(self.camera, key) else {
            return false;
        };
        if nearest < MIN_DISTANCE {
            return false;
        }
        let elevation = (bounds.max_top - self.camera.y).atan2(nearest.max(1.0));
        let limit = nearest - DISTANCE_MARGIN;
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

/// Adds one occluder to every azimuth bin its angular span covers.
fn push_occluder(
    bins: &mut [Vec<(f32, f32)>],
    distance: f32,
    azimuth: f32,
    elevation: f32,
    half_angle: f32,
) {
    let center = bin_of(azimuth);
    let span = (half_angle / TAU * BINS as f32).ceil() as isize;
    if span >= BINS as isize / 2 {
        for bin in bins {
            bin.push((distance, elevation));
        }
    } else {
        for offset in -span..=span {
            let bin = (center as isize + offset).rem_euclid(BINS as isize) as usize;
            bins[bin].push((distance, elevation));
        }
    }
}

/// Nearest and farthest horizontal distances, centre azimuth, and half-angle of
/// one section footprint.
fn section_frame(camera: Vec3, key: SectionKey) -> Option<(f32, f32, f32, f32)> {
    let origin = key.origin();
    let extent = key.extent() as f32;
    let min = Vec2::new(origin.x as f32, origin.z as f32);
    let max = min + Vec2::splat(extent);
    let camera_xz = Vec2::new(camera.x, camera.z);
    let nearest_point = camera_xz.clamp(min, max);
    let nearest = (nearest_point - camera_xz).length();
    if nearest < 1.0 {
        return None;
    }
    let corners = [min, Vec2::new(min.x, max.y), max, Vec2::new(max.x, min.y)];
    let center = (min + max) * 0.5 - camera_xz;
    let azimuth = center.y.atan2(center.x);
    let mut farthest = 0.0_f32;
    let mut half_angle = 0.0_f32;
    for corner in corners {
        let delta = corner - camera_xz;
        farthest = farthest.max(delta.length());
        let mut offset = delta.y.atan2(delta.x) - azimuth;
        while offset > std::f32::consts::PI {
            offset -= TAU;
        }
        while offset < -std::f32::consts::PI {
            offset += TAU;
        }
        half_angle = half_angle.max(offset.abs());
    }
    Some((nearest, farthest, azimuth, half_angle))
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
        for x in 64..96 {
            for z in 0..32 {
                world.set_block(IVec3::new(x, 40, z), Block::Stone);
            }
        }
    }

    #[test]
    fn nearer_terrain_hides_a_lower_chunk() {
        let mut world = World::default();
        wall(&mut world);
        world.set_block(IVec3::new(320, 2, 0), Block::Stone);
        let horizon = Horizon::build(Vec3::new(0.0, 10.0, 0.0), &mut world);

        assert!(horizon.occludes(ChunkCoord::new(10, 0, 0)));
    }

    #[test]
    fn an_empty_direction_keeps_the_chunk() {
        let mut world = World::default();
        world.set_block(IVec3::new(320, 2, 0), Block::Stone);
        let horizon = Horizon::build(Vec3::new(0.0, 10.0, 0.0), &mut world);

        assert!(!horizon.occludes(ChunkCoord::new(10, 0, 0)));
    }

    #[test]
    fn near_chunks_are_never_culled() {
        let mut world = World::default();
        world.set_block(IVec3::new(0, 40, 0), Block::Stone);
        world.set_block(IVec3::new(96, 2, 0), Block::Stone);
        let horizon = Horizon::build(Vec3::new(0.0, 10.0, 0.0), &mut world);

        assert!(!horizon.occludes(ChunkCoord::new(3, 0, 0)));
    }

    #[test]
    fn water_keeps_a_chunk_visible_above_a_hill_that_hides_its_floor() {
        let camera = Vec3::new(0.0, 10.0, 0.0);
        let target = ChunkCoord::new(10, 0, 0);
        let mut hidden = World::default();
        for x in 64..96 {
            for z in 0..32 {
                hidden.set_block(IVec3::new(x, 10, z), Block::Stone);
            }
        }
        hidden.set_block(IVec3::new(320, 2, 0), Block::Stone);
        assert!(Horizon::build(camera, &mut hidden).occludes(target));

        let mut visible = hidden;
        visible.set_block(IVec3::new(320, 30, 0), Block::Water);
        assert!(!Horizon::build(camera, &mut visible).occludes(target));
    }

    #[test]
    fn a_low_column_keeps_the_occluder_conservative() {
        let mut world = World::default();
        for x in 64..96 {
            for z in 0..32 {
                world.set_block(IVec3::new(x, 40, z), Block::Stone);
            }
        }
        for y in 0..=10 {
            world.set_block(IVec3::new(70, y, 10), Block::Stone);
        }
        world.set_block(IVec3::new(70, 40, 10), Block::Air);
        world.set_block(IVec3::new(320, 30, 0), Block::Stone);

        let horizon = Horizon::build(Vec3::new(0.0, 10.0, 0.0), &mut world);

        assert!(!horizon.occludes(ChunkCoord::new(10, 0, 0)));
    }

    #[test]
    fn trees_do_not_count_as_horizon_walls() {
        let mut world = World::default();
        world.set_block(IVec3::new(64, 40, 0), Block::Wood);
        world.set_block(IVec3::new(64, 41, 0), Block::Leaves);
        world.set_block(IVec3::new(320, 2, 0), Block::Stone);
        let horizon = Horizon::build(Vec3::new(0.0, 10.0, 0.0), &mut world);

        assert!(!horizon.occludes(ChunkCoord::new(10, 0, 0)));
    }

    fn section_bounds(min_top: f32, max_top: f32) -> SectionBounds {
        SectionBounds { min_top, max_top }
    }

    #[test]
    fn a_near_section_hides_a_low_far_section() {
        let mut world = World::default();
        let occluder = SectionKey::new(2, IVec3::new(5, 0, 0));
        let candidate = SectionKey::new(2, IVec3::new(10, 0, 0));
        let mut sections = HashMap::new();
        sections.insert(occluder, section_bounds(100.0, 120.0));
        sections.insert(candidate, section_bounds(10.0, 20.0));
        let horizon =
            Horizon::build_with_sections(Vec3::new(0.0, 10.0, 0.0), &mut world, &sections);

        assert!(horizon.occludes_section(candidate));
    }

    #[test]
    fn a_clear_direction_keeps_a_far_section() {
        let mut world = World::default();
        let occluder = SectionKey::new(2, IVec3::new(5, 0, 0));
        let candidate = SectionKey::new(2, IVec3::new(10, 0, 8));
        let mut sections = HashMap::new();
        sections.insert(occluder, section_bounds(100.0, 120.0));
        sections.insert(candidate, section_bounds(10.0, 20.0));
        let horizon =
            Horizon::build_with_sections(Vec3::new(0.0, 10.0, 0.0), &mut world, &sections);

        assert!(!horizon.occludes_section(candidate));
    }

    #[test]
    fn sections_without_bounds_and_near_sections_are_kept() {
        let mut world = World::default();
        let candidate = SectionKey::new(2, IVec3::new(0, 0, 0));
        let mut sections = HashMap::new();
        sections.insert(candidate, section_bounds(100.0, 120.0));
        let horizon =
            Horizon::build_with_sections(Vec3::new(0.0, 10.0, 0.0), &mut world, &sections);

        assert!(!horizon.occludes_section(candidate));
        assert!(!horizon.occludes_section(SectionKey::new(2, IVec3::new(10, 0, 0))));
    }

    #[test]
    fn the_chunk_builder_keeps_sections_visible() {
        let mut world = World::default();
        let candidate = SectionKey::new(2, IVec3::new(10, 0, 0));
        let horizon = Horizon::build(Vec3::new(0.0, 10.0, 0.0), &mut world);

        assert!(!horizon.occludes_section(candidate));
    }

    #[test]
    fn cached_occlusion_matches_a_direct_test() {
        let mut world = World::default();
        wall(&mut world);
        world.set_block(IVec3::new(320, 2, 0), Block::Stone);
        let horizon = Horizon::build(Vec3::new(0.0, 10.0, 0.0), &mut world);

        let hidden = ChunkCoord::new(10, 0, 0);
        assert!(horizon.occludes(hidden));
        assert_eq!(horizon.occludes(hidden), horizon.chunk_occluded(hidden));

        let mut clear_world = World::default();
        clear_world.set_block(IVec3::new(320, 2, 0), Block::Stone);
        let clear = Horizon::build(Vec3::new(0.0, 10.0, 0.0), &mut clear_world);
        let visible = ChunkCoord::new(10, 0, 0);
        assert!(!clear.occludes(visible));
        assert_eq!(clear.occludes(visible), clear.chunk_occluded(visible));

        let occluder = SectionKey::new(2, IVec3::new(5, 0, 0));
        let hidden_key = SectionKey::new(2, IVec3::new(10, 0, 0));
        let clear_key = SectionKey::new(2, IVec3::new(10, 0, 8));
        let mut sections = HashMap::new();
        sections.insert(occluder, section_bounds(100.0, 120.0));
        sections.insert(hidden_key, section_bounds(10.0, 20.0));
        sections.insert(clear_key, section_bounds(10.0, 20.0));
        let horizon =
            Horizon::build_with_sections(Vec3::new(0.0, 10.0, 0.0), &mut world, &sections);

        assert!(horizon.occludes_section(hidden_key));
        assert!(!horizon.occludes_section(clear_key));
        assert_eq!(
            horizon.occludes_section(hidden_key),
            horizon.section_occluded(hidden_key)
        );
        assert_eq!(
            horizon.occludes_section(clear_key),
            horizon.section_occluded(clear_key)
        );
    }

    #[test]
    fn disabled_never_culls() {
        let horizon = Horizon::disabled();
        assert!(!horizon.occludes(ChunkCoord::new(0, 0, 0)));
        assert!(!horizon.occludes_section(SectionKey::new(2, IVec3::new(10, 0, 0))));
    }
}

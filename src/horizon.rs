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
            // The candidate includes every solid block (canopies too) and
            // visible water, but the occluder is terrain-only so neither an
            // ocean column nor a tree can hide land behind it.
            let visible = data.top().max(data.water_top());
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
            let Some((distance, azimuth, half_angle)) = sample(self.camera, origin) else {
                continue;
            };
            let elevation = (occluder_top - self.camera.y).atan2(distance.max(1.0));
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
        let Some((distance, azimuth, half_angle)) = sample(self.camera, origin) else {
            return false;
        };
        if distance < MIN_DISTANCE {
            return false;
        }
        let elevation = (top - self.camera.y).atan2(distance.max(1.0));
        let limit = distance - DISTANCE_MARGIN;
        let Some(horizon) = self.horizon_at(azimuth, half_angle, limit) else {
            return false;
        };

        elevation + ANGLE_MARGIN < horizon
    }

    /// Lowest prefix-maximum overhead across every bin the interval touches.
    ///
    /// Returns `None` when any touched bin has no occluder nearer than `limit`.
    fn horizon_at(&self, azimuth: f32, half_angle: f32, limit: f32) -> Option<f32> {
        let bin_width = TAU / BINS as f32;
        let mut horizon = f32::INFINITY;
        let mut touched = false;
        for (index, entries) in self.bins.iter().enumerate() {
            let delta = wrap_angle(bin_azimuth(index) - azimuth);
            if delta.abs() - bin_width * 0.5 >= half_angle {
                continue;
            }
            touched = true;
            let count = entries.partition_point(|(d, _)| *d < limit);
            if count == 0 {
                return None;
            }
            horizon = horizon.min(entries[count - 1].1);
        }
        touched.then_some(horizon)
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
        let Some(horizon) = self.horizon_at(azimuth, half_angle, limit) else {
            return false;
        };

        elevation + ANGLE_MARGIN < horizon
    }
}

/// Adds one occluder to the bins its angular span covers.
///
/// A bin receives the occluder only when the occluder interval contains the
/// bin's whole angular range. Occluders whose half-angle reaches pi cover the
/// whole circle and therefore every bin. This keeps a narrow footprint from
/// vouching for a partially overlapping bin and hiding a candidate that shares
/// it.
fn push_occluder(
    bins: &mut [Vec<(f32, f32)>],
    distance: f32,
    azimuth: f32,
    elevation: f32,
    half_angle: f32,
) {
    if half_angle >= std::f32::consts::PI {
        for bin in bins {
            bin.push((distance, elevation));
        }
        return;
    }
    let bin_width = TAU / BINS as f32;
    for (index, bin) in bins.iter_mut().enumerate() {
        let delta = wrap_angle(bin_azimuth(index) - azimuth);
        if delta.abs() + bin_width * 0.5 <= half_angle {
            bin.push((distance, elevation));
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

/// Nearest horizontal distance, centre azimuth, and half-angle of a chunk.
///
/// The cone is centred on the footprint's box centre and widened until it
/// contains all four corners, matching [`section_frame`]. Centring it on the
/// nearest point instead would mark azimuths outside the chunk as occluded.
fn sample(camera: Vec3, origin: Vec3) -> Option<(f32, f32, f32)> {
    let min = glam::Vec2::new(origin.x, origin.z);
    let max = min + glam::Vec2::splat(CHUNK_SIZE as f32);
    let camera_xz = glam::Vec2::new(camera.x, camera.z);
    let nearest_point = camera_xz.clamp(min, max);
    let distance = (nearest_point - camera_xz).length();
    if distance < 1.0 {
        return None;
    }
    let center = (min + max) * 0.5 - camera_xz;
    let azimuth = center.y.atan2(center.x);
    let corners = [min, Vec2::new(min.x, max.y), max, Vec2::new(max.x, min.y)];
    let mut half_angle = 0.0_f32;
    for corner in corners {
        let delta = corner - camera_xz;
        let offset = wrap_angle(delta.y.atan2(delta.x) - azimuth);
        half_angle = half_angle.max(offset.abs());
    }
    Some((distance, azimuth, half_angle))
}

/// Normalizes an angle to `[-pi, pi]`.
fn wrap_angle(mut angle: f32) -> f32 {
    while angle > std::f32::consts::PI {
        angle -= TAU;
    }
    while angle < -std::f32::consts::PI {
        angle += TAU;
    }
    angle
}

/// Centre azimuth of one bin's angular range.
fn bin_azimuth(bin: usize) -> f32 {
    (bin as f32 + 0.5) / BINS as f32 * TAU - std::f32::consts::PI
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
    fn water_only_columns_do_not_hide_land() {
        let camera = Vec3::new(0.0, 10.0, 0.0);
        let target = ChunkCoord::new(10, 0, 0);
        let mut world = World::default();
        for x in 64..96 {
            for z in 0..32 {
                world.set_block(IVec3::new(x, 40, z), Block::Water);
            }
        }
        world.set_block(IVec3::new(320, 2, 0), Block::Stone);

        let horizon = Horizon::build(camera, &mut world);

        assert!(
            !horizon.occludes(target),
            "an ocean wall must not occlude land behind it"
        );
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

    #[test]
    fn a_tree_canopy_keeps_a_chunk_visible_above_the_horizon() {
        let camera = Vec3::new(0.0, 10.0, 0.0);
        let target = ChunkCoord::new(10, 0, 0);
        let mut world = World::default();
        for x in 64..96 {
            for z in 0..32 {
                world.set_block(IVec3::new(x, 10, z), Block::Stone);
            }
        }
        world.set_block(IVec3::new(320, 2, 0), Block::Stone);
        assert!(Horizon::build(camera, &mut world).occludes(target));

        // The canopy is high enough to clear the nearby hill even though the
        // chunk's terrain top stays below it.
        let mut with_tree = world;
        with_tree.set_block(IVec3::new(321, 20, 1), Block::Leaves);
        assert!(!Horizon::build(camera, &mut with_tree).occludes(target));
    }

    #[test]
    fn an_offset_chunk_keeps_its_clear_direction_visible() {
        let mut world = World::default();
        wall(&mut world);
        // This chunk's nearest point points at the wall, but most of its cone
        // lies to the clear side. Centring the cone on the nearest point would
        // wrongly mark the clear azimuths as occluded.
        world.set_block(IVec3::new(320, 2, -16), Block::Stone);
        let horizon = Horizon::build(Vec3::new(0.0, 10.0, 0.0), &mut world);

        assert!(!horizon.occludes(ChunkCoord::new(10, 0, -1)));
    }

    #[test]
    fn a_narrow_occluder_does_not_claim_a_shared_bin() {
        let mut bins: Vec<Vec<(f32, f32)>> = (0..BINS).map(|_| Vec::new()).collect();
        let bin_width = TAU / BINS as f32;
        // Centred in one bin and narrower than it, so it covers none in full.
        push_occluder(&mut bins, 100.0, 0.0, 1.0, bin_width * 0.25);

        assert!(bins.iter().all(Vec::is_empty));
    }

    #[test]
    fn a_wide_occluder_fully_covers_the_bins_it_spans() {
        let mut bins: Vec<Vec<(f32, f32)>> = (0..BINS).map(|_| Vec::new()).collect();
        let bin_width = TAU / BINS as f32;
        let half_angle = bin_width * 1.5;
        push_occluder(&mut bins, 100.0, 0.0, 1.0, half_angle);

        let covered = bins.iter().filter(|bin| !bin.is_empty()).count();
        assert!(covered >= 2, "expected the two bins around azimuth zero");
        for (index, bin) in bins.iter().enumerate() {
            if !bin.is_empty() {
                let delta = wrap_angle(bin_azimuth(index));
                assert!(delta.abs() + bin_width * 0.5 <= half_angle);
            }
        }
    }

    #[test]
    fn bin_coverage_wraps_at_the_bearing_seam() {
        let mut bins: Vec<Vec<(f32, f32)>> = (0..BINS).map(|_| Vec::new()).collect();
        let bin_width = TAU / BINS as f32;
        push_occluder(&mut bins, 100.0, std::f32::consts::PI, 1.0, bin_width * 2.0);

        assert_eq!(bins[0].len(), 1);
        assert_eq!(bins[BINS - 1].len(), 1);
    }

    fn section_bounds(min_top: f32, max_top: f32) -> SectionBounds {
        SectionBounds { min_top, max_top }
    }

    #[test]
    fn a_near_section_hides_a_low_far_section() {
        let mut world = World::default();
        let occluder = SectionKey::new(4, IVec3::new(1, 0, 0));
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

        let occluder = SectionKey::new(4, IVec3::new(1, 0, 0));
        let hidden_key = SectionKey::new(2, IVec3::new(10, 0, 0));
        let clear_key = SectionKey::new(2, IVec3::new(10, 0, 24));
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

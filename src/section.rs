//! Seed-native LOD sections: coordinate math, coarse generation, and meshing.
//!
//! A section always stores `32³` samples at a spacing of `2^level` blocks.
//! Level 0 is the existing chunk; levels 1-4 cover eight times the volume per
//! step. Sections are derived rendering data and are never saved.

#![allow(dead_code)] // The renderer and streaming path consume this slice next.

use std::collections::HashMap;

use glam::IVec3;

use crate::block::Block;
use crate::generation::{Generator, SEA_LEVEL, WORLD_HEIGHT, surface_block};
use crate::light::LightField;
use crate::mesh::{
    BorderBlocks, ChunkSnapshot, MeshData, build_section_mesh, build_section_water_mesh,
};
use crate::world::{CHUNK_SIZE, Chunk, ChunkCoord};

/// Samples per section axis.
pub(crate) const SECTION_SIZE: i32 = CHUNK_SIZE;

/// Highest hierarchy level in the first slice.
pub(crate) const MAX_LEVEL: u8 = 4;

/// Full L0 chunks whose final blocks override the seed in a coarse rebuild.
pub(crate) type SectionEdits = HashMap<ChunkCoord, Chunk>;

/// Identity of one section: a level and a coordinate in that level's grid.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SectionKey {
    pub(crate) level: u8,
    pub(crate) coord: IVec3,
}

impl SectionKey {
    pub(crate) const fn new(level: u8, coord: IVec3) -> Self {
        Self { level, coord }
    }

    /// Blocks per sample on each axis.
    pub(crate) const fn scale(self) -> i32 {
        1_i32 << self.level
    }

    /// Blocks per section axis.
    pub(crate) const fn extent(self) -> i32 {
        SECTION_SIZE * self.scale()
    }

    /// Minimum-corner world coordinate.
    pub(crate) fn origin(self) -> IVec3 {
        self.coord * self.extent()
    }

    /// Parent section, one level coarser.
    pub(crate) fn parent(self) -> Option<Self> {
        (self.level < MAX_LEVEL).then(|| Self {
            level: self.level + 1,
            coord: self.coord.div_euclid(IVec3::splat(2)),
        })
    }

    /// One of eight children, one level finer.
    pub(crate) fn child(self, index: usize) -> Option<Self> {
        if self.level == 0 || index >= 8 {
            return None;
        }
        let offset = IVec3::new(
            i32::try_from(index & 1).expect("child index fits"),
            i32::try_from((index >> 1) & 1).expect("child index fits"),
            i32::try_from((index >> 2) & 1).expect("child index fits"),
        );
        Some(Self {
            level: self.level - 1,
            coord: self.coord * 2 + offset,
        })
    }

    /// World coordinate of a sample.
    pub(crate) fn sample_origin(self, local: IVec3) -> IVec3 {
        self.origin() + local * self.scale()
    }
}

/// One generated `32³` sample grid.
#[derive(Debug)]
pub(crate) struct Section {
    key: SectionKey,
    samples: Chunk,
}

impl Section {
    pub(crate) fn new(key: SectionKey) -> Self {
        Self {
            key,
            samples: Chunk::default(),
        }
    }

    pub(crate) fn key(&self) -> SectionKey {
        self.key
    }

    pub(crate) fn sample(&self, local: IVec3) -> Block {
        self.samples.block(local)
    }

    pub(crate) fn set_sample(&mut self, local: IVec3, block: Block) {
        self.samples.set_block(local, block);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// A snapshot of the sample grid for the mesher.
    pub(crate) fn snapshot(&self) -> ChunkSnapshot {
        ChunkSnapshot::from_chunk(&self.samples)
    }

    /// Per-column coarse sky light: `15` above the top sample, `0` below.
    pub(crate) fn light(&self) -> LightField {
        let cells = usize::try_from(SECTION_SIZE * SECTION_SIZE).expect("section columns fit");
        let mut tops = vec![-1_i32; cells];
        let stride = SECTION_SIZE;
        for x in 0..SECTION_SIZE {
            for z in 0..SECTION_SIZE {
                for y in (0..SECTION_SIZE).rev() {
                    if self.samples.block(IVec3::new(x, y, z)) != Block::Air {
                        tops[usize::try_from(x * stride + z).expect("column fits")] = y;
                        break;
                    }
                }
            }
        }
        LightField::coarse(|x, z| {
            let x = x.clamp(0, SECTION_SIZE - 1);
            let z = z.clamp(0, SECTION_SIZE - 1);
            tops[usize::try_from(x * stride + z).expect("column fits")]
        })
    }

    /// Meshes the sample grid at the section's spacing.
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn mesh(&self, light: Option<&LightField>) -> MeshData {
        let snapshot = self.snapshot();
        build_section_mesh(
            &snapshot,
            &BorderBlocks::default(),
            light,
            self.key.scale() as f32,
        )
    }

    /// Meshes the water samples at the section's spacing.
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn water_mesh(&self, light: Option<&LightField>) -> MeshData {
        let snapshot = self.snapshot();
        build_section_water_mesh(
            &snapshot,
            &BorderBlocks::default(),
            light,
            self.key.scale() as f32,
        )
    }
}

/// Generates one section from the seed and the modified-chunk overlay.
pub(crate) fn generate_section(
    generator: &Generator,
    key: SectionKey,
    edits: &SectionEdits,
) -> Section {
    let scale = key.scale();
    let origin = key.origin();
    let probes = probe_offsets(scale);
    let sea_sample = (SEA_LEVEL - origin.y).div_euclid(scale);
    let mut section = Section::new(key);

    for sx in 0..SECTION_SIZE {
        for sz in 0..SECTION_SIZE {
            let (top_height, rep_x, rep_z) =
                footprint_top(generator, origin, scale, sx, sz, &probes);
            let biome = generator.biome(rep_x, rep_z);
            let top_sample = (top_height - origin.y).div_euclid(scale);
            for sy in 0..SECTION_SIZE {
                let base = origin + IVec3::new(sx, sy, sz) * scale;
                let block = if sy <= top_sample {
                    surface_block(biome, top_sample - sy)
                } else if sy <= sea_sample {
                    Block::Water
                } else {
                    Block::Air
                };
                let block = if edits.is_empty() || !overlaps_edits(edits, base, scale) {
                    block
                } else {
                    edited_sample(generator, edits, base, scale)
                };
                if block != Block::Air {
                    section.set_sample(IVec3::new(sx, sy, sz), block);
                }
            }
        }
    }

    section
}

/// One L0 block from the coarse seed rule: no caves and no trees.
pub(crate) fn seed_block(generator: &Generator, world: IVec3) -> Block {
    if !(0..WORLD_HEIGHT).contains(&world.y) {
        return Block::Air;
    }
    let height = generator.height(world.x, world.z);
    if world.y <= height {
        surface_block(generator.biome(world.x, world.z), height - world.y)
    } else if world.y <= SEA_LEVEL {
        Block::Water
    } else {
        Block::Air
    }
}

fn probe_offsets(scale: i32) -> Vec<i32> {
    if scale <= 4 {
        (0..scale).collect()
    } else {
        vec![0, scale / 2, scale - 1]
    }
}

fn footprint_top(
    generator: &Generator,
    origin: IVec3,
    scale: i32,
    sx: i32,
    sz: i32,
    probes: &[i32],
) -> (i32, i32, i32) {
    let mut top = i32::MIN;
    let mut rep = (origin.x + sx * scale, origin.z + sz * scale);
    for &dx in probes {
        for &dz in probes {
            let x = origin.x + sx * scale + dx;
            let z = origin.z + sz * scale + dz;
            let height = generator.height(x, z);
            if height > top || (height == top && (x, z) < rep) {
                top = height;
                rep = (x, z);
            }
        }
    }
    (top, rep.0, rep.1)
}

fn overlaps_edits(edits: &SectionEdits, base: IVec3, scale: i32) -> bool {
    let max = base + IVec3::splat(scale - 1);
    let min_chunk = base.div_euclid(IVec3::splat(CHUNK_SIZE));
    let max_chunk = max.div_euclid(IVec3::splat(CHUNK_SIZE));
    for x in min_chunk.x..=max_chunk.x {
        for y in min_chunk.y..=max_chunk.y {
            for z in min_chunk.z..=max_chunk.z {
                if edits.contains_key(&ChunkCoord::new(x, y, z)) {
                    return true;
                }
            }
        }
    }
    false
}

fn edited_sample(generator: &Generator, edits: &SectionEdits, base: IVec3, scale: i32) -> Block {
    let mut solid = None;
    let mut water = false;
    for dy in 0..scale {
        for dx in 0..scale {
            for dz in 0..scale {
                let world = base + IVec3::new(dx, dy, dz);
                let (chunk, local) = ChunkCoord::from_world(world);
                let block = edits
                    .get(&chunk)
                    .map_or_else(|| seed_block(generator, world), |data| data.block(local));
                if block.is_solid() {
                    solid = Some(block);
                } else if block == Block::Water {
                    water = true;
                }
            }
        }
    }
    solid.unwrap_or(if water { Block::Water } else { Block::Air })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk_at(world: IVec3) -> ChunkCoord {
        ChunkCoord::from_world(world).0
    }

    #[test]
    fn level_math_round_trips_parent_and_children() {
        let root = SectionKey::new(2, IVec3::new(-3, 1, 5));
        assert_eq!(root.scale(), 4);
        assert_eq!(root.extent(), 128);
        assert_eq!(root.origin(), IVec3::new(-384, 128, 640));
        assert_eq!(
            root.parent(),
            Some(SectionKey::new(3, IVec3::new(-2, 0, 2)))
        );
        assert_eq!(SectionKey::new(MAX_LEVEL, IVec3::ZERO).parent(), None);

        for index in 0..8 {
            let child = root.child(index).expect("child");
            assert_eq!(child.level, 1);
            assert_eq!(child.parent(), Some(root), "child {index}");
        }
        assert_eq!(root.child(8), None);
        assert_eq!(SectionKey::new(0, IVec3::ZERO).child(0), None);
    }

    #[test]
    fn level1_section_matches_the_exhaustive_seed_rule() {
        let generator = Generator::new(42);
        let key = SectionKey::new(1, IVec3::new(0, 1, 0));
        let section = generate_section(&generator, key, &SectionEdits::new());
        let scale = key.scale();

        for sx in (0..SECTION_SIZE).step_by(8) {
            for sz in (0..SECTION_SIZE).step_by(8) {
                for sy in 0..SECTION_SIZE {
                    let base = key.origin() + IVec3::new(sx, sy, sz) * scale;
                    let expected = exhaustive_seed_sample(&generator, base, scale);
                    assert_eq!(
                        section.sample(IVec3::new(sx, sy, sz)),
                        expected,
                        "sample {sx},{sy},{sz}"
                    );
                }
            }
        }
    }

    fn exhaustive_seed_sample(generator: &Generator, base: IVec3, scale: i32) -> Block {
        let mut solid = None;
        let mut water = false;
        for dy in 0..scale {
            for dx in 0..scale {
                for dz in 0..scale {
                    let block = seed_block(generator, base + IVec3::new(dx, dy, dz));
                    if block.is_solid() {
                        solid = Some(block);
                    } else if block == Block::Water {
                        water = true;
                    }
                }
            }
        }
        solid.unwrap_or(if water { Block::Water } else { Block::Air })
    }

    #[test]
    fn water_fills_samples_between_the_solid_top_and_sea_level() {
        let generator = Generator::new(7);
        let sea_sample = SEA_LEVEL.div_euclid(2);
        let mut checked = false;

        'sections: for cx in -16..=16 {
            for cz in -16..=16 {
                let key = SectionKey::new(1, IVec3::new(cx, 0, cz));
                let section = generate_section(&generator, key, &SectionEdits::new());
                for sx in 0..SECTION_SIZE {
                    for sz in 0..SECTION_SIZE {
                        let top = (0..SECTION_SIZE)
                            .rev()
                            .find(|&sy| section.sample(IVec3::new(sx, sy, sz)).is_solid());
                        if let Some(top) = top
                            && top < sea_sample
                            && section.sample(IVec3::new(sx, sea_sample, sz)) == Block::Water
                        {
                            assert_eq!(
                                section.sample(IVec3::new(sx, sea_sample + 1, sz)),
                                Block::Air
                            );
                            checked = true;
                            break 'sections;
                        }
                    }
                }
            }
        }
        assert!(checked, "a below-sea column");
    }

    #[test]
    fn edits_raise_and_clear_a_sample() {
        let generator = Generator::new(42);
        let key = SectionKey::new(1, IVec3::new(0, 1, 0));
        let world = IVec3::new(1, 70, 1);
        let chunk = chunk_at(world);
        let (_, local) = ChunkCoord::from_world(world);
        let sample = IVec3::new(
            (world.x - key.origin().x).div_euclid(2),
            (world.y - key.origin().y).div_euclid(2),
            (world.z - key.origin().z).div_euclid(2),
        );

        let mut edited = Chunk::default();
        edited.set_block(local, Block::Stone);
        let mut edits = SectionEdits::new();
        edits.insert(chunk, edited);
        let section = generate_section(&generator, key, &edits);
        assert_eq!(section.sample(sample), Block::Stone);

        edits.insert(chunk, Chunk::default());
        let cleared = generate_section(&generator, key, &edits);
        assert_eq!(cleared.sample(sample), Block::Air);
    }

    #[test]
    fn coarse_light_is_open_above_the_top_and_dark_below() {
        let generator = Generator::new(42);
        let key = SectionKey::new(1, IVec3::new(0, 0, 0));
        let section = generate_section(&generator, key, &SectionEdits::new());
        let light = section.light();

        let mut checked = false;
        for x in 0..SECTION_SIZE {
            for z in 0..SECTION_SIZE {
                let top = (0..SECTION_SIZE)
                    .rev()
                    .find(|&y| section.sample(IVec3::new(x, y, z)) != Block::Air);
                if let Some(top) = top
                    && top + 1 < SECTION_SIZE
                {
                    assert_eq!(light.sky(IVec3::new(x, top, z)), 0);
                    assert_eq!(light.sky(IVec3::new(x, top + 1, z)), 15);
                    checked = true;
                }
            }
        }
        assert!(checked, "a column with a top");
    }

    #[test]
    fn a_generated_section_meshes_at_its_spacing() {
        let generator = Generator::new(42);
        let key = SectionKey::new(1, IVec3::new(0, 0, 0));
        let section = generate_section(&generator, key, &SectionEdits::new());
        let light = section.light();
        let mesh = section.mesh(Some(&light));

        assert_ne!(mesh.quads.len(), 0);
        assert_eq!(mesh.quads.len() * 4, mesh.vertices.len());
        assert_eq!(mesh.quads.len() * 6, mesh.indices.len());
    }

    #[test]
    fn overlapping_edits_are_detected_without_touching_clean_sections() {
        let generator = Generator::new(42);
        let key = SectionKey::new(1, IVec3::new(0, 0, 0));
        let mut edits = SectionEdits::new();
        edits.insert(ChunkCoord::new(0, 0, 0), Chunk::default());
        let section = generate_section(&generator, key, &edits);

        let mut clean = SectionEdits::new();
        clean.insert(ChunkCoord::new(50, 50, 50), Chunk::default());
        let other = generate_section(&generator, key, &clean);

        assert_eq!(section.sample(IVec3::new(0, 0, 0)), Block::Air);
        assert_eq!(
            section.sample(IVec3::new(16, 16, 16)),
            other.sample(IVec3::new(16, 16, 16))
        );
    }
}

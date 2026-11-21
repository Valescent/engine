//! Deterministic world generation.
//!
//! Every field is a function of the world seed, a field tag, and world block
//! coordinates. There is no global RNG and no dependency on chunk order.

use fastnoise_lite::{FastNoiseLite, FractalType, NoiseType};
use glam::IVec3;

use crate::block::Block;
use crate::world::{CHUNK_COLUMN, CHUNK_SIZE, Chunk, ChunkCoord};

/// Blocks per world column axis.
pub(crate) const WORLD_HEIGHT: i32 = CHUNK_SIZE * CHUNK_COLUMN;

/// Water fills columns up to this height.
pub(crate) const SEA_LEVEL: i32 = 32;

/// Lowest generated terrain height.
const MIN_HEIGHT: i32 = 1;

/// Highest generated terrain height.
const MAX_HEIGHT: i32 = 126;

const CONTINENTALNESS_TAG: u64 = 0x01;
const EROSION_TAG: u64 = 0x02;
const TEMPERATURE_TAG: u64 = 0x03;
const HUMIDITY_TAG: u64 = 0x04;
const CAVE_TAG: u64 = 0x05;
const TREE_TAG: u64 = 0x06;

/// Shortest tree trunk.
const TRUNK_MIN: i32 = 4;

/// `SplitMix64`, the seed mixer pinned by ADR-0012.
pub(crate) fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = value;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Per-field seed: the lower 32 bits of the mixed world seed and field tag.
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn field_seed(seed: u64, tag: u64) -> i32 {
    (splitmix64(seed ^ tag) as u32).cast_signed()
}

/// Cave threshold; solid cells above this noise value are carved to air.
const CAVE_THRESHOLD: f32 = 0.60;

/// Surface family selected from temperature, humidity, and erosion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Biome {
    Plains,
    Hills,
    Desert,
}

/// One generation job: the seed and a chunk column.
#[derive(Debug)]
pub(crate) struct GenJob {
    pub(crate) column: glam::IVec2,
    pub(crate) seed: u64,
}

/// A generated column: four chunk block maps.
#[derive(Debug)]
pub(crate) struct GenResult {
    pub(crate) column: glam::IVec2,
    pub(crate) chunks: Vec<(ChunkCoord, Chunk)>,
    pub(crate) elapsed: std::time::Duration,
}

/// One world's noise fields and derived terrain.
pub(crate) struct Generator {
    seed: u64,
    continentalness: FastNoiseLite,
    erosion: FastNoiseLite,
    temperature: FastNoiseLite,
    humidity: FastNoiseLite,
    cave: FastNoiseLite,
}

impl Generator {
    pub(crate) fn new(seed: u64) -> Self {
        Self {
            seed,
            continentalness: field(seed, CONTINENTALNESS_TAG, 0.0015, 4),
            erosion: field(seed, EROSION_TAG, 0.0030, 4),
            temperature: field(seed, TEMPERATURE_TAG, 0.0010, 3),
            humidity: field(seed, HUMIDITY_TAG, 0.0012, 3),
            cave: field(seed, CAVE_TAG, 0.0120, 3),
        }
    }

    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn continentalness(&self, x: i32, z: i32) -> f32 {
        normalize(self.continentalness.get_noise_2d(x as f32, z as f32))
    }

    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn erosion(&self, x: i32, z: i32) -> f32 {
        normalize(self.erosion.get_noise_2d(x as f32, z as f32))
    }

    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn temperature(&self, x: i32, z: i32) -> f32 {
        normalize(self.temperature.get_noise_2d(x as f32, z as f32))
    }

    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn humidity(&self, x: i32, z: i32) -> f32 {
        normalize(self.humidity.get_noise_2d(x as f32, z as f32))
    }

    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn cave(&self, x: i32, y: i32, z: i32) -> f32 {
        normalize(self.cave.get_noise_3d(x as f32, y as f32, z as f32))
    }

    /// Biome selected by the spec's priority order.
    pub(crate) fn biome(&self, x: i32, z: i32) -> Biome {
        if self.temperature(x, z) >= 0.65 && self.humidity(x, z) < 0.40 {
            Biome::Desert
        } else if self.erosion(x, z) <= 0.40 {
            Biome::Hills
        } else {
            Biome::Plains
        }
    }

    /// Generates one chunk's blocks, including caves and water.
    pub(crate) fn generate_chunk(&self, chunk: ChunkCoord) -> Chunk {
        let origin = chunk.world_origin();
        let mut blocks = Chunk::default();
        let mut column_tops = [u8::MAX; (CHUNK_SIZE * CHUNK_SIZE) as usize];
        let first = origin.y.max(0);
        let last = (origin.y + CHUNK_SIZE - 1).min(WORLD_HEIGHT - 1);
        if first > last {
            return blocks;
        }

        for lx in 0..CHUNK_SIZE {
            for lz in 0..CHUNK_SIZE {
                let x = origin.x + lx;
                let z = origin.z + lz;
                let height = self.height(x, z);
                let biome = self.biome(x, z);

                for world_y in first..=last {
                    let block = if world_y <= height {
                        let surface = surface_block(biome, height - world_y);
                        if world_y >= 1 && self.cave(x, world_y, z) > CAVE_THRESHOLD {
                            Block::Air
                        } else {
                            surface
                        }
                    } else if world_y <= SEA_LEVEL {
                        Block::Water
                    } else {
                        Block::Air
                    };

                    if block.is_terrain() {
                        let local_y = u8::try_from(world_y - origin.y).expect("local y fits in u8");
                        let index = usize::try_from(lx * CHUNK_SIZE + lz)
                            .expect("column index fits in usize");
                        column_tops[index] = local_y;
                    }
                    blocks.set_block(IVec3::new(lx, world_y - origin.y, lz), block);
                }
            }
        }

        let first_candidate = self.tree_hash(chunk, 0);
        for index in 0..2u64 {
            let hash = if index == 0 {
                first_candidate
            } else {
                self.tree_hash(chunk, 1)
            };
            let local_x = (hash & 31) as i32;
            let local_z = ((hash >> 8) & 31) as i32;
            if index == 1 {
                let previous_x = (first_candidate & 31) as i32;
                let previous_z = ((first_candidate >> 8) & 31) as i32;
                if local_x == previous_x && local_z == previous_z {
                    continue;
                }
            }
            let trunk = TRUNK_MIN + ((hash >> 16) % 3) as i32;
            self.plant_tree(&mut blocks, chunk, IVec3::new(local_x, 0, local_z), trunk);
        }

        let mut minimum = i32::MAX;
        for top in column_tops {
            if top == u8::MAX {
                minimum = -1;
                break;
            }
            minimum = minimum.min(i32::from(top));
        }
        blocks.set_terrain_min(minimum);
        blocks
    }

    /// Generates the four chunks of one vertical column.
    pub(crate) fn generate_column(&self, column: glam::IVec2) -> Vec<(ChunkCoord, Chunk)> {
        (0..CHUNK_COLUMN)
            .map(|y| {
                let chunk = ChunkCoord::new(column.x, y, column.y);
                (chunk, self.generate_chunk(chunk))
            })
            .collect()
    }

    /// `SplitMix64` hash for one tree candidate in a chunk.
    #[allow(clippy::cast_sign_loss)]
    pub(crate) fn tree_hash(&self, chunk: ChunkCoord, index: u64) -> u64 {
        let coordinates = chunk.coordinates();
        splitmix64(
            self.seed
                ^ TREE_TAG
                ^ (coordinates.x as u64)
                ^ ((coordinates.z as u64) << 32)
                ^ index.wrapping_mul(0x9E37_79B9_7F4A_7C15),
        )
    }

    /// Places one tree when the column and canopy volume allow it.
    #[allow(clippy::cast_possible_truncation)]
    fn plant_tree(&self, blocks: &mut Chunk, chunk: ChunkCoord, local: IVec3, trunk: i32) {
        let origin = chunk.world_origin();
        let biome = self.biome(origin.x + local.x, origin.z + local.z);
        if !matches!(biome, Biome::Plains | Biome::Hills) {
            return;
        }
        if !(1..=CHUNK_SIZE - 2).contains(&local.x) || !(1..=CHUNK_SIZE - 2).contains(&local.z) {
            return;
        }

        let height = self.height(origin.x + local.x, origin.z + local.z);
        let local_height = height - origin.y;
        if blocks.block(IVec3::new(local.x, local_height, local.z)) != Block::Grass {
            return;
        }

        let top = height + trunk;
        let highest = top + 1;
        let local_top = highest - origin.y;
        if highest >= WORLD_HEIGHT || local_top >= CHUNK_SIZE {
            return;
        }

        for y in (local_height + 1)..=local_top {
            if blocks.block(IVec3::new(local.x, y, local.z)) != Block::Air {
                return;
            }
        }
        for y in [top - 1, top] {
            for dx in -1..=1 {
                for dz in -1..=1 {
                    if blocks.block(IVec3::new(local.x + dx, y - origin.y, local.z + dz))
                        != Block::Air
                    {
                        return;
                    }
                }
            }
        }
        if blocks.block(IVec3::new(local.x, highest - origin.y, local.z)) != Block::Air {
            return;
        }

        for y in [top - 1, top] {
            for dx in -1..=1 {
                for dz in -1..=1 {
                    blocks.set_block(
                        IVec3::new(local.x + dx, y - origin.y, local.z + dz),
                        Block::Leaves,
                    );
                }
            }
        }
        blocks.set_block(
            IVec3::new(local.x, highest - origin.y, local.z),
            Block::Leaves,
        );
        for y in (height + 1)..=top {
            blocks.set_block(IVec3::new(local.x, y - origin.y, local.z), Block::Wood);
        }
    }

    /// First dry spawn column scanning `+X` along `z = 0`.
    pub(crate) fn spawn_column(&self) -> (i32, i32, i32) {
        for x in 0..4096 {
            let height = self.height(x, 0);
            if height < SEA_LEVEL || self.cave(x, height, 0) > CAVE_THRESHOLD {
                continue;
            }
            return (x, 0, height);
        }
        (0, 0, SEA_LEVEL)
    }

    /// Terrain height for a world column, clamped to the buildable range.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    pub(crate) fn height(&self, x: i32, z: i32) -> i32 {
        let continentalness = self.continentalness(x, z);
        let erosion = self.erosion(x, z);
        let raw = 32.0 + (continentalness - 0.5) * 160.0 + (erosion - 0.5) * 32.0;
        (raw.round() as i32).clamp(MIN_HEIGHT, MAX_HEIGHT)
    }
}

fn field(seed: u64, tag: u64, frequency: f32, octaves: i32) -> FastNoiseLite {
    let mut noise = FastNoiseLite::with_seed(field_seed(seed, tag));
    noise.set_noise_type(Some(NoiseType::OpenSimplex2));
    noise.set_fractal_type(Some(FractalType::FBm));
    noise.set_frequency(Some(frequency));
    noise.set_fractal_octaves(Some(octaves));
    noise.set_fractal_lacunarity(Some(2.0));
    noise.set_fractal_gain(Some(0.5));
    noise
}

/// Surface block for a biome at `depth` blocks below the terrain height.
pub(crate) fn surface_block(biome: Biome, depth: i32) -> Block {
    match biome {
        Biome::Plains => {
            if depth == 0 {
                Block::Grass
            } else if depth <= 3 {
                Block::Dirt
            } else {
                Block::Stone
            }
        }
        Biome::Hills => {
            if depth == 0 {
                Block::Grass
            } else if depth == 1 {
                Block::Dirt
            } else {
                Block::Stone
            }
        }
        Biome::Desert => {
            if depth < 3 {
                Block::Sand
            } else {
                Block::Stone
            }
        }
    }
}

fn normalize(raw: f32) -> f32 {
    f32::midpoint(raw, 1.0).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splitmix64_is_deterministic_and_spreads_adjacent_inputs() {
        assert_eq!(splitmix64(0), splitmix64(0));
        assert_ne!(splitmix64(1), splitmix64(2));
        let mixed = splitmix64(1);
        assert_ne!(mixed & 0xFFFF_FFFF, mixed >> 32);
    }

    #[test]
    fn field_seeds_fold_the_world_seed_and_tag() {
        assert_eq!(
            field_seed(7, CONTINENTALNESS_TAG),
            field_seed(7, CONTINENTALNESS_TAG)
        );
        assert_ne!(
            field_seed(7, EROSION_TAG),
            field_seed(7, CONTINENTALNESS_TAG)
        );
        assert_ne!(field_seed(7, CAVE_TAG), field_seed(8, CAVE_TAG));
    }

    #[test]
    fn fields_are_normalized() {
        let generator = Generator::new(0x5EED_1234);

        for x in (-512..512).step_by(37) {
            for z in (-512..512).step_by(41) {
                for field in [
                    generator.continentalness(x, z),
                    generator.erosion(x, z),
                    generator.temperature(x, z),
                    generator.humidity(x, z),
                ] {
                    assert!((0.0..=1.0).contains(&field), "{field}");
                }
                for y in (0..WORLD_HEIGHT).step_by(17) {
                    assert!((0.0..=1.0).contains(&generator.cave(x, y, z)));
                }
            }
        }
    }

    #[test]
    fn the_same_seed_produces_identical_heightmaps() {
        let first = Generator::new(42);
        let second = Generator::new(42);

        for x in (-300..300).step_by(23) {
            for z in (-300..300).step_by(29) {
                assert_eq!(first.height(x, z), second.height(x, z));
            }
        }
    }

    #[test]
    fn different_seeds_produce_different_terrain() {
        let first = Generator::new(42);
        let second = Generator::new(43);
        let mut differences = 0;

        for x in 0..64 {
            for z in 0..64 {
                if first.height(x, z) != second.height(x, z) {
                    differences += 1;
                }
            }
        }

        assert!(differences > 0);
    }

    fn chunk_block(generator: &Generator, x: i32, y: i32, z: i32) -> Option<Block> {
        let chunk = ChunkCoord::from_world(IVec3::new(x, y, z)).0;
        let blocks = generator.generate_chunk(chunk);
        let (_, local) = ChunkCoord::from_world(IVec3::new(x, y, z));
        Some(blocks.block(local)).filter(|block| *block != Block::Air)
    }

    #[test]
    fn chunk_generation_is_deterministic() {
        let first = Generator::new(1234);
        let second = Generator::new(1234);
        let other = Generator::new(1235);

        let chunk = ChunkCoord::new(3, 1, -2);
        assert_eq!(first.generate_chunk(chunk), second.generate_chunk(chunk));

        let surface_chunk = ChunkCoord::new(0, 0, 0);
        assert_ne!(
            first.generate_chunk(surface_chunk),
            other.generate_chunk(surface_chunk)
        );
    }

    #[test]
    fn generated_chunks_stay_inside_their_bounds() {
        let generator = Generator::new(9);
        let chunk = ChunkCoord::new(-2, 2, 5);

        for (local, _) in generator.generate_chunk(chunk).iter() {
            assert!(local.cmpge(IVec3::ZERO).all() && local.cmplt(IVec3::splat(CHUNK_SIZE)).all());
        }
    }

    #[test]
    fn all_three_biomes_appear() {
        let generator = Generator::new(7);
        let mut plains = false;
        let mut hills = false;
        let mut desert = false;

        for x in (-2048..2048).step_by(16) {
            for z in (-2048..2048).step_by(16) {
                match generator.biome(x, z) {
                    Biome::Plains => plains = true,
                    Biome::Hills => hills = true,
                    Biome::Desert => desert = true,
                }
            }
        }

        assert!(plains && hills && desert);
    }

    #[test]
    fn desert_surfaces_are_sand() {
        let generator = Generator::new(7);
        let mut tested = 0;

        for x in 0..2048 {
            let z = 0;
            if generator.biome(x, z) != Biome::Desert {
                continue;
            }
            let height = generator.height(x, z);
            if generator.cave(x, height, z) > CAVE_THRESHOLD {
                continue;
            }
            for depth in 0..=4 {
                let y = height - depth;
                if y < 0 {
                    continue;
                }
                let carved = y >= 1 && generator.cave(x, y, z) > CAVE_THRESHOLD;
                let expected = if carved {
                    None
                } else if depth < 3 {
                    Some(Block::Sand)
                } else {
                    Some(Block::Stone)
                };
                assert_eq!(
                    chunk_block(&generator, x, y, z),
                    expected,
                    "x={x} z={z} y={y} depth={depth}"
                );
            }
            tested += 1;
            if tested == 3 {
                break;
            }
        }

        assert!(tested > 0);
    }

    #[test]
    fn low_columns_fill_with_water_up_to_sea_level() {
        let generator = Generator::new(7);
        let mut tested = 0;

        for x in 0..2048 {
            let z = 0;
            let height = generator.height(x, z);
            if height >= SEA_LEVEL {
                continue;
            }
            assert_eq!(
                chunk_block(&generator, x, height + 1, z),
                Some(Block::Water)
            );
            assert_eq!(chunk_block(&generator, x, SEA_LEVEL, z), Some(Block::Water));
            assert_eq!(chunk_block(&generator, x, SEA_LEVEL + 1, z), None);
            tested += 1;
            if tested == 3 {
                break;
            }
        }

        assert!(tested > 0);
    }

    #[test]
    fn water_only_generated_chunks_are_not_horizon_occluders() {
        let generator = Generator::new(7);
        let mut tested = 0;

        for x in 0..2048 {
            let z = 0;
            if generator.height(x, z) >= SEA_LEVEL {
                continue;
            }
            let chunk = ChunkCoord::new(
                x.div_euclid(CHUNK_SIZE),
                SEA_LEVEL.div_euclid(CHUNK_SIZE),
                z.div_euclid(CHUNK_SIZE),
            );
            let blocks = generator.generate_chunk(chunk);
            assert!(blocks.has_water(), "{chunk:?}");
            assert_eq!(
                blocks.terrain_min(),
                -1,
                "ocean chunk {chunk:?} must not occlude land"
            );
            tested += 1;
            if tested == 3 {
                break;
            }
        }

        assert!(tested > 0);
    }

    #[test]
    fn caves_carve_air_but_never_the_bottom_layer() {
        let generator = Generator::new(7);
        let mut carved = 0;
        let mut bottom = 0;

        for x in 0..512 {
            for z in 0..512 {
                let height = generator.height(x, z);
                for y in 1..=height {
                    if generator.cave(x, y, z) > CAVE_THRESHOLD {
                        assert_eq!(chunk_block(&generator, x, y, z), None);
                        carved += 1;
                        if carved > 20 {
                            break;
                        }
                    }
                }
                if generator.cave(x, 0, z) > CAVE_THRESHOLD {
                    assert_ne!(chunk_block(&generator, x, 0, z), None);
                    bottom += 1;
                }
                if carved > 20 && bottom > 3 {
                    return;
                }
            }
        }

        assert!(carved > 0);
        assert!(bottom > 0);
    }

    fn find_tree(generator: &Generator) -> Option<(ChunkCoord, Chunk)> {
        for cx in 0..6 {
            for cz in 0..6 {
                for cy in 0..4 {
                    let chunk = ChunkCoord::new(cx, cy, cz);
                    let blocks = generator.generate_chunk(chunk);
                    if blocks.iter().any(|(_, block)| block == Block::Wood) {
                        return Some((chunk, blocks));
                    }
                }
            }
        }
        None
    }

    #[test]
    fn trees_have_a_wood_trunk_and_leaf_canopy() {
        let generator = Generator::new(7);
        let (_, blocks) = find_tree(&generator).expect("a tree");
        let base = blocks
            .iter()
            .find(|(local, block)| {
                *block == Block::Wood && blocks.block(*local - IVec3::Y) == Block::Grass
            })
            .map(|(local, _)| local)
            .expect("trunk base");

        let mut trunk = 0;
        while blocks.block(base + IVec3::Y * trunk) == Block::Wood {
            trunk += 1;
        }
        assert!((4..=6).contains(&trunk), "trunk {trunk}");

        let top = base + IVec3::Y * (trunk - 1);
        for y in [top - IVec3::Y, top] {
            for dx in -1..=1 {
                for dz in -1..=1 {
                    let cell = y + IVec3::X * dx + IVec3::Z * dz;
                    let expected = if dx == 0 && dz == 0 {
                        Block::Wood
                    } else {
                        Block::Leaves
                    };
                    assert_eq!(blocks.block(cell), expected, "{cell}");
                }
            }
        }
        assert_eq!(blocks.block(top + IVec3::Y), Block::Leaves);
    }

    #[test]
    fn trees_only_grow_on_grass_in_plains_or_hills() {
        let generator = Generator::new(7);
        let mut bases = 0;

        for cx in 0..6 {
            for cz in 0..6 {
                for cy in 0..4 {
                    let chunk = ChunkCoord::new(cx, cy, cz);
                    let origin = chunk.world_origin();
                    let blocks = generator.generate_chunk(chunk);
                    for (local, block) in blocks.iter() {
                        if block != Block::Wood {
                            continue;
                        }
                        let below = local - IVec3::Y;
                        if below.y < 0 {
                            continue;
                        }
                        if blocks.block(below) != Block::Grass {
                            continue;
                        }
                        let world = origin + local;
                        assert!(matches!(
                            generator.biome(world.x, world.z),
                            Biome::Plains | Biome::Hills
                        ));
                        bases += 1;
                    }
                }
            }
        }

        assert!(bases > 0);
    }

    #[test]
    fn tree_hashes_are_seeded_and_indexed() {
        let generator = Generator::new(7);
        let chunk = ChunkCoord::new(1, 0, 2);

        assert_eq!(generator.tree_hash(chunk, 0), generator.tree_hash(chunk, 0));
        assert_ne!(generator.tree_hash(chunk, 0), generator.tree_hash(chunk, 1));
        assert_ne!(
            generator.tree_hash(chunk, 0),
            Generator::new(8).tree_hash(chunk, 0)
        );
    }

    #[test]
    fn spawn_column_is_dry_and_not_carved() {
        let generator = Generator::new(7);
        let (x, z, height) = generator.spawn_column();

        assert_eq!(z, 0);
        assert!(height >= SEA_LEVEL);
        assert!(generator.cave(x, height, z) <= CAVE_THRESHOLD);
        assert_eq!(chunk_block(&generator, x, height + 1, z), None);
    }

    #[test]
    fn height_is_clamped_and_covers_land_and_sea() {
        let generator = Generator::new(7);
        let mut below_sea = 0;
        let mut above_sea = 0;

        for x in (-1024..1024).step_by(31) {
            for z in (-1024..1024).step_by(31) {
                let height = generator.height(x, z);
                assert!((MIN_HEIGHT..=MAX_HEIGHT).contains(&height), "{height}");
                if height < SEA_LEVEL {
                    below_sea += 1;
                } else {
                    above_sea += 1;
                }
            }
        }

        assert!(below_sea > 0);
        assert!(above_sea > 0);
    }
}

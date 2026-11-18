//! Deterministic world generation.
//!
//! Every field is a function of the world seed, a field tag, and world block
//! coordinates. There is no global RNG and no dependency on chunk order.

use std::collections::HashMap;

use fastnoise_lite::{FastNoiseLite, FractalType, NoiseType};
use glam::IVec3;

use crate::block::Block;
use crate::world::{CHUNK_SIZE, ChunkCoord};

/// Blocks per world column axis.
pub(crate) const WORLD_HEIGHT: i32 = CHUNK_SIZE * 4;

/// Chunks per world column.
pub(crate) const CHUNK_COLUMN: i32 = 4;

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

/// One world's noise fields and derived terrain.
pub(crate) struct Generator {
    continentalness: FastNoiseLite,
    erosion: FastNoiseLite,
    temperature: FastNoiseLite,
    humidity: FastNoiseLite,
    cave: FastNoiseLite,
}

impl Generator {
    pub(crate) fn new(seed: u64) -> Self {
        Self {
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
    pub(crate) fn generate_chunk(&self, chunk: ChunkCoord) -> HashMap<IVec3, Block> {
        let origin = chunk.world_origin();
        let mut blocks = HashMap::new();
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

                    if block != Block::Air {
                        blocks.insert(IVec3::new(lx, world_y - origin.y, lz), block);
                    }
                }
            }
        }
        blocks
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
fn surface_block(biome: Biome, depth: i32) -> Block {
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
        blocks.get(&local).copied()
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

        for local in generator.generate_chunk(chunk).keys() {
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

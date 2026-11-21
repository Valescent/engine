//! Shared fixtures for the `features` unit tests.
//!
//! Every helper is `pub(super)` so the discovery and application test modules
//! can use one section/context harness without repeating it.

use crate::description::SECTION_VOLUME;
use crate::hash::{domain, hash64};
use crate::section::{BlockId, section_index};

use super::*;

pub(super) const AIR: BlockId = 0;
pub(super) const WATER: BlockId = 7;
pub(super) const BEDROCK: BlockId = 8;
pub(super) const STONE: BlockId = 20;
pub(super) const DEEP: BlockId = 21;
pub(super) const SAND: BlockId = 30;
pub(super) const LOG: BlockId = 40;
pub(super) const LEAF: BlockId = 41;
pub(super) const BOULDER: BlockId = 50;
pub(super) const FLOWER: BlockId = 60;
pub(super) const IRON: BlockId = 140;
pub(super) const COPPER: BlockId = 141;

pub(super) const SEED: u64 = 0x0bad_c0de_1234_5678;

/// A heap-backed section array.
pub(super) fn scratch(fill: BlockId) -> Box<[BlockId; SECTION_VOLUME]> {
    vec![fill; SECTION_VOLUME]
        .into_boxed_slice()
        .try_into()
        .expect("the scratch array matches the section volume")
}

pub(super) fn index(x: i32, y: i32, z: i32) -> usize {
    section_index(x.cast_unsigned(), y.cast_unsigned(), z.cast_unsigned())
}

/// A section of stone up to world `y = 140` (local 12), air above.
pub(super) fn land() -> Box<[BlockId; SECTION_VOLUME]> {
    let mut blocks = scratch(AIR);
    for y in 0..=12 {
        for z in 0..32 {
            for x in 0..32 {
                blocks[index(x, y, z)] = STONE;
            }
        }
    }
    blocks
}

/// Highest world `y` a section holds a log or leaf.
pub(super) fn highest_tree_block(blocks: &[BlockId; SECTION_VOLUME]) -> Option<i32> {
    (0..SECTION_VOLUME)
        .filter(|slot| matches!(blocks[*slot], LOG | LEAF))
        .filter_map(|slot| i32::try_from(crate::section::section_coord(slot).1).ok())
        .max()
        .map(|local| local + 128)
}

/// Owns the column arrays an [`ApplyContext`] borrows.
pub(super) struct Flat {
    tops: Box<[i32; COLUMNS]>,
    slope: Box<[i32; COLUMNS]>,
    biome: Box<[u8; COLUMNS]>,
    flags: Box<[u8; COLUMNS]>,
    ores: Vec<OreBlock>,
    host: Vec<BlockId>,
    scatter: Vec<BlockId>,
    regions: [[i32; 3]; GATHER_REGIONS],
    seed: u64,
}

impl Flat {
    pub(super) fn new(section_origin: [i32; 3], top: i32) -> Self {
        let regions = gather_origins(region_of(section_origin));
        Self {
            tops: boxed(top),
            slope: boxed(0),
            biome: boxed(3),
            flags: boxed(0),
            ores: vec![
                OreBlock {
                    name: "ore_iron",
                    id: IRON,
                },
                OreBlock {
                    name: "ore_copper",
                    id: COPPER,
                },
            ],
            host: vec![STONE, DEEP],
            scatter: vec![FLOWER],
            regions,
            seed: SEED,
        }
    }

    pub(super) fn set_column(
        &mut self,
        x: i32,
        z: i32,
        top: i32,
        slope: i32,
        biome: u8,
        flags: u8,
    ) {
        let origin = self.regions[region_slot([0, 0, 0])];
        let column = ColumnField {
            origin: [origin[0] - REGION_SIZE, origin[2] - REGION_SIZE],
            tops: self.tops.as_slice(),
            slope: self.slope.as_slice(),
            biome: self.biome.as_slice(),
            flags: self.flags.as_slice(),
        };
        let local_x = x - column.origin[0];
        let local_z = z - column.origin[1];
        let slot = usize::try_from(local_x + FOOTPRINT * local_z).expect("inside the footprint");
        self.tops[slot] = top;
        self.slope[slot] = slope;
        self.biome[slot] = biome;
        self.flags[slot] = flags;
    }

    pub(super) fn ctx(&self) -> ApplyContext<'_> {
        let origin = self.regions[region_slot([0, 0, 0])];
        ApplyContext {
            seed: self.seed,
            palette: Palette {
                air: AIR,
                water: WATER,
                bedrock: BEDROCK,
                log: LOG,
                leaf: LEAF,
                boulder: BOULDER,
            },
            ores: &self.ores,
            host_rock: &self.host,
            columns: ColumnField {
                origin: [origin[0] - REGION_SIZE, origin[2] - REGION_SIZE],
                tops: self.tops.as_slice(),
                slope: self.slope.as_slice(),
                biome: self.biome.as_slice(),
                flags: self.flags.as_slice(),
            },
            regions: self.regions,
            scatter: &self.scatter,
        }
    }
}

fn boxed<T: Clone>(value: T) -> Box<[T; COLUMNS]> {
    let mut vector = Vec::with_capacity(COLUMNS);
    vector.resize(COLUMNS, value);
    match vector.try_into() {
        Ok(rows) => rows,
        Err(_) => unreachable!("the footprint length matches"),
    }
}

pub(super) fn ore_candidate(id: u64, spec: usize, origin: [i32; 3]) -> Candidate {
    Candidate {
        id,
        region: region_of(origin),
        kind: FeatureKind::Ore,
        origin,
        variant: variant_with_spec(spec, 0x1234_5678),
    }
}

pub(super) fn tree_candidate(id: u64, origin: [i32; 3]) -> Candidate {
    Candidate {
        id,
        region: region_of(origin),
        kind: FeatureKind::Tree,
        origin,
        variant: hash64(SEED, domain("tree:oak"), &[id.cast_signed()]),
    }
}

/// The v1 spec list without scatter, for tests that assert exact writes.
pub(super) fn tree_specs() -> Vec<FeatureSpec> {
    v1_specs()
        .into_iter()
        .filter(|spec| spec.kind != FeatureKind::Scatter)
        .collect()
}

/// A small non-negative coordinate that varies with `id`.
pub(super) fn stable_origin(id: u64) -> i32 {
    i32::try_from(id % 7).expect("a small coordinate fits i32")
}

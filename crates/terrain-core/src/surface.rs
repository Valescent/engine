//! W4 - surface rule precedence and the column stack.
//!
//! `E4.1`-`E4.5`. One ordered rule list per biome, one shared evaluator, first
//! match wins per column, and a complete depth-labelled stack per column.
//!
//! # The nine slots
//!
//! Precedence is fixed (`E4.5`) and this module owns slots 1-5 and 8-9; the
//! biome's rule list contributes slots 6-7:
//!
//! 1. river or lake bed
//! 2. underwater sea floor (`top_solid_y < SEA`)
//! 3. beach band (`0 <= base <= +3` and `top >= SEA`)
//! 4. snow cap
//! 5. slope stone
//! 6. biome topsoil
//! 7. biome filler
//! 8. stone
//! 9. bedrock at `y = 0`
//!
//! Slots 6 and 7 are the first and the later matches of the biome list: the
//! first rule is the topsoil, every later rule is a filler that may stack
//! (`E4.2`). A biome list is mandatory, so "no rule matched" is a table bug;
//! [`SurfaceTable::validate_table`] rejects a missing catch-all.
//!
//! Bedrock is the absolute floor as well as the last fallback (`C4.1`): a column
//! whose top is `y = 0` is bedrock, never sea floor, so slot 9 wins there even
//! though its number is last.
//!
//! # Symbolic stone
//!
//! Slot 8 stores [`Layer::Stone`], not an id (`G1.2`). The caller resolves it
//! through [`Strata::stone_at`] (or [`Strata::stone_in`] with the cached column
//! this module hands back), so deep rock, strata bands, and the dither band stay
//! one resolver.
//!
//! # Determinism
//!
//! Predicates compare integers and `f32` and nothing else. The slope dither and
//! the snow jitter are keyed on `(x, z)` only, never on `y`, so a column has one
//! material whatever order chunks are generated in (`E3.3`). No `sin`, `cos`,
//! `pow`, `exp`, `ln`, no `mul_add`, no hash map, and no float reduction.

use crate::description::SEA;
use crate::error::{ErrorKind, Stage, TerrainError};
use crate::hash::{domain, hash01, hash2};
use crate::section::BlockId;
use crate::strata::{ColumnStrata, Strata};

/// Drop, in blocks, at which a slope exposes stone (`E4.3`).
pub const STONE_DROP: i32 = 2;

/// Share of columns in the dither band that still expose stone (`E4.3`).
pub const DITHER_SHARE: f32 = 0.35;

/// Temperature below which a snow cap is placed (`E4.4`).
///
/// Mirrors the description default `T_SNOW`; a test pins the two together so the
/// constant cannot drift from the schema.
pub const T_SNOW: f32 = 0.25;

/// Number of precedence slots (`E4.5`).
pub const SLOTS: u8 = 9;

/// Most biomes a content pack may declare (`E3.1`).
pub const MAX_BIOMES: usize = 255;

/// Lowest base offset that reads as beach, in blocks relative to sea (`C1.6`).
const BEACH_BASE_MIN: f32 = 0.0;

/// Highest base offset that reads as beach, in blocks relative to sea (`C1.6`).
const BEACH_BASE_MAX: f32 = 3.0;

// Sea-floor bands (`E4.7`). Depth is always `d = SEA - top_solid_y`, an offset
// from sea level, never an absolute `y`.
/// Last depth of the shore band: `1..=4`.
pub const SEABED_SHORE_LAST_DEPTH: i32 = 4;

/// Last depth of the shelf band: `5..=14`.
pub const SEABED_SHELF_LAST_DEPTH: i32 = 14;

/// Last depth of the slope band: `15..=24`; `>= 25` is the abyss.
pub const SEABED_SLOPE_LAST_DEPTH: i32 = 24;

/// Share of columns at a band edge that take the next, deeper material.
pub const SEABED_DITHER_SHARE: f32 = 0.35;

/// Share of warm shelf columns that take a hashed clay patch.
pub const SEABED_CLAY_SHARE: f32 = 0.15;

/// Temperature below which the shore band turns from sand to gravel.
///
/// A calibration start, not a magic number: it is the same scale as
/// [`T_SNOW`], and a colder shore is a description change.
pub const T_COLD_SHORE: f32 = 0.25;

/// Temperature below which the shelf band turns from sand to gravel.
pub const T_COLD_SHELF: f32 = 0.20;

/// Thickness of a river or lake bed layer, in blocks (`D3.3` allows 1-2).
const BED_LAYER_DEPTH: i32 = 2;

/// Thickness of a sea-floor layer, in blocks (`E4.7`: three, then stone).
const SEABED_LAYER_DEPTH: i32 = 3;

/// Thickness of a beach layer, in blocks.
const BEACH_LAYER_DEPTH: i32 = 4;

/// Base thickness of a snow cap, in blocks, before the ±2 jitter (`E4.4`).
const SNOW_LAYER_DEPTH: i32 = 2;

/// Thickness of the slope-stone veneer, in blocks (`E4.3`).
const SLOPE_LAYER_DEPTH: i32 = 3;

/// Largest river slope that still takes the sand bed (`D3.3`).
const FLAT_RIVER_DROP: i32 = 1;

/// Domain tag of the slope dither (`E4.3`).
const STONE_DITHER_DOMAIN: &str = "stone_dither";

/// Domain tag of the snow jitter (`E4.4`).
const SNOW_JITTER_DOMAIN: &str = "snow_jitter";

/// Domain tag of the sea-floor band dither (`E4.7`).
const SEAFLOOR_DITHER_DOMAIN: &str = "seafloor_dither";

/// Domain tag of the sea-floor clay patches (`E4.7`).
const SEAFLOOR_CLAY_DOMAIN: &str = "seafloor_clay";

/// How a height compares with a level (`E4.1`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Order {
    /// Strictly below the level.
    Below,
    /// At or above the level.
    AtOrAbove,
}

/// The closed predicate vocabulary of a surface rule (`E4.1`).
///
/// Integer and `f32` comparisons only. `And([])` is the always-true catch-all;
/// `Or([])` is always false. Adding a variant is a code change and a version
/// bump; reordering or adding rules is content data.
#[derive(Clone, Debug, PartialEq)]
pub enum Predicate {
    /// At least this many blocks below the column's top.
    DepthBelowTop(i32),
    /// The column's integer slope drop is at least this many blocks.
    SlopeAtLeast(i32),
    /// The column's top compares with `SEA + offset` in the given direction.
    HeightVsSea(Order, i32),
    /// `lo <= SEA - top <= hi`: the sea-floor depth band (`E4.7`).
    ///
    /// The bounds are inclusive and are offsets below sea, so `(1, 4)` is the
    /// shore band whatever the absolute sea level is. On a column at or above
    /// sea the depth is zero, which matches no band starting at `1`.
    DepthBelowSeaIn(i32, i32),
    /// The column's temperature is strictly below this value.
    TempBelow(f32),
    /// Water is within this many blocks, according to the caller's flag.
    WaterWithin(i32),
    /// The column is a river bed.
    IsRiverBed,
    /// The column is a lake bed.
    IsLakeBed,
    /// The column is inside the coastal band.
    NearCoast,
    /// Every child holds.
    And(Vec<Predicate>),
    /// At least one child holds.
    Or(Vec<Predicate>),
    /// The child does not hold.
    Not(Box<Predicate>),
}

/// One ordered surface rule (`E4.1`).
///
/// `depth_blocks` is the thickness this rule claims, measured from the depth at
/// which it is evaluated; `1` means one block.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    /// Condition to test.
    pub predicate: Predicate,
    /// Material to place.
    pub block: BlockId,
    /// Thickness in blocks. Must be positive.
    pub depth_blocks: i32,
}

/// The ordered rule list of one biome (`E4.1`).
///
/// Rule 0 is the topsoil, later rules are fillers that may stack, and the last
/// rule must be the catch-all.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BiomeRules {
    /// Rules, first match wins per slot.
    pub rules: Vec<Rule>,
}

/// Everything the surface rules need to know about one column (`E4.1`).
///
/// Built by the caller from the field tile, the height tile, and the hydrology
/// tile. The two flags `is_river` and `is_lake` refine `is_bed`; `water_adjacent`
/// is the caller's answer for whatever radius it cares about.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColumnFacts {
    /// Tile-local column `x`.
    pub x: i32,
    /// Tile-local column `z`.
    pub z: i32,
    /// The column's top solid block, from the height tile.
    pub top: i32,
    /// Base elevation in blocks relative to sea (`C1.1`).
    pub base: f32,
    /// Column temperature in `[0, 1]` (`E1.1`).
    pub temp: f32,
    /// The column's integer slope drop (`E4.3`).
    pub slope_drop: i32,
    /// The column is a river channel.
    pub is_river: bool,
    /// The column is a lake bed.
    pub is_lake: bool,
    /// The column is either kind of bed.
    pub is_bed: bool,
    /// The column is inside the coastal band.
    pub near_coast: bool,
    /// Water is within the caller's radius.
    pub water_adjacent: bool,
}

/// One surface material, either a concrete block or symbolic stone (`G1.2`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Layer {
    /// A block id the caller supplied.
    Block(BlockId),
    /// Stone, resolved per `y` through the strata resolver.
    Stone,
}

/// The resolved surface materials of one column (`E4.1`, `F4.1`).
///
/// The layers run from the top down and their `depth_blocks` sum to
/// `top + 1`, so the last block is always bedrock at `y = 0`. A `Layer::Stone`
/// layer is symbolic; use [`SurfaceStack::block_at`] to resolve it.
#[derive(Clone, Debug)]
pub struct SurfaceStack {
    /// The precedence slot that supplied the surface material, `1..=9`.
    pub slot: u8,
    /// Depth-labelled layers, top first.
    pub layers: Vec<(i32, Layer)>,
    /// The column's 2D strata fields, evaluated once (`G2.3`).
    pub column: ColumnStrata,
}

impl SurfaceStack {
    /// Total thickness of the stack in blocks: always `top + 1`.
    #[must_use]
    pub fn depth_blocks(&self) -> i32 {
        self.layers.iter().map(|(depth, _)| *depth).sum()
    }

    /// The layer covering a depth below the surface.
    ///
    /// Depth `0` is the top block. `None` means the depth is below the column.
    #[must_use]
    pub fn layer_at(&self, depth: i32) -> Option<Layer> {
        if depth < 0 {
            return None;
        }
        let mut start = 0_i32;
        for (count, layer) in &self.layers {
            let end = start.saturating_add(*count);
            if depth < end {
                return Some(*layer);
            }
            start = end;
        }
        None
    }

    /// Resolves a depth to a concrete block, through the strata resolver for
    /// symbolic stone (`G1.2`).
    #[must_use]
    pub fn block_at(&self, depth: i32, strata: &Strata, x: i32, y: i32, z: i32) -> Option<BlockId> {
        match self.layer_at(depth)? {
            Layer::Block(id) => Some(id),
            Layer::Stone => Some(strata.stone_in(self.column, x, y, z)),
        }
    }
}

/// The materials of the four sea-floor depth bands (`E4.7`).
///
/// The band *selection* is code because `E4.7` pins the depth windows; the
/// block ids behind every band are content data, so a pack can name
/// `core:sand`, `core:gravel`, and `core:clay` without this module knowing
/// numeric ids.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeabedMaterials {
    /// `d = 1..=4`, warm.
    pub shore_sand: Layer,
    /// `d = 1..=4`, `temp < T_COLD_SHORE`.
    pub shore_gravel: Layer,
    /// `d = 5..=14`, warm and no clay patch.
    pub shelf_sand: Layer,
    /// `d = 5..=14`, `temp < T_COLD_SHELF`.
    pub shelf_gravel: Layer,
    /// `d = 5..=14`, warm with a hashed patch.
    pub shelf_clay: Layer,
    /// `d = 15..=24`.
    pub slope_gravel: Layer,
    /// `d >= 25`.
    pub abyss_clay: Layer,
}

/// The nine caller-supplied materials plus the two hash key streams (`E4.5`).
///
/// No block id is hardcoded here: every slot, including bedrock, is data.
#[derive(Clone, Debug)]
pub struct SurfaceTable {
    /// World seed. Keys the slope dither and the snow jitter.
    seed: u64,
    /// Beach sand in a cold climate (slot 3).
    pub cold_beach: Layer,
    /// Beach sand in a warm climate (slot 3).
    pub warm_beach: Layer,
    /// The four sea-floor depth bands (slot 2).
    pub seabed: SeabedMaterials,
    /// Snow cap material (slot 4).
    pub snow: Layer,
    /// Exposed stone on a slope (slot 5).
    pub slope_stone: Layer,
    /// River bed on a flat slope (slot 1).
    pub river_bed_sand: Layer,
    /// River bed on a steep slope (slot 1).
    pub river_bed_gravel: Layer,
    /// The block at `y = 0` (slot 9).
    pub bedrock: Layer,
    /// Domain tag of the slope dither, hashed once at construction.
    dither_tag: u64,
    /// Domain tag of the snow jitter, hashed once at construction.
    snow_tag: u64,
    /// Domain tag of the sea-floor band dither, hashed once at construction.
    seafloor_tag: u64,
    /// Domain tag of the sea-floor clay patches, hashed once at construction.
    clay_tag: u64,
}

impl SurfaceTable {
    /// Wraps the nine materials and caches the two hash domain tags.
    ///
    /// Tag caching changes no output: the tags are pure functions of their
    /// names. It only keeps the per-column path free of a SHA-256.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        seed: u64,
        cold_beach: Layer,
        warm_beach: Layer,
        seabed_shallow: Layer,
        seabed_deep: Layer,
        snow: Layer,
        slope_stone: Layer,
        river_bed_sand: Layer,
        river_bed_gravel: Layer,
        bedrock: Layer,
    ) -> Self {
        // The two-argument legacy form maps the sand bands to `shallow` and
        // every gravel/clay band to `deep`. A pack that can name gravel and
        // clay separately should call [`SurfaceTable::with_seabed`].
        Self::with_seabed(
            seed,
            cold_beach,
            warm_beach,
            SeabedMaterials {
                shore_sand: seabed_shallow,
                shore_gravel: seabed_deep,
                shelf_sand: seabed_shallow,
                shelf_gravel: seabed_deep,
                shelf_clay: seabed_deep,
                slope_gravel: seabed_deep,
                abyss_clay: seabed_deep,
            },
            snow,
            slope_stone,
            river_bed_sand,
            river_bed_gravel,
            bedrock,
        )
    }

    /// Wraps the full set of sea-floor band materials (`E4.7`).
    ///
    /// `new` remains the compatible entry point for callers with only a
    /// shallow/deep pair; this one lets a content pack give sand, gravel, and
    /// clay distinct ids.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn with_seabed(
        seed: u64,
        cold_beach: Layer,
        warm_beach: Layer,
        seabed: SeabedMaterials,
        snow: Layer,
        slope_stone: Layer,
        river_bed_sand: Layer,
        river_bed_gravel: Layer,
        bedrock: Layer,
    ) -> Self {
        Self {
            seed,
            cold_beach,
            warm_beach,
            seabed,
            snow,
            slope_stone,
            river_bed_sand,
            river_bed_gravel,
            bedrock,
            dither_tag: domain(STONE_DITHER_DOMAIN),
            snow_tag: domain(SNOW_JITTER_DOMAIN),
            seafloor_tag: domain(SEAFLOOR_DITHER_DOMAIN),
            clay_tag: domain(SEAFLOOR_CLAY_DOMAIN),
        }
    }

    /// The seed the dither and the jitter are keyed on.
    #[must_use]
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// The precedence slot that supplies this column's surface material
    /// (`E4.5`).
    ///
    /// Returns `1..=9`. Slot 9 is returned for a column whose top is `y = 0`,
    /// because bedrock is the absolute floor (`C4.1`).
    #[must_use]
    pub fn fired_slot(&self, facts: &ColumnFacts, biome_rules: &BiomeRules) -> u8 {
        if let Some(slot) = self.locked_slot(facts) {
            return slot;
        }
        for (index, rule) in biome_rules.rules.iter().enumerate() {
            if rule.depth_blocks <= 0 {
                continue;
            }
            if holds(&rule.predicate, facts, 0) {
                return if index == 0 { 6 } else { 7 };
            }
        }
        8
    }

    /// The biome's contribution to a column's stack, in depth order (`E4.2`).
    ///
    /// Every matching rule contributes its `depth_blocks` in list order, so
    /// filler rules stack under the topsoil. The first rule is dropped when a
    /// locked slot owns the surface, which is what keeps grass off a sea floor or
    /// a beach (`E4.5`).
    #[must_use]
    pub fn resolve(&self, column: &ColumnFacts, biome_rules: &BiomeRules) -> Vec<(i32, BlockId)> {
        let (slot, layers) = self.evaluate(column, biome_rules);
        layers
            .into_iter()
            // A sea floor is three band blocks then stone (`E4.7`), so it
            // contributes no biome layers at all, not even the filler.
            .filter(|layer| slot != 2 && (slot == 6 || layer.rule != 0))
            .map(|layer| (layer.depth, layer.block))
            .collect()
    }

    /// The complete resolved stack of one column (`F4.1`).
    ///
    /// Slots 1-5 and 8-9 are decided here; the biome supplies 6-7. The returned
    /// layers cover the whole column from `top` down to `y = 0`, with the last
    /// layer always the caller's bedrock block.
    #[must_use]
    pub fn resolve_column(
        &self,
        facts: &ColumnFacts,
        biome_rules: &BiomeRules,
        strata: &Strata,
    ) -> SurfaceStack {
        let (slot, contributions) = self.evaluate(facts, biome_rules);
        let total = facts.top.max(0).saturating_add(1);
        let mut budget = total.saturating_sub(1);
        let mut layers: Vec<(i32, Layer)> = Vec::with_capacity(8);

        let surface = match slot {
            1 => Some((self.bed_layer(facts), BED_LAYER_DEPTH)),
            2 => Some((self.seabed_layer(facts), SEABED_LAYER_DEPTH)),
            3 => Some((self.beach_layer(facts), BEACH_LAYER_DEPTH)),
            4 => Some((self.snow, self.snow_depth(facts))),
            5 => Some((self.slope_stone, SLOPE_LAYER_DEPTH)),
            _ => None,
        };
        if let Some((layer, depth)) = surface {
            push(&mut layers, &mut budget, depth, layer);
        }

        // A cliff face is stone from the surface down, and a sea floor is
        // exactly three band blocks then symbolic stone (`E4.7`); neither takes
        // a biome filler.
        if slot != 5 && slot != 2 {
            for contribution in &contributions {
                if slot == 6 || contribution.rule != 0 {
                    push(
                        &mut layers,
                        &mut budget,
                        contribution.depth,
                        Layer::Block(contribution.block),
                    );
                }
            }
        }

        if budget > 0 {
            layers.push((budget, Layer::Stone));
        }
        layers.push((1, self.bedrock));

        SurfaceStack {
            slot,
            layers,
            column: strata.column(facts.x, facts.z),
        }
    }

    /// Rejects a rule table the compiler cannot accept (`E4.5`, `E3.1`).
    ///
    /// Three checks, in order:
    ///
    /// - more than [`MAX_BIOMES`] entries,
    /// - a rule that uses a predicate owning a locked slot 1-5 (a "biome rule
    ///   before slot 5"), or a non-positive `depth_blocks`,
    /// - a list whose last rule is not the catch-all `And([])`.
    ///
    /// # Errors
    ///
    /// Returns [`Stage::Surface`] with [`ErrorKind::DescriptionInvalid`] and the
    /// key `SURFACE_RULES`.
    pub fn validate_table(&self, biome_rules: &BiomeRules) -> Result<(), TerrainError> {
        if biome_rules.rules.len() > MAX_BIOMES {
            return Err(reject("biome count"));
        }
        for rule in &biome_rules.rules {
            if rule.depth_blocks <= 0 {
                return Err(reject("depth_blocks positive"));
            }
            if locked_predicate(&rule.predicate) {
                return Err(reject("before slot 5"));
            }
        }
        match biome_rules.rules.last() {
            Some(Rule {
                predicate: Predicate::And(parts),
                ..
            }) if parts.is_empty() => Ok(()),
            _ => Err(reject("catch-all")),
        }
    }

    /// The slots that do not depend on the biome list, in precedence order.
    fn locked_slot(&self, facts: &ColumnFacts) -> Option<u8> {
        if facts.top <= 0 {
            Some(9)
        } else if facts.is_bed {
            Some(1)
        } else if facts.top < SEA {
            Some(2)
        } else if is_beach(facts) {
            Some(3)
        } else if is_snow(facts) {
            Some(4)
        } else if self.is_slope_stone(facts) {
            Some(5)
        } else {
            None
        }
    }

    /// The slot plus the biome contributions, with one scan of the rule list.
    fn evaluate(&self, facts: &ColumnFacts, biome_rules: &BiomeRules) -> (u8, Vec<Contribution>) {
        let locked = self.locked_slot(facts);
        let contributions = if matches!(locked, Some(5 | 9)) {
            Vec::new()
        } else {
            biome_layers(facts, biome_rules)
        };
        let slot = locked.unwrap_or_else(|| match contributions.first() {
            Some(first) if first.rule == 0 => 6,
            Some(_) => 7,
            None => 8,
        });
        (slot, contributions)
    }

    /// Slot 1 material: sand on a flat river bed, gravel elsewhere (`D3.3`).
    fn bed_layer(&self, facts: &ColumnFacts) -> Layer {
        if facts.is_river && facts.slope_drop > FLAT_RIVER_DROP {
            self.river_bed_gravel
        } else {
            self.river_bed_sand
        }
    }

    /// Slot 2 material, banded by `d = SEA - top_solid_y` (`E4.7`).
    ///
    /// Ordered, first match wins: shore `1..=4`, shelf `5..=14`, slope
    /// `15..=24`, abyss `>= 25`. At the last depth of a band the next material
    /// wins on a `hash01(seed, "seafloor_dither", x, z) < 0.35` column, which
    /// makes the edge ragged without moving the band boundaries.
    fn seabed_layer(&self, facts: &ColumnFacts) -> Layer {
        let depth = SEA.saturating_sub(facts.top);
        let dithered = self.seabed_dithers(facts);

        if holds(
            &Predicate::DepthBelowSeaIn(1, SEABED_SHORE_LAST_DEPTH),
            facts,
            0,
        ) {
            if depth == SEABED_SHORE_LAST_DEPTH && dithered {
                return self.seabed.slope_gravel;
            }
            if facts.temp < T_COLD_SHORE {
                self.seabed.shore_gravel
            } else {
                self.seabed.shore_sand
            }
        } else if holds(
            &Predicate::DepthBelowSeaIn(SEABED_SHORE_LAST_DEPTH + 1, SEABED_SHELF_LAST_DEPTH),
            facts,
            0,
        ) {
            if depth == SEABED_SHELF_LAST_DEPTH && dithered {
                return self.seabed.slope_gravel;
            }
            if facts.temp < T_COLD_SHELF {
                self.seabed.shelf_gravel
            } else if self.shelf_clay_patch(facts) {
                self.seabed.shelf_clay
            } else {
                self.seabed.shelf_sand
            }
        } else if holds(
            &Predicate::DepthBelowSeaIn(SEABED_SHELF_LAST_DEPTH + 1, SEABED_SLOPE_LAST_DEPTH),
            facts,
            0,
        ) {
            if depth == SEABED_SLOPE_LAST_DEPTH && dithered {
                return self.seabed.abyss_clay;
            }
            self.seabed.slope_gravel
        } else {
            // Catch-all for `d >= 25`, so the bands tile every submerged depth.
            self.seabed.abyss_clay
        }
    }

    /// Whether this column is at a ragged sea-floor band edge (`E4.7`).
    ///
    /// Keyed on `(x, z)` only, so every block of a column takes the same band
    /// and chunk order cannot change the result.
    fn seabed_dithers(&self, facts: &ColumnFacts) -> bool {
        hash01(hash2(self.seed, self.seafloor_tag, facts.x, facts.z)) < SEABED_DITHER_SHARE
    }

    /// Whether this warm shelf column takes a hashed clay patch (`E4.7`).
    fn shelf_clay_patch(&self, facts: &ColumnFacts) -> bool {
        hash01(hash2(self.seed, self.clay_tag, facts.x, facts.z)) < SEABED_CLAY_SHARE
    }

    /// Slot 3 material, banded by temperature (`E4.5`).
    fn beach_layer(&self, facts: &ColumnFacts) -> Layer {
        if facts.temp < T_SNOW {
            self.cold_beach
        } else {
            self.warm_beach
        }
    }

    /// Slot 4 thickness: the base cap moved by the hashed ±2 jitter (`E4.4`).
    fn snow_depth(&self, facts: &ColumnFacts) -> i32 {
        let jitter = jitter_from(self.seed, self.snow_tag, facts.x, facts.z);
        (SNOW_LAYER_DEPTH + jitter).max(1)
    }

    /// Slot 5 test: rock at the threshold, a hashed 35 % band one below it
    /// (`E4.3`).
    fn is_slope_stone(&self, facts: &ColumnFacts) -> bool {
        if facts.slope_drop >= STONE_DROP {
            return true;
        }
        facts.slope_drop + 1 == STONE_DROP
            && hash01(hash2(self.seed, self.dither_tag, facts.x, facts.z)) < DITHER_SHARE
    }
}

/// One matching rule and the depth at which it was evaluated.
#[derive(Clone, Copy, Debug)]
struct Contribution {
    /// Index in the biome list.
    rule: usize,
    /// Thickness claimed, in blocks.
    depth: i32,
    /// Material to place.
    block: BlockId,
}

/// Every matching biome rule, with the cursor depth it was tested at.
///
/// The cursor only advances on a match, so `DepthBelowTop(n)` measures the depth
/// the rule would start at, not the number of rules scanned.
fn biome_layers(facts: &ColumnFacts, biome_rules: &BiomeRules) -> Vec<Contribution> {
    let mut layers = Vec::new();
    let mut cursor = 0_i32;
    for (index, rule) in biome_rules.rules.iter().enumerate() {
        if rule.depth_blocks <= 0 {
            continue;
        }
        if holds(&rule.predicate, facts, cursor) {
            layers.push(Contribution {
                rule: index,
                depth: rule.depth_blocks,
                block: rule.block,
            });
            cursor = cursor.saturating_add(rule.depth_blocks);
        }
    }
    layers
}

/// The `E4.3` jitter of the snow cap's underside, in `-2..=2` blocks.
///
/// Pure and deterministic: `hash2(seed, domain("snow_jitter"), x, z)`. The
/// [`SurfaceTable`] path uses the cached tag and produces the same value.
#[must_use]
pub fn snow_jitter(seed: u64, x: i32, z: i32) -> i32 {
    jitter_from(seed, domain(SNOW_JITTER_DOMAIN), x, z)
}

/// The jitter for an already-hashed domain tag.
fn jitter_from(seed: u64, tag: u64, x: i32, z: i32) -> i32 {
    let pick = hash2(seed, tag, x, z) % 5;
    i32::try_from(pick).unwrap_or(0) - 2
}

/// Whether a column is in the beach band (`C1.6`, `E4.5`).
fn is_beach(facts: &ColumnFacts) -> bool {
    (BEACH_BASE_MIN..=BEACH_BASE_MAX).contains(&facts.base) && facts.top >= SEA
}

/// Whether a column takes a snow cap (`E4.4`).
fn is_snow(facts: &ColumnFacts) -> bool {
    facts.temp < T_SNOW && facts.top >= SEA && !facts.is_bed
}

/// Evaluates one predicate at a depth below the column's top (`E4.1`).
fn holds(predicate: &Predicate, facts: &ColumnFacts, depth: i32) -> bool {
    match predicate {
        Predicate::DepthBelowTop(blocks) => depth >= *blocks,
        Predicate::SlopeAtLeast(blocks) => facts.slope_drop >= *blocks,
        Predicate::HeightVsSea(order, offset) => {
            let level = SEA.saturating_add(*offset);
            match order {
                Order::Below => facts.top < level,
                Order::AtOrAbove => facts.top >= level,
            }
        }
        Predicate::DepthBelowSeaIn(low, high) => {
            let depth = SEA.saturating_sub(facts.top);
            *low <= depth && depth <= *high
        }
        Predicate::TempBelow(limit) => facts.temp < *limit,
        Predicate::WaterWithin(radius) => *radius > 0 && facts.water_adjacent,
        Predicate::IsRiverBed => facts.is_bed && facts.is_river,
        Predicate::IsLakeBed => facts.is_bed && facts.is_lake,
        Predicate::NearCoast => facts.near_coast,
        Predicate::And(parts) => parts.iter().all(|part| holds(part, facts, depth)),
        Predicate::Or(parts) => parts.iter().any(|part| holds(part, facts, depth)),
        Predicate::Not(inner) => !holds(inner, facts, depth),
    }
}

/// Whether a predicate owns a locked slot 1-5 (`E4.5`).
///
/// Biome rules start at slot 6, so a rule that mentions river bed, lake bed,
/// height against sea, temperature, slope, or the coast is claiming a locked
/// entry and is rejected rather than silently evaluated out of order.
fn locked_predicate(predicate: &Predicate) -> bool {
    match predicate {
        Predicate::IsRiverBed
        | Predicate::IsLakeBed
        | Predicate::HeightVsSea(..)
        | Predicate::DepthBelowSeaIn(..)
        | Predicate::TempBelow(_)
        | Predicate::SlopeAtLeast(_)
        | Predicate::NearCoast => true,
        Predicate::DepthBelowTop(_) | Predicate::WaterWithin(_) => false,
        Predicate::And(parts) | Predicate::Or(parts) => parts.iter().any(locked_predicate),
        Predicate::Not(inner) => locked_predicate(inner),
    }
}

/// Appends as much of one layer as the remaining budget allows.
fn push(layers: &mut Vec<(i32, Layer)>, budget: &mut i32, depth: i32, layer: Layer) {
    if *budget <= 0 || depth <= 0 {
        return;
    }
    let take = depth.min(*budget);
    layers.push((take, layer));
    *budget -= take;
}

/// The validation error this module raises.
fn reject(rule: &'static str) -> TerrainError {
    TerrainError::new(
        Stage::Surface,
        ErrorKind::DescriptionInvalid {
            key: "SURFACE_RULES",
            rule,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::description::default_v1;

    /// The biome topsoil block of the fixture tables.
    const TOP: BlockId = 41;
    /// The first filler block of the fixture tables.
    const FILL: BlockId = 42;
    /// The catch-all filler block of the fixture tables.
    const CATCH: BlockId = 43;
    /// Cold beach sand.
    const COLD: BlockId = 51;
    /// Warm beach sand.
    const WARM: BlockId = 52;
    /// Shallow sea-floor material.
    const SHALLOW: BlockId = 53;
    /// Deep sea-floor material.
    const DEEP: BlockId = 54;
    /// Snow cap material.
    const SNOW: BlockId = 55;
    /// Slope stone veneer.
    const SLOPE: BlockId = 56;
    /// River bed sand.
    const SAND: BlockId = 57;
    /// River bed gravel.
    const GRAVEL: BlockId = 58;
    /// Cold shore gravel (`E4.7`).
    const SHORE_GRAVEL: BlockId = 59;
    /// Warm shelf sand (`E4.7`).
    const SHELF_SAND: BlockId = 60;
    /// Cold shelf gravel (`E4.7`).
    const SHELF_GRAVEL: BlockId = 61;
    /// Hashed shelf clay (`E4.7`).
    const SHELF_CLAY: BlockId = 62;
    /// Slope gravel (`E4.7`).
    const SLOPE_GRAVEL: BlockId = 63;
    /// Abyss clay (`E4.7`).
    const ABYSS_CLAY: BlockId = 64;
    /// Bedrock, supplied by the caller like every other id.
    const BEDROCK: BlockId = 1;

    fn table(seed: u64) -> SurfaceTable {
        SurfaceTable::new(
            seed,
            Layer::Block(COLD),
            Layer::Block(WARM),
            Layer::Block(SHALLOW),
            Layer::Block(DEEP),
            Layer::Block(SNOW),
            Layer::Block(SLOPE),
            Layer::Block(SAND),
            Layer::Block(GRAVEL),
            Layer::Block(BEDROCK),
        )
    }

    /// A table with a distinct id per `E4.7` band, so a band mix-up shows.
    fn banded_table(seed: u64) -> SurfaceTable {
        SurfaceTable::with_seabed(
            seed,
            Layer::Block(COLD),
            Layer::Block(WARM),
            SeabedMaterials {
                shore_sand: Layer::Block(SHALLOW),
                shore_gravel: Layer::Block(SHORE_GRAVEL),
                shelf_sand: Layer::Block(SHELF_SAND),
                shelf_gravel: Layer::Block(SHELF_GRAVEL),
                shelf_clay: Layer::Block(SHELF_CLAY),
                slope_gravel: Layer::Block(SLOPE_GRAVEL),
                abyss_clay: Layer::Block(ABYSS_CLAY),
            },
            Layer::Block(SNOW),
            Layer::Block(SLOPE),
            Layer::Block(SAND),
            Layer::Block(GRAVEL),
            Layer::Block(BEDROCK),
        )
    }

    /// Topsoil, one filler, then the catch-all.
    fn rules() -> BiomeRules {
        BiomeRules {
            rules: vec![
                Rule {
                    predicate: Predicate::DepthBelowTop(0),
                    block: TOP,
                    depth_blocks: 3,
                },
                Rule {
                    predicate: Predicate::DepthBelowTop(3),
                    block: FILL,
                    depth_blocks: 4,
                },
                Rule {
                    predicate: Predicate::And(Vec::new()),
                    block: CATCH,
                    depth_blocks: 8,
                },
            ],
        }
    }

    /// A table whose topsoil rule cannot fire, so the first match is a filler.
    fn filler_first() -> BiomeRules {
        BiomeRules {
            rules: vec![
                Rule {
                    predicate: Predicate::DepthBelowTop(1),
                    block: TOP,
                    depth_blocks: 3,
                },
                Rule {
                    predicate: Predicate::And(Vec::new()),
                    block: CATCH,
                    depth_blocks: 20,
                },
            ],
        }
    }

    fn facts(top: i32) -> ColumnFacts {
        ColumnFacts {
            x: 40,
            z: 90,
            top,
            base: 12.0,
            temp: 0.6,
            slope_drop: 0,
            is_river: false,
            is_lake: false,
            is_bed: false,
            near_coast: false,
            water_adjacent: false,
        }
    }

    fn strata() -> Strata {
        Strata::new(0x5EED, 24, 8.0, 256, 90, vec![80, 81, 82]).expect("the table is valid")
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn precedence_is_the_answered_order() {
        let table = table(7);
        let strata = strata();
        let rules = rules();
        let mut slots = [0_u8; SLOTS as usize];

        // Slot 1: a river bed, even on a high cold column.
        let bed = ColumnFacts {
            top: 300,
            temp: 0.1,
            base: 40.0,
            is_river: true,
            is_bed: true,
            ..facts(300)
        };
        // Slot 2: a submerged column, shallow enough for the shallow band.
        let seabed = facts(124);
        // Slot 3: the beach band, warm.
        let beach = ColumnFacts {
            top: 140,
            base: 1.5,
            ..facts(140)
        };
        // Slot 4: cold, high, dry land.
        let snow = ColumnFacts {
            top: 300,
            base: 40.0,
            temp: 0.1,
            ..facts(300)
        };
        // Slot 5: at the stone threshold.
        let slope = ColumnFacts {
            top: 200,
            slope_drop: STONE_DROP,
            ..facts(200)
        };
        // Slot 6 and 7: the two biome cases.
        let topsoil = facts(140);
        let filler_column = facts(140);
        // Slot 8: no rule matched.
        let empty = BiomeRules::default();
        let empty_column = facts(140);
        // Slot 9: nothing above bedrock.
        let floor = facts(0);

        let cases: [(u8, &ColumnFacts, &BiomeRules, Layer); 9] = [
            (1, &bed, &rules, Layer::Block(SAND)),
            (2, &seabed, &rules, Layer::Block(SHALLOW)),
            (3, &beach, &rules, Layer::Block(WARM)),
            (4, &snow, &rules, Layer::Block(SNOW)),
            (5, &slope, &rules, Layer::Block(SLOPE)),
            (6, &topsoil, &rules, Layer::Block(TOP)),
            (7, &filler_column, &filler_first(), Layer::Block(CATCH)),
            (8, &empty_column, &empty, Layer::Stone),
            (9, &floor, &rules, Layer::Block(BEDROCK)),
        ];

        for (slot, column, biome, first) in cases {
            assert_eq!(table.fired_slot(column, biome), slot, "slot {slot}");
            let stack = table.resolve_column(column, biome, &strata);
            assert_eq!(stack.slot, slot);
            assert_eq!(stack.layers[0].1, first, "slot {slot} material");
            assert_eq!(
                stack.depth_blocks(),
                column.top.max(0) + 1,
                "slot {slot} must cover the whole column"
            );
            assert_eq!(stack.layers.last().unwrap().1, Layer::Block(BEDROCK));
            slots[usize::from(slot) - 1] = slot;
        }
        assert_eq!(slots, [1, 2, 3, 4, 5, 6, 7, 8, 9]);

        // Deep sea floor takes the other band.
        let deep = table.resolve_column(&facts(100), &rules, &strata);
        assert_eq!(deep.layer_at(0), Some(Layer::Block(DEEP)));
        // A cold beach stays a beach: slot 3 wins over slot 4.
        let cold_beach = ColumnFacts {
            top: 140,
            base: 2.0,
            temp: 0.0,
            ..facts(140)
        };
        assert_eq!(table.fired_slot(&cold_beach, &rules), 3);
        assert_eq!(
            table
                .resolve_column(&cold_beach, &rules, &strata)
                .layer_at(0),
            Some(Layer::Block(COLD))
        );
        // A steep river bed takes gravel.
        let steep_bed = ColumnFacts {
            slope_drop: 4,
            is_river: true,
            is_bed: true,
            ..facts(140)
        };
        assert_eq!(
            table
                .resolve_column(&steep_bed, &rules, &strata)
                .layer_at(0),
            Some(Layer::Block(GRAVEL))
        );
    }

    #[test]
    fn no_topsoil_under_water() {
        let table = table(7);
        let strata = strata();
        let rules = rules();

        let columns = [
            ColumnFacts {
                is_river: true,
                is_bed: true,
                ..facts(140)
            },
            facts(120),
            ColumnFacts {
                base: 1.0,
                ..facts(140)
            },
            ColumnFacts {
                temp: 0.1,
                base: 40.0,
                ..facts(300)
            },
            ColumnFacts {
                slope_drop: 3,
                ..facts(200)
            },
        ];

        for column in columns {
            let slot = table.fired_slot(&column, &rules);
            assert!((1..=5).contains(&slot), "fixture must fire a locked slot");
            assert!(
                table
                    .resolve(&column, &rules)
                    .iter()
                    .all(|(_, block)| *block != TOP),
                "slot {slot} emitted a biome topsoil rule"
            );
            let stack = table.resolve_column(&column, &rules, &strata);
            assert!(
                !stack
                    .layers
                    .iter()
                    .any(|(_, layer)| *layer == Layer::Block(TOP)),
                "slot {slot} emitted a biome topsoil layer"
            );
            // The biome filler is still available below the surface material,
            // except on a cliff (stone all the way) and a sea floor (three
            // band blocks then stone, `E4.7`).
            if slot != 5 && slot != 2 {
                assert!(
                    stack
                        .layers
                        .iter()
                        .any(|(_, layer)| *layer == Layer::Block(FILL)
                            || *layer == Layer::Block(CATCH)),
                    "slot {slot} dropped the biome filler as well"
                );
            }
        }
    }

    /// A column whose sea-floor dither and shelf clay hashes are both quiet.
    fn quiet_seabed_column(table: &SurfaceTable) -> (i32, i32) {
        for z in 0..64 {
            for x in 0..64 {
                let column = ColumnFacts {
                    x,
                    z,
                    top: SEA - 10,
                    ..facts(SEA - 10)
                };
                if !table.seabed_dithers(&column) && !table.shelf_clay_patch(&column) {
                    return (x, z);
                }
            }
        }
        panic!("the fixture found no quiet sea-floor column");
    }

    #[test]
    fn depth_below_sea_in_is_an_inclusive_offset_band() {
        let table = banded_table(7);
        let on_sea = facts(SEA);
        let below = |depth: i32| ColumnFacts {
            top: SEA - depth,
            ..facts(SEA - depth)
        };

        assert!(!holds(&Predicate::DepthBelowSeaIn(1, 4), &on_sea, 0));
        assert!(holds(&Predicate::DepthBelowSeaIn(1, 4), &below(1), 0));
        assert!(holds(&Predicate::DepthBelowSeaIn(1, 4), &below(4), 0));
        assert!(!holds(&Predicate::DepthBelowSeaIn(1, 4), &below(5), 0));
        assert!(holds(&Predicate::DepthBelowSeaIn(5, 14), &below(5), 0));
        assert!(holds(&Predicate::DepthBelowSeaIn(5, 14), &below(14), 0));
        assert!(!holds(&Predicate::DepthBelowSeaIn(5, 14), &below(15), 0));

        // It owns the locked sea-floor slot, so a biome rule cannot use it.
        assert!(locked_predicate(&Predicate::DepthBelowSeaIn(1, 4)));
        let early = BiomeRules {
            rules: vec![
                Rule {
                    predicate: Predicate::DepthBelowSeaIn(1, 4),
                    block: TOP,
                    depth_blocks: 3,
                },
                Rule {
                    predicate: Predicate::And(Vec::new()),
                    block: CATCH,
                    depth_blocks: 4,
                },
            ],
        };
        assert!(table.validate_table(&early).is_err());
    }

    #[test]
    fn each_seafloor_band_has_its_known_depth_and_material() {
        let table = banded_table(7);
        let strata = strata();
        let rules = rules();
        let (x, z) = quiet_seabed_column(&table);
        let column = |depth: i32| ColumnFacts {
            x,
            z,
            top: SEA - depth,
            temp: 0.6,
            ..facts(SEA - depth)
        };

        let cases = [
            (1, SHALLOW),
            (2, SHALLOW),
            (4, SHALLOW),
            (5, SHELF_SAND),
            (9, SHELF_SAND),
            (14, SHELF_SAND),
            (15, SLOPE_GRAVEL),
            (24, SLOPE_GRAVEL),
            (25, ABYSS_CLAY),
            (40, ABYSS_CLAY),
        ];

        for (depth, expected) in cases {
            let facts = column(depth);
            assert_eq!(table.fired_slot(&facts, &rules), 2, "depth {depth}");
            let stack = table.resolve_column(&facts, &rules, &strata);
            assert_eq!(
                stack.layers[0],
                (SEABED_LAYER_DEPTH, Layer::Block(expected)),
                "sea-floor depth {depth}"
            );
            assert_eq!(
                stack.layers[1].1,
                Layer::Stone,
                "band material is exactly three blocks, then symbolic stone"
            );
            assert_eq!(
                stack.depth_blocks(),
                facts.top + 1,
                "the sea-floor stack still covers the whole column"
            );
            assert_eq!(stack.layers.last().unwrap().1, Layer::Block(BEDROCK));
            assert!(
                !stack
                    .layers
                    .iter()
                    .any(|(_, layer)| *layer == Layer::Block(TOP)),
                "biome topsoil appeared under water at depth {depth}"
            );
        }
    }

    #[test]
    fn cold_seafloor_variants_pick_gravel() {
        let table = banded_table(7);
        let strata = strata();
        let rules = rules();
        let (x, z) = quiet_seabed_column(&table);
        let layer = |depth: i32, temp: f32| {
            let facts = ColumnFacts {
                x,
                z,
                top: SEA - depth,
                temp,
                ..facts(SEA - depth)
            };
            table.resolve_column(&facts, &rules, &strata).layers[0].1
        };

        // Warm is sand; cold shore and cold shelf are gravel.
        assert_eq!(layer(2, 0.6), Layer::Block(SHALLOW));
        assert_eq!(layer(2, 0.0), Layer::Block(SHORE_GRAVEL));
        assert_eq!(layer(10, 0.6), Layer::Block(SHELF_SAND));
        assert_eq!(layer(10, 0.0), Layer::Block(SHELF_GRAVEL));

        // The thresholds are the named constants, not magic values.
        assert_eq!(layer(2, T_COLD_SHORE - 0.01), Layer::Block(SHORE_GRAVEL));
        assert_eq!(layer(2, T_COLD_SHORE), Layer::Block(SHALLOW));
        assert_eq!(layer(10, T_COLD_SHELF - 0.01), Layer::Block(SHELF_GRAVEL));
        assert_eq!(layer(10, T_COLD_SHELF), Layer::Block(SHELF_SAND));

        // A warm shelf column with a hashed patch takes clay instead of sand.
        let mut patched = None;
        'scan: for scan_z in 0..128 {
            for scan_x in 0..128 {
                let facts = ColumnFacts {
                    x: scan_x,
                    z: scan_z,
                    top: SEA - 10,
                    temp: 0.6,
                    ..facts(SEA - 10)
                };
                if table.shelf_clay_patch(&facts) {
                    patched = Some(table.resolve_column(&facts, &rules, &strata).layers[0].1);
                    break 'scan;
                }
            }
        }
        assert_eq!(patched, Some(Layer::Block(SHELF_CLAY)));
    }

    #[test]
    fn seafloor_dither_only_fires_at_band_edges() {
        let table = banded_table(7);
        let strata = strata();
        let rules = rules();

        let normal = |facts: &ColumnFacts, depth: i32| -> BlockId {
            if depth <= SEABED_SHORE_LAST_DEPTH {
                if facts.temp < T_COLD_SHORE {
                    SHORE_GRAVEL
                } else {
                    SHALLOW
                }
            } else if depth <= SEABED_SHELF_LAST_DEPTH {
                if facts.temp < T_COLD_SHELF {
                    SHELF_GRAVEL
                } else if table.shelf_clay_patch(facts) {
                    SHELF_CLAY
                } else {
                    SHELF_SAND
                }
            } else if depth <= SEABED_SLOPE_LAST_DEPTH {
                SLOPE_GRAVEL
            } else {
                ABYSS_CLAY
            }
        };
        let deeper = |depth: i32| -> BlockId {
            if depth <= SEABED_SHELF_LAST_DEPTH {
                SLOPE_GRAVEL
            } else {
                ABYSS_CLAY
            }
        };
        let material =
            |facts: &ColumnFacts| table.resolve_column(facts, &rules, &strata).layers[0].1;

        // Interior depths ignore the hash entirely.
        for depth in [1, 2, 3, 5, 9, 13, 15, 20, 23, 25, 31] {
            for z in 0..32 {
                for x in 0..32 {
                    let facts = ColumnFacts {
                        x,
                        z,
                        top: SEA - depth,
                        temp: 0.6,
                        ..facts(SEA - depth)
                    };
                    assert_eq!(
                        material(&facts),
                        Layer::Block(normal(&facts, depth)),
                        "interior depth {depth} at {x},{z}"
                    );
                }
            }
        }

        // At the last depth of a band only, both materials appear.
        for depth in [
            SEABED_SHORE_LAST_DEPTH,
            SEABED_SHELF_LAST_DEPTH,
            SEABED_SLOPE_LAST_DEPTH,
        ] {
            let mut base = 0_u32;
            let mut next = 0_u32;
            for z in 0..64 {
                for x in 0..64 {
                    let facts = ColumnFacts {
                        x,
                        z,
                        top: SEA - depth,
                        temp: 0.6,
                        ..facts(SEA - depth)
                    };
                    let got = material(&facts);
                    let expected = if table.seabed_dithers(&facts) {
                        Layer::Block(deeper(depth))
                    } else {
                        Layer::Block(normal(&facts, depth))
                    };
                    assert_eq!(got, expected, "edge depth {depth} at {x},{z}");
                    if got == Layer::Block(normal(&facts, depth)) {
                        base += 1;
                    } else {
                        next += 1;
                    }
                }
            }
            assert!(base > 0 && next > 0, "edge depth {depth} was not ragged");
        }

        // The dither is keyed on `(x, z)` only: the same column with a
        // different top still takes the same decision.
        let edge = ColumnFacts {
            x: 11,
            z: 23,
            top: SEA - SEABED_SHORE_LAST_DEPTH,
            ..facts(SEA - SEABED_SHORE_LAST_DEPTH)
        };
        let shifted = ColumnFacts {
            top: SEA - SEABED_SHORE_LAST_DEPTH - 1,
            ..edge
        };
        assert_eq!(table.seabed_dithers(&edge), table.seabed_dithers(&shifted));
    }

    #[test]
    fn bed_beats_everything() {
        let table = table(7);
        let column = ColumnFacts {
            top: 300,
            base: 1.0,
            temp: 0.0,
            slope_drop: 9,
            is_river: true,
            is_bed: true,
            near_coast: true,
            water_adjacent: true,
            ..facts(300)
        };
        assert_eq!(table.fired_slot(&column, &rules()), 1);
        let stack = table.resolve_column(&column, &rules(), &strata());
        assert_eq!(stack.slot, 1);
        assert_eq!(stack.layer_at(0), Some(Layer::Block(GRAVEL)));
    }

    #[test]
    fn snow_needs_cold_and_dry_land() {
        let table = table(7);
        let rules = rules();

        assert_eq!(T_SNOW.to_bits(), default_v1().t_snow.to_bits());

        let land = ColumnFacts {
            top: 300,
            base: 40.0,
            temp: 0.1,
            ..facts(300)
        };
        assert_eq!(table.fired_slot(&land, &rules), 4);

        let sea_floor = facts(110);
        assert_eq!(table.fired_slot(&sea_floor, &rules), 2);

        let beach = ColumnFacts {
            top: 140,
            base: 1.0,
            temp: 0.0,
            ..facts(140)
        };
        assert_eq!(table.fired_slot(&beach, &rules), 3);

        // The jitter is deterministic, in range, and actually varies.
        let mut seen = [false; 5];
        for x in -8..=8 {
            for z in -8..=8 {
                let jitter = snow_jitter(7, x, z);
                assert!((-2..=2).contains(&jitter), "{x},{z} gave {jitter}");
                assert_eq!(jitter, snow_jitter(7, x, z));
                seen[usize::try_from(jitter + 2).unwrap()] = true;
            }
        }
        assert_eq!(seen.iter().filter(|hit| **hit).count(), 5);

        // The cap's thickness uses the same jitter, clamped to at least a block.
        for x in 0..32 {
            for z in 0..32 {
                let column = ColumnFacts { x, z, ..land };
                let stack = table.resolve_column(&column, &rules, &strata());
                let depth = stack.layers[0].0;
                assert!((1..=SNOW_LAYER_DEPTH + 2).contains(&depth), "{depth}");
            }
        }
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn slope_stone_and_its_dither_band() {
        let table = table(7);
        let rules = rules();

        let stone_at_two = ColumnFacts {
            top: 200,
            slope_drop: 2,
            ..facts(200)
        };
        assert_eq!(table.fired_slot(&stone_at_two, &rules), 5);

        let never = ColumnFacts {
            top: 200,
            slope_drop: 0,
            ..facts(200)
        };
        for x in 0..64 {
            for z in 0..64 {
                let column = ColumnFacts { x, z, ..never };
                assert_ne!(table.fired_slot(&column, &rules), 5, "{x},{z}");
            }
        }

        let mut hits = 0_i32;
        let mut total = 0_i32;
        for x in 0..64 {
            for z in 0..64 {
                let column = ColumnFacts {
                    x,
                    z,
                    slope_drop: 1,
                    ..facts(200)
                };
                total += 1;
                if table.fired_slot(&column, &rules) == 5 {
                    hits += 1;
                }
            }
        }
        assert_eq!(total, 64 * 64);
        let share = hits as f32 / total as f32;
        assert!(
            (0.30..=0.40).contains(&share),
            "the dither band covered {share} of {total} columns"
        );
    }

    #[test]
    fn table_validation_rejects_bad_rules() {
        let table = table(7);
        table
            .validate_table(&rules())
            .expect("the fixture is valid");

        // A biome rule that claims a locked slot.
        let early = BiomeRules {
            rules: vec![
                Rule {
                    predicate: Predicate::SlopeAtLeast(3),
                    block: TOP,
                    depth_blocks: 3,
                },
                Rule {
                    predicate: Predicate::And(Vec::new()),
                    block: FILL,
                    depth_blocks: 4,
                },
            ],
        };
        let error = table.validate_table(&early).expect_err("slot 5 is locked");
        assert_eq!(error.stage, Stage::Surface);
        assert!(
            matches!(
                error.kind,
                ErrorKind::DescriptionInvalid {
                    key: "SURFACE_RULES",
                    ..
                }
            ),
            "{error:?}"
        );

        // No catch-all.
        let open = BiomeRules {
            rules: vec![Rule {
                predicate: Predicate::DepthBelowTop(2),
                block: TOP,
                depth_blocks: 3,
            }],
        };
        assert!(table.validate_table(&open).is_err());

        // An empty list has no catch-all either.
        assert!(table.validate_table(&BiomeRules::default()).is_err());

        // More entries than a content pack may hold.
        let mut many = Vec::with_capacity(MAX_BIOMES + 1);
        for _ in 0..MAX_BIOMES {
            many.push(Rule {
                predicate: Predicate::DepthBelowTop(1),
                block: FILL,
                depth_blocks: 1,
            });
        }
        many.push(Rule {
            predicate: Predicate::And(Vec::new()),
            block: CATCH,
            depth_blocks: 1,
        });
        let crowded = BiomeRules { rules: many };
        assert!(table.validate_table(&crowded).is_err());

        // A zero-thickness rule would shift every later rule up a slot.
        let flat = BiomeRules {
            rules: vec![
                Rule {
                    predicate: Predicate::And(Vec::new()),
                    block: TOP,
                    depth_blocks: 0,
                },
                Rule {
                    predicate: Predicate::And(Vec::new()),
                    block: CATCH,
                    depth_blocks: 4,
                },
            ],
        };
        assert!(table.validate_table(&flat).is_err());
    }

    #[test]
    fn stone_layer_stays_symbolic() {
        let table = table(7);
        let strata = strata();
        let column = facts(140);

        // No biome rules means no biome contribution: the surface is stone.
        let stack = table.resolve_column(&column, &BiomeRules::default(), &strata);
        assert_eq!(stack.slot, 8);
        assert_eq!(stack.layers[0], (140, Layer::Stone));
        assert!(
            stack.layers[1..]
                .iter()
                .all(|(_, layer)| *layer == Layer::Block(BEDROCK)),
            "only bedrock follows the stone body"
        );

        // The symbolic marker resolves through the strata resolver.
        let resolved = stack
            .block_at(0, &strata, column.x, column.top, column.z)
            .expect("depth 0 is inside the column");
        assert_eq!(resolved, strata.stone_at(column.x, column.top, column.z));
        assert_ne!(resolved, BEDROCK);

        // The cache the stack carries agrees with a fresh column evaluation.
        assert_eq!(
            strata.stone_in(stack.column, column.x, column.top, column.z),
            strata.stone_at(column.x, column.top, column.z)
        );
    }
}

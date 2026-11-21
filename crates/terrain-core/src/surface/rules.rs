//! The surface rule vocabulary, the ordered rule table, and column resolution.
//!
//! `E4.1`-`E4.5`. [`Predicate`] is the closed condition vocabulary, [`Rule`] one
//! ordered entry, [`BiomeRules`] one biome's list, and [`SurfaceTable`] the shared
//! evaluator. The locked-slot materials live in `seafloor`; the table calls them
//! from [`SurfaceTable::resolve_column`].

use crate::description::SEA;
use crate::error::{ErrorKind, Stage, TerrainError};
use crate::hash::{domain, hash01, hash2};
use crate::section::BlockId;
use crate::strata::{ColumnStrata, Strata};

use super::seafloor::{
    BEACH_LAYER_DEPTH, BED_LAYER_DEPTH, SEABED_LAYER_DEPTH, SEAFLOOR_CLAY_DOMAIN,
    SEAFLOOR_DITHER_DOMAIN, SeabedMaterials,
};

/// Drop, in blocks, at which a slope exposes stone (`E4.3`).
pub(crate) const STONE_DROP: i32 = 2;

/// Share of columns in the dither band that still expose stone (`E4.3`).
pub(crate) const DITHER_SHARE: f32 = 0.35;

/// Temperature below which a snow cap is placed (`E4.4`).
///
/// Mirrors the description default `T_SNOW`; a test pins the two together so the
/// constant cannot drift from the schema.
pub(crate) const T_SNOW: f32 = 0.25;

/// Number of precedence slots (`E4.5`).
pub(crate) const SLOTS: u8 = 9;

/// Most biomes a content pack may declare (`E3.1`).
pub(crate) const MAX_BIOMES: usize = 255;

/// Lowest base offset that reads as beach, in blocks relative to sea (`C1.6`).
const BEACH_BASE_MIN: f32 = 0.0;

/// Highest base offset that reads as beach, in blocks relative to sea (`C1.6`).
const BEACH_BASE_MAX: f32 = 3.0;

/// Base thickness of a snow cap, in blocks, before the ±2 jitter (`E4.4`).
pub(super) const SNOW_LAYER_DEPTH: i32 = 2;

/// Thickness of the slope-stone veneer, in blocks (`E4.3`).
const SLOPE_LAYER_DEPTH: i32 = 3;

/// Domain tag of the slope dither (`E4.3`).
const STONE_DITHER_DOMAIN: &str = "stone_dither";

/// Domain tag of the snow jitter (`E4.4`).
const SNOW_JITTER_DOMAIN: &str = "snow_jitter";

/// How a height compares with a level (`E4.1`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) enum Order {
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
pub(crate) enum Predicate {
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
pub(crate) struct Rule {
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
pub(crate) struct BiomeRules {
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
pub(crate) struct ColumnFacts {
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
pub(crate) enum Layer {
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
pub(crate) struct SurfaceStack {
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
    pub(crate) fn depth_blocks(&self) -> i32 {
        self.layers.iter().map(|(depth, _)| *depth).sum()
    }

    /// The layer covering a depth below the surface.
    ///
    /// Depth `0` is the top block. `None` means the depth is below the column.
    #[must_use]
    pub(crate) fn layer_at(&self, depth: i32) -> Option<Layer> {
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
    pub(crate) fn block_at(
        &self,
        depth: i32,
        strata: &Strata,
        x: i32,
        y: i32,
        z: i32,
    ) -> Option<BlockId> {
        match self.layer_at(depth)? {
            Layer::Block(id) => Some(id),
            Layer::Stone => Some(strata.stone_in(self.column, x, y, z)),
        }
    }
}

/// The nine caller-supplied materials plus the two hash key streams (`E4.5`).
///
/// No block id is hardcoded here: every slot, including bedrock, is data.
#[derive(Clone, Debug)]
pub(crate) struct SurfaceTable {
    /// World seed. Keys the slope dither and the snow jitter.
    pub(super) seed: u64,
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
    pub(super) seafloor_tag: u64,
    /// Domain tag of the sea-floor clay patches, hashed once at construction.
    pub(super) clay_tag: u64,
}

impl SurfaceTable {
    /// Wraps the nine materials and caches the two hash domain tags.
    ///
    /// Tag caching changes no output: the tags are pure functions of their
    /// names. It only keeps the per-column path free of a SHA-256.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub(crate) fn new(
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
    pub(crate) fn with_seabed(
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
    pub(crate) fn seed(&self) -> u64 {
        self.seed
    }

    /// The precedence slot that supplies this column's surface material
    /// (`E4.5`).
    ///
    /// Returns `1..=9`. Slot 9 is returned for a column whose top is `y = 0`,
    /// because bedrock is the absolute floor (`C4.1`).
    #[must_use]
    pub(crate) fn fired_slot(&self, facts: &ColumnFacts, biome_rules: &BiomeRules) -> u8 {
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
    pub(crate) fn resolve(
        &self,
        column: &ColumnFacts,
        biome_rules: &BiomeRules,
    ) -> Vec<(i32, BlockId)> {
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
    pub(crate) fn resolve_column(
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
    #[allow(clippy::unused_self)] // method shape kept: visibility-only change
    pub(crate) fn validate_table(&self, biome_rules: &BiomeRules) -> Result<(), TerrainError> {
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
pub(crate) fn snow_jitter(seed: u64, x: i32, z: i32) -> i32 {
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
pub(super) fn holds(predicate: &Predicate, facts: &ColumnFacts, depth: i32) -> bool {
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
pub(super) fn locked_predicate(predicate: &Predicate) -> bool {
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

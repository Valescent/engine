//! Applying gathered candidates to one section: the write order, the
//! per-block sink, and the `H2` filters (`H2`, `H2.1`, `H2.2`, `H2.4`).

use crate::description::SECTION_VOLUME;
use crate::error::Stage;
use crate::section::{BlockId, Section, section_index};

use super::discover::{
    Candidate, FeatureKind, FeatureSpec, GATHER_REGIONS, REGION_SIZE, region_slot, uniform_writable,
};
use super::masks::{ColumnField, Mask, OreBlock, Palette, mask_accepts};
use super::{ore, scatter, tree};

/// The write order, as a constant (`H2.1`).
///
/// Position `0` is terrain, which this module never writes; positions `1..=4`
/// are the feature passes. [`Stage`] has no per-feature variant, so boulders,
/// trees, and plants share [`Stage::Features`]: the *position* in this array is
/// the contract, and [`WRITE_PASSES`] maps each position to its kind.
pub(crate) const WRITE_ORDER: [Stage; 5] = [
    Stage::Volume,
    Stage::Strata,
    Stage::Features,
    Stage::Features,
    Stage::Features,
];

/// The feature kind applied at each [`WRITE_ORDER`] position, `None` for terrain.
pub(crate) const WRITE_PASSES: [Option<FeatureKind>; 5] = [
    None,
    Some(FeatureKind::Ore),
    Some(FeatureKind::Boulder),
    Some(FeatureKind::Tree),
    Some(FeatureKind::Scatter),
];

/// The stage a feature pass is attributed to in [`WRITE_ORDER`].
#[must_use]
pub(crate) const fn stage_for(kind: FeatureKind) -> Stage {
    match kind {
        FeatureKind::Ore => Stage::Strata,
        FeatureKind::Boulder | FeatureKind::Tree | FeatureKind::Scatter => Stage::Features,
    }
}

/// Everything application needs besides the block array (`H2.2`, `H3.4`).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ApplyContext<'a> {
    /// World seed, used by scatter and tree acceptance.
    pub seed: u64,
    /// Structural block ids.
    pub palette: Palette,
    /// Block id per ore spec, matched by stable name.
    pub ores: &'a [OreBlock],
    /// Ore and boulder host set. Must not contain air, water, or bedrock.
    pub host_rock: &'a [BlockId],
    /// Column data for the `3 x 3` footprint.
    pub columns: ColumnField<'a>,
    /// The `3 x 3 x 3` region origins around this section, by [`region_slot`].
    pub regions: [[i32; 3]; GATHER_REGIONS],
    /// Block ids a scattered plant may write.
    pub scatter: &'a [BlockId],
}

impl ApplyContext<'_> {
    /// The section's own origin, aligned to [`REGION_SIZE`].
    #[must_use]
    pub(crate) fn section_origin(&self) -> [i32; 3] {
        self.regions[region_slot([0, 0, 0])]
    }
}

/// What one [`apply`] pass did.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ApplyStats {
    /// Block writes that changed a cell.
    pub writes: u32,
    /// Candidates offered to the pass.
    pub candidates: u32,
    /// Block tests refused by a mask, plus candidates refused by a filter.
    pub skipped_mask: u32,
}

/// [`ApplyStats`] plus the number of per-block tests performed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ApplyReport {
    /// Write, skip, and candidate counts.
    pub stats: ApplyStats,
    /// Blocks that passed the bounding-box cull and reached a mask test.
    pub probes: u32,
}

/// Writes gathered candidates into one section (`H2`, `H2.1`, `H2.4`).
#[must_use]
pub(crate) fn apply(
    blocks: &mut [BlockId; SECTION_VOLUME],
    candidates: &[Candidate],
    ctx: &ApplyContext<'_>,
    specs: &[FeatureSpec],
) -> ApplyStats {
    apply_counted(blocks, candidates, ctx, specs).stats
}

/// [`apply`] with the per-block test counter exposed.
///
/// The counter is what proves a `Uniform` section costs nothing (`H2.2`).
#[must_use]
pub(crate) fn apply_counted(
    blocks: &mut [BlockId; SECTION_VOLUME],
    candidates: &[Candidate],
    ctx: &ApplyContext<'_>,
    specs: &[FeatureSpec],
) -> ApplyReport {
    let mut stats = ApplyStats {
        writes: 0,
        candidates: u32::try_from(candidates.len()).unwrap_or(u32::MAX),
        skipped_mask: 0,
    };
    let mut probes = 0_u32;

    // Application order is id order whatever order the caller supplied.
    let mut ordered: Vec<&Candidate> = candidates.iter().collect();
    ordered.sort_unstable();

    {
        let mut sink = Sink {
            blocks,
            origin: ctx.section_origin(),
            ctx,
            stats: &mut stats,
            probes: &mut probes,
        };
        for (stage, kind) in WRITE_ORDER.into_iter().zip(WRITE_PASSES) {
            if let Some(kind) = kind {
                debug_assert_eq!(
                    stage,
                    stage_for(kind),
                    "write order and pass table disagree"
                );
                match kind {
                    FeatureKind::Ore | FeatureKind::Boulder => {
                        ore::apply(&mut sink, &ordered, specs, kind);
                    }
                    FeatureKind::Tree => tree::apply(&mut sink, &ordered),
                    FeatureKind::Scatter => scatter::apply(&mut sink, specs),
                }
            }
        }
    }

    ApplyReport { stats, probes }
}

/// Applies features to a [`Section`], skipping only uniform sections nothing can write.
///
/// A candidate's bounding box is culled against the section before any per-block
/// test. A uniform section is then skipped only when no intersecting
/// `(candidate, mask)` pair can legally write its one id (`H2.2`): an all-air
/// section under a reaching canopy is processed, while uniform stone under a
/// plant is free, and uniform water or bedrock is never writable. The decoded
/// blocks are copied back only for a section that received a write.
#[must_use]
pub(crate) fn apply_section(
    section: &mut Section,
    candidates: &[Candidate],
    ctx: &ApplyContext<'_>,
    specs: &[FeatureSpec],
) -> ApplyStats {
    apply_section_counted(section, candidates, ctx, specs).stats
}

/// [`apply_section`] with the per-block test counter exposed.
#[must_use]
pub(crate) fn apply_section_counted(
    section: &mut Section,
    candidates: &[Candidate],
    ctx: &ApplyContext<'_>,
    specs: &[FeatureSpec],
) -> ApplyReport {
    // `H2.2`: the uniform exemption is decided by the feature's own write mask,
    // never by "solid versus air" alone. A canopy that crosses a section top
    // must reach the all-air section above it, while a uniform block no mask can
    // accept stays free. Without this rule a tree based in the top layer loses
    // the leaves above the seam.
    let origin = ctx.section_origin();
    if section.is_uniform().is_some_and(|id| {
        !uniform_writable(id, origin, candidates, ctx.palette, ctx.host_rock, specs)
    }) {
        return ApplyReport {
            stats: ApplyStats {
                writes: 0,
                candidates: u32::try_from(candidates.len()).unwrap_or(u32::MAX),
                skipped_mask: 0,
            },
            probes: 0,
        };
    }

    let mut blocks: Box<[BlockId; SECTION_VOLUME]> = vec![0_u16; SECTION_VOLUME]
        .into_boxed_slice()
        .try_into()
        .expect("the scratch array matches the section volume");
    section.write_blocks(&mut blocks);
    let report = apply_counted(&mut blocks, candidates, ctx, specs);
    if report.stats.writes > 0 {
        for (index, id) in blocks.iter().enumerate() {
            section.set_index(index, *id);
        }
    }
    report
}

/// Per-block writes for one section, with the cull and mask rules in one place.
pub(crate) struct Sink<'w, 'c> {
    /// The section's cells, `x | z << 5 | y << 10`.
    blocks: &'w mut [BlockId; SECTION_VOLUME],
    /// World origin of the section.
    origin: [i32; 3],
    /// Shared application context.
    ctx: &'c ApplyContext<'c>,
    /// Counters shared with [`apply_counted`].
    stats: &'w mut ApplyStats,
    /// Per-block test counter.
    probes: &'w mut u32,
}

impl<'c> Sink<'_, 'c> {
    /// The shared context.
    pub(crate) fn ctx(&self) -> &'c ApplyContext<'c> {
        self.ctx
    }

    /// The section's world origin.
    pub(crate) fn origin(&self) -> [i32; 3] {
        self.origin
    }

    /// Records one candidate refused by a filter.
    pub(crate) fn skip_candidate(&mut self) {
        self.stats.skipped_mask = self.stats.skipped_mask.saturating_add(1);
    }

    /// Whether a world-space box intersects the section (`H2.2`).
    ///
    /// This is the bounding-box cull: it runs before any per-block test, so a
    /// candidate that cannot touch the section costs one comparison per axis.
    pub(crate) fn intersects(&self, low: [i32; 3], high: [i32; 3]) -> bool {
        (0..3).all(|axis| {
            low[axis] < self.origin[axis] + REGION_SIZE && high[axis] >= self.origin[axis]
        })
    }

    /// Reads one world block inside the section.
    pub(crate) fn block(&self, position: [i32; 3]) -> Option<BlockId> {
        let local = self.local(position)?;
        Some(
            self.blocks[section_index(
                local[0].cast_unsigned(),
                local[1].cast_unsigned(),
                local[2].cast_unsigned(),
            )],
        )
    }

    /// Writes one world block if its mask allows it (`H2.3`).
    pub(crate) fn write(&mut self, position: [i32; 3], mask: Mask, id: BlockId) {
        let Some(local) = self.local(position) else {
            return;
        };
        *self.probes = self.probes.saturating_add(1);
        let index = section_index(
            local[0].cast_unsigned(),
            local[1].cast_unsigned(),
            local[2].cast_unsigned(),
        );
        let current = self.blocks[index];
        if !mask_allows(mask, current, self.ctx) {
            self.stats.skipped_mask = self.stats.skipped_mask.saturating_add(1);
            return;
        }
        if current == id {
            return;
        }
        self.blocks[index] = id;
        self.stats.writes = self.stats.writes.saturating_add(1);
    }

    /// Local cell of a world position, or `None` outside the section.
    fn local(&self, position: [i32; 3]) -> Option<[i32; 3]> {
        let local = [
            position[0] - self.origin[0],
            position[1] - self.origin[1],
            position[2] - self.origin[2],
        ];
        if local.iter().all(|axis| (0..REGION_SIZE).contains(axis)) {
            Some(local)
        } else {
            None
        }
    }
}

/// Whether one block write is legal (`H2.3`).
///
/// Water and bedrock are refused before the mask is consulted, so no mask can
/// ever overwrite them.
fn mask_allows(mask: Mask, current: BlockId, ctx: &ApplyContext<'_>) -> bool {
    mask_accepts(mask, current, ctx.palette, ctx.host_rock)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::description::SEA;
    use crate::features::tests::*;
    use crate::features::{COLUMN_BANK, v1_specs, variant_with_spec};
    #[test]
    fn overlaps_resolve_to_the_lowest_id() {
        let specs = v1_specs();
        let flat = Flat::new([0, 0, 0], 20);
        let ctx = flat.ctx();
        let origin = [16, 16, 16];

        let mut blocks = scratch(STONE);
        let low = ore_candidate(10, 0, origin);
        let high = ore_candidate(20, 1, origin);
        let stats = apply(&mut blocks, &[high, low], &ctx, &specs);

        assert_eq!(blocks[index(16, 16, 16)], IRON, "the lower id must win");
        assert!(stats.skipped_mask > 0, "the loser must be refused");
        assert_eq!(stats.candidates, 2);
    }

    #[test]
    fn ores_only_replace_host_rock() {
        let specs = v1_specs();
        let flat = Flat::new([0, 0, 0], 20);
        let ctx = flat.ctx();

        let mut base = scratch(AIR);
        for y in 0..32 {
            for z in 0..32 {
                for x in 0..32 {
                    base[index(x, y, z)] = match y {
                        0..=1 => BEDROCK,
                        2..=3 => DEEP,
                        4..=7 => STONE,
                        8 => SAND,
                        9 => WATER,
                        _ => AIR,
                    };
                }
            }
        }

        let mut total_skips = 0_u32;
        let mut total_writes = 0_u32;
        for variant in 0..64_u64 {
            let mut blocks = base.clone();
            let candidate = Candidate {
                id: 7,
                region: [0, 0, 0],
                kind: FeatureKind::Ore,
                origin: [16, 6, 16],
                variant: variant_with_spec(0, variant.wrapping_mul(0x9e37_79b9)),
            };
            let stats = apply(&mut blocks, &[candidate], &ctx, &specs);
            total_skips += stats.skipped_mask;
            total_writes += stats.writes;

            for (slot, current) in blocks.iter().enumerate() {
                let before = base[slot];
                if matches!(before, AIR | WATER | BEDROCK | SAND) {
                    assert_eq!(*current, before, "slot {slot} was not host rock");
                } else {
                    assert!(
                        *current == before || *current == IRON,
                        "slot {slot} left the host set"
                    );
                }
            }
        }
        assert!(total_writes > 0, "no ore ever wrote");
        assert!(total_skips > 0, "no mask refusal was counted");
    }

    #[test]
    fn write_order_is_fixed() {
        let specs = v1_specs();
        let mut flat = Flat::new([0, 128, 0], 140);
        flat.set_column(16, 16, 140, 0, 3, 0);
        let ctx = flat.ctx();

        let mut blocks = scratch(AIR);
        for y in 0..=12 {
            for z in 0..32 {
                for x in 0..32 {
                    blocks[index(x, y, z)] = STONE;
                }
            }
        }
        // A stone shelf the canopy reaches through, at world y = 147.
        blocks[index(17, 19, 16)] = STONE;

        let ore = ore_candidate(1, 0, [17, 147, 16]);
        let tree = (0..2048_u64)
            .map(|id| tree_candidate(id, [16, 140, 16]))
            .find(|candidate| {
                tree::accepts(SEED, candidate, 3)
                    && tree::place(candidate).iter().any(|placement| {
                        placement.layer == tree::Layer::Leaf && placement.offset == [1, 6, 0]
                    })
            })
            .expect("no canopy covers the shelf");
        assert!(
            tree::place(&tree).iter().any(|placement| {
                placement.layer == tree::Layer::Leaf && placement.offset == [1, 6, 0]
            }),
            "the canopy must cover the shelf for this test to mean anything"
        );

        let stats = apply(&mut blocks, &[tree, ore], &ctx, &specs);
        assert_eq!(
            blocks[index(17, 19, 16)],
            IRON,
            "the ore survived the tree pass"
        );
        assert!(stats.writes > 0, "the tree wrote nothing");
    }

    #[test]
    fn tree_leaves_stay_within_four_of_a_stem() {
        let limit = tree::LEAF_MAX_DIST * tree::LEAF_MAX_DIST;
        for id in 0..64_u64 {
            let candidate = tree_candidate(id, [stable_origin(id), 140, stable_origin(id + 1)]);
            let placements = tree::place(&candidate);
            let stems: Vec<[i32; 3]> = placements
                .iter()
                .filter(|placement| placement.layer == tree::Layer::Log)
                .map(|placement| placement.offset)
                .collect();
            assert!(
                placements
                    .iter()
                    .any(|placement| placement.layer == tree::Layer::Log),
                "a tree without a stem"
            );
            for placement in &placements {
                if placement.layer != tree::Layer::Leaf {
                    continue;
                }
                let near = stems.iter().any(|stem| {
                    let dx = stem[0] - placement.offset[0];
                    let dy = stem[1] - placement.offset[1];
                    let dz = stem[2] - placement.offset[2];
                    dx * dx + dy * dy + dz * dz <= limit
                });
                assert!(near, "leaf {} floats free", placement.offset[0]);
            }
        }
    }

    #[test]
    fn spacing_beats_density() {
        let specs = tree_specs();
        let mut flat = Flat::new([0, 128, 0], 140);
        flat.set_column(16, 16, 140, 0, 3, 0);
        flat.set_column(19, 16, 140, 0, 3, 0);
        let ctx = flat.ctx();

        let mut pair = None;
        'search: for left in 0..512_u64 {
            for right in left + 1..512_u64 {
                let first = tree_candidate(left, [16, 140, 16]);
                let second = tree_candidate(right, [19, 140, 16]);
                if !tree::accepts(SEED, &first, 3) || !tree::accepts(SEED, &second, 3) {
                    continue;
                }
                let champion = if tree::beats(&first, &second) {
                    first
                } else {
                    second
                };
                pair = Some((first, second, champion));
                break 'search;
            }
        }
        let (first, second, champion) = pair.expect("no accepted pair found");

        let mut forward = scratch(AIR);
        let forward_stats = apply(&mut forward, &[first, second], &ctx, &specs);
        let mut backward = scratch(AIR);
        let backward_stats = apply(&mut backward, &[second, first], &ctx, &specs);
        assert_eq!(
            forward.as_slice(),
            backward.as_slice(),
            "order changed writes"
        );
        assert_eq!(forward_stats, backward_stats, "order changed the counters");

        let loser = if champion.id == first.id {
            second
        } else {
            first
        };
        let expected: Vec<[i32; 3]> = tree::place(&champion)
            .iter()
            .map(|placement| {
                [
                    champion.origin[0] + placement.offset[0],
                    141 + placement.offset[1],
                    champion.origin[2] + placement.offset[2],
                ]
            })
            .collect();
        let written: Vec<[i32; 3]> = (0..SECTION_VOLUME)
            .filter(|slot| forward[*slot] != AIR)
            .map(|slot| {
                let (local_x, local_y, local_z) = crate::section::section_coord(slot);
                [
                    i32::try_from(local_x).expect("local x fits"),
                    i32::try_from(local_y).expect("local y fits"),
                    i32::try_from(local_z).expect("local z fits"),
                ]
            })
            .collect();
        let Some(first_written) = written.first() else {
            panic!("the winner wrote nothing");
        };
        assert!(
            first_written[2] < 32,
            "the winner wrote outside the section"
        );
        for position in &written {
            let world = [position[0], position[1] + 128, position[2]];
            assert!(
                expected.contains(&world),
                "the loser wrote {world:?} (loser id {})",
                loser.id
            );
        }
    }

    #[test]
    fn tree_filters() {
        let specs = tree_specs();

        let mut good = Flat::new([0, 128, 0], 140);
        good.set_column(16, 16, 140, 0, 3, 0);
        let mut blocks = scratch(AIR);
        for y in 0..=12 {
            for z in 0..32 {
                for x in 0..32 {
                    blocks[index(x, y, z)] = STONE;
                }
            }
        }
        let control = (0..512_u64)
            .find(|id| tree::accepts(SEED, &tree_candidate(*id, [16, 140, 16]), 3))
            .map(|id| tree_candidate(id, [16, 140, 16]))
            .expect("no accepted control tree");
        let stats = apply(&mut blocks, &[control], &good.ctx(), &specs);
        assert!(stats.writes > 0, "the control tree did not write");

        let cases = [
            ("steep", 140, 3, 3, 0, None),
            ("underwater", SEA - 8, 0, 0, 0, None),
            ("bank", 140, 0, 3, COLUMN_BANK, None),
            ("roofed", 140, 0, 3, 0, Some(19)),
        ];
        for (name, top, slope, biome, flags, roof) in cases {
            let mut flat = Flat::new([0, 128, 0], top);
            flat.set_column(16, 16, top, slope, biome, flags);
            let mut blocks = scratch(AIR);
            for y in 0..=12 {
                for z in 0..32 {
                    for x in 0..32 {
                        blocks[index(x, y, z)] = STONE;
                    }
                }
            }
            if let Some(roof_y) = roof {
                blocks[index(16, roof_y, 16)] = STONE;
            }
            let baseline = blocks.clone();
            let rejected = tree_candidate(control.id, [16, top, 16]);
            let stats = apply(&mut blocks, &[rejected], &flat.ctx(), &specs);
            assert_eq!(stats.writes, 0, "{name} column generated a tree");
            assert_eq!(
                blocks.as_slice(),
                baseline.as_slice(),
                "{name} wrote a block"
            );
        }
    }

    #[test]
    fn tree_centres_clear_water_and_banks() {
        let specs = tree_specs();
        let control = (0..2048_u64)
            .find(|id| tree::accepts(SEED, &tree_candidate(*id, [19, 140, 16]), 3))
            .map(|id| tree_candidate(id, [19, 140, 16]))
            .expect("no accepted tree");

        // One water column at (16, 16): below the sea, so the footprint already
        // calls it water without a second hydrology read.
        let mut wet = Flat::new([0, 128, 0], 140);
        wet.set_column(16, 16, SEA - 4, 0, 3, 0);
        let wet_ctx = wet.ctx();
        for (x, z, label) in [
            (16, 16, "on the water column"),
            (17, 16, "one column away"),
            (18, 16, "two columns away"),
            (18, 18, "two columns diagonally"),
        ] {
            let mut blocks = land();
            let candidate = tree_candidate(control.id, [x, 140, z]);
            let stats = apply(&mut blocks, &[candidate], &wet_ctx, &specs);
            assert_eq!(stats.writes, 0, "a tree grew {label} from water");
        }
        let mut blocks = land();
        let stats = apply(&mut blocks, &[control], &wet_ctx, &specs);
        assert!(
            stats.writes > 0,
            "a tree three columns from water was rejected"
        );

        // A bank bit is rejected within two columns and clear at three.
        let mut bank = Flat::new([0, 128, 0], 140);
        bank.set_column(20, 16, 140, 0, 3, COLUMN_BANK);
        let bank_ctx = bank.ctx();
        let mut blocks = land();
        let near = tree_candidate(control.id, [18, 140, 16]);
        assert_eq!(
            apply(&mut blocks, &[near], &bank_ctx, &specs).writes,
            0,
            "a tree grew within two columns of a bank"
        );
        let mut blocks = land();
        let far = tree_candidate(control.id, [17, 140, 16]);
        assert!(
            apply(&mut blocks, &[far], &bank_ctx, &specs).writes > 0,
            "a tree three columns from a bank was rejected"
        );
    }

    #[test]
    fn a_canopy_over_water_keeps_the_water() {
        let specs = tree_specs();
        let flat = Flat::new([0, 128, 0], 140);
        let ctx = flat.ctx();
        let (control, leaf) = (0..4096_u64)
            .find_map(|id| {
                let candidate = tree_candidate(id, [16, 140, 16]);
                if !tree::accepts(SEED, &candidate, 3) {
                    return None;
                }
                tree::place(&candidate)
                    .into_iter()
                    .find(|placement| {
                        placement.layer == tree::Layer::Leaf
                            && (placement.offset[0] != 0 || placement.offset[2] != 0)
                    })
                    .map(|placement| (candidate, placement))
            })
            .expect("no accepted tree with a side leaf");
        let target = [
            16 + leaf.offset[0],
            141 + leaf.offset[1],
            16 + leaf.offset[2],
        ];
        let local_y = target[1] - 128;
        assert!(
            (0..32).contains(&local_y),
            "the leaf is outside the section"
        );
        let slot = index(target[0], local_y, target[2]);

        // The dry control proves the canopy covers the cell.
        let mut dry = land();
        assert!(apply(&mut dry, &[control], &ctx, &specs).writes > 0);
        assert_eq!(dry[slot], LEAF, "the leaf does not cover the target");

        // With water in that one cell only the over-water block is lost.
        let mut wet = land();
        wet[slot] = WATER;
        let stats = apply(&mut wet, &[control], &ctx, &specs);
        assert!(stats.writes > 0, "the land part of the canopy was lost");
        assert_eq!(wet[slot], WATER, "a leaf replaced water");
        assert!(wet.contains(&LEAF), "the canopy lost every leaf");
        assert!(wet.contains(&LOG), "the trunk lost every log");

        // Logs and leaves both refuse water before their masks are consulted.
        assert!(!mask_allows(Mask::AirOrLeaf, WATER, &ctx));
        assert!(!mask_allows(Mask::AirOnly, WATER, &ctx));
    }

    #[test]
    fn trunk_base_sinks_to_the_lowest_neighbour() {
        let specs = tree_specs();
        let control = (0..2048_u64)
            .find(|id| tree::accepts(SEED, &tree_candidate(*id, [16, 140, 16]), 3))
            .map(|id| tree_candidate(id, [16, 140, 16]))
            .expect("no accepted tree");

        let mut flat = Flat::new([0, 128, 0], 140);
        flat.set_column(17, 16, 138, 0, 3, 0);
        assert_eq!(
            tree::trunk_base(flat.ctx().columns, 16, 16),
            Some(139),
            "the base did not sink to the low neighbour"
        );

        let plain = Flat::new([0, 128, 0], 140);
        let mut dry = land();
        assert!(apply(&mut dry, &[control], &plain.ctx(), &specs).writes > 0);
        let mut sunk = land();
        assert!(apply(&mut sunk, &[control], &flat.ctx(), &specs).writes > 0);
        let dry_top = highest_tree_block(&dry).expect("the control wrote nothing");
        let sunk_top = highest_tree_block(&sunk).expect("the sunk tree wrote nothing");
        assert_eq!(
            dry_top - sunk_top,
            2,
            "a two-block sink did not lower the canopy"
        );

        // A base at `SEA + 1` is allowed, one block lower is rejected.
        let mut seabed = scratch(AIR);
        for z in 0..32 {
            for x in 0..32 {
                seabed[index(x, 0, z)] = STONE;
            }
        }

        let edge = Flat::new([0, 128, 0], SEA);
        assert_eq!(
            tree::trunk_base(edge.ctx().columns, 16, 16),
            Some(SEA + 1),
            "the base at the sea edge is wrong"
        );
        let mut blocks = seabed.clone();
        assert!(
            apply(&mut blocks, &[control], &edge.ctx(), &specs).writes > 0,
            "a base at SEA + 1 was rejected"
        );

        let low = Flat::new([0, 128, 0], SEA - 1);
        assert_eq!(
            tree::trunk_base(low.ctx().columns, 16, 16),
            Some(SEA),
            "the below-sea base is wrong"
        );
        let mut blocks = seabed;
        assert_eq!(
            apply(&mut blocks, &[control], &low.ctx(), &specs).writes,
            0,
            "a trunk base below SEA + 1 was accepted"
        );
    }

    #[test]
    fn application_is_idempotent() {
        let specs = v1_specs();
        let mut flat = Flat::new([0, 128, 0], 140);
        flat.set_column(16, 16, 140, 0, 3, 0);
        flat.set_column(19, 16, 140, 0, 3, 0);
        let ctx = flat.ctx();

        let mut blocks = scratch(AIR);
        for y in 0..=12 {
            for z in 0..32 {
                for x in 0..32 {
                    blocks[index(x, y, z)] = STONE;
                }
            }
        }
        let candidates = vec![
            ore_candidate(1, 0, [16, 138, 16]),
            ore_candidate(2, 1, [17, 139, 15]),
            tree_candidate(3, [16, 140, 16]),
            tree_candidate(4, [19, 140, 16]),
        ];

        let first = apply(&mut blocks, &candidates, &ctx, &specs);
        assert!(first.writes > 0, "the first pass wrote nothing");
        let snapshot = blocks.clone();

        let second = apply(&mut blocks, &candidates, &ctx, &specs);
        assert_eq!(second.writes, 0, "the second pass wrote new blocks");
        assert_eq!(blocks.as_slice(), snapshot.as_slice());
    }

    #[test]
    fn uniform_sections_are_skipped_only_when_unwritable() {
        let specs = v1_specs();
        let flat = Flat::new([0, 128, 0], 140);
        let ctx = flat.ctx();
        let tree = tree_candidate(2, [16, 140, 16]);
        let ore = ore_candidate(1, 0, [16, 132, 16]);

        // A plant's air-only masks cannot write stone, so a solid section at the
        // target stays free (`H2.2`).
        let mut solid = Section::uniform(STONE);
        let report = apply_section_counted(&mut solid, &[tree], &ctx, &specs);
        assert_eq!(
            report.probes, 0,
            "a uniform solid section cost a block test"
        );
        assert_eq!(report.stats.writes, 0);
        assert_eq!(report.stats.candidates, 1);

        // No mask ever writes water or bedrock.
        for id in [WATER, BEDROCK] {
            let mut section = Section::uniform(id);
            let report = apply_section_counted(&mut section, &[tree], &ctx, &specs);
            assert_eq!(report.probes, 0, "uniform {id} was processed");
        }

        // A uniform air section that no candidate can reach is still free.
        let empty = Flat::new([0, 128, 0], 20);
        let mut air = Section::uniform(AIR);
        let report = apply_section_counted(&mut air, &[], &empty.ctx(), &specs);
        assert_eq!(report.probes, 0, "an unreachable air section cost a test");
        assert_eq!(report.stats.writes, 0);

        // A candidate reaches this all-air section and its leaf mask accepts
        // air, so it is processed instead of truncating the canopy (`H2.2`).
        let mut canopy = Section::uniform(AIR);
        let report = apply_section_counted(&mut canopy, &[tree], &ctx, &specs);
        assert!(
            report.probes > 0,
            "an air section under a candidate must be processed"
        );
        assert!(
            report.stats.writes > 0,
            "the tree wrote nothing above the seam"
        );

        // The rule is the mask, not the material: a uniform stone section is
        // writable by a host-replacing ore even though a tree cannot touch it.
        let mut rock = Section::uniform(STONE);
        let report = apply_section_counted(&mut rock, &[ore], &ctx, &specs);
        assert!(
            report.probes > 0,
            "a host-replacing ore skipped uniform stone"
        );
        assert!(
            report.stats.writes > 0,
            "the ore wrote nothing into uniform stone"
        );

        // A uniform stone section above a plant is still skipped: the box reaches
        // it, but no plant mask can write stone.
        let above = Flat::new([0, 160, 0], 140);
        let mut stone_above = Section::uniform(STONE);
        let high = tree_candidate(3, [16, 150, 16]);
        let report = apply_section_counted(&mut stone_above, &[high], &above.ctx(), &specs);
        assert_eq!(
            report.probes, 0,
            "a uniform stone section above a plant was processed"
        );
    }

    #[test]
    fn canopy_crosses_into_the_section_above() {
        let specs = tree_specs();
        let flat = Flat::new([0, 160, 0], 158);
        let ctx = flat.ctx();
        let candidate = (0..2048_u64)
            .find(|id| tree::accepts(SEED, &tree_candidate(*id, [16, 159, 16]), 3))
            .map(|id| tree_candidate(id, [16, 159, 16]))
            .expect("no accepted tree");

        let mut empty = Section::uniform(AIR);
        let report = apply_section_counted(&mut empty, &[], &ctx, &specs);
        assert_eq!(report.probes, 0, "an unreachable section was processed");

        let mut canopy = Section::uniform(AIR);
        let report = apply_section_counted(&mut canopy, &[candidate], &ctx, &specs);
        assert!(report.probes > 0, "the section above the tree was skipped");
        assert!(
            report.stats.writes > 0,
            "nothing was written above the seam"
        );
        let leaves = (0..SECTION_VOLUME)
            .filter(|slot| canopy.get_index(*slot) == LEAF)
            .count();
        assert!(leaves > 0, "the canopy above the seam was truncated");
    }

    #[test]
    fn sections_copy_back_only_after_writes() {
        let specs = tree_specs();
        let flat = Flat::new([0, 128, 0], 140);
        let ctx = flat.ctx();

        let mut blocks = scratch(AIR);
        for y in 0..=12 {
            for z in 0..32 {
                for x in 0..32 {
                    blocks[index(x, y, z)] = STONE;
                }
            }
        }
        let mut section = Section::from_blocks(&blocks);
        let before = section.content_hash();

        let control = (0..512_u64)
            .find(|id| tree::accepts(SEED, &tree_candidate(*id, [16, 140, 16]), 3))
            .map(|id| tree_candidate(id, [16, 140, 16]))
            .expect("no accepted control tree");
        let stats = apply_section(&mut section, &[control], &ctx, &specs);
        assert!(stats.writes > 0, "the paletted section took no tree");
        assert_ne!(
            section.content_hash(),
            before,
            "the write was not copied back"
        );

        let again = apply_section(&mut section, &[control], &ctx, &specs);
        assert_eq!(again.writes, 0, "the second section pass wrote again");
    }

    #[test]
    fn scatter_clumps_and_is_keyed_on_xz_only() {
        let seed = 0x5eed_5eed;
        let density = 700;
        let mut present = 0_u64;
        let mut pairs = 0_u64;
        for z in 0..128 {
            for x in 0..128 {
                let here = scatter::present(seed, x, z, density);
                if here {
                    present += 1;
                    if scatter::present(seed, x + 1, z, density) {
                        pairs += 1;
                    }
                }
            }
        }
        let total = 128_u64 * 128;
        assert!(
            present > total / 20,
            "the patch field is too sparse to test"
        );
        assert!(
            pairs * total * 100 > present * present * 105,
            "no clumping: {pairs} adjacent pairs for {present} present columns"
        );

        let specs = v1_specs();
        let low = Flat::new([0, 128, 0], 140);
        let high = Flat::new([0, 160, 0], 172);
        let mut low_blocks = scratch(AIR);
        let mut high_blocks = scratch(AIR);
        let low_stats = apply(&mut low_blocks, &[], &low.ctx(), &specs);
        let high_stats = apply(&mut high_blocks, &[], &high.ctx(), &specs);
        assert_eq!(low_stats, high_stats, "scatter depends on y");
        let columns = |blocks: &[BlockId; SECTION_VOLUME]| -> Vec<(i32, i32)> {
            let mut out = Vec::new();
            for z in 0..32 {
                for x in 0..32 {
                    if (0..32).any(|y| blocks[index(x, y, z)] == FLOWER) {
                        out.push((x, z));
                    }
                }
            }
            out
        };
        let low_columns = columns(&low_blocks);
        let Some(first_column) = low_columns.first() else {
            panic!("scatter wrote nothing");
        };
        assert!(first_column.1 < 32, "a column outside the section");
        assert_eq!(low_columns, columns(&high_blocks), "scatter depends on y");
    }
}

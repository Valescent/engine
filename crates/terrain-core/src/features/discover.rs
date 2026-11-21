//! Feature discovery: the region grid, `v1` specs, candidate generation, and
//! the `3 x 3 x 3` gather (`H1`).
//!
//! Everything here is a pure function of a seed, a region, and the spec list.
//! Candidates carry a stable id and are always returned sorted by it.

use crate::description::SECTION_SIZE;
use crate::error::{ErrorKind, Stage, TerrainError};
use crate::hash::{domain, hash64};
use crate::section::BlockId;

use super::masks::{Mask, Palette, is_air_mask, is_solid_mask, mask_accepts};
use super::{ore, tree};

/// Edge of a feature region in blocks (`H1.1`).
pub(crate) const REGION_SIZE: i32 = 32;

/// Regions gathered on each axis before application (`H1.1`).
pub(crate) const GATHER_RADIUS: i32 = 1;

/// Regions in one gather: `3 x 3 x 3`.
pub(crate) const GATHER_REGIONS: usize = 27;

const _: () = assert!(REGION_SIZE == 32 && SECTION_SIZE == 32);
const _: () = assert!(GATHER_REGIONS == 27 && GATHER_RADIUS == 1);

/// Which generator a candidate belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum FeatureKind {
    /// An ore vein (`G3.1`).
    Ore,
    /// A surface or subsurface boulder.
    Boulder,
    /// A tree (`H3`).
    Tree,
    /// Small ground cover (`H4`).
    Scatter,
}

/// One authored feature type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FeatureSpec {
    /// Stable name. It is also the hash domain namespace of the feature.
    pub name: &'static str,
    /// Largest distance in blocks from a candidate centre to any written block.
    pub max_extent: i32,
    /// The mask the feature writes through.
    pub mask: Mask,
    /// Candidate count per region, low and high.
    ///
    /// For [`FeatureKind::Tree`] the jittered grid is the authority (`H3.3`),
    /// so the range is an advisory bound and discovery ignores it.
    pub count_per_region: (u32, u32),
    /// Which generator this spec belongs to.
    pub kind: FeatureKind,
}

impl FeatureSpec {
    /// Every mask this spec writes through, inherited ones included.
    ///
    /// A tree declares logs and leaves separately (`H3.2`), so the spec's own
    /// [`Self::mask`] is unioned with the masks its kind must use.
    #[must_use]
    pub(crate) fn masks(&self) -> Vec<Mask> {
        let mut masks = vec![self.mask];
        match self.kind {
            FeatureKind::Ore | FeatureKind::Boulder => {
                if !masks.contains(&Mask::ReplaceHost) {
                    masks.push(Mask::ReplaceHost);
                }
            }
            FeatureKind::Tree => {
                masks.push(Mask::AirOrLeaf);
                masks.push(Mask::AirOnly);
            }
            FeatureKind::Scatter => {
                masks.push(Mask::AirOnly);
            }
        }
        masks.sort_unstable();
        masks.dedup();
        masks
    }
}

/// Rejects a spec whose masks would contradict each other (`H2.3`).
///
/// A mask set is contradictory when it both replaces solid rock and writes only
/// into air: the two passes cannot agree on what a legal target is.
pub(crate) fn validate_masks(masks: &[Mask]) -> Result<(), TerrainError> {
    let solid = masks.iter().copied().any(is_solid_mask);
    let air = masks.iter().copied().any(is_air_mask);
    if solid && air {
        return Err(feature_error(
            "feature.mask",
            "not both solid-replacing and air-only",
        ));
    }
    Ok(())
}

/// Rejects a spec the compiler cannot place (`H1.1`, `H3.2`).
pub(crate) fn validate_spec(spec: &FeatureSpec) -> Result<(), TerrainError> {
    if spec.name.is_empty() {
        return Err(feature_error("feature.name", "non-empty"));
    }
    if spec.max_extent < 0 || spec.max_extent > REGION_SIZE {
        return Err(feature_error("feature.max_extent", "0..=REGION_SIZE"));
    }
    match spec.kind {
        FeatureKind::Ore if spec.max_extent > ore::ORE_MAX_EXTENT => {
            return Err(feature_error("feature.max_extent", "ore <= 8"));
        }
        FeatureKind::Tree if spec.max_extent > tree::TREE_MAX_EXTENT => {
            return Err(feature_error("feature.max_extent", "tree <= 12"));
        }
        _ => {}
    }
    if spec.count_per_region.0 > spec.count_per_region.1 {
        return Err(feature_error("feature.count_per_region", "low <= high"));
    }
    validate_masks(&spec.masks())
}

/// The v1 feature set (`G3.1`, `H3`, `H4`).
///
/// Names are stable: they are the hash domain of every candidate and the key
/// the caller uses to supply the matching block id.
#[must_use]
pub(crate) fn v1_specs() -> Vec<FeatureSpec> {
    vec![
        FeatureSpec {
            name: "ore_iron",
            max_extent: 8,
            mask: Mask::ReplaceHost,
            count_per_region: (1, 4),
            kind: FeatureKind::Ore,
        },
        FeatureSpec {
            name: "ore_copper",
            max_extent: 6,
            mask: Mask::ReplaceHost,
            count_per_region: (1, 3),
            kind: FeatureKind::Ore,
        },
        FeatureSpec {
            name: "boulder",
            max_extent: 3,
            mask: Mask::ReplaceHost,
            count_per_region: (0, 2),
            kind: FeatureKind::Boulder,
        },
        FeatureSpec {
            name: "tree",
            max_extent: tree::TREE_MAX_EXTENT,
            mask: Mask::AirOrLeaf,
            count_per_region: (0, 36),
            kind: FeatureKind::Tree,
        },
        FeatureSpec {
            name: "scatter",
            max_extent: 1,
            mask: Mask::AirOnly,
            count_per_region: (0, 1),
            kind: FeatureKind::Scatter,
        },
    ]
}

/// One discovered feature (`H1.2`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct Candidate {
    /// Stable identity: `hash64(name, region, index)`, never seed-dependent.
    pub id: u64,
    /// The region that owns this candidate.
    pub region: [i32; 3],
    /// Which generator produced it.
    pub kind: FeatureKind,
    /// World position of the centre, always inside [`Self::region`].
    pub origin: [i32; 3],
    /// Per-candidate variant word; the meaning is the kind's business.
    pub variant: u64,
}

/// The region index that owns a world point.
#[must_use]
pub(crate) fn region_of(point: [i32; 3]) -> [i32; 3] {
    [
        point[0].div_euclid(REGION_SIZE),
        point[1].div_euclid(REGION_SIZE),
        point[2].div_euclid(REGION_SIZE),
    ]
}

/// The lowest world coordinate of a region.
#[must_use]
pub(crate) fn region_origin(region: [i32; 3]) -> [i32; 3] {
    [
        region[0] * REGION_SIZE,
        region[1] * REGION_SIZE,
        region[2] * REGION_SIZE,
    ]
}

/// Whether a world point lies inside a region.
#[must_use]
pub(crate) fn region_contains(region: [i32; 3], point: [i32; 3]) -> bool {
    region_of(point) == region
}

/// Slot of a `-1..=1` gather offset in [`crate::features::ApplyContext::regions`].
///
/// `x` is fastest, then `z`, then `y`, so the centre offset `[0, 0, 0]` is slot
/// [`GATHER_REGIONS`] `/ 2`.
#[must_use]
pub(crate) fn region_slot(offset: [i32; 3]) -> usize {
    let index = (offset[0] + 1) + 3 * ((offset[2] + 1) + 3 * (offset[1] + 1));
    usize::try_from(index).expect("a gather offset is inside 0..27")
}

/// The `3 x 3 x 3` region origins around one region.
#[must_use]
pub(crate) fn gather_origins(region: [i32; 3]) -> [[i32; 3]; GATHER_REGIONS] {
    let mut origins = [[0_i32; 3]; GATHER_REGIONS];
    for dy in -GATHER_RADIUS..=GATHER_RADIUS {
        for dz in -GATHER_RADIUS..=GATHER_RADIUS {
            for dx in -GATHER_RADIUS..=GATHER_RADIUS {
                let offset = [dx, dy, dz];
                origins[region_slot(offset)] =
                    region_origin([region[0] + dx, region[1] + dy, region[2] + dz]);
            }
        }
    }
    origins
}

/// Bit mask of the spec index carried in the low byte of a blob variant.
const SPEC_MASK: u64 = 0xFF;

/// Packs the producing spec index into a blob variant word.
///
/// Ore and boulder candidates need to know which spec produced them so
/// application can find the matching block id. [`Candidate`] has no name field,
/// so the low byte of the variant carries the spec index; the remaining 56 bits
/// are the per-candidate RNG word.
pub(crate) fn variant_with_spec(index: usize, word: u64) -> u64 {
    debug_assert!(index < 256, "a spec list is shorter than 256 entries");
    let tag = u64::try_from(index).expect("a spec index fits u64") & SPEC_MASK;
    (word & !SPEC_MASK) | tag
}

/// The spec index packed into a blob variant.
pub(crate) fn variant_spec(variant: u64) -> usize {
    usize::try_from(variant & SPEC_MASK).expect("a spec index fits usize")
}

/// Discovers every candidate whose centre lies in one region (`H1.1`, `H1.2`).
///
/// The result is sorted by [`Candidate::id`], so the same region always produces
/// the same list in the same order.
#[must_use]
pub(crate) fn discover(region: [i32; 3], seed: u64, specs: &[FeatureSpec]) -> Vec<Candidate> {
    let mut out = Vec::new();
    let words = [
        i64::from(region[0]),
        i64::from(region[1]),
        i64::from(region[2]),
    ];
    for (index, spec) in specs.iter().enumerate() {
        let tag = domain(&format!("feat:{}", spec.name));
        let root = hash64(seed, tag, &words);
        match spec.kind {
            FeatureKind::Tree => tree::discover(region, seed, spec, &mut out),
            FeatureKind::Scatter => {}
            FeatureKind::Ore | FeatureKind::Boulder => {
                let count = region_count(spec, root, region[1]);
                for candidate in 0..count {
                    out.push(blob_candidate(region, spec, index, root, candidate));
                }
            }
        }
    }
    out.sort_unstable();
    out
}

/// Whether a candidate's declared bounding box can reach a section (`H2.2`).
///
/// The box is the candidate centre grown by its spec's [`FeatureSpec::max_extent`].
/// That is conservative for a tree, whose real box starts at its column top, but
/// it is never smaller than the blocks a candidate can write. The uniform
/// exemption of [`crate::features::apply_section_counted`] uses it, so an
/// empty section that no feature can touch still costs no per-block test.
#[must_use]
pub(crate) fn candidates_reach(
    origin: [i32; 3],
    candidates: &[Candidate],
    specs: &[FeatureSpec],
) -> bool {
    candidates
        .iter()
        .any(|candidate| candidate_intersects(origin, candidate, specs))
}

/// Whether one candidate's declared bounding box intersects a section (`H2.2`).
fn candidate_intersects(origin: [i32; 3], candidate: &Candidate, specs: &[FeatureSpec]) -> bool {
    let extent = extent_of(candidate, specs);
    (0..3).all(|axis| {
        candidate.origin[axis] - extent < origin[axis] + REGION_SIZE
            && candidate.origin[axis] + extent >= origin[axis]
    })
}

/// Whether a uniform section of `id` can legally take a write from any
/// candidate whose bounding box intersects it (`H2.2`).
///
/// The test is the same `(candidate, mask)` pair application would use: a
/// uniform id is writable when some intersecting candidate's write mask accepts
/// it. Air is writable by the air-filling masks, stone by host-replacing ones,
/// and water or bedrock by nothing. It needs only the palette and host set, not
/// column data, so a caller applying features to a raw block array such as the
/// pipeline can use it to skip a uniform section without weakening the rule.
#[must_use]
pub(crate) fn uniform_writable(
    id: BlockId,
    origin: [i32; 3],
    candidates: &[Candidate],
    palette: Palette,
    host_rock: &[BlockId],
    specs: &[FeatureSpec],
) -> bool {
    candidates.iter().any(|candidate| {
        candidate_intersects(origin, candidate, specs)
            && candidate_may_write(candidate, specs, id, palette, host_rock)
    })
}

/// Whether any mask belonging to a candidate accepts a uniform `id`.
///
/// A tree and a scatter pass own their mask set through their kind; an ore or
/// boulder names its spec through the variant word, exactly as application does.
fn candidate_may_write(
    candidate: &Candidate,
    specs: &[FeatureSpec],
    id: BlockId,
    palette: Palette,
    host_rock: &[BlockId],
) -> bool {
    let allowed = |spec: &FeatureSpec| {
        spec.masks()
            .into_iter()
            .any(|mask| mask_accepts(mask, id, palette, host_rock))
    };
    match candidate.kind {
        FeatureKind::Tree => specs
            .iter()
            .filter(|spec| spec.kind == FeatureKind::Tree)
            .any(allowed),
        FeatureKind::Scatter => specs
            .iter()
            .filter(|spec| spec.kind == FeatureKind::Scatter)
            .any(allowed),
        FeatureKind::Ore | FeatureKind::Boulder => specs
            .get(variant_spec(candidate.variant))
            .filter(|spec| spec.kind == candidate.kind)
            .is_some_and(allowed),
    }
}

/// The declared half-extent of one candidate.
fn extent_of(candidate: &Candidate, specs: &[FeatureSpec]) -> i32 {
    match candidate.kind {
        FeatureKind::Ore | FeatureKind::Boulder => specs
            .get(variant_spec(candidate.variant))
            .filter(|spec| spec.kind == candidate.kind)
            .map_or(REGION_SIZE, |spec| spec.max_extent),
        FeatureKind::Tree => specs
            .iter()
            .find(|spec| spec.kind == FeatureKind::Tree)
            .map_or(tree::TREE_MAX_EXTENT, |spec| spec.max_extent),
        FeatureKind::Scatter => 0,
    }
}

/// Merges the `3 x 3 x 3` neighbourhood of one region (`H1.4`).
///
/// Duplicate ids are removed and the result is sorted by id, which is the order
/// application writes in.
#[must_use]
pub(crate) fn gather(region: [i32; 3], seed: u64, specs: &[FeatureSpec]) -> Vec<Candidate> {
    let mut out = Vec::new();
    for dy in -GATHER_RADIUS..=GATHER_RADIUS {
        for dz in -GATHER_RADIUS..=GATHER_RADIUS {
            for dx in -GATHER_RADIUS..=GATHER_RADIUS {
                out.extend(discover(
                    [region[0] + dx, region[1] + dy, region[2] + dz],
                    seed,
                    specs,
                ));
            }
        }
    }
    out.sort_unstable();
    out.dedup_by_key(|candidate| candidate.id);
    out
}

/// A typed validation error for a feature spec.
fn feature_error(key: &'static str, rule: &'static str) -> TerrainError {
    TerrainError::new(Stage::Features, ErrorKind::DescriptionInvalid { key, rule })
        .frame(Stage::Features, "feature spec")
}

/// Candidates a blob spec emits in one region, from its depth curve (`G3.3`).
fn region_count(spec: &FeatureSpec, root: u64, region_y: i32) -> u32 {
    let (low, high) = spec.count_per_region;
    if high <= low {
        return low;
    }
    let curve = depth_permille(region_y);
    let span = high - low;
    let scaled = u64::from(span) * u64::from(curve) / 1000;
    let wobble = u32::try_from(hash64(root, domain("feat:count"), &[i64::from(region_y)]) % 2)
        .expect("a parity bit fits u32");
    let count = low + u32::try_from(scaled).expect("a scaled count fits u32") + wobble;
    count.min(high)
}

/// Ore density against depth, in permille (`G3.3`).
///
/// Zero near and above the sea, full below [`crate::description::DEEP_Y`].
fn depth_permille(region_y: i32) -> u32 {
    let top = region_y * REGION_SIZE;
    let high = crate::description::SEA + REGION_SIZE;
    let low = crate::description::DEEP_Y;
    if top >= high {
        return 80;
    }
    if top <= low {
        return 1000;
    }
    let span = high - low;
    let scaled = (high - top) * 920 / span + 80;
    u32::try_from(scaled).expect("a depth share fits u32")
}

/// One ore or boulder candidate (`G3.1`).
fn blob_candidate(
    region: [i32; 3],
    spec: &FeatureSpec,
    index: usize,
    root: u64,
    ordinal: u32,
) -> Candidate {
    let origin_region = region_origin(region);
    let id = hash64(
        0,
        domain(spec.name),
        &[
            i64::from(region[0]),
            i64::from(region[1]),
            i64::from(region[2]),
            i64::from(ordinal),
        ],
    );
    let word = hash64(root, domain("feat:variant"), &[i64::from(ordinal)]);
    let mut origin = [0_i32; 3];
    for (axis, slot) in origin.iter_mut().enumerate() {
        let axis_word = hash64(
            word,
            domain("feat:jitter"),
            &[
                i64::from(ordinal),
                i64::try_from(axis).expect("an axis fits i64"),
            ],
        );
        *slot = origin_region[axis] + within(axis_word, REGION_SIZE);
    }
    Candidate {
        id,
        region,
        kind: spec.kind,
        origin,
        variant: variant_with_spec(index, word),
    }
}

/// A hash mapped into `0..span`.
pub(crate) fn within(word: u64, span: i32) -> i32 {
    let span = u32::try_from(span).expect("a span is positive");
    let value = u32::try_from(word % u64::from(span)).expect("a remainder fits the span");
    i32::try_from(value).expect("a span fits i32")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::tests::SEED;
    use std::collections::BTreeMap;
    #[test]
    fn specs_declare_legal_masks_and_extents() {
        for spec in v1_specs() {
            assert!(
                validate_spec(&spec).is_ok(),
                "{} failed validation",
                spec.name
            );
        }

        let too_wide = FeatureSpec {
            name: "tower",
            max_extent: 33,
            mask: Mask::ReplaceSurface,
            count_per_region: (0, 1),
            kind: FeatureKind::Boulder,
        };
        assert!(validate_spec(&too_wide).is_err(), "extent above a region");

        let tree_too_wide = FeatureSpec {
            name: "tree",
            max_extent: 13,
            mask: Mask::AirOrLeaf,
            count_per_region: (0, 1),
            kind: FeatureKind::Tree,
        };
        assert!(validate_spec(&tree_too_wide).is_err(), "tree above 12");

        let ore_too_wide = FeatureSpec {
            name: "ore",
            max_extent: 9,
            mask: Mask::ReplaceHost,
            count_per_region: (0, 1),
            kind: FeatureKind::Ore,
        };
        assert!(validate_spec(&ore_too_wide).is_err(), "ore above 8");

        let contradictory = FeatureSpec {
            name: "bad",
            max_extent: 1,
            mask: Mask::AirOnly,
            count_per_region: (0, 1),
            kind: FeatureKind::Ore,
        };
        assert!(
            validate_spec(&contradictory).is_err(),
            "air-only and host-replacing cannot coexist"
        );
        assert!(validate_masks(&[Mask::AirOrLeaf, Mask::ReplaceHost]).is_err());
        assert!(validate_masks(&[Mask::AirOnly, Mask::AirOrLeaf]).is_ok());
    }

    #[test]
    fn centres_stay_inside_their_region() {
        let specs = v1_specs();
        let mut total = 0_usize;
        for rx in 0..6 {
            for rz in 0..6 {
                for ry in 0..10 {
                    let region = [rx * 7 - 12, ry * 3 - 5, rz * 5 - 8];
                    let list = discover(region, SEED, &specs);
                    for candidate in &list {
                        assert_eq!(
                            region_of(candidate.origin),
                            candidate.region,
                            "candidate {} escaped its region",
                            candidate.id
                        );
                        for axis in 0..3 {
                            let base = candidate.region[axis] * REGION_SIZE;
                            assert!(
                                (base..base + REGION_SIZE).contains(&candidate.origin[axis]),
                                "axis {axis} centre {} outside {base}..{}",
                                candidate.origin[axis],
                                base + REGION_SIZE
                            );
                        }
                    }
                    total += list.len();
                }
            }
        }
        assert!(total >= 10_000, "only {total} candidates discovered");
    }

    #[test]
    fn discovery_is_order_independent() {
        let specs = v1_specs();
        let regions: Vec<[i32; 3]> = (0..4)
            .flat_map(|x| (0..4).map(move |z| [x - 2, 3, z + 1]))
            .collect();

        let mut forward = BTreeMap::new();
        for region in &regions {
            forward.insert(*region, discover(*region, SEED, &specs));
        }

        let mut backward = BTreeMap::new();
        for region in regions.iter().rev() {
            backward.insert(*region, discover(*region, SEED, &specs));
        }

        assert_eq!(forward, backward);
    }

    #[test]
    fn gather_deduplicates_and_sorts() {
        let specs = v1_specs();
        let centre = [2, -3, 1];
        let gathered = gather(centre, SEED, &specs);

        assert!(gathered.len() > 1);
        for pair in gathered.windows(2) {
            assert!(pair[0].id < pair[1].id, "gather is not strictly sorted");
        }
        assert!(
            gathered.windows(2).all(|pair| pair[0].id != pair[1].id),
            "gather kept a duplicate id"
        );
        for candidate in &gathered {
            for (axis_region, axis_centre) in candidate.region.iter().zip(centre) {
                assert!((axis_region - axis_centre).abs() <= GATHER_RADIUS);
            }
            assert!(
                gathered
                    .binary_search_by_key(&candidate.id, |entry| entry.id)
                    .is_ok()
            );
        }

        let own = discover(centre, SEED, &specs);
        for candidate in own {
            assert!(
                gathered
                    .binary_search_by_key(&candidate.id, |entry| entry.id)
                    .is_ok(),
                "the centre's own candidates are missing from its gather"
            );
        }
    }
}

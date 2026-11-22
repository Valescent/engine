//! Ore and boulder candidates: shapes and host-rock writes (`G3.1`-`G3.3`).
//!
//! A candidate's `Candidate::variant` drives its shape, so application needs
//! no seed and no per-candidate bookkeeping. Two shapes exist (`G3.1`):
//!
//! - an ellipsoid, sized by the variant word, or
//! - a short random walk of single blocks, clamped to `WALK_RADIUS`.
//!
//! Both stay within `ORE_MAX_EXTENT`, which is what lets a `3 x 3 x 3` region
//! gather cover every spill.
//!
//! # Host rock
//!
//! Writes go through `Mask::ReplaceHost` against a caller-supplied id set
//! (`G3.2`). Nothing here names stone or deep rock: the caller passes the set by
//! stable name, and water, bedrock, surface, beach, snow, and air are excluded
//! because they are not in it.

use crate::hash::{domain, hash64};
use crate::section::BlockId;

use super::{Candidate, FeatureKind, FeatureSpec, Mask, Sink, variant_spec};

/// Largest distance from a candidate centre to any written block (`G3.1`).
pub(crate) const ORE_MAX_EXTENT: i32 = 8;

/// Largest ellipsoid radius on one axis.
const ORE_MAX_RADIUS: i32 = 3;

/// Smallest boulder radius on one axis.
const BOULDER_MIN_RADIUS: i32 = 1;

/// Largest boulder radius on one axis.
const BOULDER_MAX_RADIUS: i32 = 2;

/// Fewest blocks in a random walk.
const WALK_MIN_STEPS: u32 = 6;

/// Most blocks in a random walk.
const WALK_MAX_STEPS: u32 = 16;

/// Largest distance the walk may drift from its origin on one axis (`G3.1`).
const WALK_RADIUS: i32 = 4;

const _: () = assert!(ORE_MAX_RADIUS <= ORE_MAX_EXTENT && WALK_RADIUS <= ORE_MAX_EXTENT);

/// The world-space offsets a candidate writes, relative to its centre.
///
/// The result is sorted and deduplicated, so overlapping shapes write each block
/// once and the caller can derive a bounding box from it.
#[must_use]
pub(crate) fn blocks(candidate: &Candidate) -> Vec<[i32; 3]> {
    let is_boulder = candidate.kind == FeatureKind::Boulder;
    if is_boulder || candidate.variant & 1 == 0 {
        ellipsoid(candidate.variant, is_boulder)
    } else {
        walk(candidate.variant)
    }
}

/// An ellipsoid centred on the candidate.
fn ellipsoid(variant: u64, is_boulder: bool) -> Vec<[i32; 3]> {
    let (min_radius, max_radius) = if is_boulder {
        (BOULDER_MIN_RADIUS, BOULDER_MAX_RADIUS)
    } else {
        (1, ORE_MAX_RADIUS)
    };
    let span = u32::try_from(max_radius - min_radius + 1).expect("a radius span fits u32");
    let radius = |shift: u32| -> i32 {
        let value = (variant >> shift) % u64::from(span);
        min_radius + i32::try_from(value).expect("a radius fits i32")
    };
    let rx = radius(8);
    let ry = radius(16);
    let rz = radius(24);
    let (rx2, ry2, rz2) = (rx * rx, ry * ry, rz * rz);
    let limit = rx2 * ry2 * rz2;

    let mut out = Vec::new();
    for dy in -ry..=ry {
        for dz in -rz..=rz {
            for dx in -rx..=rx {
                let value = dx * dx * ry2 * rz2 + dy * dy * rx2 * rz2 + dz * dz * rx2 * ry2;
                if value <= limit {
                    out.push([dx, dy, dz]);
                }
            }
        }
    }
    out.sort_unstable();
    out
}

/// A short random walk of single blocks.
fn walk(variant: u64) -> Vec<[i32; 3]> {
    let steps = WALK_MIN_STEPS
        + u32::try_from((variant >> 32) % u64::from(WALK_MAX_STEPS - WALK_MIN_STEPS + 1))
            .expect("a step count fits u32");

    let mut position = [0_i32; 3];
    let mut out = Vec::with_capacity(usize::try_from(steps).expect("a step count fits usize") + 1);
    out.push(position);
    for step in 0..steps {
        let word = hash64(
            variant,
            domain("ore:walk"),
            &[i64::from(step), (variant & 0xFFFF).cast_signed()],
        );
        let axis = usize::try_from(word % 3).expect("an axis fits usize");
        let direction = if (word >> 8) & 1 == 0 { -1 } else { 1 };
        position[axis] = (position[axis] + direction).clamp(-WALK_RADIUS, WALK_RADIUS);
        out.push(position);
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Writes every candidate of `kind` through [`Mask::ReplaceHost`].
pub(crate) fn apply(
    sink: &mut Sink<'_, '_>,
    ordered: &[&Candidate],
    specs: &[FeatureSpec],
    kind: FeatureKind,
) {
    for candidate in ordered {
        if candidate.kind != kind {
            continue;
        }
        let Some(id) = block_for(sink.ctx(), specs, candidate) else {
            continue;
        };
        let shape = blocks(candidate);
        let (low, high) = bounds(candidate.origin, &shape);
        if !sink.intersects(low, high) {
            continue;
        }
        for offset in &shape {
            let position = [
                candidate.origin[0] + offset[0],
                candidate.origin[1] + offset[1],
                candidate.origin[2] + offset[2],
            ];
            sink.write(position, Mask::ReplaceHost, id);
        }
    }
}

/// The world-space bounding box of a shape.
fn bounds(origin: [i32; 3], shape: &[[i32; 3]]) -> ([i32; 3], [i32; 3]) {
    let mut low = [i32::MAX; 3];
    let mut high = [i32::MIN; 3];
    for offset in shape {
        for axis in 0..3 {
            let value = origin[axis] + offset[axis];
            low[axis] = low[axis].min(value);
            high[axis] = high[axis].max(value);
        }
    }
    (low, high)
}

/// The block id a candidate writes.
///
/// The variant's low byte names the producing spec ([`variant_spec`]); the spec
/// name then selects the ore id from the context. Boulders have one id.
fn block_for(
    ctx: &super::ApplyContext<'_>,
    specs: &[FeatureSpec],
    candidate: &Candidate,
) -> Option<BlockId> {
    match candidate.kind {
        FeatureKind::Boulder => Some(ctx.palette.boulder),
        FeatureKind::Ore => {
            let spec = specs.get(variant_spec(candidate.variant))?;
            if spec.kind != FeatureKind::Ore {
                return None;
            }
            ctx.ores
                .iter()
                .find(|entry| entry.name == spec.name)
                .map(|entry| entry.id)
        }
        FeatureKind::Tree | FeatureKind::Scatter => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{region_of, variant_with_spec};

    fn candidate(kind: FeatureKind, variant: u64) -> Candidate {
        Candidate {
            id: 1,
            region: [0, 0, 0],
            kind,
            origin: [0, 0, 0],
            variant,
        }
    }

    #[test]
    fn every_shape_stays_inside_the_declared_extent() {
        for variant in 0..4096_u64 {
            let shape = blocks(&candidate(FeatureKind::Ore, variant));
            assert!(
                shape.contains(&[0, 0, 0]),
                "variant {variant} lost its origin block"
            );
            for offset in &shape {
                for axis in offset {
                    assert!(
                        axis.abs() <= ORE_MAX_EXTENT,
                        "offset {offset:?} exceeds the extent"
                    );
                }
            }
        }
    }

    #[test]
    fn shapes_are_sorted_and_unique() {
        for variant in 0..512_u64 {
            let shape = blocks(&candidate(FeatureKind::Ore, variant));
            assert!(shape.windows(2).all(|pair| pair[0] < pair[1]));
        }
    }

    #[test]
    fn the_origin_is_always_part_of_the_shape() {
        for variant in 0..512_u64 {
            let shape = blocks(&candidate(FeatureKind::Ore, variant));
            assert!(shape.contains(&[0, 0, 0]));
        }
        let boulder = blocks(&candidate(FeatureKind::Boulder, 3));
        assert!(boulder.contains(&[0, 0, 0]));
    }

    #[test]
    fn the_spec_index_survives_the_pack() {
        for index in 0..8 {
            assert_eq!(
                variant_spec(variant_with_spec(index, 0xDEAD_BEEF_0000_0000)),
                index
            );
        }
        assert_eq!(region_of([0, 0, 0]), [0, 0, 0]);
    }
}

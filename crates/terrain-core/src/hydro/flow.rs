//! D8 routing, accumulation, river width, bed shape and stepped levels.
//!
//! `D2.1` fixes the router: on the 4-block macro grid, eight neighbours, the
//! receiver is the one with the largest `drop = (h_cell - h_nbr) / dist`, with
//! `dist = 1` orthogonally and the pinned `SQRT_2` diagonally. Ties break by
//! the fixed order `ORDER`, so the result never depends on iteration order.
//!
//! Routing runs on the *filled* surface, never on terrain: the flood has
//! already removed interior sinks, so every non-sea cell has a receiver and
//! following receivers is guaranteed to terminate (`D2.1`).
//!
//! Accumulation is an upstream cell count in `u32`, processed in
//! `(filled descending, index ascending)` order, which is exactly the order in
//! which a cell's own upstream total is complete before it is added to its
//! receiver's. Sorting is the only ordering primitive here, so there is no
//! recursion, no hash map, and no float reduction.

use std::collections::VecDeque;

use super::BED_FLOOR;

/// The fixed D8 tie-break order (`D2.1`): `N, NE, E, SE, S, SW, W, NW`.
pub(crate) const ORDER: [D8; 8] = [D8::N, D8::NE, D8::E, D8::SE, D8::S, D8::SW, D8::W, D8::NW];

/// Diagonal step length, pinned: `sqrt(2)`, `0x3FB5_04F3`.
///
/// A named constant rather than a literal at each call site, so the drop
/// comparison cannot drift. `sqrt` is correctly rounded and allowed (`A2.3`);
/// the value is the `f32` constant, not a measured approximation.
pub(crate) const SQRT_2: f32 = core::f32::consts::SQRT_2;

/// One of the eight D8 directions.
///
/// `z` grows south, so `N` is `z - 1`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum D8 {
    /// North, `z - 1`.
    N,
    /// North-east.
    NE,
    /// East, `x + 1`.
    E,
    /// South-east.
    SE,
    /// South, `z + 1`.
    S,
    /// South-west.
    SW,
    /// West, `x - 1`.
    W,
    /// North-west.
    NW,
}

impl D8 {
    /// The cell offset, in `(x, z)`.
    #[must_use]
    pub(crate) const fn offset(self) -> (i32, i32) {
        match self {
            Self::N => (0, -1),
            Self::NE => (1, -1),
            Self::E => (1, 0),
            Self::SE => (1, 1),
            Self::S => (0, 1),
            Self::SW => (-1, 1),
            Self::W => (-1, 0),
            Self::NW => (-1, -1),
        }
    }

    /// The step length in cells: `1.0` or [`SQRT_2`].
    #[must_use]
    pub(crate) const fn dist(self) -> f32 {
        match self {
            Self::N | Self::E | Self::S | Self::W => 1.0,
            Self::NE | Self::SE | Self::SW | Self::NW => SQRT_2,
        }
    }
}

/// Returns `index` moved one cell in `direction`, or `None` off the grid.
pub(crate) fn neighbour(index: usize, side: usize, direction: D8) -> Option<usize> {
    let (dx, dz) = direction.offset();
    let x = i32::try_from(index % side).ok()?.checked_add(dx)?;
    let z = i32::try_from(index / side).ok()?.checked_add(dz)?;
    if x < 0 || z < 0 {
        return None;
    }
    let x = usize::try_from(x).ok()?;
    let z = usize::try_from(z).ok()?;
    if x >= side || z >= side {
        return None;
    }
    Some(z * side + x)
}

/// Chooses one receiver per cell on the filled surface (`D2.1`).
///
/// A cell at or below `sea` is already at the outlet and has no receiver. Every
/// other cell picks the largest drop, ties by [`ORDER`]. `filled` must be a
/// `side * side` grid; cells beyond it are left without a receiver.
#[must_use]
pub(crate) fn receivers(filled: &[f32], side: usize, sea: f32) -> Vec<Option<u32>> {
    let cells = side * side;
    let mut result = vec![None; cells];
    if side == 0 {
        return result;
    }

    for (index, &level) in filled.iter().enumerate().take(cells) {
        if level <= sea {
            continue;
        }
        let mut best = 0.0_f32;
        let mut chosen = None;
        for direction in ORDER {
            let Some(near) = neighbour(index, side, direction) else {
                continue;
            };
            // `>` keeps the first direction in ORDER on an exact tie, which is
            // the whole tie-break rule.
            let drop = (level - filled[near]) / direction.dist();
            if drop > best {
                best = drop;
                chosen = Some(near);
            }
        }
        result[index] = chosen.and_then(|near| u32::try_from(near).ok());
    }
    result
}

/// Sums upstream cell counts plus boundary `inflow` (`D2.1`).
///
/// `inflow` is the integer accumulation the neighbouring tile has already
/// counted across the seam. It is the only cross-tile input, and because it is
/// supplied by the caller the result never depends on tile generation order.
///
/// A cell starts at `1 + inflow[c]` (it counts itself), then every cell is
/// processed in `(filled descending, index ascending)` order and its total added
/// to its receiver. Additions saturate instead of panicking on overflow, so a
/// pathological inflow cannot abort a tile.
#[must_use]
pub(crate) fn accumulate(
    filled: &[f32],
    receiver: &[Option<u32>],
    inflow: &[u32],
    side: usize,
) -> Vec<u32> {
    let cells = side * side;
    let mut accumulation: Vec<u32> = (0..cells)
        .map(|index| inflow.get(index).copied().unwrap_or(0).saturating_add(1))
        .collect();
    if side == 0 {
        return accumulation;
    }

    let mut order: Vec<usize> = (0..cells).collect();
    order.sort_unstable_by(|&a, &b| {
        let left = filled.get(a).copied().unwrap_or(0.0);
        let right = filled.get(b).copied().unwrap_or(0.0);
        right.total_cmp(&left).then(a.cmp(&b))
    });

    for &index in &order {
        let Some(Some(target)) = receiver.get(index).copied() else {
            continue;
        };
        let Ok(target) = usize::try_from(target) else {
            continue;
        };
        if target >= cells {
            continue;
        }
        debug_assert!(
            filled.get(target).copied().unwrap_or(0.0) < filled.get(index).copied().unwrap_or(0.0),
            "receiver must sit lower on the filled surface"
        );
        let carried = accumulation[index];
        accumulation[target] = accumulation[target].saturating_add(carried);
    }

    accumulation
}

/// Width of the headwater ramp above [`RIVER_MIN_CELLS`](super::RIVER_MIN_CELLS).
///
/// `D3.2` asks the channel to taper from zero over the first ~50 cells of
/// accumulation above the threshold.
pub(crate) const RAMP_CELLS: f32 = 50.0;

/// Widest a channel gets, in blocks (`D3.2`).
pub(crate) const MAX_WIDTH: f32 = 10.0;

/// Scale in `width = clamp(W_SCALE * sqrt(acc - min + 1), 1, 10)` (`D3.2`).
///
/// Chosen so the ramp reaches two blocks at ~50 cells above the threshold and
/// the ten-block ceiling is reached by `acc = 5000`:
///
/// ```text
/// 0.28 * sqrt(51)   = 1.9996   ~= 2 blocks   (acc = 350, end of the ramp)
/// 0.28 * sqrt(4701) = 19.19    -> clamped to 10 (acc = 5000)
/// ```
///
/// `W_SCALE` is a pinned `f32` bit constant in the description in the long run;
/// here it is the same number, so a test can show the arithmetic.
pub(crate) const W_SCALE: f32 = 0.28;

/// Maps accumulation to a channel width in blocks (`D3.1`, `D3.2`).
///
/// Below `min_cells` there is no river and the width is exactly `0`. Above it
/// the width is the `D3.2` square root (never `pow`); the square-root term is
/// clamped to `1..=MAX_WIDTH`, then multiplied by a smoothstep over
/// [`RAMP_CELLS`] so a headwater starts narrow instead of jumping to a full
/// block. Both factors are non-negative and non-decreasing, so width is
/// monotone in accumulation and never exceeds [`MAX_WIDTH`].
#[must_use]
#[allow(clippy::cast_precision_loss)]
pub(crate) fn river_width(accumulation: u32, min_cells: u32, w_scale: f32) -> f32 {
    if accumulation < min_cells {
        return 0.0;
    }
    let excess = accumulation - min_cells;
    let root = w_scale * (excess as f32 + 1.0).sqrt();
    let base = root.clamp(1.0, MAX_WIDTH);
    base * smoothstep(0.0, RAMP_CELLS, excess as f32)
}

/// Hermite `smoothstep`, clamped to `[0, 1]`, using only `+ - * /`.
///
/// Deterministic and branch-free after the clamp, so it is safe on the
/// generation path (`A2.3`).
#[must_use]
pub(crate) fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// U-profile bed depth in blocks: `clamp(width / 2, 2, 6)` (`D3.3`).
#[must_use]
pub(crate) fn bed_depth(width: f32) -> f32 {
    (width / 2.0).clamp(2.0, 6.0)
}

/// Per-cell river influence in `[0, 1]` (`D3.3`).
///
/// `1` across the bed half-width `width / 2`, `0` at and beyond the bank
/// half-width `width` (the bed plus `width / 2` of bank on each side), and a
/// smoothstep in between. W4 multiplies this by the bed depth to carve; the
/// distance comes from the interpolated field, never from a per-block path
/// search (`D3.3`). A zero or negative width has no influence at all.
#[must_use]
pub(crate) fn river_influence(width: f32, distance_cells: f32) -> f32 {
    if width <= 0.0 {
        return 0.0;
    }
    let bed = width / 2.0;
    let bank = width;
    if distance_cells <= bed {
        return 1.0;
    }
    if distance_cells >= bank {
        return 0.0;
    }
    1.0 - smoothstep(bed, bank, distance_cells)
}

/// Clamps a river-bed top so it never reaches the floor (`D3.3`).
///
/// The bed is never below [`BED_FLOOR`] (`SEA - 30`) or `y = 1`.
#[must_use]
pub(crate) fn clamp_bed(y: i32) -> i32 {
    y.max(BED_FLOOR).max(1)
}

/// Water level of a river cell: `floor(center_height - 1)` (`D7.4`).
#[must_use]
pub(crate) fn river_level(center_height: f32) -> i32 {
    super::floor_i32(center_height - 1.0)
}

/// Makes river levels stepped and monotone downstream (`D7.4`).
///
/// Applies `level[c] = min(level[c], level[receiver[c]] + 1)` in
/// downstream-to-upstream order. That order is a Kahn sweep from the sinks over
/// the receiver forest: each cell is a node with at most one outgoing edge, so
/// the sweep is linear and every cell is relaxed after its receiver. A cycle
/// would leave nodes unvisited and is caught by a debug assertion, not by a
/// hang.
#[allow(dead_code)]
#[must_use]
pub(crate) fn monotone_levels(level: &[i32], receiver: &[Option<u32>]) -> Vec<i32> {
    let cells = level.len();
    let mut out = level.to_vec();
    if cells == 0 {
        return out;
    }

    // Children in compressed form: `start[node]..start[node + 1]` indexes
    // `children`. Built by two ascending passes, so the child order is fixed.
    let mut start = vec![0_usize; cells + 1];
    for target in receiver.iter().flatten() {
        if let Ok(target) = usize::try_from(*target)
            && target < cells
        {
            start[target + 1] += 1;
        }
    }
    for index in 0..cells {
        start[index + 1] += start[index];
    }
    let mut cursor = start[..cells].to_vec();
    let mut children = vec![0_u32; start[cells]];
    for (child, target) in receiver.iter().enumerate() {
        let Some(target) = *target else {
            continue;
        };
        let Ok(target) = usize::try_from(target) else {
            continue;
        };
        if target >= cells {
            continue;
        }
        let slot = cursor[target];
        if let Some(entry) = children.get_mut(slot) {
            *entry = u32::try_from(child).unwrap_or(u32::MAX);
        }
        cursor[target] += 1;
    }

    let mut queue: VecDeque<u32> = VecDeque::with_capacity(cells);
    for (index, target) in receiver.iter().enumerate() {
        if target.is_none() {
            queue.push_back(u32::try_from(index).unwrap_or(u32::MAX));
        }
    }

    let mut processed = 0_usize;
    for _ in 0..=cells {
        let Some(node) = queue.pop_front() else {
            break;
        };
        let Ok(index) = usize::try_from(node) else {
            break;
        };
        if index >= cells {
            break;
        }
        processed += 1;
        let bound = out[index].saturating_add(1);
        for &child in &children[start[index]..start[index + 1]] {
            let Ok(child_index) = usize::try_from(child) else {
                continue;
            };
            if child_index >= cells {
                continue;
            }
            out[child_index] = out[child_index].min(bound);
            queue.push_back(child);
        }
    }
    debug_assert_eq!(processed, cells, "receiver graph has a cycle");

    out
}

#[cfg(test)]
mod tests {
    use super::super::fixture::Rng;
    use super::super::flood::priority_flood;
    use super::*;

    fn filled_grid(side: usize, value: f32) -> Vec<f32> {
        vec![value; side * side]
    }

    fn set(filled: &mut [f32], side: usize, x: usize, z: usize, value: f32) {
        filled[z * side + x] = value;
    }

    fn receiver_at(filled: &[f32], side: usize, x: usize, z: usize) -> Option<u32> {
        receivers(filled, side, 0.0)[z * side + x]
    }

    #[test]
    fn d8_ties_break_by_fixed_order() {
        assert_eq!(ORDER.len(), 8);
        assert_eq!(D8::N.offset(), (0, -1));
        assert_eq!(D8::NW.offset(), (-1, -1));
        assert_eq!(D8::E.dist(), 1.0);
        assert_eq!(SQRT_2.to_bits(), 0x3FB5_04F3);
        assert_eq!(D8::NE.dist(), SQRT_2);

        // Centre height `2`: `2 - sqrt(2)` is exact in `f32` (the subtraction
        // keeps every mantissa bit of `sqrt(2)`), so a diagonal neighbour at
        // that height has the same drop as an orthogonal one at `1`. Exact
        // ties are what the order is for; a rounded tie would test rounding.
        let side = 5;
        let centre = (2_usize, 2_usize);
        let index = centre.1 * side + centre.0;

        // N over E: equal drops, N comes first.
        let mut filled = filled_grid(side, 2.0);
        set(&mut filled, side, 2, 1, 1.0); // N
        set(&mut filled, side, 3, 2, 1.0); // E
        assert_eq!(
            receiver_at(&filled, side, centre.0, centre.1),
            Some(u32::try_from(index - side).unwrap())
        );

        // NE over E: equal drops once the diagonal is divided by sqrt(2).
        let mut filled = filled_grid(side, 2.0);
        set(&mut filled, side, 3, 1, 2.0 - SQRT_2); // NE
        set(&mut filled, side, 3, 2, 1.0); // E
        assert_eq!(
            receiver_at(&filled, side, centre.0, centre.1),
            Some(u32::try_from(index - side + 1).unwrap())
        );

        // SE over S.
        let mut filled = filled_grid(side, 2.0);
        set(&mut filled, side, 3, 3, 2.0 - SQRT_2); // SE
        set(&mut filled, side, 2, 3, 1.0); // S
        assert_eq!(
            receiver_at(&filled, side, centre.0, centre.1),
            Some(u32::try_from(index + side + 1).unwrap())
        );

        // A diagonal is normalized, so an equal-height orthogonal neighbour
        // beats it. Not a tie, a slope rule.
        let mut filled = filled_grid(side, 2.0);
        set(&mut filled, side, 3, 1, 1.0); // NE, drop 1/sqrt(2)
        set(&mut filled, side, 1, 2, 1.0); // W, drop 1
        assert_eq!(
            receiver_at(&filled, side, centre.0, centre.1),
            Some(u32::try_from(index - 1).unwrap())
        );

        // No lower neighbour means no receiver.
        let filled = filled_grid(side, 2.0);
        assert_eq!(receiver_at(&filled, side, centre.0, centre.1), None);
    }

    #[test]
    fn every_cell_has_a_receiver_and_no_cycle() {
        let side = 21;
        let sea = super::super::sea_level();
        let mut rng = Rng::new(0xC0FF_EE12);
        let mut height = vec![0.0_f32; side * side];
        let mut interior = 0;
        for z in 0..side {
            for x in 0..side {
                let border = x == 0 || z == 0 || x + 1 == side || z + 1 == side;
                let index = z * side + x;
                if border {
                    height[index] = sea - 4.0;
                } else {
                    height[index] = sea + 1.0 + f32::from(u8::try_from(rng.below(60)).unwrap());
                    interior += 1;
                }
            }
        }

        let filled = priority_flood(&height, side);
        let receiver = receivers(&filled, side, sea);

        let mut routed = 0;
        for index in 0..side * side {
            if height[index] <= sea {
                assert_eq!(receiver[index], None, "sea cell {index} routes");
                continue;
            }
            routed += 1;
            let mut current = index;
            let mut steps = 0;
            while let Some(next) = receiver[current] {
                let next = usize::try_from(next).unwrap();
                assert!(
                    filled[next] < filled[current],
                    "receiver must be lower at {current}"
                );
                current = next;
                steps += 1;
                assert!(steps <= side * side, "cycle from {index}");
            }
            assert!(steps > 0 || height[current] <= sea);
        }
        assert_eq!(routed, interior, "every interior cell must route");
    }

    #[test]
    fn accumulation_matches_a_hand_count() {
        // 3x3 funnel: eight cells drain into the centre, which drains nowhere.
        let side = 3;
        let filled = [4.0, 4.0, 4.0, 4.0, 0.0, 4.0, 4.0, 4.0, 4.0];
        let centre = 4_u32;
        let mut receiver = vec![Some(centre); 9];
        receiver[4] = None;

        // Nothing extra: each cell counts itself and the centre receives all.
        let inflow = vec![0_u32; 9];
        let accumulated = accumulate(&filled, &receiver, &inflow, side);
        assert_eq!(
            accumulated,
            vec![1, 1, 1, 1, 9, 1, 1, 1, 1],
            "hand count, no inflow"
        );

        // Ten cells of inflow at a border cell: it carries eleven, and the
        // centre sees 9 + 10.
        let mut inflow = vec![0_u32; 9];
        inflow[0] = 10;
        let accumulated = accumulate(&filled, &receiver, &inflow, side);
        assert_eq!(
            accumulated,
            vec![11, 1, 1, 1, 19, 1, 1, 1, 1],
            "hand count with border inflow"
        );
    }

    #[test]
    fn river_width_is_monotone_and_bounded() {
        let min = 300;
        let width = |cells| river_width(cells, min, W_SCALE);

        assert_eq!(width(0), 0.0);
        assert_eq!(width(min - 1), 0.0);
        // The ramp starts at exactly zero on the threshold.
        assert_eq!(width(min), 0.0);

        // Show the `W_SCALE` arithmetic: 0.28 * sqrt(51) = 1.9996 blocks.
        let near = width(min + 50);
        assert!((near - 2.0).abs() < 0.01, "acc=350 gave {near}");

        // 0.28 * sqrt(4701) = 19.19, clamped to the ten-block ceiling.
        assert_eq!(width(5_000), MAX_WIDTH);
        assert_eq!(width(100_000), MAX_WIDTH);

        let mut previous = 0.0_f32;
        for cells in 0..12_000_u32 {
            let current = width(cells);
            assert!(current >= previous, "width fell at {cells}");
            assert!((0.0..=MAX_WIDTH).contains(&current), "width {current}");
            previous = current;
        }
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn bed_depth_and_influence() {
        assert_eq!(bed_depth(0.0), 2.0);
        assert_eq!(bed_depth(4.0), 2.0);
        assert_eq!(bed_depth(8.0), 4.0);
        assert_eq!(bed_depth(12.0), 6.0);
        assert_eq!(bed_depth(40.0), 6.0);
        assert_eq!(clamp_bed(0), 98);
        assert_eq!(clamp_bed(150), 150);

        let width = 4.0;
        assert_eq!(river_influence(width, 0.0), 1.0);
        assert_eq!(river_influence(width, 2.0), 1.0);
        assert_eq!(river_influence(width, 3.0), 0.5);
        assert_eq!(river_influence(width, 4.0), 0.0);
        assert_eq!(river_influence(width, 40.0), 0.0);
        assert_eq!(river_influence(0.0, 0.0), 0.0);

        // Monotone across the bed and the bank, and always in `[0, 1]`.
        let mut previous = 1.0_f32;
        for step in 0..=40 {
            let distance = step as f32 / 4.0;
            let influence = river_influence(width, distance);
            assert!((0.0..=1.0).contains(&influence), "influence {influence}");
            assert!(influence <= previous, "influence rose at {distance}");
            previous = influence;
        }
    }

    #[test]
    fn river_level_is_one_below_the_centre() {
        assert_eq!(river_level(100.9), 99);
        assert_eq!(river_level(100.0), 99);
        assert_eq!(river_level(99.0), 98);
        assert_eq!(river_level(0.5), -1);
    }

    #[test]
    fn monotone_levels_never_exceed_the_receiver() {
        let side = 24;
        let cells = side * side;
        let mut rng = Rng::new(0x1DEA_0007);

        // A random forest: every cell either drains to a higher index (which
        // cannot cycle) or nowhere.
        let mut receiver: Vec<Option<u32>> = (0..cells)
            .map(|index| {
                let span = u32::try_from(cells - index).unwrap();
                if index + 1 == cells || rng.below(3) == 0 {
                    None
                } else {
                    Some(u32::try_from(index).unwrap() + 1 + rng.below(span - 1))
                }
            })
            .collect();
        receiver[cells - 1] = None;

        let mut level: Vec<i32> = (0..cells)
            .map(|_| i32::try_from(rng.below(400)).unwrap() - 100)
            .collect();
        level[0] = 10_000;
        let original = level.clone();
        let stepped = monotone_levels(&level, &receiver);

        for index in 0..cells {
            assert!(stepped[index] <= original[index], "level rose at {index}");
            if let Some(target) = receiver[index] {
                let target = usize::try_from(target).unwrap();
                assert!(
                    stepped[index] <= stepped[target] + 1,
                    "cell {index} sits above its receiver {target}"
                );
            }
        }
    }

    #[test]
    fn monotone_levels_walks_downstream_first() {
        // 0 -> 1 -> 2, with 2 as the sink. Cell 1 caps at the sink plus one
        // and cell 0 at the *stepped* cell 1, which is only correct when the
        // sweep starts downstream. Upstream-first would give cell 0 a 4.
        let receiver = vec![Some(1), Some(2), None];
        let level = vec![10, 3, 0];
        assert_eq!(monotone_levels(&level, &receiver), vec![2, 1, 0]);
    }

    #[test]
    fn sea_cells_do_not_route() {
        let side = 3;
        let sea = super::super::sea_level();
        let filled = filled_grid(side, sea);
        let receiver = receivers(&filled, side, sea);
        assert!(receiver.iter().all(Option::is_none));
    }
}

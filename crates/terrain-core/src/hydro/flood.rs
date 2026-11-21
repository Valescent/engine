//! Priority-flood depression filling (`D1.1`, `D2.5`, `D2.6`).
//!
//! The flood is the only thing that makes D8 total: it raises every pit to its
//! spill level and gives every flat a strict slope, so routing on the result
//! has no interior sinks and no cycles (`D2.1`). Filled heights are never
//! terrain; they exist for routing and accumulation only (`D1.2`).
//!
//! # The queue key
//!
//! One entry is `(filled_height, cell index)`. Heights are compared with
//! [`f32::total_cmp`], then the index ascending, so the key is a total order
//! over distinct entries and the pop order is a pure function of the grid. That
//! is what makes the result independent of insertion order, heap layout, the
//! order the caller filled the height array, and the tile order (`D2.1`,
//! `D8.2`). The index is the tie-break, never the insertion sequence.

use std::cmp::{Ordering, Reverse};
use std::collections::BinaryHeap;

use super::SEA;
use super::flow::{ORDER, neighbour};

/// Per-cell rise added by the flood (`D2.5`): exactly `1/64`, `0x3C80_0000`.
///
/// Exact in `f32` up to about `2^17` blocks of accumulated rise, so the epsilon
/// itself never rounds a filled height. A 128-cell flat crossing therefore
/// distorts by at most `2` blocks, which is the bound `D2.5` quotes.
pub(crate) const EPS: f32 = 1.0 / 64.0;

/// One priority-queue entry, ordered by `(height, index)` exactly.
#[derive(Clone, Copy, Debug)]
struct Key {
    value: f32,
    index: u32,
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Key {}

impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        self.value
            .total_cmp(&other.value)
            .then(self.index.cmp(&other.index))
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Fills depressions with the epsilon flood of `D2.5`.
///
/// `height` is a `side * side` row-major grid. Seeds are the outer ring of the
/// grid at its own height and every sea cell (`height <= SEA`); everything else
/// is raised to `max(h[n], filled[c] + EPS)` from its lowest predecessor. The
/// eight D8 neighbours are used, matching the router, so no escape path the
/// router can take is invisible to the flood.
///
/// The seeded border is never raised. Inside a [`HydroTile`](super::HydroTile)
/// that border is the halo, whose published values are the neighbour's own
/// terrain (`D1.3`); here it is just the open boundary of whatever grid was
/// passed.
///
/// Returns an empty vector when `height.len() != side * side`.
pub(crate) fn priority_flood(height: &[f32], side: usize) -> Vec<f32> {
    priority_flood_from(height, side, 1)
}

/// [`priority_flood`] with a `border`-cell band pinned instead of one ring.
///
/// A tile passes its halo width, so the whole halo is boundary state and only
/// the interior is filled. A bare test grid passes `1`.
#[allow(clippy::cast_precision_loss)]
pub(crate) fn priority_flood_from(height: &[f32], side: usize, border: usize) -> Vec<f32> {
    let cells = side * side;
    if side == 0 || height.len() != cells {
        return Vec::new();
    }
    let border = border.max(1);

    let sea = SEA as f32;
    let mut filled = vec![0.0_f32; cells];
    let mut closed = vec![false; cells];
    let mut heap: BinaryHeap<Reverse<Key>> = BinaryHeap::with_capacity(cells);

    for (index, &value) in height.iter().enumerate() {
        let x = index % side;
        let z = index / side;
        let pinned = x < border || z < border || x + border >= side || z + border >= side;
        if pinned || value <= sea {
            heap.push(Reverse(key(value, index)));
        }
    }

    // Every cell enters the heap at most once as a seed and at most once per
    // closed neighbour, so this bound is never reached.
    let limit = cells.saturating_mul(9).saturating_add(1);
    for _ in 0..limit {
        let Some(Reverse(entry)) = heap.pop() else {
            break;
        };
        let index = entry.index as usize;
        if closed[index] {
            continue;
        }
        closed[index] = true;
        filled[index] = entry.value;

        for direction in ORDER {
            let Some(near) = neighbour(index, side, direction) else {
                continue;
            };
            if closed[near] {
                continue;
            }
            let candidate = height[near].max(entry.value + EPS);
            heap.push(Reverse(key(candidate, near)));
        }
    }

    debug_assert!(
        closed.iter().all(|&done| done),
        "priority flood left a cell unvisited"
    );
    filled
}

/// Packs one heap entry. The index cannot exceed `2^32` cells in practice.
#[allow(clippy::cast_possible_truncation)]
fn key(value: f32, index: usize) -> Key {
    Key {
        value,
        index: index as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixture::Rng;
    use super::*;

    /// A 7x7 bowl: rim `200`, interior `150`. Both are above the sea, so only
    /// the rim seeds the flood and the interior has to fill.
    fn bowl() -> (Vec<f32>, usize) {
        let side = 7;
        let mut height = vec![150.0_f32; side * side];
        for z in 0..side {
            for x in 0..side {
                if x == 0 || z == 0 || x + 1 == side || z + 1 == side {
                    height[z * side + x] = 200.0;
                }
            }
        }
        (height, side)
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn flood_fills_a_bowl() {
        let (height, side) = bowl();
        let filled = priority_flood(&height, side);

        // The rim is a seed, so it is unchanged.
        for z in 0..side {
            for x in 0..side {
                let rim = x == 0 || z == 0 || x + 1 == side || z + 1 == side;
                let index = z * side + x;
                if rim {
                    assert_eq!(filled[index], 200.0, "rim at {x},{z}");
                }
            }
        }

        // The interior is level at the rim height. The epsilon flood expresses
        // "level" as a staircase of `EPS` steps measured from the rim, so the
        // exact statement is: above the rim, never more than one step per cell
        // of distance from it, and never below it.
        for z in 1..side - 1 {
            for x in 1..side - 1 {
                let index = z * side + x;
                let steps = x.min(z).min(side - 1 - x).min(side - 1 - z) + 1;
                let upper = 200.0 + steps as f32 * EPS;
                assert!(filled[index] >= 200.0, "filled below the rim at {x},{z}");
                assert!(
                    filled[index] <= upper,
                    "filled above the rim staircase at {x},{z}: {}",
                    filled[index]
                );
            }
        }

        // Every interior cell is held at the rim, not at its own floor.
        for z in 1..side - 1 {
            for x in 1..side - 1 {
                assert!(filled[z * side + x] > 199.0);
            }
        }
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn flood_epsilon_is_one_64th() {
        assert_eq!(EPS.to_bits(), 0x3C80_0000);
        assert_eq!(EPS, 0.015_625);

        // A flat plain: the only relief the flood can create is its own
        // epsilon, one step per cell away from the seeded ring.
        let side = 11;
        let height = vec![200.0_f32; side * side];
        let filled = priority_flood(&height, side);

        for z in 0..side {
            for x in 0..side {
                let index = z * side + x;
                let steps = x.min(z).min(side - 1 - x).min(side - 1 - z);
                let expected = 200.0 + steps as f32 * EPS;
                assert_eq!(filled[index], expected, "filled at {x},{z}");
            }
        }

        // Adjacent cells differ by EPS along a rising path and by nothing at
        // all along the ring a given distance out. Never more than EPS.
        for z in 0..side {
            for x in 0..side {
                let index = z * side + x;
                if x + 1 < side {
                    let delta = filled[index + 1] - filled[index];
                    assert!(delta.abs() <= EPS, "x step at {x},{z}: {delta}");
                }
                if z + 1 < side {
                    let delta = filled[index + side] - filled[index];
                    assert!(delta.abs() <= EPS, "z step at {x},{z}: {delta}");
                }
            }
        }
    }

    #[test]
    fn flood_is_order_independent() {
        // A plain with pits and ridges: flats exercise the index tie-break,
        // and the pits exercise the fill.
        let side = 13;
        let mut height = vec![0.0_f32; side * side];
        for (index, value) in height.iter_mut().enumerate() {
            let x = index % side;
            let z = index / side;
            *value = if (x * 7 + z * 11) % 5 == 0 {
                20.0
            } else {
                10.0 + f32::from(u8::try_from((x * z) % 3).unwrap())
            };
        }
        let expected = priority_flood(&height, side);

        // Permute the input array. Cell indices move with the data, so the
        // heap tie-break and the seed insertion order both change. The values
        // must not.
        let mut order: Vec<usize> = (0..side * side).collect();
        let mut rng = Rng::new(0x5EED_1234);
        for round in 0..4 {
            rng.shuffle(&mut order);
            let mut permuted = vec![0.0_f32; side * side];
            for (slot, &source) in order.iter().enumerate() {
                permuted[slot] = height[source];
            }
            let got = priority_flood(&permuted, side);
            for (slot, &source) in order.iter().enumerate() {
                assert_eq!(
                    got[slot].to_bits(),
                    expected[source].to_bits(),
                    "round {round}, cell {source}"
                );
            }
        }
    }
}

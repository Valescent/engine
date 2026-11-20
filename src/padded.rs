//! `W13` — the shared padded `34³` block and light scratch (`I2.3`, `I2.4`).
//!
//! A job copies one [`Section`] plus its 26 neighbours once, then runs without
//! holding a borrow on the world. The block scratch is [`PaddedBlocks`]; the
//! light scratch is [`PaddedLight`], two nibble-expanded planes of the same
//! shape. The mesher reads both and the lighting job produces [`PaddedLight`],
//! so there is exactly one padded layout in the engine.
//!
//! # Index formula (`I2.3`)
//!
//! Local coordinates are `-1..=32` on every axis. The cell order is `x`
//! fastest, then `z`, then `y`:
//!
//! ```text
//! index(x, y, z) = (x + 1) + 34 * ((z + 1) + 34 * (y + 1))
//! ```
//!
//! [`index`] clamps each axis to `-1..=32`, so a stray lookup cannot escape the
//! array.
//!
//! # Neighbour order (`I2.4`)
//!
//! [`NEIGHBOUR_ORDER`] is total: six faces, then twelve edges, then eight
//! corners, 26 entries. The order is:
//!
//! ```text
//! faces   (+1, 0, 0)  (-1, 0, 0)  (0, +1, 0)  (0, -1, 0)  (0, 0, +1)  (0, 0, -1)
//! edges along X   (0, +1, +1)  (0, +1, -1)  (0, -1, +1)  (0, -1, -1)
//! edges along Y   (+1, 0, +1)  (+1, 0, -1)  (-1, 0, +1)  (-1, 0, -1)
//! edges along Z   (+1, +1, 0)  (+1, -1, 0)  (-1, +1, 0)  (-1, -1, 0)
//! corners (+1, +1, +1)  (+1, +1, -1)  (+1, -1, +1)  (+1, -1, -1)
//!         (-1, +1, +1)  (-1, +1, -1)  (-1, -1, +1)  (-1, -1, -1)
//! ```
//!
//! Each edge group is ordered by its first varying axis (`+` before `-`) and
//! then its second; each corner group by `x`, then `y`, then `z`, `+` before
//! `-`.
//!
//! # Missing neighbours
//!
//! A `None` neighbour mirrors the edge row of the target section, which is the
//! `I2.3` world-edge rule: a cell at local `-1` reads local `0` and a cell at
//! local `32` reads local `31` on that axis. The same mirror applies to edges
//! and corners, so a world-edge section pads as if the world continued by
//! reflection. A caller with an unresolved (but not world-edge) neighbour must
//! pass an explicit air section. Vertical out-of-world blocks are the caller's
//! job too: pass a uniform bedrock section below the world and air above.
//!
//! Light is different (`L1.1`): below the world is dark and above it is full
//! sky. [`PaddedLight`] is therefore a plain scratch that a producer fills; it
//! has no neighbour-array constructor.

use crate::description::SECTION_SIZE;
use crate::section::{BlockId, Section};

/// Blocks of padding on each side of a section (`I2.3`).
pub const PAD: i32 = 1;

/// Edge length of the padded scratch cube.
pub const PADDED_WIDTH: usize = SECTION_SIZE as usize + 2;

/// Cells in the padded scratch cube.
pub const PADDED_VOLUME: usize = PADDED_WIDTH * PADDED_WIDTH * PADDED_WIDTH;

/// [`SECTION_SIZE`] as the signed coordinate type used by the scratch.
#[allow(
    clippy::cast_possible_wrap,
    reason = "SECTION_SIZE is 32, far inside i32"
)]
pub const LOCAL_SIZE: i32 = SECTION_SIZE as i32;

/// Lowest local coordinate of the scratch (`-1`).
pub const LOCAL_MIN: i32 = -PAD;

/// Highest local coordinate of the scratch (`32`).
pub const LOCAL_MAX: i32 = LOCAL_SIZE;

/// Highest nibble value.
pub const NIBBLE_MAX: u8 = 15;

/// Low nibble mask.
pub const NIBBLE_MASK: u8 = 0x0F;

/// Neighbours in a section's one-block pad, in the documented total order.
///
/// Faces first (`+X, -X, +Y, -Y, +Z, -Z`), then the twelve edges, then the
/// eight corners. Every offset is one step on each non-zero axis, so the table
/// is exactly the 26 cells that touch the closed `32³` box.
pub const NEIGHBOUR_ORDER: [(i32, i32, i32); 26] = [
    // Faces.
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
    // Edges parallel to X.
    (0, 1, 1),
    (0, 1, -1),
    (0, -1, 1),
    (0, -1, -1),
    // Edges parallel to Y.
    (1, 0, 1),
    (1, 0, -1),
    (-1, 0, 1),
    (-1, 0, -1),
    // Edges parallel to Z.
    (1, 1, 0),
    (1, -1, 0),
    (-1, 1, 0),
    (-1, -1, 0),
    // Corners.
    (1, 1, 1),
    (1, 1, -1),
    (1, -1, 1),
    (1, -1, -1),
    (-1, 1, 1),
    (-1, 1, -1),
    (-1, -1, 1),
    (-1, -1, -1),
];

/// Index of a local coordinate in the scratch, clamped to `-1..=32`.
pub fn index(x: i32, y: i32, z: i32) -> usize {
    axis(x) + PADDED_WIDTH * (axis(z) + PADDED_WIDTH * axis(y))
}

/// Shifts one clamped local axis coordinate into `0..34`.
fn axis(value: i32) -> usize {
    let clamped = value.clamp(LOCAL_MIN, LOCAL_MAX);
    usize::try_from(clamped + 1).expect("a padded axis coordinate fits usize")
}

/// `-1`, `0`, or `1` for a local coordinate in `-1..=32`.
fn outside(value: i32) -> i32 {
    match value {
        ..=-1 => -1,
        LOCAL_SIZE.. => 1,
        _ => 0,
    }
}

/// The neighbour-local coordinate of a pad cell: wraps across the section.
fn wrapped(value: i32) -> u32 {
    match value {
        ..=-1 => 31,
        LOCAL_SIZE.. => 0,
        _ => u32::try_from(value).expect("an interior coordinate is non-negative"),
    }
}

/// The own-section coordinate a missing neighbour mirrors.
fn mirrored(value: i32) -> u32 {
    u32::try_from(value.clamp(0, LOCAL_SIZE - 1)).expect("a mirrored coordinate is non-negative")
}

/// The slot of a non-zero neighbour offset.
fn neighbour_slot(dx: i32, dy: i32, dz: i32) -> usize {
    match (dx, dy, dz) {
        (1, 0, 0) => 0,
        (-1, 0, 0) => 1,
        (0, 1, 0) => 2,
        (0, -1, 0) => 3,
        (0, 0, 1) => 4,
        (0, 0, -1) => 5,
        (0, 1, 1) => 6,
        (0, 1, -1) => 7,
        (0, -1, 1) => 8,
        (0, -1, -1) => 9,
        (1, 0, 1) => 10,
        (1, 0, -1) => 11,
        (-1, 0, 1) => 12,
        (-1, 0, -1) => 13,
        (1, 1, 0) => 14,
        (1, -1, 0) => 15,
        (-1, 1, 0) => 16,
        (-1, -1, 0) => 17,
        (1, 1, 1) => 18,
        (1, 1, -1) => 19,
        (1, -1, 1) => 20,
        (1, -1, -1) => 21,
        (-1, 1, 1) => 22,
        (-1, 1, -1) => 23,
        (-1, -1, 1) => 24,
        (-1, -1, -1) => 25,
        _ => panic!("a padded neighbour offset is one step on each non-zero axis"),
    }
}

/// Padded block scratch for one section plus its 26 neighbours.
///
/// Local coordinates are `-1..=32` on every axis. Coordinates outside that
/// range are clamped, so a stray lookup can never read past the array. The
/// interior is copied from the section; the pad starts as air.
#[derive(Clone)]
pub struct PaddedBlocks {
    /// `x` fastest, then `z`, then `y`, matching `I2.3`.
    blocks: [BlockId; PADDED_VOLUME],
}

impl PaddedBlocks {
    /// An all-air scratch cube.
    #[allow(
        clippy::large_stack_arrays,
        reason = "I2.3 wants one 79 KB padded scratch per worker, not a heap indirection"
    )]
    pub fn new() -> Self {
        Self {
            blocks: [0; PADDED_VOLUME],
        }
    }

    /// Index of a local coordinate in the pad, clamped to `-1..=32`.
    pub fn index(x: i32, y: i32, z: i32) -> usize {
        index(x, y, z)
    }

    /// Reads one padded cell. Coordinates are clamped to `-1..=32`.
    pub fn get(&self, x: i32, y: i32, z: i32) -> BlockId {
        self.blocks[index(x, y, z)]
    }

    /// Writes one padded cell. Coordinates are clamped to `-1..=32`.
    pub fn set(&mut self, x: i32, y: i32, z: i32, id: BlockId) {
        self.blocks[index(x, y, z)] = id;
    }

    /// Fills the `32³` interior from `section` and clears the pad to air.
    pub fn fill_from(&mut self, section: &Section) {
        self.blocks.fill(0);
        for y in 0..LOCAL_SIZE {
            for z in 0..LOCAL_SIZE {
                for x in 0..LOCAL_SIZE {
                    let block_x = u32::try_from(x).expect("a local coordinate fits u32");
                    let block_y = u32::try_from(y).expect("a local coordinate fits u32");
                    let block_z = u32::try_from(z).expect("a local coordinate fits u32");
                    self.blocks[index(x, y, z)] = section.get(block_x, block_y, block_z);
                }
            }
        }
    }

    /// Copies one section plus its 26 neighbours into a fresh scratch (`I2.4`).
    ///
    /// `neighbours` is indexed by [`NEIGHBOUR_ORDER`]. A `None` entry mirrors
    /// the target section's edge row, which is the `I2.3` world-edge rule; see
    /// the module docs. Interior cells ignore the array.
    pub fn from_sections(section: &Section, neighbours: &[Option<&Section>; 26]) -> Self {
        let mut padded = Self::new();
        for y in LOCAL_MIN..=LOCAL_MAX {
            for z in LOCAL_MIN..=LOCAL_MAX {
                for x in LOCAL_MIN..=LOCAL_MAX {
                    let offset = [outside(x), outside(y), outside(z)];
                    let id = if offset == [0, 0, 0] {
                        section.get(wrapped(x), wrapped(y), wrapped(z))
                    } else {
                        let slot = neighbour_slot(offset[0], offset[1], offset[2]);
                        if let Some(owner) = neighbours[slot] {
                            owner.get(wrapped(x), wrapped(y), wrapped(z))
                        } else {
                            section.get(mirrored(x), mirrored(y), mirrored(z))
                        }
                    };
                    padded.set(x, y, z, id);
                }
            }
        }
        padded
    }
}

impl Default for PaddedBlocks {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for PaddedBlocks {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PaddedBlocks")
            .field("width", &PADDED_WIDTH)
            .finish_non_exhaustive()
    }
}

/// Padded sky and block light nibbles for one section plus its 26 neighbours.
///
/// Sky and block light are separate arrays of expanded nibbles (`L2.2`). Local
/// coordinates and the index formula match [`PaddedBlocks`]. A fresh scratch is
/// the uniform air rule: sky [`NIBBLE_MAX`], block `0`.
#[derive(Clone)]
pub struct PaddedLight {
    /// Expanded sky nibbles, one byte per cell.
    sky: [u8; PADDED_VOLUME],
    /// Expanded block nibbles, one byte per cell.
    block: [u8; PADDED_VOLUME],
}

impl PaddedLight {
    /// A scratch cube filled with one sky and one block nibble.
    #[allow(
        clippy::large_stack_arrays,
        reason = "I2.3 wants one padded light scratch per worker, not a heap indirection"
    )]
    pub fn new(sky: u8, block: u8) -> Self {
        Self {
            sky: [sky & NIBBLE_MASK; PADDED_VOLUME],
            block: [block & NIBBLE_MASK; PADDED_VOLUME],
        }
    }

    /// Index of a local coordinate in the pad, clamped to `-1..=32`.
    pub fn index(x: i32, y: i32, z: i32) -> usize {
        index(x, y, z)
    }

    /// Reads `(sky, block)` at one cell. Coordinates are clamped.
    pub fn get(&self, x: i32, y: i32, z: i32) -> (u8, u8) {
        let cell = index(x, y, z);
        (self.sky[cell], self.block[cell])
    }

    /// Writes `(sky, block)` at one cell. Values are masked to a nibble.
    pub fn set(&mut self, x: i32, y: i32, z: i32, sky: u8, block: u8) {
        let cell = index(x, y, z);
        self.sky[cell] = sky & NIBBLE_MASK;
        self.block[cell] = block & NIBBLE_MASK;
    }
}

impl Default for PaddedLight {
    /// The uniform air rule: sky 15, block 0 (`L1.1`).
    fn default() -> Self {
        Self::new(NIBBLE_MAX, 0)
    }
}

impl std::fmt::Debug for PaddedLight {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PaddedLight")
            .field("width", &PADDED_WIDTH)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The owning-section read a pad cell must match.
    fn direct(
        section: &Section,
        neighbours: &[Option<&Section>; 26],
        x: i32,
        y: i32,
        z: i32,
    ) -> BlockId {
        let offset = [outside(x), outside(y), outside(z)];
        if offset == [0, 0, 0] {
            return section.get(wrapped(x), wrapped(y), wrapped(z));
        }
        let slot = neighbour_slot(offset[0], offset[1], offset[2]);
        neighbours[slot].map_or_else(
            || section.get(mirrored(x), mirrored(y), mirrored(z)),
            |owner| owner.get(wrapped(x), wrapped(y), wrapped(z)),
        )
    }

    #[test]
    fn padded_copy_matches_direct_reads() {
        let section = Section::uniform(99);
        let owners: [Section; 26] = std::array::from_fn(|slot| {
            Section::uniform(u16::try_from(slot + 1).expect("a slot fits u16"))
        });
        let neighbours: [Option<&Section>; 26] = std::array::from_fn(|slot| Some(&owners[slot]));
        let padded = PaddedBlocks::from_sections(&section, &neighbours);

        for y in LOCAL_MIN..=LOCAL_MAX {
            for z in LOCAL_MIN..=LOCAL_MAX {
                for x in LOCAL_MIN..=LOCAL_MAX {
                    assert_eq!(
                        padded.get(x, y, z),
                        direct(&section, &neighbours, x, y, z),
                        "cell {x},{y},{z}"
                    );
                }
            }
        }
    }

    #[test]
    fn index_is_the_documented_formula() {
        for (x, y, z, expected) in [
            (-1, -1, -1, 0),
            (32, -1, -1, 33),
            (-1, -1, 32, PADDED_WIDTH * 33),
            (-1, 32, -1, PADDED_WIDTH * PADDED_WIDTH * 33),
            (32, 32, -1, PADDED_WIDTH * PADDED_WIDTH * 33 + 33),
            (32, -1, 32, PADDED_WIDTH * 33 + 33),
            (
                -1,
                32,
                32,
                PADDED_WIDTH * PADDED_WIDTH * 33 + PADDED_WIDTH * 33,
            ),
            (32, 32, 32, PADDED_VOLUME - 1),
        ] {
            let formula = usize::try_from((x + 1) + 34 * ((z + 1) + 34 * (y + 1)))
                .expect("a padded index is non-negative");
            assert_eq!(index(x, y, z), expected, "cell {x},{y},{z}");
            assert_eq!(index(x, y, z), formula, "cell {x},{y},{z}");
            assert_eq!(PaddedBlocks::index(x, y, z), formula);
            assert_eq!(PaddedLight::index(x, y, z), formula);
        }
    }

    #[test]
    fn missing_neighbour_mirrors_the_edge_row() {
        let mut section = Section::uniform(0);
        for y in 0..SECTION_SIZE {
            for z in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    let id = u16::try_from(1 + (x + 3 * y + 5 * z) % 1000).expect("an id fits");
                    section.set(x, y, z, id);
                }
            }
        }
        let padded = PaddedBlocks::from_sections(&section, &[None; 26]);

        for y in LOCAL_MIN..=LOCAL_MAX {
            for z in LOCAL_MIN..=LOCAL_MAX {
                for x in LOCAL_MIN..=LOCAL_MAX {
                    assert_eq!(
                        padded.get(x, y, z),
                        direct(&section, &[None; 26], x, y, z),
                        "cell {x},{y},{z}"
                    );
                }
            }
        }
        assert_eq!(padded.get(-1, 5, 7), section.get(0, 5, 7));
        assert_eq!(padded.get(32, 5, 7), section.get(31, 5, 7));
        assert_eq!(padded.get(9, -1, 11), section.get(9, 0, 11));
        assert_eq!(padded.get(9, 32, 11), section.get(9, 31, 11));
        assert_eq!(padded.get(-1, -1, 32), section.get(0, 0, 31));
        assert_eq!(padded.get(32, 32, 32), section.get(31, 31, 31));
    }

    #[test]
    fn neighbour_order_is_unique() {
        assert_eq!(NEIGHBOUR_ORDER.len(), 26);
        for (slot, offset) in NEIGHBOUR_ORDER.iter().enumerate() {
            assert!(
                (-1..=1).contains(&offset.0)
                    && (-1..=1).contains(&offset.1)
                    && (-1..=1).contains(&offset.2),
                "slot {slot} is a unit offset"
            );
            assert_ne!(*offset, (0, 0, 0), "slot {slot} is not the section itself");
            assert_eq!(
                neighbour_slot(offset.0, offset.1, offset.2),
                slot,
                "slot {slot} is {offset:?}"
            );
            for (other_slot, other) in NEIGHBOUR_ORDER.iter().enumerate().skip(slot + 1) {
                assert_ne!(offset, other, "slots {slot} and {other_slot} collide");
            }
        }
        assert_eq!(
            NEIGHBOUR_ORDER,
            [
                (1, 0, 0),
                (-1, 0, 0),
                (0, 1, 0),
                (0, -1, 0),
                (0, 0, 1),
                (0, 0, -1),
                (0, 1, 1),
                (0, 1, -1),
                (0, -1, 1),
                (0, -1, -1),
                (1, 0, 1),
                (1, 0, -1),
                (-1, 0, 1),
                (-1, 0, -1),
                (1, 1, 0),
                (1, -1, 0),
                (-1, 1, 0),
                (-1, -1, 0),
                (1, 1, 1),
                (1, 1, -1),
                (1, -1, 1),
                (1, -1, -1),
                (-1, 1, 1),
                (-1, 1, -1),
                (-1, -1, 1),
                (-1, -1, -1),
            ]
        );
    }

    #[test]
    fn padded_light_defaults_match_the_uniform_rule() {
        let light = PaddedLight::default();
        let mut sky = 0_usize;
        let mut block = 0_usize;
        for y in LOCAL_MIN..=LOCAL_MAX {
            for z in LOCAL_MIN..=LOCAL_MAX {
                for x in LOCAL_MIN..=LOCAL_MAX {
                    let (cell_sky, cell_block) = light.get(x, y, z);
                    sky += usize::from(cell_sky);
                    block += usize::from(cell_block);
                }
            }
        }
        assert_eq!(sky, usize::from(NIBBLE_MAX) * PADDED_VOLUME);
        assert_eq!(block, 0);
        assert_eq!(light.get(0, 0, 0), (NIBBLE_MAX, 0));
        assert_eq!(light.get(-1, -1, -1), (NIBBLE_MAX, 0));
        assert_eq!(light.get(32, 32, 32), (NIBBLE_MAX, 0));
    }
}

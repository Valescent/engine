//! Small scatter: grass, flowers, and ground cover (`H4`).
//!
//! Scatter is the same pipeline as the other features but has no candidate
//! list (`H4.1`). Every column of the section is tested with a keyed hash
//! against a density curve, modulated by a low-frequency patch field so plants
//! clump instead of spreading evenly.
//!
//! The hash is keyed on `(x, z)` only, never `y`, so a column scatters the same
//! way whatever vertical section is being generated.
//!
//! # Render shape
//!
//! Scatter blocks carry a [`RenderShape`] and never collide (`H4.2`). A
//! cross-quad never occludes and never blocks skylight; it emits into the cutout
//! pass and disappears at LOD1. The block registry owns the id-to-shape mapping;
//! this module owns the descriptor set a caller attaches to those ids.

use crate::description::SEA;
use crate::hash::{domain, hash64};

use crate::features::{
    COLUMN_BANK, COLUMN_RIVER, FeatureKind, FeatureSpec, Mask, REGION_SIZE, Sink,
};

/// Edge of one patch cell in blocks (`H4.1`).
pub const PATCH_CELL: i32 = 16;

/// Permille scale used by the density and patch fields.
pub const SCALE: u32 = 1000;

/// Domain of the low-frequency patch field.
const PATCH_DOMAIN: &str = "scatter:patch";

/// Domain of the per-column presence roll.
const ROLL_DOMAIN: &str = "scatter:roll";

/// Bits kept from the patch corner hash: ten, so a corner is `0..=1023`.
const CORNER_BITS: u32 = 54;

/// Scatter density per biome, in permille (`H4.1`).
///
/// Indexed by the v1 biome ids: ocean, beach, plains, forest, desert, alpine.
pub const DENSITY: [u32; 6] = [0, 0, 300, 700, 80, 250];

/// How a scatter block is drawn (`H4.2`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum RenderShape {
    /// A full cube.
    Cube,
    /// Two crossed quads: no occlusion, no skylight blocking.
    CrossQuad,
    /// A thin layer on top of the block below.
    SnowLayer,
}

impl RenderShape {
    /// Whether the shape hides the face behind it.
    #[must_use]
    pub const fn occludes(self) -> bool {
        !matches!(self, Self::CrossQuad)
    }

    /// Whether the shape stops skylight.
    #[must_use]
    pub const fn blocks_skylight(self) -> bool {
        !matches!(self, Self::CrossQuad)
    }
}

/// One registered scatter block (`H4.2`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScatterBlock {
    /// How it draws.
    pub shape: RenderShape,
    /// Scatter never collides.
    pub collides: bool,
    /// Placing a block over it replaces it.
    pub replaceable: bool,
}

/// The v1 scatter descriptors, in the same order as `ApplyContext::scatter`.
pub const V1_SCATTER: [ScatterBlock; 2] = [
    ScatterBlock {
        shape: RenderShape::CrossQuad,
        collides: false,
        replaceable: true,
    },
    ScatterBlock {
        shape: RenderShape::SnowLayer,
        collides: false,
        replaceable: true,
    },
];

/// Scatter density of one biome in permille, or zero outside the table.
#[must_use]
pub fn density(biome: u8) -> u32 {
    DENSITY.get(usize::from(biome)).copied().unwrap_or_default()
}

/// The patch field at one column, in permille (`H4.1`).
///
/// Bilinearly blended from one hashed corner per [`PATCH_CELL`] cell, so
/// neighbouring columns share a value and plants clump. Integer weights only:
/// no float and no `sin`.
#[must_use]
pub fn patch(seed: u64, x: i32, z: i32) -> u32 {
    let cell_x = x.div_euclid(PATCH_CELL);
    let cell_z = z.div_euclid(PATCH_CELL);
    let fraction_x = u32::try_from(x - cell_x * PATCH_CELL).expect("a cell remainder is positive");
    let fraction_z = u32::try_from(z - cell_z * PATCH_CELL).expect("a cell remainder is positive");
    let edge = u32::try_from(PATCH_CELL).expect("the patch cell fits u32");

    let corner = |cx: i32, cz: i32| -> u32 {
        let word = hash64(seed, domain(PATCH_DOMAIN), &[i64::from(cx), i64::from(cz)]);
        u32::try_from(word >> CORNER_BITS).expect("a ten-bit corner fits u32")
    };
    let near = corner(cell_x, cell_z);
    let along_x = corner(cell_x + 1, cell_z);
    let along_z = corner(cell_x, cell_z + 1);
    let far = corner(cell_x + 1, cell_z + 1);

    let blend = near * (edge - fraction_x) * (edge - fraction_z)
        + along_x * fraction_x * (edge - fraction_z)
        + along_z * (edge - fraction_x) * fraction_z
        + far * fraction_x * fraction_z;
    blend / (edge * edge) * SCALE / 1024
}

/// Whether a plant stands in this column (`H4.1`).
///
/// `density` is in permille and comes from the biome; the patch field scales it
/// so the same column is more likely to be a plant inside a clump.
#[must_use]
pub fn present(seed: u64, x: i32, z: i32, density: u32) -> bool {
    let chance = density.min(SCALE) * patch(seed, x, z) / SCALE;
    let roll = hash64(seed, domain(ROLL_DOMAIN), &[i64::from(x), i64::from(z)]) % u64::from(SCALE);
    roll < u64::from(chance)
}

/// Writes scattered plants into one section (`H4.1`).
pub(crate) fn apply(sink: &mut Sink<'_, '_>, specs: &[FeatureSpec]) {
    if !specs.iter().any(|spec| spec.kind == FeatureKind::Scatter) {
        return;
    }
    let ctx = sink.ctx();
    let Some(&id) = ctx.scatter.first() else {
        return;
    };
    let origin = sink.origin();
    let columns = ctx.columns;
    let seed = ctx.seed;

    for z in 0..REGION_SIZE {
        for x in 0..REGION_SIZE {
            let world_x = origin[0] + x;
            let world_z = origin[2] + z;
            let Some(top) = columns.top(world_x, world_z) else {
                continue;
            };
            if top < SEA {
                continue;
            }
            let flags = columns.flags(world_x, world_z).unwrap_or_default();
            if flags & (COLUMN_RIVER | COLUMN_BANK) != 0 {
                continue;
            }
            let biome = columns.biome(world_x, world_z).unwrap_or(u8::MAX);
            if !present(seed, world_x, world_z, density(biome)) {
                continue;
            }
            sink.write([world_x, top + 1, world_z], Mask::AirOnly, id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_quads_are_transparent_in_both_senses() {
        assert!(!RenderShape::CrossQuad.occludes());
        assert!(!RenderShape::CrossQuad.blocks_skylight());
        assert!(RenderShape::Cube.occludes());
        assert!(RenderShape::Cube.blocks_skylight());
        assert!(RenderShape::SnowLayer.occludes());
    }

    #[test]
    fn scatter_never_collides_and_is_replaceable() {
        for block in V1_SCATTER {
            assert!(!block.collides);
            assert!(block.replaceable);
        }
    }

    #[test]
    fn the_patch_field_is_bounded_and_smooth() {
        let mut previous = patch(3, 0, 0);
        for step in 1..256 {
            let value = patch(3, step, 0);
            assert!(value <= SCALE);
            assert!(value.abs_diff(previous) < 100, "the patch jumped at {step}");
            previous = value;
        }
    }

    #[test]
    fn presence_is_stable_for_a_column() {
        for x in -8..8 {
            for z in -8..8 {
                let first = present(5, x, z, 500);
                assert_eq!(first, present(5, x, z, 500));
            }
        }
    }
}

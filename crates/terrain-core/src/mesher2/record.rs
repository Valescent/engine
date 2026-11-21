//! The packed `u64` quad record, its bit layout, and the section ranges.

use super::skirt::SkirtEdges;

/// Face direction code for a `+X` quad.
pub const NORMAL_POS_X: u8 = 0;
/// Face direction code for a `-X` quad.
pub const NORMAL_NEG_X: u8 = 1;
/// Face direction code for a `+Y` quad.
pub const NORMAL_POS_Y: u8 = 2;
/// Face direction code for a `-Y` quad.
pub const NORMAL_NEG_Y: u8 = 3;
/// Face direction code for a `+Z` quad.
pub const NORMAL_POS_Z: u8 = 4;
/// Face direction code for a `-Z` quad.
pub const NORMAL_NEG_Z: u8 = 5;
/// First diagonal of a cutout cross quad.
pub const NORMAL_CROSS_A: u8 = 6;
/// Second diagonal of a cutout cross quad.
pub const NORMAL_CROSS_B: u8 = 7;

/// One packed 8-byte quad record (`M3.1`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Quad(u64);

/// Bit offset of `x`.
const X_SHIFT: u32 = 0;
/// Bit offset of `y`.
const Y_SHIFT: u32 = 5;
/// Bit offset of `z`.
const Z_SHIFT: u32 = 10;
/// Bit offset of `w1`.
const W1_SHIFT: u32 = 15;
/// Bit offset of `h1`.
const H1_SHIFT: u32 = 20;
/// Bit offset of `normal`.
const NORMAL_SHIFT: u32 = 25;
/// Bit offset of `ao`.
const AO_SHIFT: u32 = 28;
/// Bit offset of `sky`.
const SKY_SHIFT: u32 = 36;
/// Bit offset of `blk`.
const BLK_SHIFT: u32 = 40;
/// Bit offset of `tile`.
const TILE_SHIFT: u32 = 44;
/// Bit offset of `tint`.
const TINT_SHIFT: u32 = 60;

/// Mask of a 5-bit field.
const FIVE_BITS: u64 = 0x1F;
/// Mask of a 3-bit field.
const THREE_BITS: u64 = 0x07;
/// Mask of a 4-bit field.
const FOUR_BITS: u64 = 0x0F;
/// Mask of an 8-bit field.
const EIGHT_BITS: u64 = 0xFF;
/// Mask of an 16-bit field.
const SIXTEEN_BITS: u64 = 0xFFFF;

impl Quad {
    /// Packs every field. Values are masked to their declared width.
    #[allow(
        clippy::too_many_arguments,
        reason = "one argument per record field keeps the layout explicit"
    )]
    pub fn encode(
        x: u8,
        y: u8,
        z: u8,
        w1: u8,
        h1: u8,
        normal: u8,
        ao: u8,
        sky: u8,
        blk: u8,
        tile: u16,
        tint: u8,
    ) -> Self {
        Self(
            ((u64::from(x) & FIVE_BITS) << X_SHIFT)
                | ((u64::from(y) & FIVE_BITS) << Y_SHIFT)
                | ((u64::from(z) & FIVE_BITS) << Z_SHIFT)
                | ((u64::from(w1) & FIVE_BITS) << W1_SHIFT)
                | ((u64::from(h1) & FIVE_BITS) << H1_SHIFT)
                | ((u64::from(normal) & THREE_BITS) << NORMAL_SHIFT)
                | ((u64::from(ao) & EIGHT_BITS) << AO_SHIFT)
                | ((u64::from(sky) & FOUR_BITS) << SKY_SHIFT)
                | ((u64::from(blk) & FOUR_BITS) << BLK_SHIFT)
                | ((u64::from(tile) & SIXTEEN_BITS) << TILE_SHIFT)
                | ((u64::from(tint) & FOUR_BITS) << TINT_SHIFT),
        )
    }

    /// The raw record.
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// A record read back from storage.
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// Block origin `x`, `0..=31`.
    pub fn x(self) -> u8 {
        field_5(self.0, X_SHIFT)
    }

    /// Block origin `y`, `0..=31`.
    pub fn y(self) -> u8 {
        field_5(self.0, Y_SHIFT)
    }

    /// Block origin `z`, `0..=31`.
    pub fn z(self) -> u8 {
        field_5(self.0, Z_SHIFT)
    }

    /// Quad width minus one along the face tangent `u`.
    pub fn w1(self) -> u8 {
        field_5(self.0, W1_SHIFT)
    }

    /// Quad height minus one along the face tangent `v`.
    pub fn h1(self) -> u8 {
        field_5(self.0, H1_SHIFT)
    }

    /// Face direction code, or a cross-quad diagonal code.
    pub fn normal(self) -> u8 {
        u8::try_from((self.0 >> NORMAL_SHIFT) & THREE_BITS).expect("a normal fits u8")
    }

    /// Four packed 2-bit AO corner values.
    pub fn ao(self) -> u8 {
        u8::try_from((self.0 >> AO_SHIFT) & EIGHT_BITS).expect("an AO byte fits u8")
    }

    /// Sky nibble of the air-side cell.
    pub fn sky(self) -> u8 {
        u8::try_from((self.0 >> SKY_SHIFT) & FOUR_BITS).expect("a nibble fits u8")
    }

    /// Block-light nibble of the air-side cell.
    pub fn blk(self) -> u8 {
        u8::try_from((self.0 >> BLK_SHIFT) & FOUR_BITS).expect("a nibble fits u8")
    }

    /// Texture-array layer.
    pub fn tile(self) -> u16 {
        u16::try_from((self.0 >> TILE_SHIFT) & SIXTEEN_BITS).expect("a tile fits u16")
    }

    /// Biome tint index.
    pub fn tint(self) -> u8 {
        u8::try_from((self.0 >> TINT_SHIFT) & FOUR_BITS).expect("a tint fits u8")
    }
}

/// Reads a 5-bit field.
fn field_5(bits: u64, shift: u32) -> u8 {
    u8::try_from((bits >> shift) & FIVE_BITS).expect("a 5-bit field fits u8")
}

/// The three output ranges of one section (`M2.1`).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SectionMesh {
    /// Opaque full-cube faces, greedy-merged.
    pub opaque: Vec<Quad>,
    /// Cutout faces: leaves (one record per face) and cross quads.
    pub cutout: Vec<Quad>,
    /// Water/air faces, greedy-merged.
    pub water: Vec<Quad>,
    /// Sides whose skirts are already appended (`N3.2`).
    ///
    /// This is the append guard of [`add_skirts`](crate::mesher2::add_skirts),
    /// not part of the geometry: a freshly built mesh starts at
    /// [`SkirtEdges::NONE`].
    pub skirts: SkirtEdges,
}

/// Total records across the three ranges.
pub fn quad_count(mesh: &SectionMesh) -> usize {
    mesh.opaque.len() + mesh.cutout.len() + mesh.water.len()
}

//! `W6` - the light-aware greedy mesher over packed `u64` quad records.
//!
//! One [`Section`] becomes a [`SectionMesh`]: three independent ranges of
//! [`Quad`] records (`opaque`, `cutout`, `water`) in one per-section buffer
//! model (`M2.1`). Nothing here allocates a GPU buffer, knows a chunk, or
//! streams: the mesher reads one padded scratch copy and returns records.
//!
//! # Record layout (`M3.1`)
//!
//! Every quad is one `u64`, least-significant field first:
//!
//! ```text
//! bits  0.. 5  x       block origin, 0..=31
//! bits  5..10  y
//! bits 10..15  z
//! bits 15..20  w1      width - 1 along the face tangent `u`, so 32 is encodable
//! bits 20..25  h1      height - 1 along the face tangent `v`
//! bits 25..28  normal  0..=5 for the six faces, 6 and 7 for cross quad diagonals
//! bits 28..36  ao      four 2-bit corner values, corner 0 in the low bits
//! bits 36..40  sky     sky nibble of the air-side cell
//! bits 40..44  blk     block nibble of the air-side cell
//! bits 44..60  tile    texture-array layer
//! bits 60..64  tint    biome tint index
//! ```
//!
//! The record is exactly 64 bits, so the encoder masks nothing away when every
//! field is at its maximum.
//!
//! # Merge rules (`M1.1`, `L4.1`)
//!
//! - Opaque faces merge on `(block id, face direction, sky, block, packed AO)`.
//! - Water merges on `(face direction, sky, block)` with no AO (`M2.2`).
//! - LOD merges on `(block id, sky)`: AO and block light are zero at coarse
//!   levels, and the per-face sky approximation breaks a merge (`N3.5`).
//! - Cutout blocks are never greedily merged (`M2.4`). Cross quads emit two
//!   records with normal `6` and `7`.
//!
//! Only opaque full cubes occlude AO and cull a facing quad. Water emits only
//! against air: water/water and water/solid emit nothing (`M2.2`).
//!
//! # Deterministic order (`M1.4`)
//!
//! Ranges are separate and never interleave. Within a range the six face
//! directions appear in the fixed order `+X, -X, +Y, -Y, +Z, -Z`, then records
//! are sorted by `(y, z, x)`; cross quads follow the six face groups, sorted by
//! `(y, z, x, normal)`.
//!
//! # Skirts (`N3.2`)
//!
//! [`add_skirts`] appends a downward wall along selected section edges, into the
//! opaque range, so a coarse section's lower-resolution boundary hides the gap
//! under it instead of stitching to its finer neighbours. [`add_step_skirts`]
//! is the drop-aware variant: it reads the coarse padded scratch and fills only
//! the vertical step to the adjacent column, so a flush edge appends nothing.
//! Skirt records copy the boundary face's `tile` and `tint`, take its air-side
//! `sky`/`blk`, and force fully open AO. [`SectionMesh::skirts`] records the
//! sides already done, so a skirt set is never appended twice.
//!
//! # Padded inputs (`I2.3`, `I2.4`)
//!
//! [`PaddedBlocks`] and [`PaddedLight`] come from [`crate::padded`], the one
//! engine-wide `34³` scratch. The mesher reads the section plus all 26
//! neighbours, so corner AO at a section corner sees the diagonal sections
//! instead of defaulting them to air. [`mesh_from_neighbours`] builds that
//! scratch from the 26-section ownership array; [`mesh_from_face_neighbours`]
//! keeps the old six-face call shape and pads the edge and corner slots with
//! air.

use crate::description::SECTION_SIZE;
use crate::lod::SKIRT_DEPTH;
use crate::padded::{LOCAL_MAX, LOCAL_MIN, LOCAL_SIZE, NIBBLE_MAX, PaddedBlocks, PaddedLight};
use crate::revisions::Face as ColumnFace;
use crate::section::{BlockId, Section};

/// Cells in one section face plane.
const PLANE_WIDTH: usize = SECTION_SIZE as usize;

/// Cells in one section face plane.
const PLANE_CELLS: usize = PLANE_WIDTH * PLANE_WIDTH;

/// Sky-light loss per block of depth below the first opaque block (`N3.5`).
pub const SHADE_PER_CELL: u8 = 1;

/// Extra sky-light loss per water block above a coarse face (`L2.4`, `N3.5`).
pub const WATER_SKY_ATTEN: u8 = 1;

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

/// Material properties the mesher needs for one block id (`M1.1`, `M2.4`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "the material manifest is a set of independent flags"
)]
pub struct Material {
    /// Occupies a cell: water and air are not solid.
    pub solid: bool,
    /// A full cube that culls faces, occludes AO, and stops sky light.
    pub opaque: bool,
    /// A water block.
    pub water: bool,
    /// A scatter block drawn as two crossed quads.
    pub cross_quad: bool,
    /// Texture-array layer.
    pub texture_tile: u16,
    /// Biome tint index.
    pub tint: u8,
    /// Emits block light. Not encoded in the quad record; carried for `light2`.
    pub emissive: bool,
}

impl Material {
    /// The reserved air material.
    pub const AIR: Self = Self {
        solid: false,
        opaque: false,
        water: false,
        cross_quad: false,
        texture_tile: 0,
        tint: 0,
        emissive: false,
    };

    /// Fallback for an id outside the table: visible stone-like geometry.
    pub const UNKNOWN: Self = Self {
        solid: true,
        opaque: true,
        water: false,
        cross_quad: false,
        texture_tile: 0,
        tint: 0,
        emissive: false,
    };
}

/// Per-block material table, resolved by the caller before meshing.
///
/// Fields are parallel arrays indexed by [`BlockId`], which keeps the table
/// flat and easy to swap for the content manifest later. Ids beyond the table
/// fall back to [`Material::UNKNOWN`].
#[derive(Clone, Debug, Default)]
pub struct MaterialProps {
    /// Solid flag per id.
    solid: Vec<bool>,
    /// Opaque flag per id.
    opaque: Vec<bool>,
    /// Water flag per id.
    water: Vec<bool>,
    /// Cross-quad flag per id.
    cross_quad: Vec<bool>,
    /// Texture layer per id.
    texture_tile: Vec<u16>,
    /// Tint index per id.
    tint: Vec<u8>,
    /// Emissive flag per id.
    emissive: Vec<bool>,
}

impl MaterialProps {
    /// An empty table. Every id falls back to [`Material::UNKNOWN`].
    pub fn new() -> Self {
        Self::default()
    }

    /// The default table for the ids in `block.rs`: air, stone, dirt, grass,
    /// wood, leaves, lamp, water, sand.
    ///
    /// No other ids are installed. Grass is a cross-quad cutout; leaves are a
    /// cutout cube that does not occlude. Water is non-solid and non-opaque.
    #[allow(
        clippy::too_many_lines,
        reason = "one explicit row per block id matches block.rs and is easier to audit"
    )]
    pub fn default_table() -> Self {
        let entries: [(BlockId, Material); 9] = [
            (0, Material::AIR),
            (
                1,
                Material {
                    solid: true,
                    opaque: true,
                    water: false,
                    cross_quad: false,
                    texture_tile: 0,
                    tint: 0,
                    emissive: false,
                },
            ),
            (
                2,
                Material {
                    solid: true,
                    opaque: true,
                    water: false,
                    cross_quad: false,
                    texture_tile: 1,
                    tint: 0,
                    emissive: false,
                },
            ),
            (
                3,
                Material {
                    solid: true,
                    opaque: false,
                    water: false,
                    cross_quad: true,
                    texture_tile: 3,
                    tint: 1,
                    emissive: false,
                },
            ),
            (
                4,
                Material {
                    solid: true,
                    opaque: true,
                    water: false,
                    cross_quad: false,
                    texture_tile: 6,
                    tint: 0,
                    emissive: false,
                },
            ),
            (
                5,
                Material {
                    solid: true,
                    opaque: false,
                    water: false,
                    cross_quad: false,
                    texture_tile: 7,
                    tint: 1,
                    emissive: false,
                },
            ),
            (
                6,
                Material {
                    solid: true,
                    opaque: true,
                    water: false,
                    cross_quad: false,
                    texture_tile: 8,
                    tint: 0,
                    emissive: true,
                },
            ),
            (
                7,
                Material {
                    solid: false,
                    opaque: false,
                    water: true,
                    cross_quad: false,
                    texture_tile: 9,
                    tint: 0,
                    emissive: false,
                },
            ),
            (
                8,
                Material {
                    solid: true,
                    opaque: true,
                    water: false,
                    cross_quad: false,
                    texture_tile: 10,
                    tint: 0,
                    emissive: false,
                },
            ),
        ];
        let mut props = Self::new();
        for (id, material) in entries {
            props.set(BlockId::from(id), material);
        }
        props
    }

    /// Installs one material, growing the table if the id is new.
    pub fn set(&mut self, id: BlockId, material: Material) {
        let slot = usize::from(id);
        if slot >= self.solid.len() {
            let length = slot + 1;
            self.solid.resize(length, false);
            self.opaque.resize(length, false);
            self.water.resize(length, false);
            self.cross_quad.resize(length, false);
            self.texture_tile.resize(length, 0);
            self.tint.resize(length, 0);
            self.emissive.resize(length, false);
        }
        self.solid[slot] = material.solid;
        self.opaque[slot] = material.opaque;
        self.water[slot] = material.water;
        self.cross_quad[slot] = material.cross_quad;
        self.texture_tile[slot] = material.texture_tile;
        self.tint[slot] = material.tint;
        self.emissive[slot] = material.emissive;
    }

    /// Resolves one block id.
    pub fn material(&self, id: BlockId) -> Material {
        let slot = usize::from(id);
        if slot >= self.solid.len() {
            return Material::UNKNOWN;
        }
        Material {
            solid: self.solid[slot],
            opaque: self.opaque[slot],
            water: self.water[slot],
            cross_quad: self.cross_quad[slot],
            texture_tile: self.texture_tile[slot],
            tint: self.tint[slot],
            emissive: self.emissive[slot],
        }
    }
}

/// Mesh detail level (`N1.1`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum MeshLod {
    /// One block per cell: full AO and light.
    Lod0,
    /// Two blocks per cell.
    Lod1,
    /// Four blocks per cell.
    Lod2,
}

impl MeshLod {
    /// World blocks covered by one coarse cell.
    pub const fn cells_per_block(self) -> u32 {
        match self {
            Self::Lod0 => 1,
            Self::Lod1 => 2,
            Self::Lod2 => 4,
        }
    }
}

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
    /// This is the append guard of [`add_skirts`], not part of the geometry: a
    /// freshly built mesh starts at [`SkirtEdges::NONE`].
    pub skirts: SkirtEdges,
}

/// Total records across the three ranges.
pub fn quad_count(mesh: &SectionMesh) -> usize {
    mesh.opaque.len() + mesh.cutout.len() + mesh.water.len()
}

/// A face direction and its two in-plane tangents (`u`, `v`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Face {
    /// `+X`, tangent `u = z`, `v = y`.
    PosX,
    /// `-X`, tangent `u = z`, `v = y`.
    NegX,
    /// `+Y`, tangent `u = x`, `v = z`.
    PosY,
    /// `-Y`, tangent `u = x`, `v = z`.
    NegY,
    /// `+Z`, tangent `u = x`, `v = y`.
    PosZ,
    /// `-Z`, tangent `u = x`, `v = y`.
    NegZ,
}

/// The fixed face order of `M1.4`.
const FACES: [Face; 6] = [
    Face::PosX,
    Face::NegX,
    Face::PosY,
    Face::NegY,
    Face::PosZ,
    Face::NegZ,
];

impl Face {
    /// The record's normal code.
    const fn code(self) -> u8 {
        match self {
            Self::PosX => NORMAL_POS_X,
            Self::NegX => NORMAL_NEG_X,
            Self::PosY => NORMAL_POS_Y,
            Self::NegY => NORMAL_NEG_Y,
            Self::PosZ => NORMAL_POS_Z,
            Self::NegZ => NORMAL_NEG_Z,
        }
    }

    /// Outward normal as a unit step.
    const fn normal(self) -> [i32; 3] {
        match self {
            Self::PosX => [1, 0, 0],
            Self::NegX => [-1, 0, 0],
            Self::PosY => [0, 1, 0],
            Self::NegY => [0, -1, 0],
            Self::PosZ => [0, 0, 1],
            Self::NegZ => [0, 0, -1],
        }
    }

    /// `(normal axis, u axis, v axis)`.
    const fn axes(self) -> (usize, usize, usize) {
        match self {
            Self::PosX | Self::NegX => (0, 2, 1),
            Self::PosY | Self::NegY => (1, 0, 2),
            Self::PosZ | Self::NegZ => (2, 0, 1),
        }
    }
}

/// The merge key of one cell face (`M1.1`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FaceKey {
    /// Block id, so unlike blocks never merge.
    block: BlockId,
    /// Sky nibble of the air-side cell.
    sky: u8,
    /// Block nibble of the air-side cell.
    blk: u8,
    /// Packed four-corner AO. Always `0` for water and coarse levels.
    ao: u8,
}

impl FaceKey {
    /// A water key: no AO (`M2.2`).
    const fn water(block: BlockId, sky: u8, blk: u8) -> Self {
        Self {
            block,
            sky,
            blk,
            ao: 0,
        }
    }
}

/// The local cell on one face plane.
fn cell_at(face: Face, slice: i32, u: i32, v: i32) -> [i32; 3] {
    let (normal_axis, u_axis, v_axis) = face.axes();
    let mut cell = [0; 3];
    cell[normal_axis] = slice;
    cell[u_axis] = u;
    cell[v_axis] = v;
    cell
}

/// Index of an in-plane coordinate in a 32x32 mask.
fn plane_index(u: i32, v: i32) -> usize {
    let u = usize::try_from(u).expect("a plane coordinate fits usize");
    let v = usize::try_from(v).expect("a plane coordinate fits usize");
    u + PLANE_WIDTH * v
}

/// Narrows a signed local coordinate into a record field.
fn signed(value: i32) -> u8 {
    u8::try_from(value).expect("a local coordinate fits 5 bits")
}

/// The source of per-face light.
#[derive(Clone, Copy)]
enum LightSource<'a> {
    /// Full nibbles from `light2`.
    Nibbles(&'a PaddedLight),
    /// Per-face sky approximation, with no block light (`N3.5`).
    Coarse(u32),
}

/// Greedy-meshes one section into packed records (`W6`).
///
/// `Lod0` reads AO and light nibbles; `Lod1` and `Lod2` skip AO and block light
/// and use the coarse sky rule of [`mesh_coarse`]. The `light` argument is
/// ignored above `Lod0`.
pub fn mesh(
    section: &Section,
    blocks: &PaddedBlocks,
    light: &PaddedLight,
    props: &MaterialProps,
    lod: MeshLod,
) -> SectionMesh {
    if section.is_empty(0) {
        return SectionMesh::default();
    }
    let source = match lod {
        MeshLod::Lod0 => LightSource::Nibbles(light),
        MeshLod::Lod1 | MeshLod::Lod2 => LightSource::Coarse(lod.cells_per_block()),
    };
    let mut out = SectionMesh::default();
    collect_all(blocks, source, props, &mut out);
    out
}

/// Meshes one section from its full 26-neighbour ownership array (`I2.4`).
///
/// This is the constructor that sees diagonal sections: `neighbours` is indexed
/// by [`crate::padded::NEIGHBOUR_ORDER`] (six faces, twelve edges, eight
/// corners), and a corner AO sample that falls in a diagonal section reads that
/// section's real block instead of defaulting to air. `None` entries keep the
/// [`PaddedBlocks::from_sections`] world-edge mirror rule.
///
/// The six face neighbours are the first six entries; see
/// [`mesh_from_face_neighbours`] for the older six-neighbour call shape.
pub fn mesh_from_neighbours(
    section: &Section,
    neighbours: &[Option<&Section>; 26],
    light: &PaddedLight,
    props: &MaterialProps,
    lod: MeshLod,
) -> SectionMesh {
    let blocks = PaddedBlocks::from_sections(section, neighbours);
    mesh(section, &blocks, light, props, lod)
}

/// Meshes one section from the legacy six face neighbours only.
///
/// `neighbours` uses the first six entries of
/// [`crate::padded::NEIGHBOUR_ORDER`]: `+X, -X, +Y, -Y, +Z, -Z`. The twenty edge
/// and corner neighbours are filled with explicit air, so corner AO that would
/// look diagonally stays fully open. For any section whose AO samples never
/// leave the six faces this is byte-identical to [`mesh_from_neighbours`] with
/// air diagonals, and to the pre-26-neighbour behaviour. `None` face entries
/// keep the world-edge mirror rule.
pub fn mesh_from_face_neighbours(
    section: &Section,
    neighbours: &[Option<&Section>; 6],
    light: &PaddedLight,
    props: &MaterialProps,
    lod: MeshLod,
) -> SectionMesh {
    let air = Section::uniform(0);
    let mut owners: [Option<&Section>; 26] = [Some(&air); 26];
    owners[..6].copy_from_slice(neighbours);
    mesh_from_neighbours(section, &owners, light, props, lod)
}

/// Coarse mesh with no stored light (`N3.5`).
///
/// AO and block light are zero. Sky per face is `15` when the air-side cell is
/// open to the ceiling; otherwise `15 - SHADE_PER_CELL * depth` where `depth` is
/// the distance in world blocks to the first opaque block above, then minus
/// `WATER_SKY_ATTEN` per water block above, clamped at zero. `cells_per_block`
/// scales a coarse cell to world blocks; `0` means "use `lod`'s default".
pub fn mesh_coarse(
    blocks: &PaddedBlocks,
    props: &MaterialProps,
    lod: MeshLod,
    cells_per_block: u32,
) -> SectionMesh {
    let scale = if cells_per_block == 0 {
        lod.cells_per_block()
    } else {
        cells_per_block
    };
    let mut out = SectionMesh::default();
    collect_all(blocks, LightSource::Coarse(scale), props, &mut out);
    out
}

/// Collects all three ranges in the fixed face order.
fn collect_all(
    blocks: &PaddedBlocks,
    source: LightSource<'_>,
    props: &MaterialProps,
    out: &mut SectionMesh,
) {
    for face in FACES {
        let mut opaque = Vec::new();
        let mut cutout = Vec::new();
        let mut water = Vec::new();
        collect_face(
            face,
            blocks,
            source,
            props,
            &mut opaque,
            &mut cutout,
            &mut water,
        );
        sort_quads(&mut opaque);
        sort_quads(&mut cutout);
        sort_quads(&mut water);
        out.opaque.append(&mut opaque);
        out.cutout.append(&mut cutout);
        out.water.append(&mut water);
    }
    let mut cross = Vec::new();
    collect_cross(blocks, source, props, &mut cross);
    sort_quads(&mut cross);
    out.cutout.append(&mut cross);
}

/// Collects the merged faces of one direction.
fn collect_face(
    face: Face,
    blocks: &PaddedBlocks,
    source: LightSource<'_>,
    props: &MaterialProps,
    out_opaque: &mut Vec<Quad>,
    out_cutout: &mut Vec<Quad>,
    out_water: &mut Vec<Quad>,
) {
    let normal = face.normal();
    let mut opaque_mask = [None; PLANE_CELLS];
    let mut water_mask = [None; PLANE_CELLS];
    for slice in 0..LOCAL_SIZE {
        for v in 0..LOCAL_SIZE {
            for u in 0..LOCAL_SIZE {
                let cell = cell_at(face, slice, u, v);
                let id = blocks.get(cell[0], cell[1], cell[2]);
                if id == 0 {
                    continue;
                }
                let material = props.material(id);
                if material.cross_quad {
                    continue;
                }
                let neighbour = [
                    cell[0] + normal[0],
                    cell[1] + normal[1],
                    cell[2] + normal[2],
                ];
                let neighbour_material =
                    props.material(blocks.get(neighbour[0], neighbour[1], neighbour[2]));
                let plane = plane_index(u, v);
                if material.water {
                    if is_air(neighbour_material) {
                        let (sky, blk) = face_light(source, blocks, props, neighbour);
                        water_mask[plane] = Some(FaceKey::water(id, sky, blk));
                    }
                    continue;
                }
                if neighbour_material.opaque {
                    continue;
                }
                let (sky, blk) = face_light(source, blocks, props, neighbour);
                let ao = match source {
                    LightSource::Nibbles(_) => face_ao(blocks, props, cell, face),
                    LightSource::Coarse(_) => 0,
                };
                let key = FaceKey {
                    block: id,
                    sky,
                    blk,
                    ao,
                };
                if material.opaque {
                    opaque_mask[plane] = Some(key);
                } else {
                    out_cutout.push(quad_from(face, cell, 1, 1, key, props));
                }
            }
        }
        greedy_emit(face, slice, &mut opaque_mask, props, out_opaque);
        greedy_emit(face, slice, &mut water_mask, props, out_water);
    }
}

/// Collects the two crossed quads of every scatter cell (`M2.4`).
fn collect_cross(
    blocks: &PaddedBlocks,
    source: LightSource<'_>,
    props: &MaterialProps,
    out: &mut Vec<Quad>,
) {
    for y in 0..LOCAL_SIZE {
        for z in 0..LOCAL_SIZE {
            for x in 0..LOCAL_SIZE {
                let id = blocks.get(x, y, z);
                if id == 0 {
                    continue;
                }
                let material = props.material(id);
                if !material.cross_quad {
                    continue;
                }
                let (sky, blk) = face_light(source, blocks, props, [x, y, z]);
                for normal in [NORMAL_CROSS_A, NORMAL_CROSS_B] {
                    out.push(Quad::encode(
                        signed(x),
                        signed(y),
                        signed(z),
                        0,
                        0,
                        normal,
                        0,
                        sky,
                        blk,
                        material.texture_tile,
                        material.tint,
                    ));
                }
            }
        }
    }
}

/// Greedily merges one mask plane into records.
fn greedy_emit(
    face: Face,
    slice: i32,
    mask: &mut [Option<FaceKey>; PLANE_CELLS],
    props: &MaterialProps,
    out: &mut Vec<Quad>,
) {
    let mut v = 0;
    while v < LOCAL_SIZE {
        let mut u = 0;
        while u < LOCAL_SIZE {
            let Some(key) = mask[plane_index(u, v)] else {
                u += 1;
                continue;
            };
            let mut width = 1;
            while u + width < LOCAL_SIZE && mask[plane_index(u + width, v)] == Some(key) {
                width += 1;
            }
            let mut height = 1;
            'grow: while v + height < LOCAL_SIZE {
                for offset in 0..width {
                    if mask[plane_index(u + offset, v + height)] != Some(key) {
                        break 'grow;
                    }
                }
                height += 1;
            }
            for row in 0..height {
                for column in 0..width {
                    mask[plane_index(u + column, v + row)] = None;
                }
            }
            let cell = cell_at(face, slice, u, v);
            out.push(quad_from(face, cell, width, height, key, props));
            u += width;
        }
        v += 1;
    }
}

/// Builds one record from a merged rectangle.
fn quad_from(
    face: Face,
    cell: [i32; 3],
    width: i32,
    height: i32,
    key: FaceKey,
    props: &MaterialProps,
) -> Quad {
    let material = props.material(key.block);
    Quad::encode(
        signed(cell[0]),
        signed(cell[1]),
        signed(cell[2]),
        signed(width - 1),
        signed(height - 1),
        face.code(),
        key.ao,
        key.sky,
        key.blk,
        material.texture_tile,
        material.tint,
    )
}

/// Sorts one face group into `(y, z, x)` order, with stable tie-breakers.
fn sort_quads(quads: &mut [Quad]) {
    quads.sort_by_key(|quad| {
        (
            quad.y(),
            quad.z(),
            quad.x(),
            quad.normal(),
            quad.w1(),
            quad.h1(),
        )
    });
}

/// Whether a material is an air cell for water-face purposes.
fn is_air(material: Material) -> bool {
    !material.solid && !material.water
}

/// Reads light for the air-side cell.
fn face_light(
    source: LightSource<'_>,
    blocks: &PaddedBlocks,
    props: &MaterialProps,
    at: [i32; 3],
) -> (u8, u8) {
    match source {
        LightSource::Nibbles(light) => light.get(at[0], at[1], at[2]),
        LightSource::Coarse(scale) => (coarse_sky(blocks, props, at, scale), 0),
    }
}

/// Coarse per-face sky from the open-ceiling rule (`N3.5`).
fn coarse_sky(
    blocks: &PaddedBlocks,
    props: &MaterialProps,
    at: [i32; 3],
    cells_per_block: u32,
) -> u8 {
    let mut water = 0_u32;
    let mut opaque_depth = None;
    let mut y = at[1] + 1;
    while y <= LOCAL_MAX {
        let material = props.material(blocks.get(at[0], y, at[2]));
        if material.opaque {
            opaque_depth = Some(y - at[1]);
            break;
        }
        if material.water {
            water += 1;
        }
        y += 1;
    }
    let depth = opaque_depth.map_or(0, |depth| {
        let depth = u32::try_from(depth).expect("a depth fits u32");
        depth
            .saturating_mul(cells_per_block)
            .saturating_mul(u32::from(SHADE_PER_CELL))
    });
    let water = water
        .saturating_mul(cells_per_block)
        .saturating_mul(u32::from(WATER_SKY_ATTEN));
    let sky = u32::from(NIBBLE_MAX)
        .saturating_sub(depth)
        .saturating_sub(water);
    u8::try_from(sky).expect("a clamped nibble fits u8")
}

/// Four-corner AO for one face (`L4.1`).
///
/// Corner `0` is `(-u, -v)`, then `(+u, -v)`, `(+u, +v)`, `(-u, +v)`, packed two
/// bits each. Only opaque full cubes occlude.
fn face_ao(blocks: &PaddedBlocks, props: &MaterialProps, cell: [i32; 3], face: Face) -> u8 {
    let normal = face.normal();
    let (_, u_axis, v_axis) = face.axes();
    let mut packed = 0_u8;
    for (corner, (du, dv)) in [(-1, -1), (1, -1), (1, 1), (-1, 1)].into_iter().enumerate() {
        let mut base = [
            cell[0] + normal[0],
            cell[1] + normal[1],
            cell[2] + normal[2],
        ];
        let mut side_u = base;
        let mut side_v = base;
        side_u[u_axis] += du;
        side_v[v_axis] += dv;
        base[u_axis] += du;
        base[v_axis] += dv;
        let occludes_u = occludes(blocks, props, side_u);
        let occludes_v = occludes(blocks, props, side_v);
        let occludes_corner = occludes(blocks, props, base);
        let value = if occludes_u && occludes_v {
            0
        } else {
            3_u8.saturating_sub(u8::from(occludes_u))
                .saturating_sub(u8::from(occludes_v))
                .saturating_sub(u8::from(occludes_corner))
        };
        packed |= value << (2 * corner);
    }
    packed
}

/// Whether a cell holds an opaque full cube.
fn occludes(blocks: &PaddedBlocks, props: &MaterialProps, cell: [i32; 3]) -> bool {
    props.material(blocks.get(cell[0], cell[1], cell[2])).opaque
}

// ---------------------------------------------------------------------------
// Skirts (`N3.2`)
// ---------------------------------------------------------------------------

/// Fully open AO: every corner at `3`, so a skirt never darkens a seam.
const AO_OPEN: u8 = 0xFF;

/// One horizontal side of a section, in [`Face`](crate::revisions::Face) order.
///
/// The compass meaning follows the hydro flow graph: north is `z - 1`, east is
/// `x + 1`, south is `z + 1`, west is `x - 1`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Side {
    /// The `z = 0` edge, facing `-Z`.
    North,
    /// The `z = 31` edge, facing `+Z`.
    South,
    /// The `x = 31` edge, facing `+X`.
    East,
    /// The `x = 0` edge, facing `-X`.
    West,
}

impl Side {
    /// Every side, in the fixed order skirts are appended.
    pub const ALL: [Self; crate::revisions::FACE_COUNT] =
        [Self::North, Self::South, Self::East, Self::West];

    /// The set bit of this side in [`SkirtEdges`].
    const fn bit(self) -> u8 {
        match self {
            Self::North => 0b0001,
            Self::South => 0b0010,
            Self::East => 0b0100,
            Self::West => 0b1000,
        }
    }

    /// The outward normal code of this side's wall.
    const fn normal(self) -> u8 {
        match self {
            Self::North => NORMAL_NEG_Z,
            Self::South => NORMAL_POS_Z,
            Self::East => NORMAL_POS_X,
            Self::West => NORMAL_NEG_X,
        }
    }

    /// The cell plane inside the section that the wall lies on.
    fn plane(self) -> u8 {
        match self {
            Self::North | Self::West => 0,
            Self::South | Self::East => {
                u8::try_from(SECTION_SIZE - 1).expect("a section is 32 cells wide")
            }
        }
    }
}

impl From<ColumnFace> for Side {
    fn from(face: ColumnFace) -> Self {
        match face {
            ColumnFace::North => Self::North,
            ColumnFace::South => Self::South,
            ColumnFace::East => Self::East,
            ColumnFace::West => Self::West,
        }
    }
}

/// Which sides of a section carry a downward skirt (`N3.2`).
///
/// The bits follow [`Side`] order, so a mask is also a four-slot side set.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct SkirtEdges(u8);

impl SkirtEdges {
    /// No side: no skirts.
    pub const NONE: Self = Self(0);
    /// The north edge only.
    pub const NORTH: Self = Self(0b0001);
    /// The south edge only.
    pub const SOUTH: Self = Self(0b0010);
    /// The east edge only.
    pub const EAST: Self = Self(0b0100);
    /// The west edge only.
    pub const WEST: Self = Self(0b1000);
    /// Every edge.
    pub const ALL: Self = Self(0b1111);

    /// Whether the north edge is selected.
    pub const fn north(self) -> bool {
        self.0 & 0b0001 != 0
    }

    /// Whether the south edge is selected.
    pub const fn south(self) -> bool {
        self.0 & 0b0010 != 0
    }

    /// Whether the east edge is selected.
    pub const fn east(self) -> bool {
        self.0 & 0b0100 != 0
    }

    /// Whether the west edge is selected.
    pub const fn west(self) -> bool {
        self.0 & 0b1000 != 0
    }

    /// Adds one side.
    #[must_use]
    pub const fn with(self, side: Side) -> Self {
        Self(self.0 | side.bit())
    }

    /// Removes one side.
    #[must_use]
    pub const fn without(self, side: Side) -> Self {
        Self(self.0 & !side.bit())
    }

    /// Whether `side` is selected.
    pub const fn contains(self, side: Side) -> bool {
        self.0 & side.bit() != 0
    }

    /// Whether no side is selected.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// World-block height of a skirt at `lod`: [`SKIRT_DEPTH`] cells.
pub const fn skirt_blocks(lod: MeshLod) -> u32 {
    SKIRT_DEPTH.unsigned_abs() * lod.cells_per_block()
}

/// One boundary surface cell: its height and the fields a skirt copies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SkirtCell {
    /// Block origin `y` of the boundary top face.
    y: u8,
    /// Sky nibble of the face's air-side cell.
    sky: u8,
    /// Block-light nibble of the face's air-side cell.
    blk: u8,
    /// Texture layer, copied from the boundary face.
    tile: u16,
    /// Biome tint, copied from the boundary face.
    tint: u8,
}

/// Appends the downward skirts of `edges` to the opaque range (`N3.2`).
///
/// A coarse section draws only where its covered fine sections are absent, so
/// its lower-resolution edges can sit over a hole. A skirt continues the exposed
/// boundary wall downward by [`SKIRT_DEPTH`] cells, which is [`skirt_blocks`]
/// world blocks, and hides the gap without stitching.
///
/// For each selected side the lowest exposed top face of every boundary column
/// supplies one strip: the `+Y` quads whose rectangle touches that side's plane.
/// Columns with an equal face merge into one run, and a run emits one quad per
/// cell layer. Sides come in the fixed order `North, South, East, West`, then
/// runs ascend in `z`/`x`. Records go to the opaque range, copy the boundary
/// face's `tile` and `tint`, take its air-side `sky`/`blk`, and always use fully
/// open AO.
///
/// A boundary face within [`SKIRT_DEPTH`] cells of the section floor gets fewer
/// layers, because world geometry below `y = 0` belongs to the section below.
/// An empty mesh adds nothing.
///
/// This function is additive and deterministic, and a skirt set is never
/// appended twice: [`SectionMesh::skirts`] remembers the sides already done, so
/// a repeated call appends nothing and returns `0`. The marker belongs to one
/// mesh, so a rebuilt section needs a fresh [`SectionMesh`].
///
/// The return value is the number of quads added.
pub fn add_skirts(mesh: &mut SectionMesh, edges: SkirtEdges, lod: MeshLod) -> u32 {
    if edges.is_empty() || mesh.opaque.is_empty() {
        return 0;
    }
    let depth = SKIRT_DEPTH.unsigned_abs();
    debug_assert_eq!(
        skirt_blocks(lod),
        depth * lod.cells_per_block(),
        "a skirt spans SKIRT_DEPTH cells, so its block height scales with the level"
    );
    let mut strip = Vec::new();
    for side in Side::ALL {
        if edges.contains(side) && !mesh.skirts.contains(side) {
            push_side_skirts(&mut strip, mesh, side, depth);
        }
    }
    let added = u32::try_from(strip.len()).expect("a skirt batch fits u32");
    for side in Side::ALL {
        if edges.contains(side) {
            mesh.skirts = mesh.skirts.with(side);
        }
    }
    mesh.opaque.append(&mut strip);
    added
}

/// Appends only the skirt cells needed to reach a lower adjacent column.
///
/// This is the drop-aware sibling of [`add_skirts`]. [`add_skirts`] always
/// continues every exposed boundary top face downward; this variant reads the
/// coarse padded scratch and fills only the vertical step to the column just
/// outside `edges`:
///
/// - a flush edge (the adjacent coarse column reaches or passes the boundary
///   top) appends nothing;
/// - a one-cell drop appends one cell;
/// - a larger drop or an open neighbour appends at most [`SKIRT_DEPTH`] cells.
///
/// Cells below the section floor are never emitted. Records, edge order, the
/// per-edge normal, fully open AO (`0xFF`, so a skirt never darkens), `sky`/`blk`
/// from the boundary face, and [`SectionMesh::skirts`] all match [`add_skirts`];
/// a repeated call returns `0`. The return value is the number of quads added.
pub fn add_step_skirts(
    mesh: &mut SectionMesh,
    blocks: &PaddedBlocks,
    props: &MaterialProps,
    edges: SkirtEdges,
    lod: MeshLod,
) -> u32 {
    if edges.is_empty() || mesh.opaque.is_empty() {
        return 0;
    }
    let depth = SKIRT_DEPTH.unsigned_abs();
    debug_assert_eq!(
        skirt_blocks(lod),
        depth * lod.cells_per_block(),
        "a skirt spans SKIRT_DEPTH cells, so its block height scales with the level"
    );
    let mut strip = Vec::new();
    for side in Side::ALL {
        if edges.contains(side) && !mesh.skirts.contains(side) {
            push_side_step_skirts(&mut strip, mesh, blocks, props, side, depth);
        }
    }
    let added = u32::try_from(strip.len()).expect("a skirt batch fits u32");
    for side in Side::ALL {
        if edges.contains(side) {
            mesh.skirts = mesh.skirts.with(side);
        }
    }
    mesh.opaque.append(&mut strip);
    added
}

/// Appends one side's merged drop-aware runs, top cell first.
fn push_side_step_skirts(
    out: &mut Vec<Quad>,
    mesh: &SectionMesh,
    blocks: &PaddedBlocks,
    props: &MaterialProps,
    side: Side,
    depth: u32,
) {
    let cells = boundary_cells(mesh, side);
    let mut column = 0;
    while column < PLANE_WIDTH {
        let Some(cell) = cells[column] else {
            column += 1;
            continue;
        };
        let layers = step_layers(blocks, props, side, column, cell.y, depth);
        if layers == 0 {
            column += 1;
            continue;
        }
        let mut run = 1;
        while column + run < PLANE_WIDTH {
            let Some(next) = cells[column + run] else {
                break;
            };
            let next_layers = step_layers(blocks, props, side, column + run, next.y, depth);
            if next != cell || next_layers != layers {
                break;
            }
            run += 1;
        }
        for layer in 1..=layers {
            let y = cell.y - u8::try_from(layer).expect("a layer fits one cell index");
            push_skirt(out, side, column, run, y, cell);
        }
        column += run;
    }
}

/// Cells needed to bridge a boundary column down to its adjacent coarse column.
fn step_layers(
    blocks: &PaddedBlocks,
    props: &MaterialProps,
    side: Side,
    column: usize,
    top: u8,
    depth: u32,
) -> u32 {
    let drop = match neighbour_top(blocks, props, side, column) {
        Some(adjacent) if adjacent >= top => 0,
        Some(adjacent) => u32::from(top).saturating_sub(u32::from(adjacent)),
        None => u32::from(top).saturating_add(1),
    };
    // Never go below the section floor; `top` is at most 31.
    drop.min(depth).min(u32::from(top))
}

/// Highest opaque cell in the coarse column just outside `side`.
fn neighbour_top(
    blocks: &PaddedBlocks,
    props: &MaterialProps,
    side: Side,
    column: usize,
) -> Option<u8> {
    let column = i32::try_from(column).expect("a column fits i32");
    let (x, z) = match side {
        Side::North => (column, LOCAL_MIN),
        Side::South => (column, LOCAL_MAX),
        Side::East => (LOCAL_MAX, column),
        Side::West => (LOCAL_MIN, column),
    };
    (LOCAL_MIN..=LOCAL_MAX)
        .rev()
        .find(|&y| props.material(blocks.get(x, y, z)).opaque)
        .and_then(|y| u8::try_from(y).ok())
}

/// Appends one side's merged runs, top cell first.
fn push_side_skirts(out: &mut Vec<Quad>, mesh: &SectionMesh, side: Side, depth: u32) {
    let cells = boundary_cells(mesh, side);
    let mut column = 0;
    while column < PLANE_WIDTH {
        let Some(cell) = cells[column] else {
            column += 1;
            continue;
        };
        let mut run = 1;
        while column + run < PLANE_WIDTH && cells[column + run] == Some(cell) {
            run += 1;
        }
        let layers = u32::from(cell.y).min(depth);
        // Start one cell below the boundary face: its own side wall already
        // covers `cell.y`, and a coplanar duplicate would z-fight.
        for layer in 1..=layers {
            let y = cell.y - u8::try_from(layer).expect("a layer fits one cell index");
            push_skirt(out, side, column, run, y, cell);
        }
        column += run;
    }
}

/// Appends one merged skirt quad of one cell layer.
fn push_skirt(out: &mut Vec<Quad>, side: Side, column: usize, run: usize, y: u8, cell: SkirtCell) {
    let start = u8::try_from(column).expect("a column fits one cell index");
    let width = u8::try_from(run - 1).expect("a run fits one record width");
    let plane = side.plane();
    let (x, z) = match side {
        Side::North | Side::South => (start, plane),
        Side::East | Side::West => (plane, start),
    };
    out.push(Quad::encode(
        x,
        y,
        z,
        width,
        0,
        side.normal(),
        AO_OPEN,
        cell.sky,
        cell.blk,
        cell.tile,
        cell.tint,
    ));
}

/// The lowest exposed top face of every boundary column along `side`.
fn boundary_cells(mesh: &SectionMesh, side: Side) -> [Option<SkirtCell>; PLANE_WIDTH] {
    let mut cells: [Option<SkirtCell>; PLANE_WIDTH] = [None; PLANE_WIDTH];
    for quad in &mesh.opaque {
        let Some((start, end)) = boundary_span(*quad, side) else {
            continue;
        };
        let candidate = SkirtCell {
            y: quad.y(),
            sky: quad.sky(),
            blk: quad.blk(),
            tile: quad.tile(),
            tint: quad.tint(),
        };
        for column in start..=end {
            let slot = usize::from(column);
            if slot >= PLANE_WIDTH {
                continue;
            }
            if cells[slot].is_none_or(|current| current.y > candidate.y) {
                cells[slot] = Some(candidate);
            }
        }
    }
    cells
}

/// The inclusive column range a `+Y` face covers on `side`'s boundary plane.
fn boundary_span(quad: Quad, side: Side) -> Option<(u8, u8)> {
    if quad.normal() != NORMAL_POS_Y {
        return None;
    }
    let (plane, thickness, along, length) = match side {
        Side::North | Side::South => (quad.z(), quad.h1(), quad.x(), quad.w1()),
        Side::East | Side::West => (quad.x(), quad.w1(), quad.z(), quad.h1()),
    };
    let on_plane = match side {
        Side::North | Side::West => plane == 0,
        Side::South | Side::East => plane.checked_add(thickness) == Some(side.plane()),
    };
    if !on_plane {
        return None;
    }
    Some((along, along.checked_add(length)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::description::SECTION_VOLUME;

    /// The runtime id of a stable block.
    fn id(block: Block) -> BlockId {
        BlockId::from(block.id())
    }

    /// A section holding the given sparse cells in air.
    fn section_with(cells: &[([u32; 3], BlockId)]) -> Section {
        let mut section = Section::uniform(0);
        for (position, block) in cells {
            section.set(position[0], position[1], position[2], *block);
        }
        section
    }

    /// A padded copy of a section with air neighbours.
    ///
    /// Tests use explicit air sections rather than `None`: a missing neighbour
    /// mirrors the edge row (the world-edge rule), which is not what an
    /// interior section with unresolvable neighbours should read.
    fn padded(section: &Section) -> PaddedBlocks {
        let air = Section::uniform(0);
        PaddedBlocks::from_sections(section, &[Some(&air); 26])
    }

    /// A 32x32 stone slab at `y = 0`.
    fn slab() -> Section {
        let mut section = Section::uniform(0);
        for x in 0..SECTION_SIZE {
            for z in 0..SECTION_SIZE {
                section.set(x, 0, z, id(Block::Stone));
            }
        }
        section
    }

    /// Full light.
    fn lit() -> PaddedLight {
        PaddedLight::new(15, 0)
    }

    /// The default material table, asserting it resolves the stable ids.
    fn props() -> MaterialProps {
        let props = MaterialProps::default_table();
        assert_eq!(props.material(id(Block::Air)), Material::AIR);
        assert!(props.material(id(Block::Stone)).opaque);
        assert!(props.material(id(Block::Grass)).cross_quad);
        assert!(props.material(id(Block::Leaves)).solid);
        assert!(!props.material(id(Block::Leaves)).opaque);
        assert!(props.material(id(Block::Water)).water);
        assert!(props.material(id(Block::Sand)).opaque);
        props
    }

    /// Every record from all three ranges, in output order.
    fn raw(mesh: &SectionMesh) -> Vec<u64> {
        mesh.opaque
            .iter()
            .chain(&mesh.cutout)
            .chain(&mesh.water)
            .map(|quad| quad.bits())
            .collect()
    }

    /// The quads the last [`add_skirts`] call appended.
    fn added_skirts(mesh: &SectionMesh, before: usize) -> &[Quad] {
        &mesh.opaque[before..]
    }

    /// A 32x32 stone cap on the top cell layer of a section.
    fn cap() -> Section {
        let mut section = Section::uniform(0);
        for x in 0..SECTION_SIZE {
            for z in 0..SECTION_SIZE {
                section.set(x, SECTION_SIZE - 1, z, id(Block::Stone));
            }
        }
        section
    }

    /// The deterministic skirt fixture: a top cap with all four skirts.
    fn skirted_cap() -> SectionMesh {
        let section = cap();
        let blocks = padded(&section);
        let mut mesh = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);
        assert_eq!(
            add_skirts(&mut mesh, SkirtEdges::ALL, MeshLod::Lod0),
            8,
            "the cap has four skirted sides at SKIRT_DEPTH cells each"
        );
        mesh
    }

    /// A top-face count for the face code.
    fn count_normal(quads: &[Quad], normal: u8) -> usize {
        quads.iter().filter(|quad| quad.normal() == normal).count()
    }

    /// The single `+Y` quad with the given origin.
    fn top_quad(mesh: &SectionMesh, at: (u8, u8, u8)) -> Quad {
        mesh.opaque
            .iter()
            .copied()
            .find(|quad| quad.normal() == NORMAL_POS_Y && (quad.x(), quad.y(), quad.z()) == at)
            .expect("the fixture has a top face at the requested cell")
    }

    /// The AO value of one packed corner.
    fn corner(ao: u8, index: u32) -> u8 {
        (ao >> (2 * index)) & 0b11
    }

    #[test]
    fn record_layout_is_64_bits() {
        let zero = Quad::encode(0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
        assert_eq!(zero.bits(), 0);
        assert_eq!(
            (
                zero.x(),
                zero.y(),
                zero.z(),
                zero.w1(),
                zero.h1(),
                zero.normal(),
                zero.ao(),
                zero.sky(),
                zero.blk(),
                zero.tile(),
                zero.tint(),
            ),
            (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0)
        );

        let max = Quad::encode(31, 31, 31, 31, 31, 7, 255, 15, 15, u16::MAX, 15);
        assert_eq!(
            max.bits(),
            u64::MAX,
            "every bit must belong to a field; the encoder masks nothing away"
        );
        assert_eq!(
            (
                max.x(),
                max.y(),
                max.z(),
                max.w1(),
                max.h1(),
                max.normal(),
                max.ao(),
                max.sky(),
                max.blk(),
                max.tile(),
                max.tint(),
            ),
            (31, 31, 31, 31, 31, 7, 255, 15, 15, u16::MAX, 15)
        );
    }

    #[test]
    fn coordinates_round_trip_over_the_whole_section() {
        for x in 0..32_u8 {
            for y in 0..32_u8 {
                for z in 0..32_u8 {
                    let quad = Quad::encode(x, y, z, 0, 0, 0, 0, 0, 0, 0, 0);
                    assert_eq!((quad.x(), quad.y(), quad.z()), (x, y, z));
                }
            }
        }
    }

    #[test]
    fn single_block_emits_six_faces() {
        let section = section_with(&[([5, 5, 5], id(Block::Stone))]);
        let blocks = padded(&section);
        let result = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);

        assert_eq!(result.opaque.len(), 6);
        assert_eq!(result.cutout.len(), 0);
        assert_eq!(result.water.len(), 0);
    }

    #[test]
    fn flat_ground_merges_into_one_quad() {
        let section = slab();
        let blocks = padded(&section);
        let result = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);

        assert_eq!(count_normal(&result.opaque, NORMAL_POS_Y), 1);
        let top = result
            .opaque
            .iter()
            .find(|quad| quad.normal() == NORMAL_POS_Y)
            .expect("the slab has a top face");
        assert_eq!((top.x(), top.y(), top.z()), (0, 0, 0));
        assert_eq!((top.w1(), top.h1()), (31, 31));
    }

    #[test]
    fn light_difference_breaks_merging() {
        let section = slab();
        let blocks = padded(&section);
        let mut light = lit();
        light.set(0, 1, 0, 7, 0);
        let result = mesh(&section, &blocks, &light, &props(), MeshLod::Lod0);

        // A single dark cell in a uniform plane is a rectangular hole, so the
        // rest of the plane cannot be one rectangle: greedy produces the hole
        // plus two rectangles. The merge still breaks, which is the property
        // M1.1 asks for.
        assert_eq!(count_normal(&result.opaque, NORMAL_POS_Y), 3);
        let skies: Vec<u8> = result
            .opaque
            .iter()
            .filter(|quad| quad.normal() == NORMAL_POS_Y)
            .map(|quad| quad.sky())
            .collect();
        assert!(skies.contains(&7));
        assert!(skies.contains(&15));
    }

    #[test]
    fn ao_difference_breaks_merging() {
        let section = slab();
        let mut blocks = padded(&section);
        for x in -1..=LOCAL_MAX {
            blocks.set(x, 1, -1, id(Block::Stone));
        }
        let result = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);

        let tops: Vec<&Quad> = result
            .opaque
            .iter()
            .filter(|quad| quad.normal() == NORMAL_POS_Y)
            .collect();
        assert_eq!(
            tops.len(),
            2,
            "the dark edge row must not merge with the rest"
        );
        assert_ne!(tops[0].ao(), tops[1].ao(), "the wall darkens one corner");
    }

    #[test]
    fn corner_ao_reads_the_diagonal_neighbour() {
        let stone = id(Block::Stone);
        let section = section_with(&[([0, 31, 0], stone)]);
        let air = Section::uniform(0);
        let corner = section_with(&[([31, 0, 31], stone)]);
        let mut owners: [Option<&Section>; 26] = [Some(&air); 26];
        owners[23] = Some(&corner);

        let blocks = PaddedBlocks::from_sections(&section, &owners);
        let result = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);
        let top = result
            .opaque
            .iter()
            .find(|quad| quad.normal() == NORMAL_POS_Y)
            .expect("the block has a top face");

        // Corner 0 of a `+Y` face is at local `(-1, 32, -1)`, inside the
        // `(-X, +Y, -Z)` corner section. One occluded diagonal is AO 2.
        assert_eq!(top.ao() & 0x03, 2, "the corner section darkens the corner");
        assert_eq!(top.ao(), 254, "the three other corners stay open");
    }

    #[test]
    fn six_face_neighbours_match_the_padded_path_without_diagonals() {
        // A slab whose AO samples never touch a diagonal section: every
        // face-neighbour result must be byte-identical to the 26-neighbour
        // result with explicit air edges and corners.
        let section = slab();
        let air = Section::uniform(0);
        let faces = [Some(&air); 6];
        let material = props();
        let light = lit();

        let six = mesh_from_face_neighbours(&section, &faces, &light, &material, MeshLod::Lod0);
        let padded_all = mesh(
            &section,
            &padded(&section),
            &light,
            &material,
            MeshLod::Lod0,
        );
        assert_eq!(
            raw(&six),
            raw(&padded_all),
            "the six-neighbour path is unchanged where diagonals cannot matter"
        );
    }

    #[test]
    fn diagonal_neighbour_darkens_a_corner_but_a_face_neighbour_does_not() {
        let stone = id(Block::Stone);
        let section = section_with(&[([0, 31, 0], stone)]);
        let air = Section::uniform(0);
        let material = props();
        let light = lit();

        // With no diagonal data the corner is fully open.
        let faces = [Some(&air); 6];
        let open = mesh_from_face_neighbours(&section, &faces, &light, &material, MeshLod::Lod0);
        let open_top = top_quad(&open, (0, 31, 0));
        assert_eq!(
            corner(open_top.ao(), 0),
            3,
            "a corner with no diagonal block is open"
        );

        // A block sharing the `-Z` face of the source sits below the `+Y` AO
        // sample plane, so it culls a side face but cannot darken corner 0.
        let face_neighbour = section_with(&[([0, 31, 31], stone)]);
        let mut owners: [Option<&Section>; 26] = [Some(&air); 26];
        owners[5] = Some(&face_neighbour);
        let side_probe = mesh_from_neighbours(&section, &owners, &light, &material, MeshLod::Lod0);
        let probe_top = top_quad(&side_probe, (0, 31, 0));
        assert_eq!(
            corner(probe_top.ao(), 0),
            3,
            "a face-adjacent block does not darken the diagonal corner"
        );
        assert_eq!(
            side_probe.opaque.len(),
            5,
            "the face-adjacent block culls the shared side face"
        );

        // One opaque block in the `(-X, +Y, -Z)` corner section darkens only
        // corner 0: the diagonal is AO 2.
        let diagonal = section_with(&[([31, 0, 31], stone)]);
        let mut owners: [Option<&Section>; 26] = [Some(&air); 26];
        owners[23] = Some(&diagonal);
        let dark = mesh_from_neighbours(&section, &owners, &light, &material, MeshLod::Lod0);
        let dark_top = top_quad(&dark, (0, 31, 0));
        assert_eq!(corner(dark_top.ao(), 0), 2, "one diagonal is AO 2");
        for index in 1..4 {
            assert_eq!(
                corner(dark_top.ao(), index),
                3,
                "the other corners stay open"
            );
        }
    }

    #[test]
    fn only_opaque_full_cubes_occlude_ao() {
        let stone = id(Block::Stone);
        let section = section_with(&[([5, 5, 5], stone)]);
        let base = padded(&section);
        let material = props();
        let light = lit();

        for (block, why) in [
            (id(Block::Water), "water"),
            (id(Block::Leaves), "cutout leaves"),
            (id(Block::Grass), "cross quads"),
        ] {
            let mut blocks = base.clone();
            // Corner 0 of the `+Y` face samples this cell.
            blocks.set(4, 6, 4, block);
            let result = mesh(&section, &blocks, &light, &material, MeshLod::Lod0);
            let top = top_quad(&result, (5, 5, 5));
            assert_eq!(corner(top.ao(), 0), 3, "{why} must not occlude a corner");
        }

        let mut blocks = base;
        blocks.set(4, 6, 4, stone);
        let result = mesh(&section, &blocks, &light, &material, MeshLod::Lod0);
        let top = top_quad(&result, (5, 5, 5));
        assert_eq!(corner(top.ao(), 0), 2, "a full opaque cube is AO 2");
    }

    #[test]
    fn empty_sections_mesh_to_nothing() {
        let section = Section::uniform(0);
        let air = Section::uniform(0);
        let faces = [Some(&air); 6];
        let material = props();
        let light = lit();

        let from_faces =
            mesh_from_face_neighbours(&section, &faces, &light, &material, MeshLod::Lod0);
        let owners: [Option<&Section>; 26] = [Some(&air); 26];
        let from_all = mesh_from_neighbours(&section, &owners, &light, &material, MeshLod::Lod0);
        assert_eq!(quad_count(&from_faces), 0);
        assert_eq!(quad_count(&from_all), 0);
    }

    #[test]
    fn water_emits_only_against_air() {
        let water = id(Block::Water);
        let stone = id(Block::Stone);

        let section = section_with(&[([0, 0, 0], water), ([0, 1, 0], stone), ([1, 0, 0], stone)]);
        let blocks = padded(&section);
        let result = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);
        assert!(
            !result
                .water
                .iter()
                .any(|quad| quad.normal() == NORMAL_POS_X || quad.normal() == NORMAL_POS_Y),
            "water must not emit against stone"
        );

        let pair = section_with(&[([0, 0, 0], water), ([0, 0, 1], water)]);
        let pair_blocks = padded(&pair);
        let pair_mesh = mesh(&pair, &pair_blocks, &lit(), &props(), MeshLod::Lod0);
        assert!(
            !pair_mesh.water.iter().any(|quad| {
                (quad.normal() == NORMAL_POS_Z && quad.x() == 0 && quad.y() == 0 && quad.z() == 0)
                    || (quad.normal() == NORMAL_NEG_Z
                        && quad.x() == 0
                        && quad.y() == 0
                        && quad.z() == 1)
            }),
            "water/water emits no face on the shared plane"
        );

        let mut ocean = Section::uniform(0);
        for x in 0..SECTION_SIZE {
            for z in 0..SECTION_SIZE {
                ocean.set(x, 0, z, water);
            }
        }
        let mut ocean_blocks = padded(&ocean);
        for x in -1..=LOCAL_MAX {
            for z in -1..=LOCAL_MAX {
                ocean_blocks.set(x, -1, z, water);
            }
        }
        for z in -1..=LOCAL_MAX {
            ocean_blocks.set(-1, 0, z, water);
            ocean_blocks.set(LOCAL_MAX, 0, z, water);
        }
        for x in -1..=LOCAL_MAX {
            ocean_blocks.set(x, 0, -1, water);
            ocean_blocks.set(x, 0, LOCAL_MAX, water);
        }
        let ocean_mesh = mesh(&ocean, &ocean_blocks, &lit(), &props(), MeshLod::Lod0);
        assert_eq!(ocean_mesh.water.len(), 1, "a full ocean top is one quad");
        let top = ocean_mesh.water[0];
        assert_eq!(top.normal(), NORMAL_POS_Y);
        assert_eq!((top.w1(), top.h1()), (31, 31));
    }

    #[test]
    fn cutout_is_never_merged() {
        let leaves = id(Block::Leaves);
        let section = section_with(&[
            ([0, 0, 0], leaves),
            ([1, 0, 0], leaves),
            ([2, 0, 0], leaves),
            ([3, 0, 0], leaves),
        ]);
        let blocks = padded(&section);
        let result = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);

        assert_eq!(result.opaque.len(), 0);
        assert_eq!(count_normal(&result.cutout, NORMAL_POS_Y), 4);
        let mut tops: Vec<Quad> = result
            .cutout
            .iter()
            .copied()
            .filter(|quad| quad.normal() == NORMAL_POS_Y)
            .collect();
        assert_eq!(tops.len(), 4, "each leaf keeps its own record");
        tops.sort_by_key(|quad| quad.bits());
        tops.dedup();
        assert_eq!(tops.len(), 4, "records differ by origin");
        assert!(
            result
                .cutout
                .iter()
                .all(|quad| quad.w1() == 0 && quad.h1() == 0),
            "cutout faces are never merged"
        );
    }

    #[test]
    fn cross_quads_are_two_quads_and_never_occlude() {
        let grass = id(Block::Grass);
        let stone = id(Block::Stone);
        let section = section_with(&[([0, 0, 0], grass), ([1, 0, 0], stone)]);
        let blocks = padded(&section);
        let result = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);

        assert_eq!(result.cutout.len(), 2, "one cross quad per diagonal");
        assert!(
            result
                .cutout
                .iter()
                .any(|quad| quad.normal() == NORMAL_CROSS_A)
        );
        assert!(
            result
                .cutout
                .iter()
                .any(|quad| quad.normal() == NORMAL_CROSS_B)
        );
        assert!(
            !result
                .opaque
                .iter()
                .any(|quad| quad.x() == 0 && quad.y() == 0 && quad.z() == 0),
            "grass emits no opaque record"
        );
        assert_eq!(
            count_normal(&result.opaque, NORMAL_NEG_X),
            1,
            "grass does not cull the neighbour's face"
        );
    }

    #[test]
    fn output_order_is_deterministic() {
        let stone = id(Block::Stone);
        let leaves = id(Block::Leaves);

        let mut data: Box<[BlockId; SECTION_VOLUME]> = vec![0_u16; SECTION_VOLUME]
            .into_boxed_slice()
            .try_into()
            .expect("the scratch array matches the section volume");
        for y in 0..32_usize {
            for z in 0..32_usize {
                for x in 0..32_usize {
                    let block = match (x * 7 + z * 13 + y) % 11 {
                        0 | 1 => 0,
                        2 => leaves,
                        _ => stone,
                    };
                    data[crate::section::section_index(
                        u32::try_from(x).expect("fits"),
                        u32::try_from(y).expect("fits"),
                        u32::try_from(z).expect("fits"),
                    )] = block;
                }
            }
        }

        let direct = Section::from_blocks(&data);
        let mut incremental = Section::uniform(0);
        for (index, block) in data.iter().enumerate().rev() {
            incremental.set_index(index, *block);
        }
        assert_eq!(direct, incremental);

        let direct_pad = padded(&direct);
        let mut incremental_pad = PaddedBlocks::new();
        incremental_pad.fill_from(&incremental);
        let material = props();
        let light = lit();

        let first = mesh(&direct, &direct_pad, &light, &material, MeshLod::Lod0);
        let second = mesh(&direct, &direct_pad, &light, &material, MeshLod::Lod0);
        let third = mesh(
            &incremental,
            &incremental_pad,
            &light,
            &material,
            MeshLod::Lod0,
        );
        assert_eq!(raw(&first), raw(&second));
        assert_eq!(raw(&first), raw(&third));
    }

    #[test]
    fn coarse_skips_ao_and_block_light() {
        let stone = id(Block::Stone);
        let mut floor = PaddedBlocks::new();
        for x in 0..LOCAL_SIZE {
            for z in 0..LOCAL_SIZE {
                floor.set(x, 0, z, stone);
            }
        }
        let mut roofed = floor.clone();
        for x in 0..LOCAL_SIZE {
            for z in 0..LOCAL_SIZE {
                roofed.set(x, 10, z, stone);
            }
        }

        let open = mesh_coarse(&floor, &props(), MeshLod::Lod1, 1);
        let open_tops: Vec<&Quad> = open
            .opaque
            .iter()
            .filter(|quad| quad.normal() == NORMAL_POS_Y && quad.y() == 0)
            .collect();
        assert_eq!(open_tops.len(), 1);
        assert_eq!(open_tops[0].sky(), 15, "open ceiling is full sky");

        let shaded = mesh_coarse(&roofed, &props(), MeshLod::Lod1, 1);
        let shaded_tops: Vec<&Quad> = shaded
            .opaque
            .iter()
            .filter(|quad| quad.normal() == NORMAL_POS_Y && quad.y() == 0)
            .collect();
        assert_eq!(shaded_tops.len(), 1, "the uniform sky still merges");
        assert_eq!(shaded_tops[0].sky(), 6, "15 - (10 - 1)");
        assert!(shaded.opaque.iter().all(|quad| quad.ao() == 0));
        assert!(shaded.opaque.iter().all(|quad| quad.blk() == 0));
    }

    #[test]
    fn no_face_between_two_opaque_blocks() {
        let stone = id(Block::Stone);
        let section = section_with(&[([0, 0, 0], stone), ([1, 0, 0], stone)]);
        let blocks = padded(&section);
        let result = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);

        // A 2x1x1 box has six coplanar outer faces, so greedy merging produces
        // one quad per box face rather than the naive ten per-block faces. The
        // shared face is still culled, which is the property this test pins.
        assert_eq!(result.opaque.len(), 6, "one merged quad per box face");
        assert!(
            !result
                .opaque
                .iter()
                .any(|quad| quad.normal() == NORMAL_POS_X && quad.x() == 0),
            "the +X face of the left block is inside the pair"
        );
        assert!(
            !result
                .opaque
                .iter()
                .any(|quad| quad.normal() == NORMAL_NEG_X && quad.x() == 1),
            "the -X face of the right block is inside the pair"
        );
    }

    #[test]
    fn skirt_extends_the_boundary_downward() {
        let section = cap();
        let blocks = padded(&section);
        let mut mesh = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);
        let before = mesh.opaque.len();
        let top = mesh
            .opaque
            .iter()
            .find(|quad| quad.normal() == NORMAL_POS_Y)
            .copied()
            .expect("the cap has a top face");
        assert_eq!(top.y(), 31, "the cap sits on the top cell layer");

        let added = add_skirts(&mut mesh, SkirtEdges::ALL, MeshLod::Lod0);

        let depth = u32::try_from(SKIRT_DEPTH).expect("SKIRT_DEPTH is positive");
        assert_eq!(added, 8, "four sides at SKIRT_DEPTH cells each");
        assert_eq!(mesh.opaque.len(), before + 8);
        let skirts = added_skirts(&mesh, before);
        assert_eq!(skirts.len(), 8);

        // One quad per cell layer, top layer first, hanging under the cap.
        let expected: Vec<u8> = (1..=depth)
            .map(|layer| top.y() - u8::try_from(layer).expect("a layer fits a cell index"))
            .collect();
        assert_eq!(expected, vec![30, 29]);

        for (side, normal) in [
            (Side::North, NORMAL_NEG_Z),
            (Side::South, NORMAL_POS_Z),
            (Side::East, NORMAL_POS_X),
            (Side::West, NORMAL_NEG_X),
        ] {
            let wall: Vec<Quad> = skirts
                .iter()
                .copied()
                .filter(|quad| quad.normal() == normal)
                .collect();
            assert_eq!(wall.len(), 2, "{side:?} is SKIRT_DEPTH cells deep");
            let ys: Vec<u8> = wall.iter().map(|quad| quad.y()).collect();
            assert_eq!(ys, expected, "{side:?} hangs straight down");
            for quad in wall {
                assert_eq!(quad.w1(), 31, "{side:?} is one run across the edge");
                assert_eq!(quad.h1(), 0, "{side:?} quads are one cell tall");
            }
        }

        // World extents: the north wall lies on the z = 0 plane, spans all 32
        // cells of x, and reaches two blocks below the boundary face.
        let north = skirts
            .iter()
            .find(|quad| quad.normal() == NORMAL_NEG_Z)
            .copied()
            .expect("the north edge has a skirt");
        assert_eq!((north.x(), north.z()), (0, 0));
        assert_eq!(u32::from(north.w1()) + 1, 32, "the full x width in blocks");
        let lowest = skirts
            .iter()
            .filter(|quad| quad.normal() == NORMAL_NEG_Z)
            .map(|quad| quad.y())
            .min()
            .expect("the north edge has a skirt");
        let expected_blocks = skirt_blocks(MeshLod::Lod0);
        assert_eq!(
            u32::from(top.y()) - u32::from(lowest),
            expected_blocks,
            "the strip extends the boundary face down by SKIRT_DEPTH blocks"
        );
        assert_eq!(expected_blocks, 2);

        let south = skirts
            .iter()
            .find(|quad| quad.normal() == NORMAL_POS_Z)
            .copied()
            .expect("the south edge has a skirt");
        assert_eq!((south.x(), south.z()), (0, 31));
        let east = skirts
            .iter()
            .find(|quad| quad.normal() == NORMAL_POS_X)
            .copied()
            .expect("the east edge has a skirt");
        assert_eq!((east.x(), east.z()), (31, 0));
        assert_eq!(u32::from(east.w1()) + 1, 32, "the full z width in blocks");
        let west = skirts
            .iter()
            .find(|quad| quad.normal() == NORMAL_NEG_X)
            .copied()
            .expect("the west edge has a skirt");
        assert_eq!((west.x(), west.z()), (0, 0));
    }

    #[test]
    fn skirt_depth_scales_with_lod() {
        let section = cap();
        let blocks = padded(&section);
        let material = props();
        let light = lit();

        let mut fine = mesh(&section, &blocks, &light, &material, MeshLod::Lod0);
        let fine_before = fine.opaque.len();
        assert_eq!(add_skirts(&mut fine, SkirtEdges::ALL, MeshLod::Lod0), 8);
        let mut coarse = mesh(&section, &blocks, &light, &material, MeshLod::Lod1);
        let coarse_before = coarse.opaque.len();
        assert_eq!(add_skirts(&mut coarse, SkirtEdges::ALL, MeshLod::Lod1), 8);

        let wall = |built: &SectionMesh, before: usize| -> Vec<u8> {
            added_skirts(built, before)
                .iter()
                .filter(|quad| quad.normal() == NORMAL_NEG_Z)
                .map(|quad| quad.y())
                .collect()
        };
        let fine_wall = wall(&fine, fine_before);
        let coarse_wall = wall(&coarse, coarse_before);
        assert_eq!(fine_wall.len(), 2, "one quad per skirt cell");
        assert_eq!(
            coarse_wall, fine_wall,
            "the same cells carry the skirt at every level"
        );

        // Decode the world-space height from the `y` field: one coarse cell is
        // `cells_per_block` blocks tall, so LOD1's strip is twice as tall.
        let cell_span = |wall: &[u8]| -> u8 {
            let low = wall.iter().copied().min().expect("a wall has cells");
            let high = wall.iter().copied().max().expect("a wall has cells") + 1;
            high - low
        };
        assert_eq!(cell_span(&fine_wall), 2, "two cells of stone");
        assert_eq!(cell_span(&coarse_wall), 2);
        let fine_height = u32::from(cell_span(&fine_wall)) * MeshLod::Lod0.cells_per_block();
        let coarse_height = u32::from(cell_span(&coarse_wall)) * MeshLod::Lod1.cells_per_block();
        assert_eq!(fine_height, skirt_blocks(MeshLod::Lod0));
        assert_eq!(coarse_height, skirt_blocks(MeshLod::Lod1));
        assert_eq!(fine_height, 2);
        assert_eq!(coarse_height, 4, "LOD1 cells are two blocks tall");
        assert_eq!(coarse_height, 2 * fine_height, "twice as tall in blocks");
    }

    #[test]
    fn skirt_closes_a_one_block_step_at_the_edge() {
        // A single exposed block on the east boundary, one cell above the
        // section floor: the strip has exactly one cell of room below it.
        let section = section_with(&[([31, 1, 10], id(Block::Stone))]);
        let blocks = padded(&section);
        let mut mesh = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);
        let top = top_quad(&mesh, (31, 1, 10));
        assert_eq!(top.y(), 1, "the step top is one cell above the floor");

        let before = mesh.opaque.len();
        assert_eq!(
            add_skirts(&mut mesh, SkirtEdges::EAST, MeshLod::Lod0),
            1,
            "a one-cell step emits one skirt quad, not SKIRT_DEPTH of them"
        );
        let skirts = added_skirts(&mesh, before);
        assert_eq!(skirts.len(), 1);
        let skirt = skirts[0];
        assert_eq!(
            (skirt.x(), skirt.y(), skirt.z()),
            (31, 0, 10),
            "the strip starts directly under the boundary top"
        );
        assert_eq!(skirt.normal(), NORMAL_POS_X, "the east wall faces +X");
        assert_eq!((skirt.w1(), skirt.h1()), (0, 0), "one cell wide, one tall");
        assert_eq!(
            u32::from(top.y()) - u32::from(skirt.y()),
            1,
            "the strip height equals the step height"
        );
    }

    #[test]
    fn step_skirt_fills_a_one_block_drop() {
        let stone = id(Block::Stone);
        let section = section_with(&[([31, 4, 10], stone), ([31, 5, 10], stone)]);
        let plus_x = section_with(&[([0, 4, 10], stone)]);
        let air = Section::uniform(0);
        let mut owners: [Option<&Section>; 26] = [Some(&air); 26];
        owners[0] = Some(&plus_x);
        let blocks = PaddedBlocks::from_sections(&section, &owners);
        let material = props();
        let light = lit();
        let mut mesh = mesh(&section, &blocks, &light, &material, MeshLod::Lod0);
        let top = top_quad(&mesh, (31, 5, 10));
        assert_eq!(top.y(), 5);

        let before = mesh.opaque.len();
        assert_eq!(
            add_step_skirts(
                &mut mesh,
                &blocks,
                &material,
                SkirtEdges::EAST,
                MeshLod::Lod0
            ),
            1,
            "a one-cell drop needs exactly one skirt cell"
        );
        let skirts = added_skirts(&mesh, before);
        assert_eq!(skirts.len(), 1);
        let skirt = skirts[0];
        assert_eq!(
            (skirt.x(), skirt.y(), skirt.z()),
            (31, 4, 10),
            "the strip fills the cell between the top and the lower column"
        );
        assert_eq!(skirt.normal(), NORMAL_POS_X);
        assert_eq!(
            u32::from(top.y()) - u32::from(skirt.y()),
            1,
            "the strip height equals the drop"
        );
    }

    #[test]
    fn step_skirt_is_empty_on_a_flush_edge() {
        let stone = id(Block::Stone);
        let section = section_with(&[([31, 5, 10], stone)]);
        let air = Section::uniform(0);
        let material = props();
        let light = lit();

        for (height, why) in [(5_u32, "flush"), (6_u32, "neighbour above")] {
            let neighbour = section_with(&[([0, height, 10], stone)]);
            let mut owners: [Option<&Section>; 26] = [Some(&air); 26];
            owners[0] = Some(&neighbour);
            let blocks = PaddedBlocks::from_sections(&section, &owners);
            let mut mesh = mesh(&section, &blocks, &light, &material, MeshLod::Lod0);
            let before = mesh.opaque.len();
            assert_eq!(
                add_step_skirts(
                    &mut mesh,
                    &blocks,
                    &material,
                    SkirtEdges::EAST,
                    MeshLod::Lod0
                ),
                0,
                "{why} edge has no drop to fill"
            );
            assert_eq!(mesh.opaque.len(), before, "{why} edge appends nothing");
        }
    }

    #[test]
    fn step_skirts_never_appear_on_interior_edges() {
        let stone = id(Block::Stone);

        let interior = section_with(&[([10, 10, 10], stone)]);
        let blocks = padded(&interior);
        let material = props();
        let mut middle = mesh(&interior, &blocks, &lit(), &material, MeshLod::Lod0);
        let before = middle.opaque.len();
        assert_eq!(
            add_step_skirts(
                &mut middle,
                &blocks,
                &material,
                SkirtEdges::ALL,
                MeshLod::Lod0
            ),
            0,
            "an interior column has no boundary face"
        );
        assert_eq!(middle.opaque.len(), before);

        // A west-edge column asked for an east skirt emits nothing.
        let west = section_with(&[([0, 10, 15], stone)]);
        let blocks = padded(&west);
        let mut east_only = mesh(&west, &blocks, &lit(), &material, MeshLod::Lod0);
        let before = east_only.opaque.len();
        assert_eq!(
            add_step_skirts(
                &mut east_only,
                &blocks,
                &material,
                SkirtEdges::NONE.with(Side::East),
                MeshLod::Lod0
            ),
            0
        );
        assert_eq!(east_only.opaque.len(), before);
    }

    #[test]
    fn step_skirts_are_deterministic_and_append_once() {
        let stone = id(Block::Stone);
        let section = section_with(&[([31, 4, 10], stone), ([31, 5, 10], stone)]);
        let plus_x = section_with(&[([0, 4, 10], stone)]);
        let air = Section::uniform(0);
        let mut owners: [Option<&Section>; 26] = [Some(&air); 26];
        owners[0] = Some(&plus_x);
        let blocks = PaddedBlocks::from_sections(&section, &owners);
        let material = props();
        let light = lit();

        let build = || {
            let mut mesh = mesh(&section, &blocks, &light, &material, MeshLod::Lod0);
            assert_eq!(
                add_step_skirts(
                    &mut mesh,
                    &blocks,
                    &material,
                    SkirtEdges::ALL,
                    MeshLod::Lod0
                ),
                1
            );
            mesh
        };
        let first = build();
        let second = build();
        assert_eq!(raw(&first), raw(&second), "same input, same bytes");

        let mut mesh = build();
        let once = raw(&mesh);
        assert_eq!(
            add_step_skirts(
                &mut mesh,
                &blocks,
                &material,
                SkirtEdges::ALL,
                MeshLod::Lod0
            ),
            0
        );
        assert_eq!(raw(&mesh), once, "a skirt set is never appended twice");
    }

    #[test]
    fn flat_boundary_has_a_zero_height_skirt() {
        // No exposed top face at the edge: an empty section adds nothing.
        let mut empty = SectionMesh::default();
        assert_eq!(add_skirts(&mut empty, SkirtEdges::ALL, MeshLod::Lod0), 0);
        assert_eq!(empty.opaque.len(), 0);
        assert!(empty.skirts.is_empty());

        // A boundary surface on the section floor has no cell below it inside
        // this section, so the skirt clamps to zero height. This is the
        // documented flat-edge case: zero quads, not a strip into the section
        // below.
        let section = slab();
        let blocks = padded(&section);
        let mut flat = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);
        let before = flat.opaque.len();
        assert_eq!(
            add_skirts(&mut flat, SkirtEdges::ALL, MeshLod::Lod0),
            0,
            "a floor-level boundary has zero skirt height"
        );
        assert_eq!(flat.opaque.len(), before, "no record is appended");
    }

    #[test]
    fn interior_edges_never_get_a_skirt() {
        // A block in the middle of the section has no face on any boundary
        // plane, so no side can hang a skirt from it.
        let interior = section_with(&[([10, 10, 10], id(Block::Stone))]);
        let blocks = padded(&interior);
        let mut middle = mesh(&interior, &blocks, &lit(), &props(), MeshLod::Lod0);
        let before = middle.opaque.len();
        assert_eq!(add_skirts(&mut middle, SkirtEdges::ALL, MeshLod::Lod0), 0);
        assert_eq!(middle.opaque.len(), before);

        // A west-edge block asked for an east skirt emits nothing: the
        // requested edge has no boundary face.
        let west = section_with(&[([0, 10, 15], id(Block::Stone))]);
        let blocks = padded(&west);
        let mut east_only = mesh(&west, &blocks, &lit(), &props(), MeshLod::Lod0);
        let before = east_only.opaque.len();
        assert_eq!(
            add_skirts(
                &mut east_only,
                SkirtEdges::NONE.with(Side::East),
                MeshLod::Lod0
            ),
            0
        );
        assert_eq!(east_only.opaque.len(), before);

        // Selecting the edge that does own the face emits there and nowhere
        // else, which is the interior-edge invariant.
        let mut west_only = mesh(&west, &blocks, &lit(), &props(), MeshLod::Lod0);
        let before = west_only.opaque.len();
        assert_eq!(
            add_skirts(
                &mut west_only,
                SkirtEdges::NONE.with(Side::West),
                MeshLod::Lod0
            ),
            2
        );
        let skirts = added_skirts(&west_only, before);
        assert_eq!(skirts.len(), 2);
        assert!(
            skirts.iter().all(|quad| quad.normal() == NORMAL_NEG_X),
            "only the west wall is emitted"
        );
    }

    #[test]
    fn no_skirt_without_a_boundary_face() {
        // An empty mesh has nothing to hang a skirt from.
        let mut empty = SectionMesh::default();
        assert_eq!(add_skirts(&mut empty, SkirtEdges::ALL, MeshLod::Lod0), 0);
        assert_eq!(empty.opaque.len(), 0);
        assert!(empty.skirts.is_empty());

        // A block in the middle of the section has six faces, none of them on a
        // boundary plane.
        let stone = section_with(&[([10, 10, 10], id(Block::Stone))]);
        let blocks = padded(&stone);
        let mut interior = mesh(&stone, &blocks, &lit(), &props(), MeshLod::Lod0);
        let before = interior.opaque.len();
        assert_eq!(before, 6);
        assert_eq!(add_skirts(&mut interior, SkirtEdges::ALL, MeshLod::Lod0), 0);
        assert_eq!(interior.opaque.len(), before);

        // A section buried in solid rock emits no face at all.
        let solid = Section::uniform(id(Block::Stone));
        let buried = PaddedBlocks::from_sections(&solid, &[Some(&solid); 26]);
        let mut buried_mesh = mesh(&solid, &buried, &lit(), &props(), MeshLod::Lod0);
        assert_eq!(quad_count(&buried_mesh), 0);
        assert_eq!(
            add_skirts(&mut buried_mesh, SkirtEdges::ALL, MeshLod::Lod0),
            0
        );
        assert_eq!(buried_mesh.opaque.len(), 0);
    }

    #[test]
    fn skirts_are_deterministic() {
        let first = skirted_cap();
        let second = skirted_cap();
        assert_eq!(raw(&first), raw(&second), "same input, same bytes");

        // The marker makes a second apply a no-op instead of a doubled strip.
        let mut mesh = skirted_cap();
        let once = raw(&mesh);
        assert_eq!(add_skirts(&mut mesh, SkirtEdges::ALL, MeshLod::Lod0), 0);
        assert_eq!(raw(&mesh), once, "a skirt set is never appended twice");
    }

    #[test]
    fn skirt_ao_never_darkens() {
        let section = cap();
        let blocks = padded(&section);
        let mut mesh = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);
        let before = mesh.opaque.len();
        assert_eq!(add_skirts(&mut mesh, SkirtEdges::ALL, MeshLod::Lod0), 8);
        let skirts = added_skirts(&mesh, before);

        assert_eq!(skirts.len(), 8);
        for quad in skirts {
            assert_eq!(quad.ao(), 0xFF, "every corner is fully open");
            for corner in 0_u32..4 {
                assert_eq!(
                    (quad.ao() >> (2 * corner)) & 0b11,
                    3,
                    "corner {corner} of a skirt never darkens the seam"
                );
            }
        }
    }

    #[test]
    fn one_side_only_touches_that_side() {
        let edges = SkirtEdges::NONE.with(Side::East);
        assert!(edges.east());
        assert!(!edges.north());
        assert!(!edges.south());
        assert!(!edges.west());
        assert!(SkirtEdges::ALL.north());
        assert!(SkirtEdges::ALL.south());
        assert!(SkirtEdges::ALL.east());
        assert!(SkirtEdges::ALL.west());
        assert!(SkirtEdges::NONE.is_empty());
        assert_eq!(
            SkirtEdges::NONE.with(Side::East).without(Side::East),
            SkirtEdges::NONE
        );

        // `Side` keeps the `revisions::Face` order and meaning.
        for (index, face) in ColumnFace::ALL.into_iter().enumerate() {
            assert_eq!(Side::from(face), Side::ALL[index]);
        }

        let section = cap();
        let blocks = padded(&section);
        let mut mesh = mesh(&section, &blocks, &lit(), &props(), MeshLod::Lod0);
        let before = mesh.opaque.len();
        assert_eq!(add_skirts(&mut mesh, edges, MeshLod::Lod0), 2);
        let skirts = added_skirts(&mesh, before);

        assert_eq!(skirts.len(), 2);
        for quad in skirts {
            assert_eq!(quad.normal(), NORMAL_POS_X, "only the east wall");
            assert_eq!(quad.x(), 31, "the east edge plane");
            assert_eq!(quad.z(), 0, "the run starts at the north end");
            assert_eq!(quad.w1(), 31, "the run covers the whole east edge");
        }
    }
}

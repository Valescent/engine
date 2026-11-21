//! Block material flags and the per-id material table.

use crate::section::BlockId;

use super::record::{NORMAL_NEG_Y, NORMAL_POS_Y};

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
    /// Texture-array layer for `+Y`.
    pub texture_top: u16,
    /// Texture-array layer for the four sides and cross quads.
    pub texture_side: u16,
    /// Texture-array layer for `-Y`.
    pub texture_bottom: u16,
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
        texture_top: 0,
        texture_side: 0,
        texture_bottom: 0,
        tint: 0,
        emissive: false,
    };

    /// Fallback for an id outside the table: visible stone-like geometry.
    pub const UNKNOWN: Self = Self {
        solid: true,
        opaque: true,
        water: false,
        cross_quad: false,
        texture_top: 0,
        texture_side: 0,
        texture_bottom: 0,
        tint: 0,
        emissive: false,
    };

    /// Texture layer for a packed normal code (`ADR-0015`).
    #[must_use]
    pub const fn tile_for(&self, normal: u8) -> u16 {
        match normal {
            NORMAL_POS_Y => self.texture_top,
            NORMAL_NEG_Y => self.texture_bottom,
            _ => self.texture_side,
        }
    }
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
    /// Texture layer per id and face.
    textures: Vec<[u16; 3]>,
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
                    texture_top: 0,
                    texture_side: 0,
                    texture_bottom: 0,
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
                    texture_top: 1,
                    texture_side: 1,
                    texture_bottom: 1,
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
                    texture_top: 2,
                    texture_side: 3,
                    texture_bottom: 4,
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
                    texture_top: 5,
                    texture_side: 6,
                    texture_bottom: 5,
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
                    texture_top: 7,
                    texture_side: 7,
                    texture_bottom: 7,
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
                    texture_top: 8,
                    texture_side: 8,
                    texture_bottom: 8,
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
                    texture_top: 9,
                    texture_side: 9,
                    texture_bottom: 9,
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
                    texture_top: 10,
                    texture_side: 10,
                    texture_bottom: 10,
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
            self.textures.resize(length, [0; 3]);
            self.tint.resize(length, 0);
            self.emissive.resize(length, false);
        }
        self.solid[slot] = material.solid;
        self.opaque[slot] = material.opaque;
        self.water[slot] = material.water;
        self.cross_quad[slot] = material.cross_quad;
        self.textures[slot] = [
            material.texture_top,
            material.texture_side,
            material.texture_bottom,
        ];
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
            texture_top: self.textures[slot][0],
            texture_side: self.textures[slot][1],
            texture_bottom: self.textures[slot][2],
            tint: self.tint[slot],
            emissive: self.emissive[slot],
        }
    }
}

//! The description data model: schema constants, keys, and validation.

use crate::error::{ErrorKind, Stage, TerrainError};

/// File name of the description inside a world directory.
pub(crate) const DESCRIPTION_FILE: &str = "description.toml";

/// Schema version of the description format itself.
pub(crate) const SCHEMA_VERSION: u16 = 1;

/// Height of one section on every axis (`I1.3`).
pub const SECTION_SIZE: u32 = 32;

/// Vertical sections per column (`I1.3`).
pub const SECTIONS_PER_COLUMN: u32 = 12;

/// Blocks in a section.
pub const SECTION_VOLUME: usize = (SECTION_SIZE * SECTION_SIZE * SECTION_SIZE) as usize;

/// Tallest world the layout supports, `0..=383` (`C4.3`).
pub const WORLD_HEIGHT: u32 = 384;

/// Sea level (`C1.2`).
pub const SEA: i32 = 128;

/// Highest possible terrain top (`C1.5`).
pub const MAX_TERRAIN_Y: i32 = 352;

/// Depth below which only deep rock appears (`G1.4`).
pub(crate) const DEEP_Y: i32 = 64;

/// Blocks per macro lattice cell (`B4.1`).
pub const MACRO_CELL: i32 = 4;

/// Blocks per field tile edge (`B4.2`).
pub const TILE_BLOCKS: i32 = 128;

/// Blocks per hydrology tile edge (`D8.1`, confirm).
pub const HYDRO_TILE: i32 = 512;

/// Halo width of a hydrology tile (`D8.1`, confirm).
pub(crate) const HYDRO_BORDER: i32 = 64;

/// Accumulated cells that start a river (`D3.1`).
pub(crate) const RIVER_MIN_CELLS: u32 = 300;

/// Accumulation scale of the river width curve (`D3.2`).
pub(crate) const W_SCALE: f32 = 0.28;

/// Accumulation over which a river ramps up from zero width (`D3.2`).
pub(crate) const RIVER_RAMP_CELLS: f32 = 50.0;

/// Minimum roof above a cave (`F2.5`).
pub(crate) const CAVE_MIN_ROOF: i32 = 4;

/// Detail band depth below the macro top (`F3.3`).
pub(crate) const DETAIL_DOWN: i32 = 24;

/// Detail band height above the macro top (`F3.3`).
pub(crate) const DETAIL_UP: i32 = 8;

/// Distance at which fog ends: `LOD2_RADIUS_BLOCKS - 64` (`O3.1`).
pub(crate) const FOG_END: i32 = LOD2_RADIUS_BLOCKS - 64;

/// LOD0 radius in blocks: 16 columns of 32 (`N1.1`).
pub(crate) const LOD0_RADIUS_BLOCKS: i32 = 512;

/// LOD1 radius in blocks (`N1.1`).
pub(crate) const LOD1_RADIUS_BLOCKS: i32 = 1_024;

/// LOD2 radius in blocks (`N1.1`).
pub(crate) const LOD2_RADIUS_BLOCKS: i32 = 2_048;

/// The wind axis for the rain shadow march (`E2.1`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Axis {
    /// Wind blows toward `-x`.
    X,
    /// Wind blows toward `-z`.
    Z,
}

/// One key in the description schema.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct KeySpec {
    /// Key name as written in `description.toml`.
    pub name: &'static str,
    /// Whether the key feeds the description hash.
    pub hashed: bool,
}

/// Every key the schema accepts. Order is canonical.
pub const KEYS: &[KeySpec] = &[
    KeySpec {
        name: "WORLD_HEIGHT",
        hashed: true,
    },
    KeySpec {
        name: "SEA",
        hashed: true,
    },
    KeySpec {
        name: "MAX_TERRAIN_Y",
        hashed: true,
    },
    KeySpec {
        name: "DEEP_Y",
        hashed: true,
    },
    KeySpec {
        name: "MACRO_CELL",
        hashed: true,
    },
    KeySpec {
        name: "TILE_BLOCKS",
        hashed: true,
    },
    KeySpec {
        name: "HYDRO_TILE",
        hashed: true,
    },
    KeySpec {
        name: "HYDRO_BORDER",
        hashed: true,
    },
    KeySpec {
        name: "WORLD_HALF_EXTENT",
        hashed: true,
    },
    KeySpec {
        name: "RIVER_MIN_CELLS",
        hashed: true,
    },
    KeySpec {
        name: "W_SCALE",
        hashed: true,
    },
    KeySpec {
        name: "RIVER_RAMP_CELLS",
        hashed: true,
    },
    KeySpec {
        name: "LAKE_MIN_DEPTH",
        hashed: true,
    },
    KeySpec {
        name: "LAKE_MIN_CELLS",
        hashed: true,
    },
    KeySpec {
        name: "MAX_LAKE_Y",
        hashed: true,
    },
    KeySpec {
        name: "CAVE_MIN_ROOF",
        hashed: true,
    },
    KeySpec {
        name: "CAVE_T",
        hashed: true,
    },
    KeySpec {
        name: "ENTRANCE_T",
        hashed: true,
    },
    KeySpec {
        name: "DETAIL_DOWN",
        hashed: true,
    },
    KeySpec {
        name: "DETAIL_UP",
        hashed: true,
    },
    KeySpec {
        name: "WATER_SKY_ATTEN",
        hashed: true,
    },
    KeySpec {
        name: "LAPSE",
        hashed: true,
    },
    KeySpec {
        name: "T_SNOW",
        hashed: true,
    },
    KeySpec {
        name: "BLEND_BLOCKS",
        hashed: true,
    },
    KeySpec {
        name: "STRATA_THICKNESS",
        hashed: true,
    },
    KeySpec {
        name: "STRATA_WARP_AMP",
        hashed: true,
    },
    KeySpec {
        name: "FAULT_CELL",
        hashed: true,
    },
    KeySpec {
        name: "ORE_REGION",
        hashed: true,
    },
    KeySpec {
        name: "TREE_CELL",
        hashed: true,
    },
    KeySpec {
        name: "WIND_DIR",
        hashed: true,
    },
    KeySpec {
        name: "Z_EQUATOR",
        hashed: true,
    },
    KeySpec {
        name: "Z_HALF_SPAN",
        hashed: true,
    },
    KeySpec {
        name: "EDGE_BAND",
        hashed: true,
    },
    KeySpec {
        name: "GRAPH",
        hashed: true,
    },
    KeySpec {
        name: "CONTENT",
        hashed: true,
    },
];

/// A named, versioned artifact referenced by the description.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct EmbeddedRef {
    /// Stable name.
    pub name: String,
    /// Record version of the artifact.
    pub record_version: u32,
    /// Digest of the artifact's canonical bytes.
    pub hash: [u8; 32],
}

/// The world description. Every field is required and hashed.
#[derive(Clone, Debug, PartialEq)]
pub struct Description {
    /// Format version of this structure.
    pub schema_version: u16,
    /// World height in blocks (`C4.3`).
    pub world_height: u32,
    /// Sea level (`C1.2`).
    pub sea: i32,
    /// Highest terrain top (`C1.5`).
    pub max_terrain_y: i32,
    /// Deep-rock floor (`G1.4`).
    pub deep_y: i32,
    /// Macro cell size (`B4.1`).
    pub macro_cell: i32,
    /// Field tile edge (`B4.2`).
    pub tile_blocks: i32,
    /// Hydrology tile edge (`D8.1`).
    pub hydro_tile: i32,
    /// Hydrology tile halo (`D8.1`).
    pub hydro_border: i32,
    /// Half-extent of the finite world (`K1.3`).
    pub world_half_extent: i32,
    /// Accumulation that starts a river (`D3.1`).
    pub river_min_cells: u32,
    /// Accumulation scale of the river width curve (`D3.2`).
    pub w_scale: f32,
    /// Accumulation over which a river ramps up from zero (`D3.2`).
    pub river_ramp_cells: f32,
    /// Minimum depth that reads as a lake (`D2.6`).
    pub lake_min_depth: f32,
    /// Minimum area that reads as a lake (`D7.2`).
    pub lake_min_cells: u32,
    /// Highest permitted lake surface (`D7.2`).
    pub max_lake_y: i32,
    /// Default cave roof (`F2.5`).
    pub cave_min_roof: i32,
    /// Cave density threshold (`F1.2`).
    pub cave_t: f32,
    /// Entrance-mask threshold (`F2.5`).
    pub entrance_t: f32,
    /// Detail band depth (`F3.3`).
    pub detail_down: i32,
    /// Detail band height (`F3.3`).
    pub detail_up: i32,
    /// Sky-light loss per water block (`L2.4`).
    pub water_sky_atten: u8,
    /// Temperature drop per block above sea (`E1.2`).
    pub lapse: f32,
    /// Snow material threshold (`E4.4`).
    pub t_snow: f32,
    /// Biome blend width in blocks (`E3.3`).
    pub blend_blocks: u32,
    /// Strata band thickness (`G1.1`).
    pub strata_thickness: i32,
    /// Strata warp amplitude (`G1.1`).
    pub strata_warp_amp: f32,
    /// Fault cell size (`G2.2`).
    pub fault_cell: i32,
    /// Ore feature region edge (`G3.1`).
    pub ore_region: i32,
    /// Tree jitter grid cell (`H3.3`).
    pub tree_cell: i32,
    /// Rain-shadow wind axis (`E2.1`).
    pub wind_dir: Axis,
    /// Latitude equator (`E1.1`).
    pub z_equator: i32,
    /// Latitude half span (`E1.1`).
    pub z_half_span: i32,
    /// Edge-ocean band width (`K1.3`).
    pub edge_band: i32,
    /// Field graph identity.
    pub graph: EmbeddedRef,
    /// Content pack identity.
    pub content: EmbeddedRef,
}

/// Fields the world header carries (`A1.2`).
///
/// `// OPEN(A1.2)` the exact field list is not confirmed. Seed and platform tag
/// are here rather than in the description because one description serves many
/// worlds (`A1.1`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorldHeader {
    /// World seed. Not part of the description hash.
    pub seed: u64,
    /// Generator version this world is pinned to (`T1.2`).
    pub generator_version: u16,
    /// Target triple of first generation (`A2.6`).
    pub platform: String,
    /// Digest of the world description.
    pub description_hash: [u8; 32],
    /// Save format version (`J4.1`).
    pub save_version: u16,
}

/// The v1 default description. Every value here is a pinned world parameter.
#[must_use]
pub fn default_v1() -> Description {
    Description {
        schema_version: SCHEMA_VERSION,
        world_height: WORLD_HEIGHT,
        sea: SEA,
        max_terrain_y: MAX_TERRAIN_Y,
        deep_y: DEEP_Y,
        macro_cell: MACRO_CELL,
        tile_blocks: TILE_BLOCKS,
        hydro_tile: HYDRO_TILE,
        hydro_border: HYDRO_BORDER,
        world_half_extent: 1_024,
        river_min_cells: RIVER_MIN_CELLS,
        w_scale: W_SCALE,
        river_ramp_cells: RIVER_RAMP_CELLS,
        lake_min_depth: 1.0,
        lake_min_cells: 12,
        max_lake_y: MAX_TERRAIN_Y - 32,
        cave_min_roof: CAVE_MIN_ROOF,
        cave_t: 0.55,
        entrance_t: 0.82,
        detail_down: DETAIL_DOWN,
        detail_up: DETAIL_UP,
        water_sky_atten: 1,
        lapse: 1.0 / 320.0,
        t_snow: 0.25,
        blend_blocks: 24,
        strata_thickness: 24,
        strata_warp_amp: 8.0,
        fault_cell: 256,
        ore_region: 32,
        tree_cell: 6,
        wind_dir: Axis::X,
        z_equator: 0,
        z_half_span: 1_024,
        edge_band: 256,
        graph: EmbeddedRef {
            name: "v1".to_owned(),
            record_version: 1,
            hash: [0; 32],
        },
        content: EmbeddedRef {
            name: "v1".to_owned(),
            record_version: 1,
            hash: [0; 32],
        },
    }
}

pub(super) fn reject(stage: Stage, key: &'static str, rule: &'static str) -> TerrainError {
    TerrainError::new(stage, ErrorKind::DescriptionInvalid { key, rule })
}

/// Rule reported when an `f32` key carries anything but eight hex digits.
pub(super) const F32_RULE: &str = "f32 must be bit-pattern hex";

impl Description {
    /// Checks ranges and finiteness. `A1.1` rejects NaN, infinity, and
    /// impossible geometry at load.
    pub(crate) fn validate(&self) -> Result<(), TerrainError> {
        let stage = Stage::Description;
        if self.schema_version != SCHEMA_VERSION {
            return Err(reject(stage, "schema_version", "unsupported"));
        }
        if self.world_height != WORLD_HEIGHT
            || !self.world_height.is_multiple_of(SECTION_SIZE)
            || self.world_height / SECTION_SIZE != SECTIONS_PER_COLUMN
        {
            return Err(reject(stage, "WORLD_HEIGHT", "layout"));
        }
        if !(0..self.world_height.cast_signed()).contains(&self.sea) {
            return Err(reject(stage, "SEA", "range"));
        }
        if self.max_terrain_y <= self.sea
            || self.max_terrain_y >= self.world_height.cast_signed() - 1
        {
            return Err(reject(stage, "MAX_TERRAIN_Y", "range"));
        }
        if self.deep_y <= 0 || self.deep_y >= self.sea {
            return Err(reject(stage, "DEEP_Y", "range"));
        }
        let Ok(tile_blocks) = u32::try_from(self.tile_blocks) else {
            return Err(reject(stage, "TILE_BLOCKS", "positive"));
        };
        let Ok(macro_cell) = u32::try_from(self.macro_cell) else {
            return Err(reject(stage, "MACRO_CELL", "positive"));
        };
        if macro_cell == 0 || !tile_blocks.is_multiple_of(macro_cell) {
            return Err(reject(stage, "MACRO_CELL", "divides TILE_BLOCKS"));
        }
        if tile_blocks == 0 || !tile_blocks.is_multiple_of(SECTION_SIZE) {
            return Err(reject(stage, "TILE_BLOCKS", "multiple of SECTION_SIZE"));
        }
        if self.max_lake_y > self.max_terrain_y {
            return Err(reject(stage, "MAX_LAKE_Y", "above MAX_TERRAIN_Y"));
        }
        if !self.lake_min_depth.is_finite() || self.lake_min_depth < 0.0 {
            return Err(reject(stage, "LAKE_MIN_DEPTH", "finite non-negative"));
        }
        if !self.w_scale.is_finite() || self.w_scale <= 0.0 {
            return Err(reject(stage, "W_SCALE", "finite positive"));
        }
        if !self.river_ramp_cells.is_finite() || self.river_ramp_cells < 0.0 {
            return Err(reject(stage, "RIVER_RAMP_CELLS", "finite non-negative"));
        }
        for (key, value) in [
            ("CAVE_T", self.cave_t),
            ("ENTRANCE_T", self.entrance_t),
            ("LAPSE", self.lapse),
            ("T_SNOW", self.t_snow),
            ("STRATA_WARP_AMP", self.strata_warp_amp),
        ] {
            if !value.is_finite() {
                return Err(reject(stage, key, "finite"));
            }
        }
        if !(0.0..=1.0).contains(&self.cave_t) {
            return Err(reject(stage, "CAVE_T", "range"));
        }
        if !(0.0..=1.0).contains(&self.entrance_t) {
            return Err(reject(stage, "ENTRANCE_T", "range"));
        }
        if !(0.0..=1.0).contains(&self.t_snow) {
            return Err(reject(stage, "T_SNOW", "range"));
        }
        if self.water_sky_atten > 15 {
            return Err(reject(stage, "WATER_SKY_ATTEN", "range"));
        }
        if self.z_half_span != self.world_half_extent {
            return Err(reject(stage, "Z_HALF_SPAN", "equals WORLD_HALF_EXTENT"));
        }
        Ok(())
    }
}

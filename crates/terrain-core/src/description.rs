//! The versioned world description: the only identity a terrain world has.
//!
//! `A1.1`, `A1.3`, `T1.2`. The description is inline, self-contained, and
//! fully hashed. Seed, platform tag, and generator version live in the world
//! header instead, because one description serves many worlds.
//!
//! Runtime settings (`VIEW_RADIUS`, LOD radii, memory caps, worker counts, fog)
//! are deliberately absent: they are not identity.
//!
//! Canonical form is struct field order, little-endian fixed-width integers,
//! `f32` as bit patterns, and length-prefixed strings. `-0.0` normalises to
//! `+0.0`; NaN and infinity are rejected rather than encoded.

use std::fmt::Write as _;
use std::path::Path;

use crate::error::{ErrorKind, Stage, TerrainError};
use crate::hash::sha256;

/// File name of the description inside a world directory.
pub const DESCRIPTION_FILE: &str = "description.toml";

/// Schema version of the description format itself.
pub const SCHEMA_VERSION: u16 = 1;

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
pub const DEEP_Y: i32 = 64;

/// Blocks per macro lattice cell (`B4.1`).
pub const MACRO_CELL: i32 = 4;

/// Blocks per field tile edge (`B4.2`).
pub const TILE_BLOCKS: i32 = 128;

/// Blocks per hydrology tile edge (`D8.1`, confirm).
pub const HYDRO_TILE: i32 = 512;

/// Halo width of a hydrology tile (`D8.1`, confirm).
pub const HYDRO_BORDER: i32 = 64;

/// Accumulated cells that start a river (`D3.1`).
pub const RIVER_MIN_CELLS: u32 = 300;

/// Accumulation scale of the river width curve (`D3.2`).
pub const W_SCALE: f32 = 0.28;

/// Accumulation over which a river ramps up from zero width (`D3.2`).
pub const RIVER_RAMP_CELLS: f32 = 50.0;

/// Minimum roof above a cave (`F2.5`).
pub const CAVE_MIN_ROOF: i32 = 4;

/// Detail band depth below the macro top (`F3.3`).
pub const DETAIL_DOWN: i32 = 24;

/// Detail band height above the macro top (`F3.3`).
pub const DETAIL_UP: i32 = 8;

/// Distance at which fog ends: `LOD2_RADIUS_BLOCKS - 64` (`O3.1`).
pub const FOG_END: i32 = LOD2_RADIUS_BLOCKS - 64;

/// LOD0 radius in blocks: 16 columns of 32 (`N1.1`).
pub const LOD0_RADIUS_BLOCKS: i32 = 512;

/// LOD1 radius in blocks (`N1.1`).
pub const LOD1_RADIUS_BLOCKS: i32 = 1_024;

/// LOD2 radius in blocks (`N1.1`).
pub const LOD2_RADIUS_BLOCKS: i32 = 2_048;

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

fn reject(stage: Stage, key: &'static str, rule: &'static str) -> TerrainError {
    TerrainError::new(stage, ErrorKind::DescriptionInvalid { key, rule })
}

/// Rule reported when an `f32` key carries anything but eight hex digits.
const F32_RULE: &str = "f32 must be bit-pattern hex";

/// Parses `description.toml` text into a [`Description`].
///
/// # Accepted language
///
/// The flat `KEY = value` subset of TOML plus one optional `[meta]` table.
/// Blank lines are ignored and `#` starts a comment, including a trailing one.
/// Keys are exactly [`KEYS`]; an unknown, duplicate, or missing key is a
/// `DescriptionInvalid` error carrying the key. Integers are plain decimal with
/// an optional leading `-`; `f32` keys are eight hex digits of a bit pattern,
/// with or without `0x` and in either case. A trailing decimal comment on an
/// `f32` value is ignored and never trusted. NaN and infinity are rejected.
pub fn parse(text: &str) -> Result<Description, TerrainError> {
    let stage = Stage::Description;
    let mut values: Vec<Option<&str>> = vec![None; KEYS.len()];
    let mut in_meta = false;
    let mut meta_seen = false;

    for raw in text.lines() {
        let line = raw.trim_start_matches('\u{feff}').trim();
        let line = line.split_once('#').map_or(line, |(head, _)| head.trim());
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let name = line[1..line.len() - 1].trim();
            if name != "meta" {
                return Err(reject(stage, leak(name), "unknown table"));
            }
            if meta_seen {
                return Err(reject(stage, "meta", "duplicate table"));
            }
            meta_seen = true;
            in_meta = true;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(reject(stage, "description.toml", "expected KEY = value"));
        };
        let key = key.trim();
        let value = value.trim();
        if in_meta {
            if key.is_empty() {
                return Err(reject(stage, "meta", "empty key"));
            }
            continue;
        }
        if key.is_empty() {
            return Err(reject(stage, "description.toml", "empty key"));
        }
        let Some(index) = KEYS.iter().position(|spec| spec.name == key) else {
            return Err(reject(stage, leak(key), "unknown key"));
        };
        if values[index].replace(value).is_some() {
            return Err(reject(stage, KEYS[index].name, "duplicate key"));
        }
    }

    for (index, spec) in KEYS.iter().enumerate() {
        if values[index].is_none() {
            return Err(reject(stage, spec.name, "missing"));
        }
    }

    let mut description = default_v1();
    for (index, spec) in KEYS.iter().enumerate() {
        let value = values[index].unwrap_or_default();
        assign(&mut description, spec.name, value)?;
    }

    description.validate()?;
    Ok(description)
}

/// Parses `description.toml` text with the strict reader used at load time.
///
/// This is the entry point for reading a description from disk without the
/// `toml` crate (see the build plan's deliberate deviation). The accepted
/// language is the flat, TOML-compatible subset the file uses:
///
/// ```text
/// WORLD_HEIGHT = 384          # decimal integer
/// CAVE_T = 0x3F000000         # f32 bits, `0x` optional, either hex case
/// GRAPH = "v1", 1, "0000…"    # double-quoted strings; bare legacy form also accepted
/// WIND_DIR = x                # bare token, `x` or `z`
/// [meta]                      # optional, unhashed, provenance only
/// preset = "v1"
/// ```
///
/// Blank lines and `#` comments (whole-line or trailing, outside strings) are
/// ignored. Keys are exactly [`KEYS`]; unknown keys, duplicate keys, missing
/// keys, unknown tables, malformed values, and unterminated strings all return
/// [`ErrorKind::DescriptionInvalid`], and no error payload copies text from the
/// file. `[meta]` has no representation in [`Description`] today: it is parsed
/// as `key = value` lines and then ignored, so it never reaches
/// [`Description::canonical_bytes`] or [`Description::description_hash`].
/// No schema key is boolean today; `true`/`false` would only occur inside
/// ignored `[meta]` values, and a boolean in a hashed key is a type error.
///
/// Every schema key is required. The returned value therefore carries only
/// values read from `text`; `default_v1` is used as scratch storage only after
/// all keys have been proven present, and every one of them is overwritten
/// before `validate` runs. No missing key falls back to a default.
///
/// # Errors
///
/// [`ErrorKind::DescriptionInvalid`] for any syntax, key, or value violation,
/// including range failures caught by [`Description::validate`].
pub fn from_toml_str(text: &str) -> Result<Description, TerrainError> {
    let values = collect(text)?;
    let mut description = default_v1();
    for (spec, value) in KEYS.iter().zip(values) {
        assign(&mut description, spec.name, value)?;
    }
    description.validate()?;
    Ok(description)
}

/// Splits the file into one value per schema key, refusing every deviation.
///
/// Returned values are comment-stripped and trimmed but otherwise unparsed;
/// `assign` performs the per-key typed conversion.
fn collect(text: &str) -> Result<Vec<&str>, TerrainError> {
    let stage = Stage::Description;
    let mut values: Vec<Option<&str>> = vec![None; KEYS.len()];
    let mut in_meta = false;
    let mut meta_seen = false;

    for raw in text.lines() {
        let Some(uncommented) = strip_comment(raw.trim()) else {
            return Err(reject(stage, "value", "unterminated string"));
        };
        let line = uncommented.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            let Some(name) = line
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(']'))
            else {
                return Err(reject(stage, "table", "expected [meta]"));
            };
            let name = name.trim();
            if name != "meta" {
                return Err(reject(stage, "table", "unknown table"));
            }
            if meta_seen {
                return Err(reject(stage, "meta", "duplicate table"));
            }
            meta_seen = true;
            in_meta = true;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(reject(stage, "description.toml", "expected KEY = value"));
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty() {
            return Err(reject(stage, "description.toml", "empty key"));
        }
        if in_meta {
            // `[meta]` is provenance, not identity. It has no field in
            // `Description` and is validated only for shape.
            continue;
        }
        let Some(index) = KEYS.iter().position(|spec| spec.name == key) else {
            return Err(reject(stage, "key", "unknown key"));
        };
        if value.is_empty() {
            return Err(reject(stage, KEYS[index].name, "empty value"));
        }
        if values[index].replace(value).is_some() {
            return Err(reject(stage, KEYS[index].name, "duplicate key"));
        }
    }

    for (index, spec) in KEYS.iter().enumerate() {
        if values[index].is_none() {
            return Err(reject(stage, spec.name, "missing"));
        }
    }
    Ok(values.into_iter().map(Option::unwrap_or_default).collect())
}

/// Cuts a trailing `#` comment, ignoring `#` inside a double-quoted string.
///
/// Returns `None` when a string is not closed, which is the only failure this
/// scan can detect. Backslash escapes `\\`, `\"`, `\n`, `\r`, and `\t` are
/// honoured; they are validated for real by [`string_literal`].
fn strip_comment(text: &str) -> Option<&str> {
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            '#' if !in_string => return Some(&text[..index]),
            _ => {}
        }
    }
    if in_string { None } else { Some(text) }
}

/// Splits `GRAPH`/`CONTENT` text on top-level commas, ignoring commas inside
/// strings. Returns `None` when a string is not closed.
fn split_outside_quotes(text: &str) -> Option<Vec<&str>> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            ',' if !in_string => {
                parts.push(&text[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if in_string {
        return None;
    }
    parts.push(&text[start..]);
    Some(parts)
}

/// Reads one string item: either a double-quoted string with `\\`, `\"`, `\n`,
/// `\r`, and `\t` escapes, or a bare legacy token with no quote characters.
fn string_literal(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if !value.starts_with('"') {
        return if value.contains('"') {
            None
        } else {
            Some(value.to_owned())
        };
    }
    let mut out = String::with_capacity(value.len());
    let mut escaped = false;
    let mut closed = false;
    for ch in value[1..].chars() {
        if closed {
            return None;
        }
        if escaped {
            match ch {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                _ => return None,
            }
            escaped = false;
        } else {
            match ch {
                '\\' => escaped = true,
                '"' => closed = true,
                other => out.push(other),
            }
        }
    }
    if escaped || !closed {
        return None;
    }
    Some(out)
}

/// Assigns one already-parsed key value to its field.
fn assign(
    description: &mut Description,
    key: &'static str,
    value: &str,
) -> Result<(), TerrainError> {
    let stage = Stage::Description;
    match key {
        "WORLD_HEIGHT" => description.world_height = decimal(value, key, false)?,
        "SEA" => description.sea = decimal(value, key, true)?,
        "MAX_TERRAIN_Y" => description.max_terrain_y = decimal(value, key, true)?,
        "DEEP_Y" => description.deep_y = decimal(value, key, true)?,
        "MACRO_CELL" => description.macro_cell = decimal(value, key, true)?,
        "TILE_BLOCKS" => description.tile_blocks = decimal(value, key, true)?,
        "HYDRO_TILE" => description.hydro_tile = decimal(value, key, true)?,
        "HYDRO_BORDER" => description.hydro_border = decimal(value, key, true)?,
        "WORLD_HALF_EXTENT" => description.world_half_extent = decimal(value, key, true)?,
        "RIVER_MIN_CELLS" => description.river_min_cells = decimal(value, key, false)?,
        "W_SCALE" => description.w_scale = f32_bits(value, key)?,
        "RIVER_RAMP_CELLS" => description.river_ramp_cells = f32_bits(value, key)?,
        "LAKE_MIN_DEPTH" => description.lake_min_depth = f32_bits(value, key)?,
        "LAKE_MIN_CELLS" => description.lake_min_cells = decimal(value, key, false)?,
        "MAX_LAKE_Y" => description.max_lake_y = decimal(value, key, true)?,
        "CAVE_MIN_ROOF" => description.cave_min_roof = decimal(value, key, true)?,
        "CAVE_T" => description.cave_t = f32_bits(value, key)?,
        "ENTRANCE_T" => description.entrance_t = f32_bits(value, key)?,
        "DETAIL_DOWN" => description.detail_down = decimal(value, key, true)?,
        "DETAIL_UP" => description.detail_up = decimal(value, key, true)?,
        "WATER_SKY_ATTEN" => description.water_sky_atten = decimal(value, key, false)?,
        "LAPSE" => description.lapse = f32_bits(value, key)?,
        "T_SNOW" => description.t_snow = f32_bits(value, key)?,
        "BLEND_BLOCKS" => description.blend_blocks = decimal(value, key, false)?,
        "STRATA_THICKNESS" => description.strata_thickness = decimal(value, key, true)?,
        "STRATA_WARP_AMP" => description.strata_warp_amp = f32_bits(value, key)?,
        "FAULT_CELL" => description.fault_cell = decimal(value, key, true)?,
        "ORE_REGION" => description.ore_region = decimal(value, key, true)?,
        "TREE_CELL" => description.tree_cell = decimal(value, key, true)?,
        "WIND_DIR" => {
            description.wind_dir = match value {
                "x" => Axis::X,
                "z" => Axis::Z,
                _ => return Err(reject(stage, key, "x or z")),
            };
        }
        "Z_EQUATOR" => description.z_equator = decimal(value, key, true)?,
        "Z_HALF_SPAN" => description.z_half_span = decimal(value, key, true)?,
        "EDGE_BAND" => description.edge_band = decimal(value, key, true)?,
        "GRAPH" => description.graph = embedded_ref(value, key)?,
        "CONTENT" => description.content = embedded_ref(value, key)?,
        other => return Err(reject(stage, leak(other), "unknown key")),
    }
    Ok(())
}

/// Reads and parses one `description.toml`.
pub fn load(path: &Path) -> Result<Description, TerrainError> {
    let text = std::fs::read_to_string(path).map_err(|error| io_error("read", path, &error))?;
    parse(&text)
}

/// Makes a file-supplied name available as a `&'static str` error payload.
///
/// `ErrorKind::DescriptionInvalid` carries a `&'static str`, while an unknown
/// key or table is arbitrary input text. This runs only on the error path, so
/// the allocation is bounded by the rejected file.
fn leak(name: &str) -> &'static str {
    Box::leak(name.to_owned().into_boxed_str())
}

/// Parses a plain decimal integer: digits with an optional leading `-`.
fn decimal<T>(value: &str, key: &'static str, signed: bool) -> Result<T, TerrainError>
where
    T: std::str::FromStr,
{
    let digits = if signed {
        value.strip_prefix('-').unwrap_or(value)
    } else {
        value
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(reject(Stage::Description, key, "integer"));
    }
    value
        .parse::<T>()
        .map_err(|_| reject(Stage::Description, key, "integer range"))
}

/// Parses an `f32` from eight hex digits of its bit pattern.
fn f32_bits(value: &str, key: &'static str) -> Result<f32, TerrainError> {
    let digits = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if digits.len() != 8 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(reject(Stage::Description, key, F32_RULE));
    }
    let bits =
        u32::from_str_radix(digits, 16).map_err(|_| reject(Stage::Description, key, F32_RULE))?;
    let number = f32::from_bits(bits);
    if !number.is_finite() {
        return Err(reject(Stage::Description, key, "finite"));
    }
    Ok(if number == 0.0 { 0.0 } else { number })
}

/// Parses `name, record_version, hash` on one line.
///
/// `name` and `hash` may be double-quoted; the legacy bare form emitted by
/// [`Description::emit`] remains accepted.
fn embedded_ref(value: &str, key: &'static str) -> Result<EmbeddedRef, TerrainError> {
    let Some(parts) = split_outside_quotes(value) else {
        return Err(reject(Stage::Description, key, "unterminated string"));
    };
    let [name, version, hash] = parts.as_slice() else {
        return Err(reject(
            Stage::Description,
            key,
            "name, record_version, hash",
        ));
    };
    let Some(name) = string_literal(name) else {
        return Err(reject(Stage::Description, key, "quoted name"));
    };
    if name.is_empty() {
        return Err(reject(Stage::Description, key, "name"));
    }
    let record_version = decimal::<u32>(version.trim(), key, false)?;
    let Some(hash) = string_literal(hash) else {
        return Err(reject(Stage::Description, key, "64 hex digits"));
    };
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(reject(Stage::Description, key, "64 hex digits"));
    }
    let mut bytes = [0_u8; 32];
    for (pair, slot) in hash
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .zip(bytes.iter_mut())
    {
        let Some(high) = hex_nibble(pair[0]) else {
            return Err(reject(Stage::Description, key, "64 hex digits"));
        };
        let Some(low) = hex_nibble(pair[1]) else {
            return Err(reject(Stage::Description, key, "64 hex digits"));
        };
        *slot = (high << 4) | low;
    }
    Ok(EmbeddedRef {
        name,
        record_version,
        hash: bytes,
    })
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn io_error(op: &'static str, path: &Path, error: &std::io::Error) -> TerrainError {
    TerrainError::new(
        Stage::Description,
        ErrorKind::IoError {
            op,
            path: path.to_path_buf(),
            os_error: error.raw_os_error().unwrap_or(0),
        },
    )
}

fn push_decimal(out: &mut String, value: impl std::fmt::Display) {
    let _ = writeln!(out, "{value}");
}

fn push_f32(out: &mut String, value: f32) {
    let bits = if value == 0.0 { 0 } else { value.to_bits() };
    let shown = f32::from_bits(bits);
    let _ = writeln!(out, "0x{bits:08X} # {shown}");
}

fn push_embedded(out: &mut String, embedded: &EmbeddedRef) {
    let mut hash = String::with_capacity(64);
    for byte in embedded.hash {
        let _ = write!(hash, "{byte:02x}");
    }
    let _ = writeln!(
        out,
        "{}, {}, {hash}",
        embedded.name, embedded.record_version
    );
}

fn push_quoted(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
}

fn push_quoted_embedded(out: &mut String, embedded: &EmbeddedRef) {
    let mut hash = String::with_capacity(64);
    for byte in embedded.hash {
        let _ = write!(hash, "{byte:02x}");
    }
    push_quoted(out, &embedded.name);
    let _ = writeln!(out, ", {}, \"{hash}\"", embedded.record_version);
}

impl Description {
    /// Renders canonical `description.toml` text.
    ///
    /// One key per line in [`KEYS`] order, integers in decimal, `f32` values as
    /// eight uppercase hex digits of their bit pattern with an ignored decimal
    /// comment, and an empty `[meta]` table last. `parse` of this text returns a
    /// description with identical [`Self::canonical_bytes`].
    #[must_use]
    pub fn emit(&self) -> String {
        let mut out = String::with_capacity(1_024);
        for spec in KEYS {
            let _ = write!(out, "{} = ", spec.name);
            match spec.name {
                "WORLD_HEIGHT" => push_decimal(&mut out, self.world_height),
                "SEA" => push_decimal(&mut out, self.sea),
                "MAX_TERRAIN_Y" => push_decimal(&mut out, self.max_terrain_y),
                "DEEP_Y" => push_decimal(&mut out, self.deep_y),
                "MACRO_CELL" => push_decimal(&mut out, self.macro_cell),
                "TILE_BLOCKS" => push_decimal(&mut out, self.tile_blocks),
                "HYDRO_TILE" => push_decimal(&mut out, self.hydro_tile),
                "HYDRO_BORDER" => push_decimal(&mut out, self.hydro_border),
                "WORLD_HALF_EXTENT" => push_decimal(&mut out, self.world_half_extent),
                "RIVER_MIN_CELLS" => push_decimal(&mut out, self.river_min_cells),
                "W_SCALE" => push_f32(&mut out, self.w_scale),
                "RIVER_RAMP_CELLS" => push_f32(&mut out, self.river_ramp_cells),
                "LAKE_MIN_DEPTH" => push_f32(&mut out, self.lake_min_depth),
                "LAKE_MIN_CELLS" => push_decimal(&mut out, self.lake_min_cells),
                "MAX_LAKE_Y" => push_decimal(&mut out, self.max_lake_y),
                "CAVE_MIN_ROOF" => push_decimal(&mut out, self.cave_min_roof),
                "CAVE_T" => push_f32(&mut out, self.cave_t),
                "ENTRANCE_T" => push_f32(&mut out, self.entrance_t),
                "DETAIL_DOWN" => push_decimal(&mut out, self.detail_down),
                "DETAIL_UP" => push_decimal(&mut out, self.detail_up),
                "WATER_SKY_ATTEN" => push_decimal(&mut out, self.water_sky_atten),
                "LAPSE" => push_f32(&mut out, self.lapse),
                "T_SNOW" => push_f32(&mut out, self.t_snow),
                "BLEND_BLOCKS" => push_decimal(&mut out, self.blend_blocks),
                "STRATA_THICKNESS" => push_decimal(&mut out, self.strata_thickness),
                "STRATA_WARP_AMP" => push_f32(&mut out, self.strata_warp_amp),
                "FAULT_CELL" => push_decimal(&mut out, self.fault_cell),
                "ORE_REGION" => push_decimal(&mut out, self.ore_region),
                "TREE_CELL" => push_decimal(&mut out, self.tree_cell),
                "WIND_DIR" => {
                    let _ = writeln!(
                        out,
                        "{}",
                        match self.wind_dir {
                            Axis::X => "x",
                            Axis::Z => "z",
                        }
                    );
                }
                "Z_EQUATOR" => push_decimal(&mut out, self.z_equator),
                "Z_HALF_SPAN" => push_decimal(&mut out, self.z_half_span),
                "EDGE_BAND" => push_decimal(&mut out, self.edge_band),
                "GRAPH" => push_embedded(&mut out, &self.graph),
                "CONTENT" => push_embedded(&mut out, &self.content),
                _ => {}
            }
        }
        out.push_str("[meta]\n");
        out
    }

    /// Renders deterministic display text for this description.
    ///
    /// Keys follow struct order, which is also [`KEYS`] order. Integers are
    /// decimal, `f32` fields are eight uppercase hex digits of their bit
    /// pattern with a `#` decimal comment, and strings are double-quoted. The
    /// text ends with exactly one newline. Comments are not hashed; the whole
    /// output is display only.
    ///
    /// There is deliberately no `[meta]` table: [`Description`] has no field
    /// for provenance, so emitting one would invent data. [`from_toml_str`]
    /// ignores `[meta]` when reading, and this text parses back to an equal
    /// description with identical [`Self::canonical_bytes`] and
    /// [`Self::description_hash`].
    #[must_use]
    pub fn to_toml_string(&self) -> String {
        let mut out = String::with_capacity(1_024);
        let _ = writeln!(out, "WORLD_HEIGHT = {}", self.world_height);
        let _ = writeln!(out, "SEA = {}", self.sea);
        let _ = writeln!(out, "MAX_TERRAIN_Y = {}", self.max_terrain_y);
        let _ = writeln!(out, "DEEP_Y = {}", self.deep_y);
        let _ = writeln!(out, "MACRO_CELL = {}", self.macro_cell);
        let _ = writeln!(out, "TILE_BLOCKS = {}", self.tile_blocks);
        let _ = writeln!(out, "HYDRO_TILE = {}", self.hydro_tile);
        let _ = writeln!(out, "HYDRO_BORDER = {}", self.hydro_border);
        let _ = writeln!(out, "WORLD_HALF_EXTENT = {}", self.world_half_extent);
        let _ = writeln!(out, "RIVER_MIN_CELLS = {}", self.river_min_cells);
        for (key, value) in [
            ("W_SCALE", self.w_scale),
            ("RIVER_RAMP_CELLS", self.river_ramp_cells),
            ("LAKE_MIN_DEPTH", self.lake_min_depth),
        ] {
            let _ = write!(out, "{key} = ");
            push_f32(&mut out, value);
        }
        let _ = writeln!(out, "LAKE_MIN_CELLS = {}", self.lake_min_cells);
        let _ = writeln!(out, "MAX_LAKE_Y = {}", self.max_lake_y);
        let _ = writeln!(out, "CAVE_MIN_ROOF = {}", self.cave_min_roof);
        for (key, value) in [("CAVE_T", self.cave_t), ("ENTRANCE_T", self.entrance_t)] {
            let _ = write!(out, "{key} = ");
            push_f32(&mut out, value);
        }
        let _ = writeln!(out, "DETAIL_DOWN = {}", self.detail_down);
        let _ = writeln!(out, "DETAIL_UP = {}", self.detail_up);
        let _ = writeln!(out, "WATER_SKY_ATTEN = {}", self.water_sky_atten);
        for (key, value) in [("LAPSE", self.lapse), ("T_SNOW", self.t_snow)] {
            let _ = write!(out, "{key} = ");
            push_f32(&mut out, value);
        }
        let _ = writeln!(out, "BLEND_BLOCKS = {}", self.blend_blocks);
        let _ = writeln!(out, "STRATA_THICKNESS = {}", self.strata_thickness);
        let _ = write!(out, "STRATA_WARP_AMP = ");
        push_f32(&mut out, self.strata_warp_amp);
        let _ = writeln!(out, "FAULT_CELL = {}", self.fault_cell);
        let _ = writeln!(out, "ORE_REGION = {}", self.ore_region);
        let _ = writeln!(out, "TREE_CELL = {}", self.tree_cell);
        let _ = writeln!(
            out,
            "WIND_DIR = {}",
            match self.wind_dir {
                Axis::X => "x",
                Axis::Z => "z",
            }
        );
        let _ = writeln!(out, "Z_EQUATOR = {}", self.z_equator);
        let _ = writeln!(out, "Z_HALF_SPAN = {}", self.z_half_span);
        let _ = writeln!(out, "EDGE_BAND = {}", self.edge_band);
        let _ = write!(out, "GRAPH = ");
        push_quoted_embedded(&mut out, &self.graph);
        let _ = write!(out, "CONTENT = ");
        push_quoted_embedded(&mut out, &self.content);
        out
    }

    /// Writes the canonical text to `path`.
    pub fn save(&self, path: &Path) -> Result<(), TerrainError> {
        std::fs::write(path, self.emit()).map_err(|error| io_error("write", path, &error))
    }

    /// Checks ranges and finiteness. `A1.1` rejects NaN, infinity, and
    /// impossible geometry at load.
    pub fn validate(&self) -> Result<(), TerrainError> {
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

    /// Canonical bytes: the input to the description hash.
    ///
    /// Field order is structure order. Strings are length-prefixed with a
    /// `u32` byte count. Floats are bit patterns with `-0.0` normalised.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, TerrainError> {
        self.validate()?;
        let mut out = Vec::with_capacity(256);
        out.extend_from_slice(&self.schema_version.to_le_bytes());
        out.extend_from_slice(&self.world_height.to_le_bytes());
        for value in [
            self.sea,
            self.max_terrain_y,
            self.deep_y,
            self.macro_cell,
            self.tile_blocks,
            self.hydro_tile,
            self.hydro_border,
            self.world_half_extent,
            self.max_lake_y,
            self.cave_min_roof,
            self.detail_down,
            self.detail_up,
            self.strata_thickness,
            self.fault_cell,
            self.ore_region,
            self.tree_cell,
            self.z_equator,
            self.z_half_span,
            self.edge_band,
        ] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&self.river_min_cells.to_le_bytes());
        out.extend_from_slice(&self.lake_min_cells.to_le_bytes());
        out.extend_from_slice(&self.blend_blocks.to_le_bytes());
        for (key, value) in [
            ("LAKE_MIN_DEPTH", self.lake_min_depth),
            ("W_SCALE", self.w_scale),
            ("RIVER_RAMP_CELLS", self.river_ramp_cells),
            ("CAVE_T", self.cave_t),
            ("ENTRANCE_T", self.entrance_t),
            ("LAPSE", self.lapse),
            ("T_SNOW", self.t_snow),
            ("STRATA_WARP_AMP", self.strata_warp_amp),
        ] {
            out.extend_from_slice(&canonical_f32(key, value)?.to_le_bytes());
        }
        out.push(self.water_sky_atten);
        out.push(match self.wind_dir {
            Axis::X => 0,
            Axis::Z => 1,
        });
        for embedded in [&self.graph, &self.content] {
            let len = u32::try_from(embedded.name.len())
                .map_err(|_| reject(Stage::Description, "GRAPH", "name length"))?;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(embedded.name.as_bytes());
            out.extend_from_slice(&embedded.record_version.to_le_bytes());
            out.extend_from_slice(&embedded.hash);
        }
        Ok(out)
    }

    /// The 32-byte world identity (`T1.2`).
    pub fn description_hash(&self) -> Result<[u8; 32], TerrainError> {
        Ok(sha256(&self.canonical_bytes()?))
    }
}

fn canonical_f32(key: &'static str, value: f32) -> Result<u32, TerrainError> {
    if !value.is_finite() {
        return Err(reject(Stage::Description, key, "finite"));
    }
    Ok(if value == 0.0 { 0 } else { value.to_bits() })
}

/// Fields the world header carries (`A1.2`).
///
/// `// OPEN(A1.2)` the exact field list is not confirmed. Seed and platform tag
/// are here rather than in the description because one description serves many
/// worlds (`A1.1`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorldHeader {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_default_is_valid() {
        default_v1().validate().unwrap();
    }

    #[test]
    fn every_schema_key_is_hashed_and_unique() {
        let mut names: Vec<&str> = KEYS.iter().map(|spec| spec.name).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count);
        assert!(KEYS.iter().all(|spec| spec.hashed));
    }

    #[test]
    fn identity_is_stable() {
        let description = default_v1();
        let hash = description.description_hash().unwrap();
        assert_eq!(
            hash,
            description.description_hash().unwrap(),
            "hashing is a pure function"
        );
        assert_eq!(hash.len(), 32);
    }

    #[test]
    fn any_value_change_changes_the_hash() {
        let base = parse(&default_v1().emit()).unwrap();
        let base = base.description_hash().unwrap();
        let emitted = default_v1().emit();
        let edits = [
            ("SEA", "126"),
            ("HYDRO_BORDER", "32"),
            ("CAVE_T", "0x3F000000"),
            ("WIND_DIR", "z"),
            ("BLEND_BLOCKS", "25"),
            (
                "GRAPH",
                "v2, 1, 0000000000000000000000000000000000000000000000000000000000000001",
            ),
        ];
        for (key, value) in edits {
            let text = replace_value(&emitted, key, value);
            let changed = parse(&text).unwrap_or_else(|error| panic!("{key}: {error}"));
            assert_ne!(
                base,
                changed.description_hash().unwrap(),
                "changing {key} must change the hash"
            );
        }
    }

    #[test]
    fn parse_emit_parse_round_trips() {
        for description in [default_v1(), non_default_description()] {
            let emitted = description.emit();
            let parsed = parse(&emitted).unwrap_or_else(|error| panic!("{emitted}\n{error}"));
            assert_eq!(parsed, description);
            assert_eq!(
                parsed.canonical_bytes().unwrap(),
                description.canonical_bytes().unwrap()
            );
            assert_eq!(
                parsed.description_hash().unwrap(),
                description.description_hash().unwrap()
            );
        }
    }

    #[test]
    fn default_emit_hash_matches_default_hash() {
        let default = default_v1();
        let parsed = parse(&default.emit()).unwrap();
        assert_eq!(parsed, default);
        assert_eq!(
            parsed.description_hash().unwrap(),
            default.description_hash().unwrap()
        );
    }

    #[test]
    fn whitespace_and_key_order_do_not_change_the_hash() {
        let base = default_v1();
        let emitted = base.emit();
        let mut lines: Vec<&str> = emitted
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('['))
            .collect();
        lines.reverse();
        lines.swap(0, 3);

        let mut text = String::from("# a reordered description\n\n");
        for line in lines {
            text.push_str("\n# a comment\n");
            text.push_str(line);
            text.push('\n');
        }
        text.push_str("\n[meta]\npreset = \"reordered\"\n");

        let parsed = parse(&text).unwrap();
        assert_eq!(
            parsed.description_hash().unwrap(),
            base.description_hash().unwrap()
        );
    }

    #[test]
    fn unknown_duplicate_and_missing_keys_are_rejected() {
        let emitted = default_v1().emit();

        let unknown = insert_before_meta(&emitted, "BOGUS = 1");
        let error = parse(&unknown).unwrap_err();
        assert_eq!(error_parts(&error), ("BOGUS", "unknown key"));

        let duplicate = insert_before_meta(&emitted, "SEA = 128");
        let error = parse(&duplicate).unwrap_err();
        assert_eq!(error_parts(&error), ("SEA", "duplicate key"));

        let missing = remove_key(&emitted, "SEA");
        let error = parse(&missing).unwrap_err();
        assert_eq!(error_parts(&error), ("SEA", "missing"));
    }

    #[test]
    fn decimal_floats_are_rejected() {
        let text = replace_value(&default_v1().emit(), "CAVE_T", "0.55");
        let error = parse(&text).unwrap_err();
        let (key, rule) = error_parts(&error);
        assert_eq!(key, "CAVE_T");
        assert!(
            rule.contains("bit-pattern"),
            "rule must mention bit patterns, found {rule:?}"
        );
    }

    #[test]
    fn nan_and_infinity_bit_patterns_are_rejected() {
        let emitted = default_v1().emit();
        for bits in ["0x7FC00000", "0x7F800000"] {
            let text = replace_value(&emitted, "CAVE_T", bits);
            let error = parse(&text).unwrap_err();
            assert_eq!(error_parts(&error), ("CAVE_T", "finite"), "bits {bits}");
        }
    }

    #[test]
    fn wrong_type_is_rejected() {
        let emitted = default_v1().emit();

        let float_as_int = replace_value(&emitted, "SEA", "0x3F800000");
        let error = parse(&float_as_int).unwrap_err();
        assert_eq!(error_parts(&error), ("SEA", "integer"));

        let int_as_float = replace_value(&emitted, "CAVE_T", "1");
        let error = parse(&int_as_float).unwrap_err();
        assert_eq!(error_parts(&error), ("CAVE_T", F32_RULE));
    }

    #[test]
    fn meta_is_provenance_only() {
        let base = default_v1().description_hash().unwrap();
        let emitted = default_v1().emit();
        let first = emitted.replace("[meta]\n", "[meta]\npreset = \"v1\"\n");
        let second = first.replace("preset = \"v1\"", "preset = \"v2\"\ntool = \"terrain-gen\"");
        assert_eq!(
            parse(&first).unwrap().description_hash().unwrap(),
            base,
            "[meta] must not be hashed"
        );
        assert_eq!(
            parse(&second).unwrap().description_hash().unwrap(),
            base,
            "changing [meta] must not change identity"
        );
    }

    #[test]
    fn load_and_save_round_trip() {
        let path = std::env::temp_dir().join(format!(
            "engine-description-{}-load_and_save_round_trip.toml",
            std::process::id()
        ));
        let description = default_v1();
        description.save(&path).unwrap();
        let loaded = load(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(loaded, description);
        assert_eq!(
            loaded.description_hash().unwrap(),
            description.description_hash().unwrap()
        );
    }

    #[test]
    fn toml_string_round_trips_to_the_same_identity() {
        let description = non_default_description();
        let text = description.to_toml_string();
        assert!(text.ends_with('\n'), "one trailing newline");
        assert!(!text.ends_with("\n\n"), "exactly one trailing newline");
        assert!(
            !text.contains("[meta]"),
            "no unrepresented [meta] table is invented"
        );
        assert_eq!(
            text,
            description.to_toml_string(),
            "emission is deterministic"
        );

        let parsed = from_toml_str(&text).unwrap_or_else(|error| panic!("{text}\n{error}"));
        assert_eq!(parsed, description);
        assert_eq!(
            parsed.canonical_bytes().unwrap(),
            description.canonical_bytes().unwrap()
        );
        assert_eq!(
            parsed.description_hash().unwrap(),
            description.description_hash().unwrap()
        );

        // The shared emitter shape stays readable by the original `parse` too.
        assert_eq!(parse(&text).unwrap(), description);
    }

    #[test]
    fn quoted_names_with_punctuation_round_trip() {
        let mut description = default_v1();
        description.graph.name = "v1, graph # 1 \"quoted\" \\ tail".to_owned();
        let text = description.to_toml_string();
        let parsed = from_toml_str(&text).unwrap_or_else(|error| panic!("{text}\n{error}"));
        assert_eq!(parsed.graph.name, description.graph.name);
        assert_eq!(
            parsed.description_hash().unwrap(),
            description.description_hash().unwrap()
        );
    }

    #[test]
    fn reformatting_does_not_change_the_hash() {
        let description = non_default_description();
        let base_hash = description.description_hash().unwrap();

        let emitted = description.to_toml_string();
        let mut lines: Vec<&str> = emitted.lines().collect();
        lines.reverse();
        let mut text = String::from("# reordered, respaced, commented\n\n");
        for line in lines {
            let (key, value) = line.split_once(" = ").unwrap();
            let value = if let Some(digits) = value.strip_prefix("0x") {
                format!("0X{}", digits.to_lowercase())
            } else {
                value.to_owned()
            };
            let _ = write!(text, "  {key}\t=\t{value}   # trailing note\n\n");
        }
        text.push_str("[meta]\npreset = \"reordered\"\ntool = \"terrain-gen\"\n");

        let parsed = from_toml_str(&text).unwrap_or_else(|error| panic!("{text}\n{error}"));
        assert_eq!(parsed.description_hash().unwrap(), base_hash);
        assert_eq!(
            parsed.canonical_bytes().unwrap(),
            description.canonical_bytes().unwrap()
        );
    }

    #[test]
    fn one_f32_bit_changes_the_hash_and_the_emitted_key() {
        let base = non_default_description();
        let mut changed = base.clone();
        changed.cave_t = f32::from_bits(base.cave_t.to_bits() ^ 1);

        assert_ne!(
            base.description_hash().unwrap(),
            changed.description_hash().unwrap()
        );
        assert_ne!(
            line_for(&base.to_toml_string(), "CAVE_T"),
            line_for(&changed.to_toml_string(), "CAVE_T"),
            "describe-diff style output must show the changed key"
        );
    }

    #[test]
    fn strict_reader_rejects_and_never_echoes_file_text() {
        let base = non_default_description().to_toml_string();

        let unknown = format!("{base}TOPAZ = 1\n");
        let error = from_toml_str(&unknown).unwrap_err();
        assert_eq!(error_parts(&error), ("key", "unknown key"));
        assert!(!error.to_string().contains("TOPAZ"));

        let duplicate = format!("{base}SEA = 120\n");
        let error = from_toml_str(&duplicate).unwrap_err();
        assert_eq!(error_parts(&error), ("SEA", "duplicate key"));

        let missing = remove_key(&base, "CAVE_T");
        let error = from_toml_str(&missing).unwrap_err();
        assert_eq!(error_parts(&error), ("CAVE_T", "missing"));

        let bad_hex = replace_value(&base, "CAVE_T", "0xZZ");
        let error = from_toml_str(&bad_hex).unwrap_err();
        assert_eq!(error_parts(&error), ("CAVE_T", F32_RULE));
        assert!(!error.to_string().contains("0xZZ"));

        let unterminated = replace_value(&base, "GRAPH", "\"v2, 2, deadbeef");
        let error = from_toml_str(&unterminated).unwrap_err();
        assert_eq!(error_parts(&error), ("value", "unterminated string"));
        assert!(!error.to_string().contains("deadbeef"));

        let extra_table = format!("{base}[graph]\nname = \"other\"\n");
        let error = from_toml_str(&extra_table).unwrap_err();
        assert_eq!(error_parts(&error), ("table", "unknown table"));
        assert!(!error.to_string().contains("[graph]"));
    }

    #[test]
    fn golden_fixture_with_every_field_parses() {
        let fixture = r#"
# every hashed key, hand written, with an unhashed provenance table
WORLD_HEIGHT = 384
SEA = 120
MAX_TERRAIN_Y = 320
DEEP_Y = 60
MACRO_CELL = 8
TILE_BLOCKS = 256
HYDRO_TILE = 1024
HYDRO_BORDER = 32
WORLD_HALF_EXTENT = 2048
RIVER_MIN_CELLS = 400
W_SCALE = 0x3E9EB852   # 0.31
RIVER_RAMP_CELLS = 0x42800000   # 64.0
LAKE_MIN_DEPTH = 0x40000000   # 2.0
LAKE_MIN_CELLS = 20
MAX_LAKE_Y = 300
CAVE_MIN_ROOF = 6
CAVE_T = 0x3F000000   # 0.5
ENTRANCE_T = 0x3F400000   # 0.75
DETAIL_DOWN = 20
DETAIL_UP = 10
WATER_SKY_ATTEN = 2
LAPSE = 0x3B6A0EA1   # 0.0035714288
T_SNOW = 0x3E99999A   # 0.3
BLEND_BLOCKS = 20
STRATA_THICKNESS = 20
STRATA_WARP_AMP = 0x40C00000   # 6.0
FAULT_CELL = 128
ORE_REGION = 24
TREE_CELL = 5
WIND_DIR = z
Z_EQUATOR = 16
Z_HALF_SPAN = 2048
EDGE_BAND = 128
GRAPH = "v2", 2, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
CONTENT = "v3", 3, "5555555555555555555555555555555555555555555555555555555555555555"

[meta]
preset = "golden, every field"
provenance = "hand edited; a # inside a string is not a comment"
"#;

        let parsed = from_toml_str(fixture).unwrap_or_else(|error| panic!("{error}"));
        let expected = non_default_description();
        assert_eq!(parsed, expected);
        assert_eq!(
            parsed.description_hash().unwrap(),
            expected.description_hash().unwrap()
        );
    }

    fn line_for<'a>(text: &'a str, key: &str) -> &'a str {
        let prefix = format!("{key} = ");
        text.lines()
            .find(|line| line.starts_with(&prefix))
            .unwrap_or_default()
    }

    fn non_default_description() -> Description {
        // `schema_version` and `world_height` are pinned by `validate`, so this
        // changes every value that can legally differ from v1.
        Description {
            sea: 120,
            max_terrain_y: 320,
            deep_y: 60,
            macro_cell: 8,
            tile_blocks: 256,
            hydro_tile: 1_024,
            hydro_border: 32,
            world_half_extent: 2_048,
            river_min_cells: 400,
            w_scale: 0.31,
            river_ramp_cells: 64.0,
            lake_min_depth: 2.0,
            lake_min_cells: 20,
            max_lake_y: 300,
            cave_min_roof: 6,
            cave_t: 0.5,
            entrance_t: 0.75,
            detail_down: 20,
            detail_up: 10,
            water_sky_atten: 2,
            lapse: 1.0 / 280.0,
            t_snow: 0.3,
            blend_blocks: 20,
            strata_thickness: 20,
            strata_warp_amp: 6.0,
            fault_cell: 128,
            ore_region: 24,
            tree_cell: 5,
            wind_dir: Axis::Z,
            z_equator: 16,
            z_half_span: 2_048,
            edge_band: 128,
            graph: EmbeddedRef {
                name: "v2".to_owned(),
                record_version: 2,
                hash: [0xAA; 32],
            },
            content: EmbeddedRef {
                name: "v3".to_owned(),
                record_version: 3,
                hash: [0x55; 32],
            },
            ..default_v1()
        }
    }

    fn error_parts(error: &TerrainError) -> (&'static str, &'static str) {
        match &error.kind {
            ErrorKind::DescriptionInvalid { key, rule } => (key, rule),
            other => panic!("expected DescriptionInvalid, found {other:?}"),
        }
    }

    fn replace_value(text: &str, key: &str, value: &str) -> String {
        let prefix = format!("{key} = ");
        let mut replaced = false;
        let mut out = String::with_capacity(text.len());
        for line in text.lines() {
            if line.starts_with(&prefix) {
                out.push_str(key);
                out.push_str(" = ");
                out.push_str(value);
                replaced = true;
            } else {
                out.push_str(line);
            }
            out.push('\n');
        }
        assert!(replaced, "key {key} is not present");
        out
    }

    fn insert_before_meta(text: &str, line: &str) -> String {
        text.replacen("[meta]", &format!("{line}\n[meta]"), 1)
    }

    fn remove_key(text: &str, key: &str) -> String {
        let prefix = format!("{key} = ");
        let mut removed = false;
        let mut out = String::with_capacity(text.len());
        for line in text.lines() {
            if line.starts_with(&prefix) {
                removed = true;
            } else {
                out.push_str(line);
                out.push('\n');
            }
        }
        assert!(removed, "key {key} is not present");
        out
    }

    #[test]
    fn negative_zero_normalises() {
        let mut positive = default_v1();
        positive.lake_min_depth = 0.0;
        let mut negative = default_v1();
        negative.lake_min_depth = -0.0;
        assert_eq!(
            positive.description_hash().unwrap(),
            negative.description_hash().unwrap(),
            "-0.0 must not be a distinct identity"
        );

        let mut non_zero = default_v1();
        non_zero.lake_min_depth = f32::from_bits(1);
        assert_ne!(
            positive.description_hash().unwrap(),
            non_zero.description_hash().unwrap()
        );
    }

    #[test]
    fn non_finite_values_are_rejected() {
        let mut description = default_v1();
        description.cave_t = f32::NAN;
        assert!(matches!(
            description.canonical_bytes(),
            Err(TerrainError {
                kind: ErrorKind::DescriptionInvalid { key: "CAVE_T", .. },
                ..
            })
        ));
        description = default_v1();
        description.t_snow = f32::INFINITY;
        assert!(description.canonical_bytes().is_err());
    }

    #[test]
    fn impossible_geometry_is_rejected() {
        let mut description = default_v1();
        description.macro_cell = 3;
        assert!(description.validate().is_err());
        description = default_v1();
        description.max_terrain_y = description.world_height.cast_signed();
        assert!(description.validate().is_err());
        description = default_v1();
        description.z_half_span = 7;
        assert!(description.validate().is_err());
    }
}

//! The description parser and canonical text emitter.

use std::fmt::Write as _;
use std::path::Path;

use crate::error::{ErrorKind, Stage, TerrainError};

use super::schema::{Axis, Description, EmbeddedRef, F32_RULE, KEYS, default_v1, reject};

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
}

//! Tests for the description schema, parser, and hash.
//!
//! These were the `tests` module of `description.rs`; every test and assertion is
//! preserved by the split.

use std::fmt::Write as _;

use super::schema::F32_RULE;
use super::*;
use crate::error::{ErrorKind, TerrainError};

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

#[test]
fn default_v1_hash_is_pinned() {
    let hash = default_v1().description_hash().unwrap();
    let hex = hash.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    });
    assert_eq!(
        hex, "bfa2be461ffe35ad6f765c1cec0929ed1d239f07e7719b8f1d92bb94b551f02e",
        "pinned identity; changing it changes every world"
    );
}

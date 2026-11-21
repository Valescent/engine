use std::fmt::Write as _;
use std::path::Path;

use terrain_core::description::{self, Description};
use terrain_core::error::{ErrorKind, TerrainError};

use crate::cli::{Options, parse_pairs};
use crate::output::{CliFailure, EXIT_INVARIANT, EXIT_OK, hex32, json_str, report};

// ---------------------------------------------------------------------------
// describe / describe-diff
// ---------------------------------------------------------------------------

struct DescribeWriteOutcome {
    file: String,
    hash: [u8; 32],
}

pub(crate) fn cmd_describe(args: &[String], options: &Options) -> u8 {
    match describe_write(args) {
        Ok(outcome) => {
            print_describe(&outcome, options.json);
            EXIT_OK
        }
        Err(failure) => report(&failure, options.json, "describe"),
    }
}

fn describe_write(args: &[String]) -> Result<DescribeWriteOutcome, CliFailure> {
    let pairs = parse_pairs(args)?;
    let file = pairs
        .get("out")
        .ok_or_else(|| CliFailure::Usage("describe needs --out <path>".to_owned()))?;
    let description = description::default_v1();
    let hash = description.description_hash()?;
    description.save(Path::new(file))?;
    Ok(DescribeWriteOutcome {
        file: file.clone(),
        hash,
    })
}

fn print_describe(outcome: &DescribeWriteOutcome, json: bool) {
    let digest = hex32(&outcome.hash);
    if json {
        println!(
            "{{\"subcommand\":\"describe\",\"file\":{},\"description_hash\":{}}}",
            json_str(&outcome.file),
            json_str(&digest)
        );
    } else {
        println!("file {}", outcome.file);
        println!("description_hash {digest}");
    }
}

struct KeyChange {
    key: &'static str,
    v1: String,
    file: String,
}

struct DescribeDiffOutcome {
    file: String,
    hash: [u8; 32],
    default_hash: [u8; 32],
    changes: Vec<KeyChange>,
}

pub(crate) fn cmd_describe_diff(args: &[String], options: &Options) -> u8 {
    match describe_diff_outcome(args) {
        Ok(outcome) => {
            print_describe_diff(&outcome, options.json);
            if outcome.hash == outcome.default_hash {
                EXIT_OK
            } else {
                EXIT_INVARIANT
            }
        }
        Err(failure) => report(&failure, options.json, "describe-diff"),
    }
}

fn describe_diff_outcome(args: &[String]) -> Result<DescribeDiffOutcome, CliFailure> {
    let pairs = parse_pairs(args)?;
    let file = pairs
        .get("file")
        .ok_or_else(|| CliFailure::Usage("describe-diff needs --file <path>".to_owned()))?;
    let description = description::load(Path::new(file)).map_err(describe_failure)?;
    let default = description::default_v1();
    let hash = description.description_hash()?;
    let default_hash = default.description_hash()?;
    Ok(DescribeDiffOutcome {
        file: file.clone(),
        hash,
        default_hash,
        changes: changed_keys(&default, &description),
    })
}

/// A `DescriptionInvalid` parse failure exits as a usage error and carries the
/// key and rule; every other [`TerrainError`] keeps the terrain exit code.
fn describe_failure(error: TerrainError) -> CliFailure {
    match error {
        TerrainError {
            kind: ErrorKind::DescriptionInvalid { key, rule },
            ..
        } => CliFailure::Description { key, rule },
        other => CliFailure::Terrain(Box::new(other)),
    }
}

fn changed_keys(default: &Description, file: &Description) -> Vec<KeyChange> {
    let mut changes = Vec::new();
    for spec in description::KEYS {
        let (v1, loaded) = match spec.name {
            "WORLD_HEIGHT" => text_pair(default.world_height, file.world_height),
            "SEA" => text_pair(default.sea, file.sea),
            "MAX_TERRAIN_Y" => text_pair(default.max_terrain_y, file.max_terrain_y),
            "DEEP_Y" => text_pair(default.deep_y, file.deep_y),
            "MACRO_CELL" => text_pair(default.macro_cell, file.macro_cell),
            "TILE_BLOCKS" => text_pair(default.tile_blocks, file.tile_blocks),
            "HYDRO_TILE" => text_pair(default.hydro_tile, file.hydro_tile),
            "HYDRO_BORDER" => text_pair(default.hydro_border, file.hydro_border),
            "WORLD_HALF_EXTENT" => text_pair(default.world_half_extent, file.world_half_extent),
            "RIVER_MIN_CELLS" => text_pair(default.river_min_cells, file.river_min_cells),
            "W_SCALE" => (float_text(default.w_scale), float_text(file.w_scale)),
            "RIVER_RAMP_CELLS" => (
                float_text(default.river_ramp_cells),
                float_text(file.river_ramp_cells),
            ),
            "LAKE_MIN_DEPTH" => (
                float_text(default.lake_min_depth),
                float_text(file.lake_min_depth),
            ),
            "LAKE_MIN_CELLS" => text_pair(default.lake_min_cells, file.lake_min_cells),
            "MAX_LAKE_Y" => text_pair(default.max_lake_y, file.max_lake_y),
            "CAVE_MIN_ROOF" => text_pair(default.cave_min_roof, file.cave_min_roof),
            "CAVE_T" => (float_text(default.cave_t), float_text(file.cave_t)),
            "ENTRANCE_T" => (float_text(default.entrance_t), float_text(file.entrance_t)),
            "DETAIL_DOWN" => text_pair(default.detail_down, file.detail_down),
            "DETAIL_UP" => text_pair(default.detail_up, file.detail_up),
            "WATER_SKY_ATTEN" => text_pair(default.water_sky_atten, file.water_sky_atten),
            "LAPSE" => (float_text(default.lapse), float_text(file.lapse)),
            "T_SNOW" => (float_text(default.t_snow), float_text(file.t_snow)),
            "BLEND_BLOCKS" => text_pair(default.blend_blocks, file.blend_blocks),
            "STRATA_THICKNESS" => text_pair(default.strata_thickness, file.strata_thickness),
            "STRATA_WARP_AMP" => (
                float_text(default.strata_warp_amp),
                float_text(file.strata_warp_amp),
            ),
            "FAULT_CELL" => text_pair(default.fault_cell, file.fault_cell),
            "ORE_REGION" => text_pair(default.ore_region, file.ore_region),
            "TREE_CELL" => text_pair(default.tree_cell, file.tree_cell),
            "WIND_DIR" => (
                axis_text(default.wind_dir).to_owned(),
                axis_text(file.wind_dir).to_owned(),
            ),
            "Z_EQUATOR" => text_pair(default.z_equator, file.z_equator),
            "Z_HALF_SPAN" => text_pair(default.z_half_span, file.z_half_span),
            "EDGE_BAND" => text_pair(default.edge_band, file.edge_band),
            "GRAPH" => (embedded_text(&default.graph), embedded_text(&file.graph)),
            "CONTENT" => (
                embedded_text(&default.content),
                embedded_text(&file.content),
            ),
            _ => continue,
        };
        if v1 != loaded {
            changes.push(KeyChange {
                key: spec.name,
                v1,
                file: loaded,
            });
        }
    }
    changes
}

fn text_pair<T: std::fmt::Display>(v1: T, file: T) -> (String, String) {
    (v1.to_string(), file.to_string())
}

fn axis_text(axis: description::Axis) -> &'static str {
    match axis {
        description::Axis::X => "x",
        description::Axis::Z => "z",
    }
}

fn float_text(value: f32) -> String {
    let bits = if value == 0.0 { 0 } else { value.to_bits() };
    format!("0x{bits:08X}")
}

fn embedded_text(embedded: &description::EmbeddedRef) -> String {
    format!(
        "{}, {}, {}",
        embedded.name,
        embedded.record_version,
        hex32(&embedded.hash)
    )
}

fn print_describe_diff(outcome: &DescribeDiffOutcome, json: bool) {
    let digest = hex32(&outcome.hash);
    let default_digest = hex32(&outcome.default_hash);
    let default_shape = outcome.changes.is_empty();
    if json {
        let mut out = format!(
            "{{\"subcommand\":\"describe-diff\",\"file\":{},\"description_hash\":{},\"default_hash\":{},\"v1_default_shape\":{},\"changed\":[",
            json_str(&outcome.file),
            json_str(&digest),
            json_str(&default_digest),
            default_shape
        );
        for (index, change) in outcome.changes.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let _ = write!(
                out,
                "{{\"key\":{},\"v1\":{},\"file\":{}}}",
                json_str(change.key),
                json_str(&change.v1),
                json_str(&change.file)
            );
        }
        out.push_str("]}");
        println!("{out}");
    } else {
        println!("file {}", outcome.file);
        println!("description_hash {digest}");
        println!("default_hash {default_digest}");
        if default_shape {
            println!("v1_default_shape yes");
        } else {
            println!("v1_default_shape no");
            for change in &outcome.changes {
                println!(
                    "changed {} v1={} file={}",
                    change.key, change.v1, change.file
                );
            }
        }
    }
}

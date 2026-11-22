use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use terrain_core::description;
use terrain_core::hash::sha256;
use terrain_core::lattice::floor_div;
use terrain_core::pipeline::Terrain;

use crate::cli::{Options, parse_pairs};
use crate::output::{
    CliFailure, EXIT_INVARIANT, EXIT_OK, Json, expect_object, hex_bytes, hex32, integer_field,
    json_str, read_json, report, string_field,
};
use crate::{SECTION_BLOCKS, SHIPPED_GENERATOR_VERSION, baseline_hash, column_bytes};

// ---------------------------------------------------------------------------
// replay
// ---------------------------------------------------------------------------

/// Manifest file name in a report bundle directory.
const MANIFEST_FILE: &str = "manifest.json";
/// Per-stage hash file name in a report bundle directory.
const STAGES_FILE: &str = "stages.json";

/// The failure record `replay` needs from `manifest.json`.
///
/// The §19 report bundle is one zip. This build has no zip dependency and must
/// not add one, so `replay` accepts the same files laid out in a directory (or
/// the `manifest.json` inside one) and reads them with the parser above.
///
/// `manifest.json`:
///
/// ```json
/// {
///   "generator_version": 4,
///   "seed": 8576271745415857709,
///   "description_hash": "<64 or 32 lowercase hex digits>",
///   "platform_tag": "x86_64-pc-windows-msvc",
///   "failure": { "column": [5, 9], "stage": "column" }
/// }
/// ```
///
/// `column` is the failing column's `(x, z)`; three entries are accepted and the
/// middle one is ignored. `column` and `stage` may also sit at the top level.
/// `failure.stage` names the first stage to inspect but does not replace the
/// list in `stages.json`.
///
/// `stages.json`:
///
/// ```json
/// {
///   "column": [5, 9],
///   "stages": [
///     { "stage": "column",    "hash": "<64 hex digits>" },
///     { "stage": "section:4", "hash": "<16 hex digits>" }
///   ]
/// }
/// ```
///
/// `column` is optional and must agree with the manifest when present. Stages
/// are compared in array order and the first difference ends the replay. This
/// build supports:
///
/// * `column` — `SHA-256` over the 384 block ids of the column, two
///   little-endian bytes each, `y = 0` first; 64 hex digits.
/// * `section:<y>` — the pinned [`baseline_hash`] of the vertical section `y`
///   (`0..12`) containing the column; 64 hex digits.
///
/// A generator version this build does not ship is refused. A differing
/// `platform_tag` is a warning on stderr, never a failure.
struct ReplayManifest {
    seed: u64,
    generator: u16,
    description_hash: String,
    platform: String,
    column: (i32, i32),
    stage: Option<String>,
}

struct ReplayStage {
    name: String,
    hash: String,
    ok: bool,
}

struct ReplayMismatch {
    stage: String,
    expected: String,
    actual: String,
}

struct ReplayOutcome {
    seed: u64,
    column: (i32, i32),
    platform: String,
    failure_stage: Option<String>,
    warnings: Vec<String>,
    declared_stages: usize,
    stages: Vec<ReplayStage>,
    mismatch: Option<ReplayMismatch>,
}

pub(crate) fn cmd_replay(args: &[String], options: &Options) -> u8 {
    match replay_outcome(args) {
        Ok(outcome) => {
            print_replay(&outcome, options.json);
            if outcome.mismatch.is_some() {
                EXIT_INVARIANT
            } else {
                EXIT_OK
            }
        }
        Err(failure) => report(&failure, options.json, "replay"),
    }
}

fn read_replay_manifest(path: &Path) -> Result<ReplayManifest, CliFailure> {
    let manifest = read_json(path)?;
    let object = expect_object(&manifest, MANIFEST_FILE)?;
    let failure = object.get("failure").and_then(Json::as_object);
    let column_value = failure
        .and_then(|failure| failure.get("column"))
        .or_else(|| object.get("column"))
        .ok_or_else(|| CliFailure::Usage("manifest.json: missing failing column".to_owned()))?;
    let stage_value = failure
        .and_then(|failure| failure.get("stage"))
        .or_else(|| object.get("stage"));
    let stage = stage_value
        .map(|value| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                CliFailure::Usage("manifest.json: failing stage must be a string".to_owned())
            })
        })
        .transpose()?;
    Ok(ReplayManifest {
        seed: integer_field::<u64>(object, "seed", MANIFEST_FILE)?,
        generator: integer_field::<u16>(object, "generator_version", MANIFEST_FILE)?,
        description_hash: string_field(object, "description_hash", MANIFEST_FILE)?,
        platform: string_field(object, "platform_tag", MANIFEST_FILE)?,
        column: parse_column(column_value, MANIFEST_FILE)?,
        stage,
    })
}

fn replay_outcome(args: &[String]) -> Result<ReplayOutcome, CliFailure> {
    let pairs = parse_pairs(args)?;
    let report = pairs
        .get("report")
        .ok_or_else(|| CliFailure::Usage("replay needs --report <dir|manifest.json>".to_owned()))?;
    let path = Path::new(report);
    let (directory, manifest_path) = if path.is_dir() {
        (path.to_path_buf(), path.join(MANIFEST_FILE))
    } else {
        let directory = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        (directory, path.to_path_buf())
    };

    let manifest = read_replay_manifest(&manifest_path)?;
    let (seed, column) = (manifest.seed, manifest.column);

    if manifest.generator != SHIPPED_GENERATOR_VERSION {
        return Err(CliFailure::Invariant(format!(
            "generator version {} is not shipped by this build (ships {SHIPPED_GENERATOR_VERSION})",
            manifest.generator
        )));
    }
    let computed = description::default_v1().description_hash()?;
    check_bundle_description(&manifest.description_hash, &computed)?;

    let mut warnings = Vec::new();
    let current_platform = current_platform_tag();
    if manifest.platform != current_platform {
        warnings.push(format!(
            "bundle platform_tag {:?} differs from this build {current_platform:?}",
            manifest.platform
        ));
    }

    let stages_json = read_json(&directory.join(STAGES_FILE))?;
    let stages_object = expect_object(&stages_json, STAGES_FILE)?;
    if let Some(value) = stages_object.get("column") {
        let stage_column = parse_column(value, STAGES_FILE)?;
        if stage_column != column {
            return Err(CliFailure::Usage(
                "stages.json: column differs from manifest.json".to_owned(),
            ));
        }
    }
    let entries = stages_object
        .get("stages")
        .and_then(Json::as_array)
        .ok_or_else(|| CliFailure::Usage("stages.json: missing \"stages\" array".to_owned()))?;
    if entries.is_empty() {
        return Err(CliFailure::Usage("stages.json: lists no stages".to_owned()));
    }

    let mut names = BTreeSet::new();
    let mut terrain = None;
    let mut stages = Vec::new();
    let mut mismatch = None;
    for entry in entries {
        let entry = expect_object(entry, STAGES_FILE)?;
        let name = string_field(entry, "stage", STAGES_FILE)?;
        let expected = string_field(entry, "hash", STAGES_FILE)?;
        if !names.insert(name.clone()) {
            return Err(CliFailure::Usage(format!(
                "stages.json: repeats stage {name:?}"
            )));
        }
        if !matches!(expected.len(), 16 | 64)
            || !expected.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(CliFailure::Usage(format!(
                "stages.json: stage {name:?} hash must be 16 or 64 hex digits"
            )));
        }
        let actual = replay_stage_hash(&mut terrain, seed, column, &name)?;
        let ok = expected.eq_ignore_ascii_case(&actual);
        stages.push(ReplayStage {
            name: name.clone(),
            hash: actual.clone(),
            ok,
        });
        if !ok {
            mismatch = Some(ReplayMismatch {
                stage: name,
                expected,
                actual,
            });
            break;
        }
    }

    Ok(ReplayOutcome {
        seed,
        column,
        platform: manifest.platform,
        failure_stage: manifest.stage,
        warnings,
        declared_stages: entries.len(),
        stages,
        mismatch,
    })
}

fn check_bundle_description(stored: &str, computed: &[u8; 32]) -> Result<(), CliFailure> {
    let stored = stored.to_ascii_lowercase();
    if !matches!(stored.len(), 32 | 64) || !stored.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CliFailure::Usage(
            "manifest.json: description_hash must be 32 or 64 hex digits".to_owned(),
        ));
    }
    let agrees = if stored.len() == 64 {
        stored == hex32(computed)
    } else {
        stored == hex_bytes(&computed[..16])
    };
    if agrees {
        Ok(())
    } else {
        Err(CliFailure::Invariant(format!(
            "description hash mismatch: bundle {stored} build {}",
            hex32(computed)
        )))
    }
}

fn replay_stage_hash(
    terrain: &mut Option<Terrain>,
    seed: u64,
    column: (i32, i32),
    stage: &str,
) -> Result<String, CliFailure> {
    if terrain.is_none() {
        *terrain = Some(Terrain::new(seed)?);
    }
    let Some(generator) = terrain.as_ref() else {
        return Err(CliFailure::Usage(
            "internal: replay generator is missing".to_owned(),
        ));
    };
    if stage == "column" {
        let blocks = generator.column(column.0, column.1)?;
        return Ok(hex32(&sha256(&column_bytes(&blocks))));
    }
    let Some(index) = stage.strip_prefix("section:") else {
        return Err(CliFailure::Usage(format!(
            "unsupported replay stage {stage:?}; expected column or section:<y>"
        )));
    };
    let section_y = index.parse::<i32>().map_err(|_| {
        CliFailure::Usage(format!(
            "invalid replay stage {stage:?}; expected section:<0..11>"
        ))
    })?;
    let section_blocks = u32::try_from(SECTION_BLOCKS).unwrap_or(32);
    let section_count = i32::try_from(description::WORLD_HEIGHT / section_blocks).unwrap_or(12);
    if !(0..section_count).contains(&section_y) {
        return Err(CliFailure::Usage(format!(
            "invalid replay stage {stage:?}; expected section:<0..{}>",
            section_count - 1
        )));
    }
    let section = generator.section(
        floor_div(column.0, SECTION_BLOCKS),
        section_y,
        floor_div(column.1, SECTION_BLOCKS),
    )?;
    Ok(hex32(&baseline_hash(&section)))
}

fn current_platform_tag() -> String {
    format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS)
}

fn print_replay(outcome: &ReplayOutcome, json: bool) {
    for warning in &outcome.warnings {
        eprintln!("warning: {warning}");
    }
    if json {
        let failure_stage = outcome
            .failure_stage
            .as_ref()
            .map_or_else(|| "null".to_owned(), |stage| json_str(stage));
        let mut out = format!(
            "{{\"subcommand\":\"replay\",\"seed\":{},\"column\":[{},{}],\"platform_tag\":{},\"failure_stage\":{failure_stage},\"stages\":{},\"results\":[",
            outcome.seed,
            outcome.column.0,
            outcome.column.1,
            json_str(&outcome.platform),
            outcome.declared_stages
        );
        for (index, stage) in outcome.stages.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let _ = write!(
                out,
                "{{\"stage\":{},\"hash\":{},\"ok\":{}}}",
                json_str(&stage.name),
                json_str(&stage.hash),
                stage.ok
            );
        }
        out.push(']');
        if let Some(mismatch) = &outcome.mismatch {
            let _ = write!(
                out,
                ",\"ok\":false,\"mismatch\":{{\"stage\":{},\"expected\":{},\"actual\":{}}}",
                json_str(&mismatch.stage),
                json_str(&mismatch.expected),
                json_str(&mismatch.actual)
            );
        } else {
            out.push_str(",\"ok\":true");
        }
        out.push('}');
        println!("{out}");
    } else {
        println!("seed {}", outcome.seed);
        println!("column {} {}", outcome.column.0, outcome.column.1);
        println!("platform {}", outcome.platform);
        if let Some(stage) = &outcome.failure_stage {
            println!("failure stage {stage}");
        }
        println!("stages {}", outcome.declared_stages);
        for stage in &outcome.stages {
            if stage.ok {
                println!("stage {} ok {}", stage.name, stage.hash);
            } else {
                println!("stage {} differ", stage.name);
            }
        }
        match &outcome.mismatch {
            Some(mismatch) => {
                println!("expected {}", mismatch.expected);
                println!("actual {}", mismatch.actual);
            }
            None => println!("ok"),
        }
    }
}

fn int_value(value: &Json, file: &str) -> Result<i32, CliFailure> {
    let Json::Number(raw) = value else {
        return Err(CliFailure::Usage(format!(
            "{file}: column entries must be integers"
        )));
    };
    raw.parse::<i32>()
        .map_err(|_| CliFailure::Usage(format!("{file}: invalid coordinate {raw:?}")))
}

fn parse_column(value: &Json, file: &str) -> Result<(i32, i32), CliFailure> {
    let values = value
        .as_array()
        .ok_or_else(|| CliFailure::Usage(format!("{file}: column must be [x, z] or [x, y, z]")))?;
    match values {
        [x, z] | [x, _, z] => Ok((int_value(x, file)?, int_value(z, file)?)),
        _ => Err(CliFailure::Usage(format!(
            "{file}: column must be [x, z] or [x, y, z]"
        ))),
    }
}

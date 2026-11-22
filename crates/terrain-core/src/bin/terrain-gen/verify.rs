use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use terrain_core::content;
use terrain_core::description;
use terrain_core::error::{ErrorKind, Stage, TerrainError};
use terrain_core::header::{self, Header};
use terrain_core::pipeline::Terrain;
use terrain_core::section;

use crate::cli::{Cursor, Options};
use crate::output::{CliFailure, EXIT_INVARIANT, EXIT_OK, hex32, json_str, path_text, report};
use crate::{SHIPPED_GENERATOR_VERSION, baseline_hash};

// ---------------------------------------------------------------------------
// Saved sections
// ---------------------------------------------------------------------------
//
// The record itself is canonical and owned by `terrain_core::section`:
// `section::encode_record` and `section::decode_record` are the only framing
// in the engine (`J4.4`). A world stores one record per file under
// `world/sections/`, named `c.<column x>.<column z>.<section y>.sect`. The
// record's own `SectionKey` is authoritative; the file name is a diagnostic
// fallback for a corrupt record and a consistency check for a valid one.
// `verify` reads the canonical record and never writes one.

/// World subdirectory holding saved section records.
const SECTIONS_DIR: &str = "sections";
/// Saved section file suffix.
const SECTION_SUFFIX: &str = "sect";

/// Upper bound on recursive section discovery, so a symlink loop cannot hang.
const MAX_SECTION_DEPTH: usize = 8;
/// Drift details printed before the report is elided.
const DETAIL_LIMIT: usize = 4;

fn io_failure(op: &'static str, path: &Path, error: &std::io::Error) -> TerrainError {
    TerrainError::new(
        Stage::Persistence,
        ErrorKind::IoError {
            op,
            path: path.to_path_buf(),
            os_error: error.raw_os_error().unwrap_or(0),
        },
    )
}

/// Recursively collects `*.sect` files below `dir`, sorted at each level.
fn collect_section_files(
    dir: &Path,
    depth: usize,
    out: &mut Vec<PathBuf>,
) -> Result<(), CliFailure> {
    if depth > MAX_SECTION_DEPTH {
        return Ok(());
    }
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(CliFailure::Usage(format!(
                "cannot read {}: {error}",
                dir.display()
            )));
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            CliFailure::Usage(format!("cannot read {}: {error}", dir.display()))
        })?;
        paths.push(entry.path());
    }
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_section_files(&path, depth + 1, out)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == SECTION_SUFFIX)
        {
            out.push(path);
        }
    }
    Ok(())
}

/// The `(x, y, z)` display coordinate of `c.<x>.<z>.<y>.sect`, when the file
/// name has that shape. The record key wins when it decodes.
fn coord_from_filename(path: &Path) -> Option<[i32; 3]> {
    let stem = path.file_stem()?.to_str()?;
    let rest = stem.strip_prefix("c.")?;
    let [x, z, y]: [&str; 3] = rest.split('.').collect::<Vec<_>>().try_into().ok()?;
    Some([x.parse().ok()?, y.parse().ok()?, z.parse().ok()?])
}

// ---------------------------------------------------------------------------
// verify
// ---------------------------------------------------------------------------

struct VerifyDrift {
    coord: Option<[i32; 3]>,
    kind: &'static str,
    detail: String,
}

struct VerifyOutcome {
    world: String,
    checked: usize,
    drifts: Vec<VerifyDrift>,
    warnings: Vec<String>,
}

pub(crate) fn cmd_verify(args: &[String], options: &Options) -> u8 {
    match verify_outcome(args) {
        Ok((outcome, report)) => {
            if let Some(directory) = report
                && let Err(error) = write_verify_bundle(&outcome, &directory)
            {
                eprintln!("cannot write verify report: {error}");
                return EXIT_INVARIANT;
            }
            print_verify(&outcome, options.json);
            if outcome.drifts.is_empty() {
                EXIT_OK
            } else {
                EXIT_INVARIANT
            }
        }
        Err(failure) => report(&failure, options.json, "verify"),
    }
}

/// Writes the `verify` report bundle and returns nothing.
fn write_verify_bundle(outcome: &VerifyOutcome, directory: &Path) -> Result<(), String> {
    fs::create_dir_all(directory)
        .map_err(|error| format!("create {}: {error}", directory.display()))?;
    let json = verify_json(outcome);
    fs::write(directory.join("verify.json"), format!("{json}\n"))
        .map_err(|error| format!("write verify.json: {error}"))?;
    fs::write(directory.join("verify.txt"), verify_text(outcome))
        .map_err(|error| format!("write verify.txt: {error}"))?;
    let manifest = format!(
        "{{\"schema\":\"valescent/verify-report@1\",\"generated\":{},\"world\":{},\"checked\":{},\"differ\":{},\"ok\":{},\"files\":[\"verify.json\",\"verify.txt\"]}}\n",
        header::now_unix_seconds(),
        json_str(&outcome.world),
        outcome.checked,
        outcome.drifts.len(),
        outcome.drifts.is_empty()
    );
    fs::write(directory.join("manifest.json"), manifest)
        .map_err(|error| format!("write manifest.json: {error}"))
}

/// The human report used by the terminal and `verify.txt`.
fn verify_text(outcome: &VerifyOutcome) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "verify {}: {} sections checked, {} differ",
        outcome.world,
        outcome.checked,
        outcome.drifts.len()
    );
    for warning in &outcome.warnings {
        let _ = writeln!(out, "warning: {warning}");
    }
    for drift in &outcome.drifts {
        match drift.coord {
            Some([x, y, z]) => {
                let _ = writeln!(out, "drift ({x},{y},{z}) {} {}", drift.kind, drift.detail);
            }
            None => {
                let _ = writeln!(out, "drift (unknown) {} {}", drift.kind, drift.detail);
            }
        }
    }
    out
}

/// Sections selected with `--sections`, when given.
type SectionFilter = Option<BTreeSet<[i32; 3]>>;

fn verify_arguments(
    args: &[String],
) -> Result<(PathBuf, SectionFilter, Option<PathBuf>), CliFailure> {
    let mut cursor = Cursor::new(args);
    let mut world = None;
    let mut only = None;
    let mut report = None;
    while let Some(argument) = cursor.arg() {
        match argument {
            "--world" => {
                if world.is_some() {
                    return Err(CliFailure::Usage("duplicate --world".to_owned()));
                }
                world = Some(PathBuf::from(cursor.value("world")?));
            }
            "--report" => {
                if report.is_some() {
                    return Err(CliFailure::Usage("duplicate --report".to_owned()));
                }
                report = Some(PathBuf::from(cursor.value("report")?));
            }
            "--sections" => {
                if only.is_some() {
                    return Err(CliFailure::Usage("duplicate --sections".to_owned()));
                }
                let mut selected = BTreeSet::new();
                while let Some(value) = cursor.peek() {
                    if value.starts_with("--") {
                        break;
                    }
                    let value = cursor
                        .arg()
                        .ok_or_else(|| "--sections needs a value".to_owned())?;
                    selected.insert(parse_section_coord(value)?);
                }
                if selected.is_empty() {
                    return Err(CliFailure::Usage(
                        "--sections needs at least one x,y,z triple".to_owned(),
                    ));
                }
                only = Some(selected);
            }
            other => {
                return Err(CliFailure::Usage(format!(
                    "unexpected argument {other:?} for verify"
                )));
            }
        }
    }
    let world = world.ok_or_else(|| CliFailure::Usage("verify needs --world <dir>".to_owned()))?;
    Ok((world, only, report))
}

fn parse_section_coord(value: &str) -> Result<[i32; 3], String> {
    let [x, y, z]: [&str; 3] = value
        .split(',')
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| format!("invalid --sections value {value:?}; expected x,y,z"))?;
    let parse = |field: &str| {
        field
            .parse::<i32>()
            .map_err(|_| format!("invalid --sections value {value:?}"))
    };
    Ok([parse(x)?, parse(y)?, parse(z)?])
}

fn verify_outcome(args: &[String]) -> Result<(VerifyOutcome, Option<PathBuf>), CliFailure> {
    let (world, only, report) = verify_arguments(args)?;
    let header = Header::read(&world.join(header::HEADER_FILE))?;
    check_world_identity(&header)?;

    let pack = content::v1_default();
    let mut files = Vec::new();
    collect_section_files(&world.join(SECTIONS_DIR), 0, &mut files)?;
    files.sort();

    let mut terrain = None;
    let mut outcome = VerifyOutcome {
        world: path_text(&world),
        checked: 0,
        drifts: Vec::new(),
        warnings: Vec::new(),
    };
    for path in files {
        let name_coord = coord_from_filename(&path);
        if let (Some(selected), Some(coord)) = (&only, name_coord)
            && !selected.contains(&coord)
        {
            continue;
        }
        verify_file(
            &path,
            name_coord,
            header.seed,
            &pack,
            &mut terrain,
            &mut outcome,
        )?;
    }
    Ok((outcome, report))
}

fn check_world_identity(header: &Header) -> Result<(), CliFailure> {
    let computed = description::default_v1().description_hash()?;
    if header.description_hash != computed {
        return Err(CliFailure::Invariant(format!(
            "description hash mismatch: header {} recomputed {}",
            hex32(&header.description_hash),
            hex32(&computed)
        )));
    }
    if header.generator_version != SHIPPED_GENERATOR_VERSION {
        return Err(CliFailure::Invariant(format!(
            "generator version {} is not shipped by this build (ships {SHIPPED_GENERATOR_VERSION})",
            header.generator_version
        )));
    }
    Ok(())
}

fn verify_file(
    path: &Path,
    name_coord: Option<[i32; 3]>,
    seed: u64,
    pack: &content::ContentPack,
    terrain: &mut Option<Terrain>,
    outcome: &mut VerifyOutcome,
) -> Result<(), CliFailure> {
    let bytes = fs::read(path)
        .map_err(|error| CliFailure::Terrain(Box::new(io_failure("read", path, &error))))?;

    let saved: section::SavedSection = match section::decode_record_at(pack, path, &bytes) {
        Ok(saved) => saved,
        Err(error) => {
            if matches!(&error.kind, ErrorKind::CorruptSection { .. }) {
                outcome.checked += 1;
                outcome.drifts.push(VerifyDrift {
                    coord: name_coord,
                    kind: "corrupt",
                    detail: corrupt_detail(&error),
                });
                return Ok(());
            }
            // A pack that cannot read saved sections is a build/manifest
            // failure, not section drift.
            return Err(CliFailure::Terrain(Box::new(error)));
        }
    };

    let coord = [
        saved.key.column[0],
        i32::from(saved.key.section),
        saved.key.column[1],
    ];
    for name in &saved.unknown_names {
        outcome.warnings.push(format!(
            "unknown block name {name:?} in section ({},{},{})",
            coord[0], coord[1], coord[2]
        ));
    }

    // Edited sections are authoritative and are never regenerated (`J4.5`).
    if saved.edited {
        return Ok(());
    }
    outcome.checked += 1;

    if name_coord.is_some_and(|from_name| from_name != coord) {
        outcome.drifts.push(VerifyDrift {
            coord: name_coord.or(Some(coord)),
            kind: "corrupt",
            detail: "file name and record key differ".to_owned(),
        });
        return Ok(());
    }

    let stored = baseline_hash(&saved.section);
    if stored != saved.baseline_hash {
        outcome.drifts.push(VerifyDrift {
            coord: Some(coord),
            kind: "stored",
            detail: format!(
                "baseline {} saved {}",
                hex32(&saved.baseline_hash),
                hex32(&stored)
            ),
        });
        return Ok(());
    }

    if terrain.is_none() {
        *terrain = Some(Terrain::new(seed)?);
    }
    let Some(generator) = terrain.as_ref() else {
        return Err(CliFailure::Usage(
            "internal: verify generator is missing".to_owned(),
        ));
    };
    let regenerated = generator.section(coord[0], coord[1], coord[2])?;
    let actual = baseline_hash(&regenerated);
    if actual != saved.baseline_hash {
        outcome.drifts.push(VerifyDrift {
            coord: Some(coord),
            kind: "baseline",
            detail: format!(
                "baseline {} regenerated {}",
                hex32(&saved.baseline_hash),
                hex32(&actual)
            ),
        });
    }
    Ok(())
}

fn corrupt_detail(error: &TerrainError) -> String {
    match &error.kind {
        ErrorKind::CorruptSection { fault, .. } => format!("{fault:?}"),
        _ => error.to_string(),
    }
}

fn verify_json(outcome: &VerifyOutcome) -> String {
    let differ = outcome.drifts.len();
    let mut out = format!(
        "{{\"subcommand\":\"verify\",\"world\":{},\"checked\":{},\"differ\":{differ},\"ok\":{},\"warnings\":[",
        json_str(&outcome.world),
        outcome.checked,
        differ == 0
    );
    for (index, warning) in outcome.warnings.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&json_str(warning));
    }
    out.push(']');
    if differ > 0 {
        out.push_str(",\"drift\":[");
        for (index, drift) in outcome.drifts.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            match drift.coord {
                Some([x, y, z]) => {
                    let _ = write!(out, "{{\"section\":[{x},{y},{z}],");
                }
                None => out.push_str("{\"section\":null,"),
            }
            let _ = write!(
                out,
                "\"kind\":{},\"detail\":{}}}",
                json_str(drift.kind),
                json_str(&drift.detail)
            );
        }
        out.push(']');
    }
    out.push('}');
    out
}

fn print_verify(outcome: &VerifyOutcome, json: bool) {
    for warning in &outcome.warnings {
        eprintln!("warning: {warning}");
    }
    let differ = outcome.drifts.len();
    if json {
        println!("{}", verify_json(outcome));
    } else {
        println!("{} sections checked, {differ} differ", outcome.checked);
        for drift in outcome.drifts.iter().take(DETAIL_LIMIT) {
            match drift.coord {
                Some([x, y, z]) => {
                    println!("drift ({x},{y},{z}) {} {}", drift.kind, drift.detail);
                }
                None => println!("drift (unknown) {} {}", drift.kind, drift.detail),
            }
        }
        if differ > DETAIL_LIMIT {
            println!("... {} more", differ - DETAIL_LIMIT);
        }
    }
}

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use terrain_core::error::TerrainError;
use terrain_core::lattice::floor_div;
use terrain_core::pipeline::{AIR, Terrain, WATER};

use crate::cli::{Options, number, parse_pairs, parse_seed};
use crate::output::{CliFailure, EXIT_INVARIANT, EXIT_OK, hex32, json_str, path_text, report};
use crate::{Column, SECTION_BLOCKS, column_digest};

/// Default golden file, anchored to the crate root so the command works from
/// any working directory.
const GOLDEN_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/golden/terrain-v1.golden"
);

// ---------------------------------------------------------------------------
// golden
// ---------------------------------------------------------------------------

struct GoldenEntry {
    name: String,
    seed: u64,
    x: i32,
    z: i32,
    hash: String,
    section: String,
}

struct GoldenOutcome {
    file: String,
    action: &'static str,
    entries: usize,
    mismatch: Option<GoldenMismatch>,
}

struct GoldenMismatch {
    name: String,
    expected_hash: String,
    expected_section: String,
    actual_hash: String,
    actual_section: String,
}

pub(crate) fn cmd_golden(args: &[String], options: &Options) -> u8 {
    match golden_outcome(args) {
        Ok(outcome) => {
            print_golden(&outcome, options.json);
            if outcome.mismatch.is_some() {
                EXIT_INVARIANT
            } else {
                EXIT_OK
            }
        }
        Err(failure) => report(&failure, options.json, "golden"),
    }
}

fn golden_outcome(args: &[String]) -> Result<GoldenOutcome, CliFailure> {
    let Some((action, rest)) = args.split_first() else {
        return Err(CliFailure::Usage("golden needs check or update".to_owned()));
    };
    let action = match action.as_str() {
        "check" => "check",
        "update" => "update",
        other => {
            return Err(CliFailure::Usage(format!(
                "unknown golden action {other:?}; expected check or update"
            )));
        }
    };
    let pairs = parse_pairs(rest)?;
    let path = pairs
        .get("file")
        .map_or(Path::new(GOLDEN_PATH), |value| Path::new(value));

    let mut entries = read_golden(path)?;
    if action == "update" {
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        let mut rendered = Vec::with_capacity(entries.len());
        for entry in &entries {
            rendered.push(regenerate(entry)?);
        }
        write_golden(path, &entries, &rendered)?;
        return Ok(GoldenOutcome {
            file: path_text(path),
            action,
            entries: entries.len(),
            mismatch: None,
        });
    }

    let mut mismatch = None;
    for entry in &entries {
        let (hash, section) = regenerate(entry)?;
        if hash != entry.hash.to_ascii_lowercase() || section != entry.section.to_ascii_lowercase()
        {
            mismatch = Some(GoldenMismatch {
                name: entry.name.clone(),
                expected_hash: entry.hash.clone(),
                expected_section: entry.section.clone(),
                actual_hash: hash,
                actual_section: section,
            });
            break;
        }
    }

    Ok(GoldenOutcome {
        file: path_text(path),
        action,
        entries: entries.len(),
        mismatch,
    })
}

fn read_golden(path: &Path) -> Result<Vec<GoldenEntry>, CliFailure> {
    let text = fs::read_to_string(path)
        .map_err(|error| CliFailure::Usage(format!("cannot read {}: {error}", path.display())))?;
    let mut entries = Vec::new();
    let mut names = BTreeSet::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() != 6 {
            return Err(CliFailure::Usage(format!(
                "{}:{}: expected 6 fields, found {}",
                path.display(),
                index + 1,
                fields.len()
            )));
        }
        let name = fields[0].to_owned();
        if !names.insert(name.clone()) {
            return Err(CliFailure::Usage(format!(
                "{}:{}: duplicate entry {name:?}",
                path.display(),
                index + 1
            )));
        }
        let seed = parse_seed(fields[1]).map_err(|error| {
            CliFailure::Usage(format!("{}:{}: {error}", path.display(), index + 1))
        })?;
        let x = number::<i32>(fields[2], "x").map_err(|error| {
            CliFailure::Usage(format!("{}:{}: {error}", path.display(), index + 1))
        })?;
        let z = number::<i32>(fields[3], "z").map_err(|error| {
            CliFailure::Usage(format!("{}:{}: {error}", path.display(), index + 1))
        })?;
        let hash = fields[4].to_ascii_lowercase();
        let section = fields[5].to_ascii_lowercase();
        if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(CliFailure::Usage(format!(
                "{}:{}: column hash must be 64 hex digits",
                path.display(),
                index + 1
            )));
        }
        if section.len() != 16 || !section.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(CliFailure::Usage(format!(
                "{}:{}: section hash must be 16 hex digits",
                path.display(),
                index + 1
            )));
        }
        entries.push(GoldenEntry {
            name,
            seed,
            x,
            z,
            hash,
            section,
        });
    }
    Ok(entries)
}

fn regenerate(entry: &GoldenEntry) -> Result<(String, String), CliFailure> {
    let terrain = Terrain::new(entry.seed)?;
    let column = terrain.column(entry.x, entry.z)?;
    let hash = hex32(&column_digest(&column));
    let section = section_hash(&terrain, entry.x, entry.z, &column)?;
    Ok((hash, section))
}

fn section_hash(
    terrain: &Terrain,
    x: i32,
    z: i32,
    column: &Column,
) -> Result<String, TerrainError> {
    let top = column
        .iter()
        .rposition(|&id| id != AIR && id != WATER)
        .unwrap_or(0);
    let top = i32::try_from(top).unwrap_or(0);
    let section = terrain.section(
        floor_div(x, SECTION_BLOCKS),
        floor_div(top, SECTION_BLOCKS),
        floor_div(z, SECTION_BLOCKS),
    )?;
    Ok(format!("{:016x}", section.content_hash()))
}

fn write_golden(
    path: &Path,
    entries: &[GoldenEntry],
    rendered: &[(String, String)],
) -> Result<(), CliFailure> {
    let mut text = String::from(
        "# terrain golden v1: name seed x z column-sha256 section-u64\n\
         # Rewritten only by `terrain-gen golden update` (R1.10).\n",
    );
    for (entry, (hash, section)) in entries.iter().zip(rendered) {
        let _ = writeln!(
            text,
            "{} {} {} {} {} {}",
            entry.name, entry.seed, entry.x, entry.z, hash, section
        );
    }
    fs::write(path, text)
        .map_err(|error| CliFailure::Usage(format!("cannot write {}: {error}", path.display())))
}

fn print_golden(outcome: &GoldenOutcome, json: bool) {
    let GoldenOutcome {
        file,
        action,
        entries,
        mismatch,
    } = outcome;
    if json {
        let mut out = format!(
            "{{\"subcommand\":\"golden\",\"action\":{},\"file\":{},\"entries\":{entries}",
            json_str(action),
            json_str(file)
        );
        if let Some(difference) = mismatch {
            let _ = write!(
                out,
                ",\"ok\":false,\"entry\":{},\"expected\":[{},{}],\"actual\":[{},{}]}}",
                json_str(&difference.name),
                json_str(&difference.expected_hash),
                json_str(&difference.expected_section),
                json_str(&difference.actual_hash),
                json_str(&difference.actual_section)
            );
            println!("{out}");
        } else {
            out.push_str(",\"ok\":true}");
            println!("{out}");
        }
    } else {
        println!("file {file}");
        println!("entries {entries}");
        match mismatch {
            Some(difference) => {
                println!("mismatch {}", difference.name);
                println!(
                    "expected {} {}",
                    difference.expected_hash, difference.expected_section
                );
                println!(
                    "actual {} {}",
                    difference.actual_hash, difference.actual_section
                );
                eprintln!(
                    "golden mismatch at {}: expected {} {}, actual {} {}",
                    difference.name,
                    difference.expected_hash,
                    difference.expected_section,
                    difference.actual_hash,
                    difference.actual_section
                );
            }
            None if *action == "update" => println!("updated"),
            None => println!("ok"),
        }
    }
}

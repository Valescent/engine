//! `terrain-gen`: the headless, CPU-only acceptance harness for the terrain
//! pipeline (`Q1.9`).
//!
//! The binary drives only `terrain_core::pipeline`, `terrain_core::section`, and
//! `terrain_core::hash`: no window, GPU, audio, or renderer code is referenced here.
//! It exists so every other workstream has one command that says whether the
//! generator is still deterministic.
//!
//! # Exit codes
//!
//! `0` success, `1` failed invariant, `2` usage or unsupported subcommand, `3` a
//! [`TerrainError`]. `--json` makes stdout exactly one JSON object; diagnostics
//! always go to stderr. Nothing printed to stdout contains timing, thread count,
//! or processing order, so `--threads N` and `--order ...` cannot change it.
//!
//! # Pinned output definitions
//!
//! * A *column digest* is `SHA-256` over the 384 block ids of one column, each
//!   as two little-endian bytes, `y = 0` first.
//! * A saved section's `baseline_hash` is `SHA-256` over its 32,768 decoded
//!   block ids, two little-endian bytes each, in `section_index` order (`x`
//!   fastest, then `z`, then `y`). `verify` and the fixture writer share this
//!   one derivation; `Section::content_hash` is never the saved baseline.
//! * `gen` hashes the column digests' input stream for every column in
//!   row-major order: `z` outer, `x` inner, `y` ascending within a column.
//! * A golden line's section hash is `Section::content_hash` of the section that
//!   contains the column's top solid block.
//! * `seams` compares the two cross-tile height samples on each side of a tile
//!   boundary, which is the one-block margin on both sides (`R1.5` layer 1).
//! * `sweep` regions are `8 x 8` block windows anchored at the origin and near
//!   `WORLD_HALF_EXTENT`. Seeds are `1..=n`.
//!
//! `Terrain` is not `Sync`, so each worker builds its own instance; the printed
//! digests are always assembled in canonical order, never in processing order.

#![forbid(unsafe_code)]

mod calibrate;
mod cli;
mod counters;
mod describe;
mod r#gen;
mod golden;
mod map;
mod migrate;
mod output;
mod repair;
mod replay;
mod seams;
mod sweep;
mod verify;

use std::process::ExitCode;
use std::thread;

use terrain_core::error::{ErrorKind, Stage, TerrainError};
use terrain_core::hash::sha256;
use terrain_core::pipeline::{AIR, Terrain, WATER};
use terrain_core::section::{self, BlockId, Section};

use crate::calibrate::cmd_calibrate_blend;
use crate::cli::{Order, parse_globals, permutation};
use crate::counters::cmd_counters;
use crate::describe::{cmd_describe, cmd_describe_diff};
use crate::r#gen::cmd_gen;
use crate::golden::cmd_golden;
use crate::map::cmd_map;
use crate::migrate::cmd_migrate;
use crate::output::{EXIT_USAGE, usage_error};
use crate::repair::cmd_repair;
use crate::replay::cmd_replay;
use crate::seams::cmd_seams;
use crate::sweep::cmd_sweep;
use crate::verify::cmd_verify;

/// Blocks in a generated column. Mirrors `description::WORLD_HEIGHT`.
pub(crate) const COLUMN: usize = 384;
/// Blocks along one section axis.
pub(crate) const SECTION_BLOCKS: i32 = 32;

/// The generator version this build evaluates (`T1.2`).
pub(crate) const SHIPPED_GENERATOR_VERSION: u16 = 4;

const USAGE_TEXT: &str = "\
usage: terrain-gen <command> [options]

commands:
  gen            --seed <u64> --from <x> <z> [--size <n>]
  golden         check|update [--file <path>]
  seams          [--window <WxW>] [--seeds <n>]
  sweep          --tier pr|nightly [--seeds <n>]
  counters       --tier pr|nightly [--seeds <n>] [--file <path>] [--update]
  calibrate-blend --seeds <n> [--step <n>] [--from <x> <z>] [--size <n>]
  describe       --out <path>
  describe-diff  --file <path>
  verify         --world <dir> [--sections <x,y,z> ...] [--report <dir>]
  replay         --report <dir|manifest.json>
  repair         --world <dir> --section <cx,cz,sy> --action restore|quarantine|salvage
  migrate        --world <dir> [--from <n>] [--to <n>]
  map            --seed <u64> --from <x> <z> [--size <n>] [--cell <n>]

global options: --json  --threads <n>  --order forward|reverse|shuffled|<n>";

pub(crate) type Column = [BlockId; COLUMN];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    ExitCode::from(run(&args))
}

fn run(args: &[String]) -> u8 {
    let Some((command, rest)) = args.split_first() else {
        eprintln!("{USAGE_TEXT}");
        return EXIT_USAGE;
    };
    let (options, rest) = match parse_globals(rest) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(false, &message),
    };
    match command.as_str() {
        "gen" => cmd_gen(&rest, &options),
        "golden" => cmd_golden(&rest, &options),
        "seams" => cmd_seams(&rest, &options),
        "sweep" => cmd_sweep(&rest, &options),
        "counters" => cmd_counters(&rest, &options),
        "calibrate-blend" => cmd_calibrate_blend(&rest, &options),
        "describe" => cmd_describe(&rest, &options),
        "describe-diff" => cmd_describe_diff(&rest, &options),
        "verify" => cmd_verify(&rest, &options),
        "replay" => cmd_replay(&rest, &options),
        "repair" => cmd_repair(&rest, &options),
        "migrate" => cmd_migrate(&rest, &options),
        "map" => cmd_map(&rest, &options),
        other => usage_error(
            options.json,
            &format!("unknown subcommand {other:?}\n\n{USAGE_TEXT}"),
        ),
    }
}

pub(crate) fn internal(check: &'static str) -> TerrainError {
    TerrainError::new(
        Stage::Fields,
        ErrorKind::InvariantViolated { check, coord: None },
    )
}

// ---------------------------------------------------------------------------
// Column generation
// ---------------------------------------------------------------------------

/// Generates the given columns, one `Terrain` per worker, and returns them in
/// the input (canonical) order regardless of `order` or `threads`.
pub(crate) fn generate_columns(
    seed: u64,
    coords: &[(i32, i32)],
    threads: usize,
    order: Order,
) -> Result<Vec<Column>, TerrainError> {
    if coords.is_empty() {
        return Ok(Vec::new());
    }
    let sequence = permutation(coords.len(), order, seed);
    let workers = threads.clamp(1, sequence.len());
    let chunk = sequence.len().div_ceil(workers);
    let mut slots = vec![[AIR; COLUMN]; sequence.len()];

    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for indices in sequence.chunks(chunk) {
            handles.push(scope.spawn(move || -> Result<Vec<Column>, TerrainError> {
                let terrain = Terrain::new(seed)?;
                let mut produced = Vec::with_capacity(indices.len());
                for &index in indices {
                    let (x, z) = coords[index];
                    produced.push(terrain.column(x, z)?);
                }
                Ok(produced)
            }));
        }
        for (indices, handle) in sequence.chunks(chunk).zip(handles) {
            let produced = match handle.join() {
                Ok(result) => result?,
                Err(_) => return Err(internal("worker_panic")),
            };
            for (index, column) in indices.iter().zip(produced) {
                slots[*index] = column;
            }
        }
        Ok(())
    })?;

    Ok(slots)
}

pub(crate) fn column_bytes(column: &[BlockId]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(column.len() * 2);
    for &id in column {
        bytes.extend_from_slice(&id.to_le_bytes());
    }
    bytes
}

pub(crate) fn column_digest(column: &[BlockId]) -> [u8; 32] {
    sha256(&column_bytes(column))
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Counts {
    solid: u64,
    water: u64,
    air: u64,
}

impl Counts {
    pub(crate) fn add(&mut self, id: BlockId) {
        if id == AIR {
            self.air += 1;
        } else if id == WATER {
            self.water += 1;
        } else {
            self.solid += 1;
        }
    }
}

/// Hashes every column in row-major order and counts block classes. Both are
/// independent of how the columns were produced.
pub(crate) fn digest_columns(columns: &[Column]) -> ([u8; 32], Counts) {
    let mut bytes = Vec::with_capacity(columns.len().saturating_mul(COLUMN * 2));
    let mut counts = Counts::default();
    for column in columns {
        for &id in column {
            bytes.extend_from_slice(&id.to_le_bytes());
            counts.add(id);
        }
    }
    (sha256(&bytes), counts)
}

/// The saved `baseline_hash` (`J4.1`): `SHA-256` over the decoded block ids of
/// one section, two little-endian bytes per cell, in `section_index` order
/// (`x` fastest, then `z`, then `y`).
///
/// This is the one pinned derivation. The save writer computes it over the
/// section as first generated and records it in the v2 record; `verify`
/// recomputes it over the decoded regenerated section and compares the two.
/// It is deliberately **not** [`Section::content_hash`]: that is a 64-bit
/// content digest with a different domain and does not fit the record's
/// 32-byte field.
pub(crate) fn baseline_hash(section: &Section) -> [u8; 32] {
    section::baseline_hash(section)
}

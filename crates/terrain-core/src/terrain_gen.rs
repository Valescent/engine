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

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;

use terrain_core::climate::smoothstep01;
use terrain_core::content;
use terrain_core::description::{
    self, Description, HYDRO_TILE, MACRO_CELL, MAX_TERRAIN_Y, SEA, SECTIONS_PER_COLUMN, TILE_BLOCKS,
};
use terrain_core::error::{ErrorKind, Stage, TerrainError};
use terrain_core::hash::sha256;
use terrain_core::header::{self, Header};
use terrain_core::hydro;
use terrain_core::lattice::floor_div;
use terrain_core::pipeline::{AIR, Terrain, WATER};
use terrain_core::save::{RepairAction, RepairOutcome, SectionStore};
use terrain_core::section::{self, BlockId, Section, SectionKey};
use terrain_core::world_save::{MigrationReport, WorldSave, write_migration_report};

/// Blocks in a generated column. Mirrors `description::WORLD_HEIGHT`.
const COLUMN: usize = 384;
/// Blocks along one section axis.
const SECTION_BLOCKS: i32 = 32;
/// Highest block id the v1 content pack defines (`Q1.9`, `R1.12`).
/// Highest block id the built-in content pack can produce (`A5.1`).
///
/// Ids are dense and name-sorted, so the ceiling is `pack.len() - 1`. It is
/// resolved once rather than hardcoded: adding a record must never make a
/// legitimate id look like corruption to the sweep.
fn max_known_block() -> BlockId {
    static MAX: std::sync::OnceLock<BlockId> = std::sync::OnceLock::new();
    *MAX.get_or_init(|| {
        let pack = terrain_core::content::v1_default();
        BlockId::try_from(pack.len().saturating_sub(1)).unwrap_or(BlockId::MAX)
    })
}
/// Default golden file, anchored to the crate root so the command works from
/// any working directory.
const GOLDEN_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/golden/terrain-v1.golden"
);
/// Default `gen` square side.
const DEFAULT_GEN_SIZE: i32 = 16;
/// Default `seams` window side.
const DEFAULT_SEAMS_SIDE: i32 = 5;
/// Blocks per sweep region axis.
const SWEEP_WINDOW: i32 = 8;
/// `pr` sweep seed count.
const PR_SEEDS: usize = 16;
/// `nightly` sweep seed count.
const NIGHTLY_SEEDS: usize = 64;
const EXIT_OK: u8 = 0;
const EXIT_INVARIANT: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_TERRAIN: u8 = 3;

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

type Column = [BlockId; COLUMN];

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

// ---------------------------------------------------------------------------
// Argument parsing
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Order {
    #[default]
    Forward,
    Reverse,
    Shuffled,
    Numeric(u64),
}

impl Order {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "forward" => Ok(Self::Forward),
            "reverse" => Ok(Self::Reverse),
            "shuffled" => Ok(Self::Shuffled),
            _ => value
                .parse::<u64>()
                .map(Self::Numeric)
                .map_err(|_| format!("invalid --order {value:?}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Options {
    json: bool,
    threads: usize,
    order: Order,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            json: false,
            threads: 1,
            order: Order::Forward,
        }
    }
}

/// Removes the global options from `args`, leaving subcommand arguments intact.
fn parse_globals(args: &[String]) -> Result<(Options, Vec<String>), String> {
    let mut options = Options::default();
    let mut rest = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => {
                options.json = true;
                index += 1;
            }
            "--threads" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "--threads needs a value".to_owned())?;
                let threads = value
                    .parse::<usize>()
                    .map_err(|_| format!("invalid --threads {value:?}"))?;
                if threads == 0 {
                    return Err("--threads must be at least 1".to_owned());
                }
                options.threads = threads;
                index += 2;
            }
            "--order" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "--order needs a value".to_owned())?;
                options.order = Order::parse(value)?;
                index += 2;
            }
            other => {
                rest.push(other.to_owned());
                index += 1;
            }
        }
    }
    Ok((options, rest))
}

/// A left-to-right cursor over subcommand arguments.
struct Cursor<'a> {
    args: &'a [String],
    index: usize,
}

impl<'a> Cursor<'a> {
    fn new(args: &'a [String]) -> Self {
        Self { args, index: 0 }
    }

    fn arg(&mut self) -> Option<&'a str> {
        let value = self.args.get(self.index)?;
        self.index += 1;
        Some(value.as_str())
    }

    fn peek(&self) -> Option<&'a str> {
        self.args.get(self.index).map(String::as_str)
    }

    fn value(&mut self, name: &str) -> Result<&'a str, String> {
        self.arg().ok_or_else(|| format!("--{name} needs a value"))
    }
}

fn number<T>(value: &str, name: &str) -> Result<T, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse::<T>()
        .map_err(|error| format!("invalid --{name} {value:?}: {error}"))
}

/// Parses a `--key value` list into a map. Every token must be a flag.
fn parse_pairs(args: &[String]) -> Result<BTreeMap<String, String>, String> {
    let mut cursor = Cursor::new(args);
    let mut pairs = BTreeMap::new();
    while let Some(argument) = cursor.arg() {
        let Some(key) = argument.strip_prefix("--") else {
            return Err(format!("unexpected argument {argument:?}"));
        };
        let value = cursor
            .arg()
            .ok_or_else(|| format!("--{key} needs a value"))?;
        if pairs.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(format!("duplicate --{key}"));
        }
    }
    Ok(pairs)
}

fn parse_seed(value: &str) -> Result<u64, String> {
    let parsed = value
        .strip_prefix("0x")
        .map_or_else(|| value.parse::<u64>(), |hex| u64::from_str_radix(hex, 16));
    parsed.map_err(|error| format!("invalid seed {value:?}: {error}"))
}

// ---------------------------------------------------------------------------
// Failures and exit codes
// ---------------------------------------------------------------------------

enum CliFailure {
    Usage(String),
    Description {
        key: &'static str,
        rule: &'static str,
    },
    Terrain(Box<TerrainError>),
    /// The command ran and found the input inconsistent with this build: a
    /// header identity mismatch or an unsupported generator version. Distinct
    /// from `Usage` because no argument was malformed.
    Invariant(String),
}

impl From<String> for CliFailure {
    fn from(message: String) -> Self {
        Self::Usage(message)
    }
}

impl From<TerrainError> for CliFailure {
    fn from(error: TerrainError) -> Self {
        Self::Terrain(Box::new(error))
    }
}

fn usage_error(json: bool, message: &str) -> u8 {
    eprintln!("{message}");
    if json {
        println!("{{\"error\":{},\"kind\":\"usage\"}}", json_str(message));
    }
    EXIT_USAGE
}

fn report(failure: &CliFailure, json: bool, command: &str) -> u8 {
    let (code, kind, message) = match failure {
        CliFailure::Usage(message) => (EXIT_USAGE, "usage", message.clone()),
        CliFailure::Description { key, rule } => (
            EXIT_USAGE,
            "description",
            format!("description key {key:?} violates {rule:?}"),
        ),
        CliFailure::Terrain(error) => (EXIT_TERRAIN, "terrain", error.to_string()),
        CliFailure::Invariant(message) => (EXIT_INVARIANT, "invariant", message.clone()),
    };
    if json {
        println!(
            "{{\"subcommand\":{},\"error\":{},\"kind\":{}}}",
            json_str(command),
            json_str(&message),
            json_str(kind)
        );
    }
    eprintln!("{message}");
    code
}

fn internal(check: &'static str) -> TerrainError {
    TerrainError::new(
        Stage::Fields,
        ErrorKind::InvariantViolated { check, coord: None },
    )
}

// ---------------------------------------------------------------------------
// Stable output helpers
// ---------------------------------------------------------------------------

fn json_str(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if u32::from(control) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(control));
            }
            plain => out.push(plain),
        }
    }
    out.push('"');
    out
}

fn hex32(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

// ---------------------------------------------------------------------------
// Deterministic work ordering
// ---------------------------------------------------------------------------

/// A fixed permutation of `0..len`. `Numeric(n)` seeds the same shuffle that
/// `Shuffled` seeds from `shuffled_key`; neither reads the clock or an RNG. For
/// `sweep`, `shuffled_key` is the first canonical seed, so the permutation is a
/// function of the seed list alone.
fn permutation(len: usize, order: Order, shuffled_key: u64) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..len).collect();
    match order {
        Order::Forward => {}
        Order::Reverse => indices.reverse(),
        Order::Shuffled => shuffle(&mut indices, shuffled_key),
        Order::Numeric(key) => shuffle(&mut indices, key),
    }
    indices
}

#[allow(clippy::cast_possible_truncation)]
fn shuffle(indices: &mut [usize], key: u64) {
    if indices.len() < 2 {
        return;
    }
    let mut state = terrain_core::hash::mix(key);
    for slot in (1..indices.len()).rev() {
        state = terrain_core::hash::mix(state ^ u64::try_from(slot).unwrap_or(0));
        // `slot + 1 <= len <= usize::MAX`, so the cast is in range on every
        // supported target.
        let offset = (state % u64::try_from(slot + 1).unwrap_or(u64::MAX)) as usize;
        indices.swap(slot, offset);
    }
}

// ---------------------------------------------------------------------------
// Column generation
// ---------------------------------------------------------------------------

/// Generates the given columns, one `Terrain` per worker, and returns them in
/// the input (canonical) order regardless of `order` or `threads`.
fn generate_columns(
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

fn column_bytes(column: &[BlockId]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(column.len() * 2);
    for &id in column {
        bytes.extend_from_slice(&id.to_le_bytes());
    }
    bytes
}

fn column_digest(column: &[BlockId]) -> [u8; 32] {
    sha256(&column_bytes(column))
}

#[derive(Clone, Copy, Debug, Default)]
struct Counts {
    solid: u64,
    water: u64,
    air: u64,
}

impl Counts {
    fn add(&mut self, id: BlockId) {
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
fn digest_columns(columns: &[Column]) -> ([u8; 32], Counts) {
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

// ---------------------------------------------------------------------------
// gen
// ---------------------------------------------------------------------------

struct GenOutcome {
    seed: u64,
    from: (i32, i32),
    size: i32,
    columns: usize,
    hash: [u8; 32],
    counts: Counts,
}

fn cmd_gen(args: &[String], options: &Options) -> u8 {
    match gen_outcome(args, options) {
        Ok(outcome) => {
            print_gen(&outcome, options.json);
            EXIT_OK
        }
        Err(failure) => report(&failure, options.json, "gen"),
    }
}

fn gen_outcome(args: &[String], options: &Options) -> Result<GenOutcome, CliFailure> {
    let mut cursor = Cursor::new(args);
    let mut seed = None;
    let mut from = None;
    let mut size = None;
    while let Some(argument) = cursor.arg() {
        match argument {
            "--seed" => {
                seed = Some(parse_seed(cursor.value("seed")?)?);
            }
            "--from" => {
                let x = number::<i32>(cursor.value("from x")?, "from x")?;
                let z = number::<i32>(cursor.value("from z")?, "from z")?;
                from = Some((x, z));
            }
            "--size" => {
                size = Some(number::<i32>(cursor.value("size")?, "size")?);
            }
            other => {
                return Err(CliFailure::Usage(format!(
                    "unexpected argument {other:?} for gen"
                )));
            }
        }
    }
    let seed = seed.ok_or_else(|| CliFailure::Usage("gen needs --seed <u64>".to_owned()))?;
    let from = from.ok_or_else(|| CliFailure::Usage("gen needs --from <x> <z>".to_owned()))?;
    let size = size.unwrap_or(DEFAULT_GEN_SIZE);
    if size <= 0 {
        return Err(CliFailure::Usage(format!(
            "--size must be at least 1, found {size}"
        )));
    }
    let extent = usize::try_from(size)
        .map_err(|_| CliFailure::Usage(format!("--size {size} is too large")))?;
    let count = extent
        .checked_mul(extent)
        .ok_or_else(|| CliFailure::Usage(format!("--size {size} is too large")))?;

    let mut coords = Vec::with_capacity(count);
    for row in 0..size {
        for column in 0..size {
            coords.push((from.0 + column, from.1 + row));
        }
    }
    let columns = generate_columns(seed, &coords, options.threads, options.order)?;
    let (hash, counts) = digest_columns(&columns);
    Ok(GenOutcome {
        seed,
        from,
        size,
        columns: columns.len(),
        hash,
        counts,
    })
}

fn print_gen(outcome: &GenOutcome, json: bool) {
    let GenOutcome {
        seed,
        from,
        size,
        columns,
        hash,
        counts,
    } = outcome;
    if json {
        println!(
            "{{\"subcommand\":\"gen\",\"seed\":{seed},\"from\":[{},{}],\"size\":{size},\"columns\":{columns},\"hash\":{},\"solid\":{},\"water\":{},\"air\":{}}}",
            from.0,
            from.1,
            json_str(&hex32(hash)),
            counts.solid,
            counts.water,
            counts.air
        );
    } else {
        println!("seed {seed}");
        println!("from {} {}", from.0, from.1);
        println!("size {size}");
        println!("columns {columns}");
        println!("hash {}", hex32(hash));
        println!("solid {}", counts.solid);
        println!("water {}", counts.water);
        println!("air {}", counts.air);
    }
}

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

fn cmd_golden(args: &[String], options: &Options) -> u8 {
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

// ---------------------------------------------------------------------------
// seams
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SeamAxis {
    X,
    Z,
}

#[derive(Clone, Copy)]
struct SeamPair {
    a: (i32, i32),
    b: (i32, i32),
    axis: SeamAxis,
}

struct SeamMismatch {
    seed: u64,
    a: (i32, i32),
    b: (i32, i32),
    axis: SeamAxis,
    local: (i32, i32),
    a_top: i32,
    b_top: i32,
}

struct SeamOutcome {
    window: i32,
    seeds: usize,
    pairs: u64,
    comparisons: u64,
    mismatch: Option<SeamMismatch>,
}

fn cmd_seams(args: &[String], options: &Options) -> u8 {
    match seams_outcome(args) {
        Ok(outcome) => {
            print_seams(&outcome, options.json);
            if outcome.mismatch.is_some() {
                EXIT_INVARIANT
            } else {
                EXIT_OK
            }
        }
        Err(failure) => report(&failure, options.json, "seams"),
    }
}

fn seams_outcome(args: &[String]) -> Result<SeamOutcome, CliFailure> {
    let pairs = parse_pairs(args)?;
    let window = match pairs.get("window") {
        Some(value) => parse_window(value)?,
        None => DEFAULT_SEAMS_SIDE,
    };
    let seeds = match pairs.get("seeds") {
        Some(value) => number::<usize>(value, "seeds")?,
        None => 1,
    };
    if seeds == 0 {
        return Err(CliFailure::Usage("--seeds must be at least 1".to_owned()));
    }

    let mut pairs_compared = 0_u64;
    let mut comparisons = 0_u64;
    let mut mismatch = None;
    'seeds: for index in 0..seeds {
        let seed = u64::try_from(index + 1).unwrap_or(u64::MAX);
        for pair in seam_pairs(window) {
            pairs_compared += 1;
            comparisons += 2 * u64::try_from(TILE_BLOCKS + 2).unwrap_or(0);
            if let Some(found) = compare_pair(seed, pair)? {
                mismatch = Some(found);
                break 'seeds;
            }
        }
    }

    Ok(SeamOutcome {
        window,
        seeds,
        pairs: pairs_compared,
        comparisons,
        mismatch,
    })
}

fn parse_window(value: &str) -> Result<i32, CliFailure> {
    let (width, height) = value
        .split_once('x')
        .ok_or_else(|| CliFailure::Usage(format!("invalid --window {value:?}, expected WxW")))?;
    let width = number::<i32>(width, "window")?;
    let height = number::<i32>(height, "window")?;
    if width < 1 || height < 1 {
        return Err(CliFailure::Usage(format!(
            "--window sides must be at least 1, found {value:?}"
        )));
    }
    if width != height {
        return Err(CliFailure::Usage(format!(
            "--window must be square, found {value:?}"
        )));
    }
    Ok(width)
}

fn seam_pairs(window: i32) -> Vec<SeamPair> {
    let mut pairs = Vec::new();
    for z in 0..window {
        for x in 0..window.saturating_sub(1) {
            pairs.push(SeamPair {
                a: (x, z),
                b: (x + 1, z),
                axis: SeamAxis::X,
            });
        }
    }
    for x in 0..window {
        for z in 0..window.saturating_sub(1) {
            pairs.push(SeamPair {
                a: (x, z),
                b: (x, z + 1),
                axis: SeamAxis::Z,
            });
        }
    }
    pairs
}

/// Builds both sides cold, in separate [`Terrain`] instances, and compares the
/// two cross-boundary samples of the `HeightTile`: the owned last column against
/// the neighbour's margin, and this tile's margin against the neighbour's first
/// owned column.
fn compare_pair(seed: u64, pair: SeamPair) -> Result<Option<SeamMismatch>, TerrainError> {
    let left = Terrain::new(seed)?;
    let right = Terrain::new(seed)?;
    let left_tile = left.tile(pair.a.0, pair.a.1)?;
    let right_tile = right.tile(pair.b.0, pair.b.1)?;

    for offset in 0..=1 {
        for along in -1..=TILE_BLOCKS {
            let (local, a_top, b_top) = match pair.axis {
                SeamAxis::X => {
                    let a_local = (TILE_BLOCKS - 1 + offset, along);
                    let b_local = (-1 + offset, along);
                    (
                        a_local,
                        left_tile.height.top(a_local.0, a_local.1),
                        right_tile.height.top(b_local.0, b_local.1),
                    )
                }
                SeamAxis::Z => {
                    let a_local = (along, TILE_BLOCKS - 1 + offset);
                    let b_local = (along, -1 + offset);
                    (
                        a_local,
                        left_tile.height.top(a_local.0, a_local.1),
                        right_tile.height.top(b_local.0, b_local.1),
                    )
                }
            };
            if a_top != b_top {
                return Ok(Some(SeamMismatch {
                    seed,
                    a: pair.a,
                    b: pair.b,
                    axis: pair.axis,
                    local,
                    a_top,
                    b_top,
                }));
            }
        }
    }
    Ok(None)
}

fn print_seams(outcome: &SeamOutcome, json: bool) {
    let SeamOutcome {
        window,
        seeds,
        pairs,
        comparisons,
        mismatch,
    } = outcome;
    let axis_name = |axis: SeamAxis| match axis {
        SeamAxis::X => "x",
        SeamAxis::Z => "z",
    };
    if json {
        let mut out = format!(
            "{{\"subcommand\":\"seams\",\"window\":{window}x{window},\"seeds\":{seeds},\"pairs\":{pairs},\"comparisons\":{comparisons}"
        );
        if let Some(found) = mismatch {
            let _ = write!(
                out,
                ",\"ok\":false,\"mismatch\":{{\"seed\":{},\"pair\":[[{},{}],[{},{}]],\"axis\":{},\"local\":[{},{}],\"a\":{},\"b\":{}}}}}",
                found.seed,
                found.a.0,
                found.a.1,
                found.b.0,
                found.b.1,
                json_str(axis_name(found.axis)),
                found.local.0,
                found.local.1,
                found.a_top,
                found.b_top
            );
            println!("{out}");
        } else {
            out.push_str(",\"ok\":true}");
            println!("{out}");
        }
    } else {
        println!("window {window}x{window}");
        println!("seeds {seeds}");
        println!("pairs {pairs}");
        println!("comparisons {comparisons}");
        match mismatch {
            Some(found) => {
                println!("mismatch");
                println!(
                    "seed {} pair ({},{})-({},{}) axis {} local ({},{}) a {} b {}",
                    found.seed,
                    found.a.0,
                    found.a.1,
                    found.b.0,
                    found.b.1,
                    axis_name(found.axis),
                    found.local.0,
                    found.local.1,
                    found.a_top,
                    found.b_top
                );
                eprintln!(
                    "seam mismatch seed {} pair ({},{})-({},{}) axis {} local ({},{}) tops {} {}",
                    found.seed,
                    found.a.0,
                    found.a.1,
                    found.b.0,
                    found.b.1,
                    axis_name(found.axis),
                    found.local.0,
                    found.local.1,
                    found.a_top,
                    found.b_top
                );
            }
            None => println!("ok"),
        }
    }
}

// ---------------------------------------------------------------------------
// sweep
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tier {
    Pr,
    Nightly,
}

impl Tier {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "pr" => Ok(Self::Pr),
            "nightly" => Ok(Self::Nightly),
            other => Err(format!("invalid --tier {other:?}; expected pr or nightly")),
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Pr => "pr",
            Self::Nightly => "nightly",
        }
    }

    const fn default_seeds(self) -> usize {
        match self {
            Self::Pr => PR_SEEDS,
            Self::Nightly => NIGHTLY_SEEDS,
        }
    }
}

#[derive(Clone, Copy)]
struct Region {
    name: &'static str,
    origin: (i32, i32),
}

#[derive(Clone, Debug)]
struct CheckFailure {
    check: &'static str,
    coord: (i32, i32, i32),
    detail: String,
}

struct RegionRun {
    bytes: Vec<u8>,
    columns: u64,
    failure: Option<CheckFailure>,
}

struct SeedResult {
    seed: u64,
    hash: [u8; 32],
    columns: u64,
    failure: Option<CheckFailure>,
}

struct SweepOutcome {
    tier: &'static str,
    seeds: usize,
    regions: usize,
    window: i32,
    results: Vec<SeedResult>,
}

fn cmd_sweep(args: &[String], options: &Options) -> u8 {
    match sweep_outcome(args, options) {
        Ok(outcome) => {
            print_sweep(&outcome, options.json);
            if outcome
                .results
                .iter()
                .any(|result| result.failure.is_some())
            {
                EXIT_INVARIANT
            } else {
                EXIT_OK
            }
        }
        Err(failure) => report(&failure, options.json, "sweep"),
    }
}

fn sweep_outcome(args: &[String], options: &Options) -> Result<SweepOutcome, CliFailure> {
    let pairs = parse_pairs(args)?;
    let tier = match pairs.get("tier") {
        Some(value) => Tier::parse(value)?,
        None => {
            return Err(CliFailure::Usage(
                "sweep needs --tier pr|nightly".to_owned(),
            ));
        }
    };
    let seed_count = match pairs.get("seeds") {
        Some(value) => number::<usize>(value, "seeds")?,
        None => tier.default_seeds(),
    };
    if seed_count == 0 {
        return Err(CliFailure::Usage("--seeds must be at least 1".to_owned()));
    }
    let regions = regions(tier);
    let seeds: Vec<u64> = (1..=seed_count)
        .map(|seed| u64::try_from(seed).unwrap_or(u64::MAX))
        .collect();
    let results = sweep_seeds(&seeds, &regions, options.threads, options.order)?;
    Ok(SweepOutcome {
        tier: tier.name(),
        seeds: seed_count,
        regions: regions.len(),
        window: SWEEP_WINDOW,
        results,
    })
}

/// The `pr` and `nightly` region sets. `nightly` adds the negative edge and a
/// polar column, and the window never crosses `WORLD_HALF_EXTENT`: columns exist
/// only for `|x|, |z| < WORLD_HALF_EXTENT` (`K1.3`).
fn regions(tier: Tier) -> Vec<Region> {
    let half = description::default_v1().world_half_extent;
    let highest = half - 1;
    let positive_edge = highest - (SWEEP_WINDOW - 1);
    let negative_edge = -highest;
    match tier {
        Tier::Pr => vec![
            Region {
                name: "origin",
                origin: (0, 0),
            },
            Region {
                name: "edge",
                origin: (positive_edge, positive_edge),
            },
        ],
        Tier::Nightly => vec![
            Region {
                name: "origin",
                origin: (0, 0),
            },
            Region {
                name: "negative",
                origin: (negative_edge, negative_edge),
            },
            Region {
                name: "polar",
                origin: (0, negative_edge),
            },
        ],
    }
}

fn sweep_seeds(
    seeds: &[u64],
    regions: &[Region],
    threads: usize,
    order: Order,
) -> Result<Vec<SeedResult>, TerrainError> {
    if seeds.is_empty() {
        return Ok(Vec::new());
    }
    let shuffled_key = seeds.first().copied().unwrap_or(0);
    let sequence = permutation(seeds.len(), order, shuffled_key);
    let workers = threads.clamp(1, sequence.len());
    let chunk = sequence.len().div_ceil(workers);
    let mut slots: Vec<Option<SeedResult>> = (0..seeds.len()).map(|_| None).collect();

    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for indices in sequence.chunks(chunk) {
            handles.push(
                scope.spawn(move || -> Result<Vec<SeedResult>, TerrainError> {
                    let mut produced = Vec::with_capacity(indices.len());
                    for &index in indices {
                        produced.push(sweep_seed(seeds[index], regions)?);
                    }
                    Ok(produced)
                }),
            );
        }
        for (indices, handle) in sequence.chunks(chunk).zip(handles) {
            let produced = match handle.join() {
                Ok(result) => result?,
                Err(_) => return Err(internal("worker_panic")),
            };
            for (index, result) in indices.iter().zip(produced) {
                slots[*index] = Some(result);
            }
        }
        Ok(())
    })?;

    slots
        .into_iter()
        .collect::<Option<Vec<SeedResult>>>()
        .ok_or_else(|| internal("sweep_result"))
}

fn sweep_seed(seed: u64, regions: &[Region]) -> Result<SeedResult, TerrainError> {
    let mut bytes = Vec::new();
    let mut columns = 0_u64;
    let mut failure = None;
    for region in regions {
        let first = run_region(seed, region)?;
        let second = run_region(seed, region)?;
        // The seed hash is a pure function of the concatenated region bytes, so
        // equal bytes from two cold runs prove the two runs agree on the hash.
        if first.bytes != second.bytes && failure.is_none() {
            failure = Some(CheckFailure {
                check: "determinism",
                coord: (region.origin.0, 0, region.origin.1),
                detail: format!(
                    "two cold runs of region {} produced different blocks",
                    region.name
                ),
            });
        }
        if failure.is_none() {
            failure.clone_from(&first.failure);
        }
        columns += first.columns;
        bytes.extend_from_slice(&first.bytes);
    }
    Ok(SeedResult {
        seed,
        hash: sha256(&bytes),
        columns,
        failure,
    })
}

fn run_region(seed: u64, region: &Region) -> Result<RegionRun, TerrainError> {
    let terrain = Terrain::new(seed)?;
    let side = usize::try_from(SWEEP_WINDOW).unwrap_or(0);
    let mut bytes = Vec::with_capacity(side.saturating_mul(side).saturating_mul(COLUMN * 2));
    let mut columns = 0_u64;
    let mut failure = None;
    for row in 0..SWEEP_WINDOW {
        for column in 0..SWEEP_WINDOW {
            let x = region.origin.0 + column;
            let z = region.origin.1 + row;
            let blocks = terrain.column(x, z)?;
            columns += 1;
            for &id in &blocks {
                bytes.extend_from_slice(&id.to_le_bytes());
            }
            if failure.is_none() {
                failure = check_column(&terrain, x, z, &blocks)?;
            }
        }
    }
    Ok(RegionRun {
        bytes,
        columns,
        failure,
    })
}

/// The `R1.12` invariants this workstream owns. The first failure in canonical
/// column, then `y`, order is reported.
fn check_column(
    terrain: &Terrain,
    x: i32,
    z: i32,
    column: &Column,
) -> Result<Option<CheckFailure>, TerrainError> {
    let max_known = max_known_block();
    for (index, &id) in column.iter().enumerate() {
        let y = i32::try_from(index).unwrap_or(0);
        if id > max_known {
            return Ok(Some(CheckFailure {
                check: "block_id",
                coord: (x, y, z),
                detail: format!("id {id} > {max_known}"),
            }));
        }
        if y > MAX_TERRAIN_Y && id != AIR {
            return Ok(Some(CheckFailure {
                check: "above_max_terrain",
                coord: (x, y, z),
                detail: format!("id {id} above MAX_TERRAIN_Y {MAX_TERRAIN_Y}"),
            }));
        }
    }

    let mut water_allowed = None;
    for (index, &id) in column.iter().enumerate() {
        if id == WATER {
            let y = i32::try_from(index).unwrap_or(0);
            if y > SEA {
                if water_allowed.is_none() {
                    water_allowed = Some(water_allowed_at(terrain, x, z)?);
                }
                if water_allowed == Some(false) {
                    return Ok(Some(CheckFailure {
                        check: "water",
                        coord: (x, y, z),
                        detail: format!("water at y {y} above SEA {SEA} outside a river or lake"),
                    }));
                }
            }
        }
    }

    if column[0] == AIR {
        return Ok(Some(CheckFailure {
            check: "bedrock",
            coord: (x, 0, z),
            detail: "y=0 is air".to_owned(),
        }));
    }
    let solids = column
        .iter()
        .filter(|&&id| id != AIR && id != WATER)
        .count();
    if solids == 0 {
        return Ok(Some(CheckFailure {
            check: "solid",
            coord: (x, 0, z),
            detail: "column has no solid block".to_owned(),
        }));
    }
    Ok(None)
}

fn water_allowed_at(terrain: &Terrain, x: i32, z: i32) -> Result<bool, TerrainError> {
    let (cell_x, cell_z) = hydro_cell(x, z);
    let tile = terrain.hydro_tile(floor_div(x, HYDRO_TILE), floor_div(z, HYDRO_TILE))?;
    Ok(tile.river_width(cell_x, cell_z) > 0.5 || tile.lake_level(cell_x, cell_z) > SEA)
}

/// The hydrology tile cell of a world block, halo included. Mirrors the
/// pipeline's private mapping.
fn hydro_cell(x: i32, z: i32) -> (i32, i32) {
    let map = |block: i32| {
        floor_div(block, description::MACRO_CELL) - floor_div(block, HYDRO_TILE) * hydro::TILE_CELLS
            + hydro::BORDER_CELLS
    };
    (map(x), map(z))
}

fn print_sweep(outcome: &SweepOutcome, json: bool) {
    let SweepOutcome {
        tier,
        seeds,
        regions,
        window,
        results,
    } = outcome;
    let failures = results
        .iter()
        .filter(|result| result.failure.is_some())
        .count();
    let columns: u64 = results.iter().map(|result| result.columns).sum();
    if json {
        let mut out = format!(
            "{{\"subcommand\":\"sweep\",\"tier\":{},\"seeds\":{seeds},\"regions\":{regions},\"window\":{window},\"results\":[",
            json_str(tier)
        );
        for (index, result) in results.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let _ = write!(
                out,
                "{{\"seed\":{},\"hash\":{},\"ok\":{}",
                result.seed,
                json_str(&hex32(&result.hash)),
                result.failure.is_none()
            );
            if let Some(failure) = &result.failure {
                let _ = write!(
                    out,
                    ",\"check\":{},\"at\":[{},{},{}],\"detail\":{}",
                    json_str(failure.check),
                    failure.coord.0,
                    failure.coord.1,
                    failure.coord.2,
                    json_str(&failure.detail)
                );
            }
            out.push('}');
        }
        let _ = write!(out, "],\"columns\":{columns},\"failures\":{failures}}}");
        println!("{out}");
    } else {
        println!("tier {tier}");
        println!("seeds {seeds}");
        println!("regions {regions}");
        println!("window {window}");
        for result in results {
            let hash = hex32(&result.hash);
            match &result.failure {
                Some(failure) => {
                    println!(
                        "seed {} {} FAIL {} at ({},{},{}) {}",
                        result.seed,
                        hash,
                        failure.check,
                        failure.coord.0,
                        failure.coord.1,
                        failure.coord.2,
                        failure.detail
                    );
                }
                None => println!("seed {} {} ok", result.seed, hash),
            }
        }
        println!("summary: seeds={seeds} regions={regions} columns={columns} failures={failures}");
    }
}

// ---------------------------------------------------------------------------
// counters
// ---------------------------------------------------------------------------

/// Default path of the checked-in PR counter baseline, anchored to the crate
/// root so the command works from any working directory.
const COUNTER_BASELINE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/golden/terrain-counters.json"
);
/// Relative drift, in percent, at which the counter comparison fails.
const COUNTER_DRIFT_PERCENT: u64 = 5;
/// Schema tag written to and required from a counter baseline.
const COUNTER_SCHEMA: &str = "valescent/terrain-counters@1";
/// Pinned `Q1.1` counters this workload can produce deterministically.
///
/// Excluded:
///
/// * every `_p50`/`_p95` latency and `upload_bytes_per_s` - they read the wall
///   clock and move with thread count and scheduling;
/// * `queue_depth`, `in_flight_jobs`, `panics_caught`, `stuck_threads`,
///   `dropped_before_start`, `discarded_stale`, and `applied` - residency and
///   worker-health state that the headless generator never records, so a
///   baseline would pin zeros and catch nothing;
/// * `upload_bytes`, `meshes`, `quads`, `light_repairs`, `lod1_columns`, and
///   `lod2_columns` - written only by the render, mesh, light, and LOD paths
///   that stop before this binary.
///
/// The four that remain are pure functions of the generated column set: the
/// three tile-cache gauges published by `Terrain::publish_column_gauges`, and
/// `lod0_columns`, which counts distinct LOD0 columns with a generated
/// section (zero here because this workload stops at columns).
const COUNTER_METRICS: &[&str] = &[
    "lod0_columns",
    "queued_columns",
    "ready_columns",
    "resident_columns",
];

/// One counter snapshot, keyed by counter name.
type CounterValues = BTreeMap<String, u64>;

/// Measured transect widths, bucketed by unordered biome pair.
type PairWidths = BTreeMap<(u8, u8), Vec<f64>>;

struct CounterDrift {
    name: String,
    baseline: u64,
    current: u64,
}

struct CountersOutcome {
    tier: &'static str,
    seeds: usize,
    action: &'static str,
    counters: CounterValues,
    drifts: Vec<CounterDrift>,
}

fn cmd_counters(args: &[String], options: &Options) -> u8 {
    match counters_outcome(args, options) {
        Ok(outcome) => {
            print_counters(&outcome, options.json);
            if outcome.drifts.is_empty() {
                EXIT_OK
            } else {
                EXIT_INVARIANT
            }
        }
        Err(failure) => report(&failure, options.json, "counters"),
    }
}

fn counters_outcome(args: &[String], options: &Options) -> Result<CountersOutcome, CliFailure> {
    let mut cursor = Cursor::new(args);
    let mut tier = None;
    let mut seed_count = None;
    let mut file = None;
    let mut update = false;
    while let Some(argument) = cursor.arg() {
        match argument {
            "--tier" => tier = Some(Tier::parse(cursor.value("tier")?)?),
            "--seeds" => seed_count = Some(number::<usize>(cursor.value("seeds")?, "seeds")?),
            "--file" => file = Some(cursor.value("file")?.to_owned()),
            "--update" => update = true,
            other => {
                return Err(CliFailure::Usage(format!(
                    "unexpected argument {other:?} for counters"
                )));
            }
        }
    }
    let tier =
        tier.ok_or_else(|| CliFailure::Usage("counters needs --tier pr|nightly".to_owned()))?;
    if file.is_none() && tier != Tier::Pr {
        return Err(CliFailure::Usage(
            "the checked-in baseline is the pr tier; pass --file <path> for nightly".to_owned(),
        ));
    }
    let seed_count = seed_count.unwrap_or_else(|| tier.default_seeds());
    if seed_count == 0 {
        return Err(CliFailure::Usage("--seeds must be at least 1".to_owned()));
    }
    let path = file.as_deref().unwrap_or(COUNTER_BASELINE_PATH);

    let regions = regions(tier);
    let seeds: Vec<u64> = (1..=seed_count)
        .map(|seed| u64::try_from(seed).unwrap_or(u64::MAX))
        .collect();
    let counters = counters_seeds(&seeds, &regions, options.threads, options.order)?;

    if update {
        write_counter_baseline(Path::new(path), tier, seed_count, &counters)?;
        return Ok(CountersOutcome {
            tier: tier.name(),
            seeds: seed_count,
            action: "update",
            counters,
            drifts: Vec::new(),
        });
    }

    let baseline = read_counter_baseline(Path::new(path))?;
    if baseline.tier != tier.name() || baseline.seeds != u64::try_from(seed_count).unwrap_or(0) {
        return Err(CliFailure::Usage(format!(
            "{path}: baseline records tier {} with {} seeds, but this run is tier {} with {seed_count}; use --update to replace it",
            baseline.tier,
            baseline.seeds,
            tier.name()
        )));
    }
    let drifts = compare_counters(&baseline.counters, &counters);
    Ok(CountersOutcome {
        tier: tier.name(),
        seeds: seed_count,
        action: "check",
        counters,
        drifts,
    })
}

/// Generates every PR (or nightly) region once per seed and sums the
/// deterministic counters. Results are assembled in canonical seed order, so
/// `--threads` and `--order` cannot change them.
fn counters_seeds(
    seeds: &[u64],
    regions: &[Region],
    threads: usize,
    order: Order,
) -> Result<CounterValues, TerrainError> {
    if seeds.is_empty() {
        return Ok(BTreeMap::new());
    }
    let shuffled_key = seeds.first().copied().unwrap_or(0);
    let sequence = permutation(seeds.len(), order, shuffled_key);
    let workers = threads.clamp(1, sequence.len());
    let chunk = sequence.len().div_ceil(workers);
    let mut slots: Vec<Option<CounterValues>> = (0..seeds.len()).map(|_| None).collect();

    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for indices in sequence.chunks(chunk) {
            handles.push(
                scope.spawn(move || -> Result<Vec<CounterValues>, TerrainError> {
                    let mut produced = Vec::with_capacity(indices.len());
                    for &index in indices {
                        produced.push(counters_seed(seeds[index], regions)?);
                    }
                    Ok(produced)
                }),
            );
        }
        for (indices, handle) in sequence.chunks(chunk).zip(handles) {
            let produced = match handle.join() {
                Ok(result) => result?,
                Err(_) => return Err(internal("worker_panic")),
            };
            for (index, result) in indices.iter().zip(produced) {
                slots[*index] = Some(result);
            }
        }
        Ok(())
    })?;

    let mut totals = BTreeMap::new();
    for slot in slots.into_iter().flatten() {
        for (name, value) in slot {
            let total = totals.entry(name).or_insert(0_u64);
            *total = total.saturating_add(value);
        }
    }
    Ok(totals)
}

fn counters_seed(seed: u64, regions: &[Region]) -> Result<CounterValues, TerrainError> {
    let terrain = Terrain::new(seed)?;
    let mut counts = Counts::default();
    let mut columns = 0_u64;
    for region in regions {
        for row in 0..SWEEP_WINDOW {
            for column in 0..SWEEP_WINDOW {
                let blocks = terrain.column(region.origin.0 + column, region.origin.1 + row)?;
                columns += 1;
                for &id in &blocks {
                    counts.add(id);
                }
            }
        }
    }

    let mut out = BTreeMap::new();
    out.insert("blocks_air".to_owned(), counts.air);
    out.insert("blocks_solid".to_owned(), counts.solid);
    out.insert("blocks_water".to_owned(), counts.water);
    out.insert("columns".to_owned(), columns);
    for name in COUNTER_METRICS {
        out.insert(
            (*name).to_owned(),
            terrain.counters().get(name).unwrap_or(0),
        );
    }
    Ok(out)
}

struct CounterBaseline {
    tier: String,
    seeds: u64,
    counters: CounterValues,
}

fn read_counter_baseline(path: &Path) -> Result<CounterBaseline, CliFailure> {
    let json = read_json(path)?;
    let name = path.to_string_lossy();
    let object = expect_object(&json, &name)?;
    let schema = string_field(object, "schema", &name)?;
    if schema != COUNTER_SCHEMA {
        return Err(CliFailure::Usage(format!(
            "{name}: schema must be {COUNTER_SCHEMA:?}, found {schema:?}"
        )));
    }
    let tier = string_field(object, "tier", &name)?;
    let seeds = integer_field::<u64>(object, "seeds", &name)?;
    let values = object
        .get("counters")
        .and_then(Json::as_object)
        .ok_or_else(|| CliFailure::Usage(format!("{name}: missing \"counters\" object")))?;
    let mut counters = BTreeMap::new();
    for (key, value) in values {
        let Json::Number(raw) = value else {
            return Err(CliFailure::Usage(format!(
                "{name}: counter {key:?} must be an integer"
            )));
        };
        let parsed = raw
            .parse::<u64>()
            .map_err(|_| CliFailure::Usage(format!("{name}: invalid counter {key:?} {raw:?}")))?;
        counters.insert(key.clone(), parsed);
    }
    Ok(CounterBaseline {
        tier,
        seeds,
        counters,
    })
}

fn write_counter_baseline(
    path: &Path,
    tier: Tier,
    seeds: usize,
    counters: &CounterValues,
) -> Result<(), CliFailure> {
    let mut text = format!(
        "{{\n  \"schema\": {},\n  \"tier\": {},\n  \"seeds\": {seeds},\n  \"counters\": {{\n",
        json_str(COUNTER_SCHEMA),
        json_str(tier.name())
    );
    for (index, (name, value)) in counters.iter().enumerate() {
        let comma = if index + 1 < counters.len() { "," } else { "" };
        let _ = writeln!(text, "    {}: {value}{comma}", json_str(name));
    }
    text.push_str("  }\n}\n");
    fs::write(path, text)
        .map_err(|error| CliFailure::Usage(format!("cannot write {}: {error}", path.display())))
}

/// Compares a baseline with a fresh snapshot. A counter may move by at most
/// [`COUNTER_DRIFT_PERCENT`] percent; a baseline of zero must stay zero.
fn compare_counters(baseline: &CounterValues, current: &CounterValues) -> Vec<CounterDrift> {
    let mut names: Vec<&String> = baseline.keys().chain(current.keys()).collect();
    names.sort();
    names.dedup();
    let mut drifts = Vec::new();
    for name in names {
        let before = baseline.get(name).copied().unwrap_or(0);
        let now = current.get(name).copied().unwrap_or(0);
        let drifted = if before == 0 {
            now != 0
        } else {
            now.abs_diff(before).saturating_mul(100) > before.saturating_mul(COUNTER_DRIFT_PERCENT)
        };
        if drifted {
            drifts.push(CounterDrift {
                name: name.clone(),
                baseline: before,
                current: now,
            });
        }
    }
    drifts
}

fn print_counters(outcome: &CountersOutcome, json: bool) {
    let CountersOutcome {
        tier,
        seeds,
        action,
        counters,
        drifts,
    } = outcome;
    if json {
        let mut out = format!(
            "{{\"subcommand\":\"counters\",\"tier\":{},\"seeds\":{seeds},\"action\":{},\"counters\":[",
            json_str(tier),
            json_str(action)
        );
        for (index, (name, value)) in counters.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let _ = write!(out, "{{\"name\":{},\"value\":{value}}}", json_str(name));
        }
        out.push(']');
        if drifts.is_empty() {
            out.push_str(",\"ok\":true}");
        } else {
            out.push_str(",\"ok\":false,\"drift\":[");
            for (index, drift) in drifts.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                let _ = write!(
                    out,
                    "{{\"name\":{},\"baseline\":{},\"current\":{}}}",
                    json_str(&drift.name),
                    drift.baseline,
                    drift.current
                );
            }
            out.push_str("]}");
        }
        println!("{out}");
    } else {
        println!("tier {tier}");
        println!("seeds {seeds}");
        println!("action {action}");
        for (name, value) in counters {
            println!("{name} {value}");
        }
        if drifts.is_empty() {
            println!("ok");
        } else {
            for drift in drifts {
                println!(
                    "drift {} baseline {} current {}",
                    drift.name, drift.baseline, drift.current
                );
                eprintln!(
                    "counter drift: {} baseline {} current {} exceeds {COUNTER_DRIFT_PERCENT}%",
                    drift.name, drift.baseline, drift.current
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// calibrate-blend
// ---------------------------------------------------------------------------

/// Default anchor stride, in blocks. Must be a multiple of [`MACRO_CELL`].
const DEFAULT_BLEND_STEP: i32 = 16;
/// How far a transect marches from its anchor in either direction, in blocks.
const BLEND_MARCH_BLOCKS: i32 = 256;
/// The deep end of a measured transect: `w = 0.95` (`E3.6`).
const BLEND_END_WEIGHT: f32 = 0.95;
/// The largest sampled `w` still treated as the `w = 0.5` border.
///
/// `q` is quantised, so a bilinear sample only reaches exactly `0.5` where all
/// four surrounding cells have `q = 0`. A local minimum at `w <= 0.51` (`q` up
/// to about 17) cannot be more than one block from the true border.
const BLEND_BORDER_WEIGHT: f32 = 0.51;
/// Lower bound of the accepted per-pair median, in blocks.
const BLEND_MEDIAN_MIN: f64 = 20.0;
/// Upper bound of the accepted per-pair median, in blocks.
const BLEND_MEDIAN_MAX: f64 = 28.0;
/// Lower bound of the accepted per-pair p10, in blocks.
const BLEND_P10_MIN: f64 = 8.0;
/// Upper bound of the accepted per-pair p90, in blocks.
const BLEND_P90_MAX: f64 = 64.0;
/// Transects a pair needs before its acceptance is judged; fewer are reported
/// only (`E3.6`).
const BLEND_MIN_SAMPLES: usize = 20;
/// Cells per field tile, `TILE_BLOCKS / MACRO_CELL` (`B4.2`).
const CLIMATE_CELLS_PER_TILE: i32 = TILE_BLOCKS / MACRO_CELL;

/// `w = 0.5 + 0.5 * smoothstep01(q / 255)`, the exact expression
/// `ClimateTile::biome_pick` evaluates (`E3.3`, `E3.5`).
fn blend_weight(q: f32) -> f32 {
    0.5 + 0.5 * smoothstep01(q / 255.0)
}

#[derive(Clone, Debug)]
struct PairSummary {
    low: u8,
    high: u8,
    samples: usize,
    median: f64,
    p10: f64,
    p90: f64,
}

impl PairSummary {
    fn assessed(&self) -> bool {
        self.samples >= BLEND_MIN_SAMPLES
    }

    fn accepted(&self) -> bool {
        self.assessed()
            && (BLEND_MEDIAN_MIN..=BLEND_MEDIAN_MAX).contains(&self.median)
            && self.p10 >= BLEND_P10_MIN
            && self.p90 <= BLEND_P90_MAX
    }
}

struct BlendOutcome {
    seeds: usize,
    from: (i32, i32),
    size: i32,
    step: i32,
    pairs: Vec<PairSummary>,
}

impl BlendOutcome {
    fn accepted(&self) -> bool {
        let assessed: Vec<&PairSummary> =
            self.pairs.iter().filter(|pair| pair.assessed()).collect();
        !assessed.is_empty() && assessed.iter().all(|pair| pair.accepted())
    }
}

fn cmd_calibrate_blend(args: &[String], options: &Options) -> u8 {
    match calibrate_outcome(args, options) {
        Ok(outcome) => {
            print_calibrate(&outcome, options.json);
            if outcome.accepted() {
                EXIT_OK
            } else {
                EXIT_INVARIANT
            }
        }
        Err(failure) => report(&failure, options.json, "calibrate-blend"),
    }
}

fn calibrate_outcome(args: &[String], options: &Options) -> Result<BlendOutcome, CliFailure> {
    let mut cursor = Cursor::new(args);
    let mut seeds = None;
    let mut from = None;
    let mut size = None;
    let mut step = None;
    while let Some(argument) = cursor.arg() {
        match argument {
            "--seeds" => seeds = Some(number::<usize>(cursor.value("seeds")?, "seeds")?),
            "--from" => {
                let x = number::<i32>(cursor.value("from x")?, "from x")?;
                let z = number::<i32>(cursor.value("from z")?, "from z")?;
                from = Some((x, z));
            }
            "--size" => size = Some(number::<i32>(cursor.value("size")?, "size")?),
            "--step" => step = Some(number::<i32>(cursor.value("step")?, "step")?),
            other => {
                return Err(CliFailure::Usage(format!(
                    "unexpected argument {other:?} for calibrate-blend"
                )));
            }
        }
    }
    let seeds =
        seeds.ok_or_else(|| CliFailure::Usage("calibrate-blend needs --seeds <n>".to_owned()))?;
    if seeds == 0 {
        return Err(CliFailure::Usage("--seeds must be at least 1".to_owned()));
    }
    let from = from.unwrap_or((
        -description::default_v1().world_half_extent,
        -description::default_v1().world_half_extent,
    ));
    let size = size.unwrap_or(description::default_v1().world_half_extent * 2);
    if size <= 0 {
        return Err(CliFailure::Usage(format!(
            "--size must be at least 1, found {size}"
        )));
    }
    let step = step.unwrap_or(DEFAULT_BLEND_STEP);
    if step <= 0 || step % MACRO_CELL != 0 {
        return Err(CliFailure::Usage(format!(
            "--step must be a positive multiple of {MACRO_CELL}, found {step}"
        )));
    }
    if step > size {
        return Err(CliFailure::Usage(format!(
            "--step {step} is larger than --size {size}"
        )));
    }
    let half = i64::from(description::default_v1().world_half_extent);
    let end_x = i64::from(from.0) + i64::from(size);
    let end_z = i64::from(from.1) + i64::from(size);
    if i64::from(from.0) < -half || i64::from(from.1) < -half || end_x > half || end_z > half {
        return Err(CliFailure::Usage(format!(
            "window {}..{} x {}..{} is outside the {}-block half extent",
            from.0, end_x, from.1, end_z, half
        )));
    }

    let seed_values: Vec<u64> = (1..=seeds)
        .map(|seed| u64::try_from(seed).unwrap_or(u64::MAX))
        .collect();
    let widths = blend_seeds(
        &seed_values,
        from,
        size,
        step,
        options.threads,
        options.order,
    )?;
    Ok(BlendOutcome {
        seeds,
        from,
        size,
        step,
        pairs: summarise_pairs(&widths),
    })
}

/// Measures every seed, then merges the per-pair widths in canonical seed order.
fn blend_seeds(
    seeds: &[u64],
    from: (i32, i32),
    size: i32,
    step: i32,
    threads: usize,
    order: Order,
) -> Result<PairWidths, TerrainError> {
    if seeds.is_empty() {
        return Ok(BTreeMap::new());
    }
    let shuffled_key = seeds.first().copied().unwrap_or(0);
    let sequence = permutation(seeds.len(), order, shuffled_key);
    let workers = threads.clamp(1, sequence.len());
    let chunk = sequence.len().div_ceil(workers);
    let mut slots: Vec<Option<PairWidths>> = (0..seeds.len()).map(|_| None).collect();

    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for indices in sequence.chunks(chunk) {
            handles.push(
                scope.spawn(move || -> Result<Vec<PairWidths>, TerrainError> {
                    let mut produced = Vec::with_capacity(indices.len());
                    for &index in indices {
                        produced.push(blend_seed(seeds[index], from, size, step)?);
                    }
                    Ok(produced)
                }),
            );
        }
        for (indices, handle) in sequence.chunks(chunk).zip(handles) {
            let produced = match handle.join() {
                Ok(result) => result?,
                Err(_) => return Err(internal("worker_panic")),
            };
            for (index, result) in indices.iter().zip(produced) {
                slots[*index] = Some(result);
            }
        }
        Ok(())
    })?;

    let mut merged: PairWidths = BTreeMap::new();
    for slot in slots.into_iter().flatten() {
        for (pair, mut widths) in slot {
            merged.entry(pair).or_default().append(&mut widths);
        }
    }
    Ok(merged)
}

/// The anchor grid, in cells: every `step / MACRO_CELL` cells whose centre
/// falls inside `[from, from + size)`.
fn anchor_cells(from: (i32, i32), size: i32, step: i32) -> Vec<(i32, i32)> {
    let stride = step / MACRO_CELL;
    let min_x = ceil_div(from.0 - 2, MACRO_CELL);
    let max_x = floor_div(from.0 + size - 3, MACRO_CELL);
    let min_z = ceil_div(from.1 - 2, MACRO_CELL);
    let max_z = floor_div(from.1 + size - 3, MACRO_CELL);
    if min_x > max_x || min_z > max_z {
        return Vec::new();
    }
    let first_x = min_x.div_euclid(stride) * stride;
    let first_z = min_z.div_euclid(stride) * stride;
    let mut cells = Vec::new();
    let mut cell_z = first_z;
    while cell_z <= max_z {
        let mut cell_x = first_x;
        while cell_x <= max_x {
            cells.push((cell_x, cell_z));
            cell_x += stride;
        }
        cell_z += stride;
    }
    cells
}

fn ceil_div(value: i32, divisor: i32) -> i32 {
    -floor_div(-value, divisor)
}

fn blend_seed(
    seed: u64,
    from: (i32, i32),
    size: i32,
    step: i32,
) -> Result<PairWidths, TerrainError> {
    let terrain = Terrain::new(seed)?;
    let half = description::default_v1().world_half_extent;
    let mut pairs: PairWidths = BTreeMap::new();
    for (cell_x, cell_z) in anchor_cells(from, size, step) {
        let anchor_x = cell_x * MACRO_CELL + 2;
        let anchor_z = cell_z * MACRO_CELL + 2;
        // Keep every sample and the 5x5 gradient stencil inside the finite
        // world. A transect that cannot reach 128 blocks from a border is not
        // measured.
        let reach = (half - 1 - anchor_x.abs()).min(half - 1 - anchor_z.abs());
        let march = BLEND_MARCH_BLOCKS.min(reach);
        if march < 128 {
            continue;
        }
        let Some(sample) = anchor_sample(&terrain, cell_x, cell_z)? else {
            continue;
        };
        let mut profile = Vec::with_capacity(usize::try_from(march * 2 + 1).unwrap_or(0));
        for offset in -march..=march {
            let cell_sample_x = f64::from(cell_x) + sample.direction.0 * f64::from(offset) / 4.0;
            let cell_sample_z = f64::from(cell_z) + sample.direction.1 * f64::from(offset) / 4.0;
            let q = bilinear_margin(&terrain, cell_sample_x, cell_sample_z)?;
            let weight = blend_weight(q);
            profile.push((f64::from(offset), weight));
        }
        if let Some(width) = transect_width(&profile) {
            pairs
                .entry((sample.low, sample.high))
                .or_default()
                .push(width);
        }
    }
    Ok(pairs)
}

struct AnchorSample {
    low: u8,
    high: u8,
    /// Unit direction of increasing margin, in block space.
    direction: (f64, f64),
}

fn anchor_sample(
    terrain: &Terrain,
    cell_x: i32,
    cell_z: i32,
) -> Result<Option<AnchorSample>, TerrainError> {
    let tile_x = floor_div(cell_x, CLIMATE_CELLS_PER_TILE);
    let tile_z = floor_div(cell_z, CLIMATE_CELLS_PER_TILE);
    let local_x = cell_x - tile_x * CLIMATE_CELLS_PER_TILE;
    let local_z = cell_z - tile_z * CLIMATE_CELLS_PER_TILE;
    if !(0..CLIMATE_CELLS_PER_TILE).contains(&local_x)
        || !(0..CLIMATE_CELLS_PER_TILE).contains(&local_z)
    {
        return Ok(None);
    }
    let tile = terrain.tile(tile_x, tile_z)?;
    let best = tile.climate.best(local_x, local_z);
    let second = tile.climate.second(local_x, local_z);
    if best == second {
        return Ok(None);
    }
    let q = tile.climate.margin(local_x, local_z);
    if blend_weight(f32::from(q)) >= BLEND_END_WEIGHT {
        return Ok(None);
    }

    // A least-squares plane over a 5x5 cell window. Central differences of the
    // quantised margin are too noisy near the border (where successive cells
    // differ by one q step) and skew the transect direction.
    let mut sum_x = 0.0_f32;
    let mut sum_z = 0.0_f32;
    let mut square_x = 0.0_f32;
    let mut square_z = 0.0_f32;
    for delta_z in -2..=2 {
        for delta_x in -2..=2 {
            let neighbour = f32::from(cell_margin(terrain, cell_x + delta_x, cell_z + delta_z)?);
            let dx = f32::from(i8::try_from(delta_x).unwrap_or(0));
            let dz = f32::from(i8::try_from(delta_z).unwrap_or(0));
            sum_x += neighbour * dx;
            sum_z += neighbour * dz;
            square_x += dx * dx;
            square_z += dz * dz;
        }
    }
    let gradient_x = sum_x / square_x;
    let gradient_z = sum_z / square_z;
    let magnitude = (gradient_x * gradient_x + gradient_z * gradient_z).sqrt();
    if magnitude == 0.0 || !magnitude.is_finite() {
        return Ok(None);
    }
    Ok(Some(AnchorSample {
        low: best.min(second),
        high: best.max(second),
        direction: (
            f64::from(gradient_x / magnitude),
            f64::from(gradient_z / magnitude),
        ),
    }))
}

/// Bilinear interpolation of the quantised margin over the cell lattice.
///
/// The cell centre for cell `c` is block `4c + 2`, so a block position `b`
/// maps to continuous cell coordinate `(b - 2) / 4`.
#[allow(clippy::cast_possible_truncation)]
fn bilinear_margin(terrain: &Terrain, cell_x: f64, cell_z: f64) -> Result<f32, TerrainError> {
    // Keep the four reads on cell centres inside the finite world: the last
    // centre is block `half - 2`, so a transect near the rim never builds a tile
    // outside it.
    let half = description::default_v1().world_half_extent;
    let min_cell = f64::from(ceil_div(-half + 1 - 2, MACRO_CELL));
    let max_cell = f64::from(floor_div(half - 1 - 2, MACRO_CELL));
    let cell_x = cell_x.clamp(min_cell, max_cell);
    let cell_z = cell_z.clamp(min_cell, max_cell);
    let x0 = cell_x.floor() as i32;
    let z0 = cell_z.floor() as i32;
    let x1 = (x0 + 1).min(max_cell as i32);
    let z1 = (z0 + 1).min(max_cell as i32);
    let fx = (cell_x - f64::from(x0)) as f32;
    let fz = (cell_z - f64::from(z0)) as f32;
    let q00 = f32::from(cell_margin(terrain, x0, z0)?);
    let q10 = f32::from(cell_margin(terrain, x1, z0)?);
    let q01 = f32::from(cell_margin(terrain, x0, z1)?);
    let q11 = f32::from(cell_margin(terrain, x1, z1)?);
    let top = q00 + (q10 - q00) * fx;
    let bottom = q01 + (q11 - q01) * fx;
    Ok(top + (bottom - top) * fz)
}

fn cell_margin(terrain: &Terrain, cell_x: i32, cell_z: i32) -> Result<u8, TerrainError> {
    let tile_x = floor_div(cell_x, CLIMATE_CELLS_PER_TILE);
    let tile_z = floor_div(cell_z, CLIMATE_CELLS_PER_TILE);
    let tile = terrain.tile(tile_x, tile_z)?;
    Ok(tile.climate.margin(
        cell_x - tile_x * CLIMATE_CELLS_PER_TILE,
        cell_z - tile_z * CLIMATE_CELLS_PER_TILE,
    ))
}

/// The distance between the `w = 0.5` and `w = 0.95` crossings, doubled.
///
/// `profile` is `(block offset, w)`, sorted by offset, with offset `0` at the
/// anchor. The anchor is inside the blend band (`w >= 0.5`, `w < 0.95`), so the
/// rising crossing lies at a non-negative offset and the border crossing at a
/// non-positive one. Returns `None` when either crossing is outside the
/// sampled profile.
///
/// `q` is quantised, so `w = 0.5` is only reached exactly at `q = 0`; between
/// cell centres the bilinear minimum can sit a fraction of a block above it.
/// The border is therefore the first local minimum behind the anchor, accepted
/// when it is within [`BLEND_BORDER_WEIGHT`], which bounds the error below one
/// block.
fn transect_width(profile: &[(f64, f32)]) -> Option<f64> {
    let center = profile.iter().position(|(offset, _)| *offset == 0.0)?;

    let mut previous: Option<(f64, f32)> = None;
    let mut rising = None;
    for &(offset, weight) in &profile[center..] {
        if let Some((low_offset, low_weight)) = previous
            && low_weight < BLEND_END_WEIGHT
            && weight >= BLEND_END_WEIGHT
        {
            rising = Some(crossing(
                (low_offset, low_weight),
                (offset, weight),
                BLEND_END_WEIGHT,
            ));
        }
        if weight >= BLEND_END_WEIGHT {
            break;
        }
        previous = Some((offset, weight));
    }
    let rising = rising?;

    let (_, anchor_weight) = profile[center];
    let mut border = (0.0_f64, anchor_weight);
    let mut descended = false;
    for &(offset, weight) in profile[..center].iter().rev() {
        if weight < border.1 {
            border = (offset, weight);
            descended = true;
        } else if descended && weight > border.1 {
            break;
        }
    }
    if border.1 > BLEND_BORDER_WEIGHT {
        return None;
    }

    let width = (rising - border.0) * 2.0;
    (width > 0.0 && width.is_finite()).then_some(width)
}

fn crossing(a: (f64, f32), b: (f64, f32), target: f32) -> f64 {
    let span = b.1 - a.1;
    let fraction = if span == 0.0 {
        0.0
    } else {
        f64::from((target - a.1) / span).clamp(0.0, 1.0)
    };
    a.0 + (b.0 - a.0) * fraction
}

fn summarise_pairs(widths: &PairWidths) -> Vec<PairSummary> {
    widths
        .iter()
        .map(|(&(first, second), values)| {
            let mut sorted = values.clone();
            sorted.sort_by(f64::total_cmp);
            PairSummary {
                low: first.min(second),
                high: first.max(second),
                samples: sorted.len(),
                median: percentile(&sorted, 0.5),
                p10: percentile(&sorted, 0.1),
                p90: percentile(&sorted, 0.9),
            }
        })
        .collect()
}

/// Linear interpolation between order statistics: `index = p * (n - 1)`.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    let Some(last) = sorted.len().checked_sub(1) else {
        return 0.0;
    };
    if last == 0 {
        return sorted[0];
    }
    let position = fraction.clamp(0.0, 1.0) * last as f64;
    let low = position.floor() as usize;
    let high = position.ceil() as usize;
    if low == high {
        return sorted[low];
    }
    let blend = position - low as f64;
    sorted[low] + (sorted[high] - sorted[low]) * blend
}

fn fixed2(value: f64) -> String {
    let value = if value == 0.0 { 0.0 } else { value };
    format!("{value:.2}")
}

fn calibrate_json(outcome: &BlendOutcome) -> String {
    let BlendOutcome {
        seeds,
        from,
        size,
        step,
        pairs,
    } = outcome;
    let mut out = format!(
        "{{\"subcommand\":\"calibrate-blend\",\"seeds\":{seeds},\"from\":[{},{}],\"size\":{size},\"step\":{step},\"pairs\":[",
        from.0, from.1
    );
    for (index, pair) in pairs.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "{{\"pair\":[{},{}],\"samples\":{},\"median_blocks\":{},\"p10_blocks\":{},\"p90_blocks\":{},\"assessed\":{},\"ok\":{}}}",
            pair.low,
            pair.high,
            pair.samples,
            fixed2(pair.median),
            fixed2(pair.p10),
            fixed2(pair.p90),
            pair.assessed(),
            pair.accepted()
        );
    }
    let _ = write!(out, "],\"ok\":{}}}", outcome.accepted());
    out
}

fn print_calibrate(outcome: &BlendOutcome, json: bool) {
    let BlendOutcome {
        seeds,
        from,
        size,
        step,
        pairs,
    } = outcome;
    let accepted = outcome.accepted();
    if json {
        println!("{}", calibrate_json(outcome));
    } else {
        println!(
            "calibrate-blend seeds {seeds} from {},{} size {size} step {step}",
            from.0, from.1
        );
        println!("pair samples median p10 p90 ok");
        for pair in pairs {
            println!(
                "{}-{} {} {} {} {} {}",
                pair.low,
                pair.high,
                pair.samples,
                fixed2(pair.median),
                fixed2(pair.p10),
                fixed2(pair.p90),
                if !pair.assessed() {
                    "n/a"
                } else if pair.accepted() {
                    "yes"
                } else {
                    "no"
                }
            );
        }
        if accepted {
            println!("ok");
        } else {
            if pairs.is_empty() {
                eprintln!("calibrate-blend measured no biome-pair borders in this window");
            }
            for pair in pairs
                .iter()
                .filter(|pair| pair.assessed() && !pair.accepted())
            {
                eprintln!(
                    "blend pair {}-{} out of acceptance: median {} p10 {} p90 {} (need median {}..={}, p10 >= {}, p90 <= {})",
                    pair.low,
                    pair.high,
                    fixed2(pair.median),
                    fixed2(pair.p10),
                    fixed2(pair.p90),
                    fixed2(BLEND_MEDIAN_MIN),
                    fixed2(BLEND_MEDIAN_MAX),
                    fixed2(BLEND_P10_MIN),
                    fixed2(BLEND_P90_MAX)
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// describe / describe-diff
// ---------------------------------------------------------------------------

struct DescribeWriteOutcome {
    file: String,
    hash: [u8; 32],
}

fn cmd_describe(args: &[String], options: &Options) -> u8 {
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

fn cmd_describe_diff(args: &[String], options: &Options) -> u8 {
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
/// The generator version this build evaluates (`T1.2`).
const SHIPPED_GENERATOR_VERSION: u16 = 2;
/// Upper bound on recursive section discovery, so a symlink loop cannot hang.
const MAX_SECTION_DEPTH: usize = 8;
/// Drift details printed before the report is elided.
const DETAIL_LIMIT: usize = 4;

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
fn baseline_hash(section: &Section) -> [u8; 32] {
    section::baseline_hash(section)
}

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

fn cmd_verify(args: &[String], options: &Options) -> u8 {
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

// ---------------------------------------------------------------------------
// repair
// ---------------------------------------------------------------------------

fn cmd_repair(args: &[String], options: &Options) -> u8 {
    match repair_outcome(args) {
        Ok((key, outcome)) => {
            let name = format!("c.{}.{}.{}", key.column[0], key.column[1], key.section);
            if options.json {
                println!(
                    "{{\"ok\":true,\"section\":{},\"outcome\":{}}}",
                    json_str(&name),
                    json_str(repair_name(&outcome))
                );
            } else {
                println!("{name}: {}", repair_name(&outcome));
            }
            EXIT_OK
        }
        Err(failure) => report(&failure, options.json, "repair"),
    }
}

fn repair_name(outcome: &RepairOutcome) -> &'static str {
    match outcome {
        RepairOutcome::Restored(_) => "restored",
        RepairOutcome::Quarantined => "quarantined",
        RepairOutcome::Salvaged(_) => "salvaged (disk unchanged)",
    }
}

fn repair_outcome(args: &[String]) -> Result<(SectionKey, RepairOutcome), CliFailure> {
    let mut cursor = Cursor::new(args);
    let mut world = None;
    let mut key = None;
    let mut action = None;
    while let Some(argument) = cursor.arg() {
        match argument {
            "--world" => {
                if world.is_some() {
                    return Err(CliFailure::Usage("duplicate --world".to_owned()));
                }
                world = Some(PathBuf::from(cursor.value("world")?));
            }
            "--section" => {
                if key.is_some() {
                    return Err(CliFailure::Usage("duplicate --section".to_owned()));
                }
                key = Some(parse_repair_key(cursor.value("section")?)?);
            }
            "--action" => {
                if action.is_some() {
                    return Err(CliFailure::Usage("duplicate --action".to_owned()));
                }
                action = Some(match cursor.value("action")? {
                    "restore" => RepairAction::RestoreBackup,
                    "quarantine" => RepairAction::Quarantine,
                    "salvage" => RepairAction::Salvage,
                    other => {
                        return Err(CliFailure::Usage(format!(
                            "invalid --action {other:?}; expected restore, quarantine, or salvage"
                        )));
                    }
                });
            }
            other => {
                return Err(CliFailure::Usage(format!(
                    "unexpected argument {other:?} for repair"
                )));
            }
        }
    }
    let world = world.ok_or_else(|| CliFailure::Usage("repair needs --world <dir>".to_owned()))?;
    let key =
        key.ok_or_else(|| CliFailure::Usage("repair needs --section <cx,cz,sy>".to_owned()))?;
    let action =
        action.ok_or_else(|| CliFailure::Usage("repair needs --action <action>".to_owned()))?;
    let store = SectionStore::new(world, content::v1_default());
    let outcome = store.repair(key, action)?;
    Ok((key, outcome))
}

fn parse_repair_key(value: &str) -> Result<SectionKey, String> {
    let [cx, cz, sy]: [&str; 3] = value
        .split(',')
        .collect::<Vec<_>>()
        .try_into()
        .map_err(|_| format!("invalid --section {value:?}; expected cx,cz,sy"))?;
    let column = [
        cx.parse::<i32>()
            .map_err(|_| format!("invalid section column x {cx:?}"))?,
        cz.parse::<i32>()
            .map_err(|_| format!("invalid section column z {cz:?}"))?,
    ];
    let section = sy
        .parse::<u8>()
        .map_err(|_| format!("invalid section slot {sy:?}"))?;
    if u32::from(section) >= SECTIONS_PER_COLUMN {
        return Err(format!(
            "section slot {section} is outside 0..{SECTIONS_PER_COLUMN}"
        ));
    }
    Ok(SectionKey { column, section })
}

fn cmd_migrate(args: &[String], options: &Options) -> u8 {
    match migrate_outcome(args) {
        Ok((from, to, migrated)) => {
            if options.json {
                println!("{{\"ok\":true,\"from\":{from},\"to\":{to},\"sections\":{migrated}}}");
            } else {
                println!("migrated {migrated} sections from format {from} to {to}");
            }
            EXIT_OK
        }
        Err(failure) => report(&failure, options.json, "migrate"),
    }
}

fn migrate_outcome(args: &[String]) -> Result<(u16, u16, usize), CliFailure> {
    let mut cursor = Cursor::new(args);
    let mut world = None;
    let mut from = 1_u16;
    let mut to = header::HEADER_FORMAT_VERSION;
    while let Some(argument) = cursor.arg() {
        match argument {
            "--world" => {
                if world.is_some() {
                    return Err(CliFailure::Usage("duplicate --world".to_owned()));
                }
                world = Some(PathBuf::from(cursor.value("world")?));
            }
            "--from" => {
                from = cursor
                    .value("from")?
                    .parse()
                    .map_err(|_| CliFailure::Usage("--from needs a u16".to_owned()))?;
            }
            "--to" => {
                to = cursor
                    .value("to")?
                    .parse()
                    .map_err(|_| CliFailure::Usage("--to needs a u16".to_owned()))?;
            }
            other => {
                return Err(CliFailure::Usage(format!(
                    "unexpected argument {other:?} for migrate"
                )));
            }
        }
    }
    let world = world.ok_or_else(|| CliFailure::Usage("migrate needs --world <dir>".to_owned()))?;
    let header = Header::read(&world.join(header::HEADER_FILE))?;
    let mut save = WorldSave::open(world, header.seed)?;
    let started = header::now_unix_seconds();
    match save.migrate(from, to) {
        Ok(sections) => {
            let report = MigrationReport {
                from_format: from,
                to_format: to,
                started,
                finished: header::now_unix_seconds(),
                sections,
                ok: true,
                error: None,
            };
            write_migration_report(save.root(), &report)?;
            Ok((from, to, sections))
        }
        Err(error) => {
            let report = MigrationReport {
                from_format: from,
                to_format: to,
                started,
                finished: header::now_unix_seconds(),
                sections: 0,
                ok: false,
                error: Some(error.to_string()),
            };
            let _ = write_migration_report(save.root(), &report);
            Err(error.into())
        }
    }
}

fn cmd_map(args: &[String], options: &Options) -> u8 {
    match map_outcome(args) {
        Ok(outcome) => {
            if options.json {
                println!(
                    "{{\"ok\":true,\"seed\":{},\"from\":[{},{}],\"size\":{},\"cell\":{},\"rows\":[{}]}}",
                    outcome.seed,
                    outcome.from.0,
                    outcome.from.1,
                    outcome.size,
                    outcome.cell,
                    outcome
                        .rows
                        .iter()
                        .map(|row| format!("\"{row}\""))
                        .collect::<Vec<_>>()
                        .join(",")
                );
            } else {
                println!(
                    "map seed={} from={},{} size={} cell={}",
                    outcome.seed, outcome.from.0, outcome.from.1, outcome.size, outcome.cell
                );
                for row in &outcome.rows {
                    println!("{row}");
                }
            }
            EXIT_OK
        }
        Err(failure) => report(&failure, options.json, "map"),
    }
}

struct MapOutcome {
    seed: u64,
    from: (i32, i32),
    size: usize,
    cell: i32,
    rows: Vec<String>,
}

fn map_outcome(args: &[String]) -> Result<MapOutcome, CliFailure> {
    let mut cursor = Cursor::new(args);
    let mut seed = None;
    let mut from = None;
    let mut size = 32_usize;
    let mut cell = 4_i32;
    while let Some(argument) = cursor.arg() {
        match argument {
            "--seed" => seed = Some(parse_seed(cursor.value("seed")?)?),
            "--from" => {
                let x = number::<i32>(cursor.value("from x")?, "from x")?;
                let z = number::<i32>(cursor.value("from z")?, "from z")?;
                from = Some((x, z));
            }
            "--size" => {
                let value = number::<i32>(cursor.value("size")?, "size")?;
                size = usize::try_from(value)
                    .ok()
                    .filter(|value| *value > 0 && *value <= 256)
                    .ok_or_else(|| CliFailure::Usage("--size must be in 1..=256".to_owned()))?;
            }
            "--cell" => {
                cell = number::<i32>(cursor.value("cell")?, "cell")?;
                if cell <= 0 {
                    return Err(CliFailure::Usage("--cell must be positive".to_owned()));
                }
            }
            other => {
                return Err(CliFailure::Usage(format!(
                    "unexpected argument {other:?} for map"
                )));
            }
        }
    }
    let seed = seed.ok_or_else(|| CliFailure::Usage("map needs --seed <u64>".to_owned()))?;
    let from = from.unwrap_or((0, 0));
    let terrain = Terrain::new(seed)?;
    let mut rows = Vec::with_capacity(size);
    for row in 0..size {
        let mut line = String::with_capacity(size);
        for column in 0..size {
            let x = from.0 + i32::try_from(column).unwrap_or(0) * cell;
            let z = from.1 + i32::try_from(row).unwrap_or(0) * cell;
            let blocks = terrain.column(x, z)?;
            let top = blocks
                .iter()
                .rposition(|id| *id != AIR)
                .map_or(SEA, |index| i32::try_from(index).unwrap_or(SEA));
            let id = blocks
                .get(usize::try_from(top).unwrap_or(0))
                .copied()
                .unwrap_or(AIR);
            let glyph = if id == WATER || top < SEA {
                '~'
            } else {
                let band = (top - SEA) / 8;
                match band {
                    i32::MIN..=0 => '.',
                    1 => ',',
                    2 => '-',
                    3 => '+',
                    4 => '*',
                    5 => '#',
                    6 => '%',
                    _ => '@',
                }
            };
            line.push(glyph);
        }
        rows.push(line);
    }
    Ok(MapOutcome {
        seed,
        from,
        size,
        cell,
        rows,
    })
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

// ---------------------------------------------------------------------------
// Minimal JSON reader
// ---------------------------------------------------------------------------
//
// The project has no JSON crate and this binary must not add one, so a report
// bundle is read with this small, total parser. It accepts standard JSON,
// rejects duplicate object keys and trailing bytes, and bounds nesting depth so
// malformed input is a typed usage error rather than a panic.

#[derive(Clone, Debug)]
enum Json {
    Null,
    Bool,
    Number(String),
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

impl Json {
    fn parse(text: &str) -> Result<Self, String> {
        let mut parser = JsonParser {
            bytes: text.as_bytes(),
            at: 0,
        };
        parser.skip_space();
        let value = parser.value(0)?;
        parser.skip_space();
        if parser.at != parser.bytes.len() {
            return Err(format!("trailing data at byte {}", parser.at));
        }
        Ok(value)
    }

    fn as_object(&self) -> Option<&BTreeMap<String, Json>> {
        match self {
            Self::Object(map) => Some(map),
            _ => None,
        }
    }

    fn as_array(&self) -> Option<&[Json]> {
        match self {
            Self::Array(values) => Some(values),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> JsonParser<'a> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.at += 1;
        Some(byte)
    }

    fn skip_space(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        match self.bump() {
            Some(found) if found == byte => Ok(()),
            Some(found) => Err(format!(
                "expected {:?} at byte {}, found {:?}",
                char::from(byte),
                self.at - 1,
                char::from(found)
            )),
            None => Err("unexpected end of input".to_owned()),
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > 32 {
            return Err("nesting is too deep".to_owned());
        }
        match self
            .peek()
            .ok_or_else(|| "unexpected end of input".to_owned())?
        {
            b'{' => self.object(depth),
            b'[' => self.array(depth),
            b'"' => self.string().map(Json::String),
            b't' => self.literal(b"true").map(|()| Json::Bool),
            b'f' => self.literal(b"false").map(|()| Json::Bool),
            b'n' => self.literal(b"null").map(|()| Json::Null),
            b'-' | b'0'..=b'9' => self.number().map(Json::Number),
            other => Err(format!("unexpected byte {other:#04x} at byte {}", self.at)),
        }
    }

    fn literal(&mut self, word: &[u8]) -> Result<(), String> {
        let end = self
            .at
            .checked_add(word.len())
            .ok_or_else(|| "input is too long".to_owned())?;
        if self.bytes.get(self.at..end) == Some(word) {
            self.at = end;
            Ok(())
        } else {
            Err(format!("invalid literal at byte {}", self.at))
        }
    }

    fn number(&mut self) -> Result<String, String> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        if !matches!(self.peek(), Some(b'0'..=b'9')) {
            return Err(format!("invalid number at byte {start}"));
        }
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.at += 1;
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(format!("invalid number at byte {start}"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.at += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if !matches!(self.peek(), Some(b'0'..=b'9')) {
                return Err(format!("invalid number at byte {start}"));
            }
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.at += 1;
            }
        }
        let raw = self
            .bytes
            .get(start..self.at)
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .ok_or_else(|| format!("invalid number at byte {start}"))?;
        Ok(raw.to_owned())
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = Vec::new();
        loop {
            match self.bump() {
                Some(b'"') => break,
                Some(b'\\') => self.escape(&mut out)?,
                Some(byte) if byte < 0x20 => {
                    return Err(format!("control byte {byte:#04x} in a string"));
                }
                Some(byte) => out.push(byte),
                None => return Err("unterminated string".to_owned()),
            }
        }
        String::from_utf8(out).map_err(|_| "string is not UTF-8".to_owned())
    }

    fn escape(&mut self, out: &mut Vec<u8>) -> Result<(), String> {
        match self.bump() {
            Some(b'"') => out.push(b'"'),
            Some(b'\\') => out.push(b'\\'),
            Some(b'/') => out.push(b'/'),
            Some(b'b') => out.push(0x08),
            Some(b'f') => out.push(0x0c),
            Some(b'n') => out.push(b'\n'),
            Some(b'r') => out.push(b'\r'),
            Some(b't') => out.push(b'\t'),
            Some(b'u') => {
                let unit = self.hex4()?;
                let character = char::from_u32(u32::from(unit))
                    .ok_or_else(|| "invalid \\u escape".to_owned())?;
                let mut buffer = [0_u8; 4];
                out.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            }
            Some(other) => return Err(format!("invalid escape {other:#04x}")),
            None => return Err("unterminated escape".to_owned()),
        }
        Ok(())
    }

    fn hex4(&mut self) -> Result<u16, String> {
        let raw = self
            .take(4)
            .ok_or_else(|| "unterminated \\u escape".to_owned())?;
        let text = std::str::from_utf8(raw).map_err(|_| "invalid \\u escape".to_owned())?;
        u16::from_str_radix(text, 16).map_err(|_| "invalid \\u escape".to_owned())
    }

    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(count)?;
        let slice = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(slice)
    }

    fn object(&mut self, depth: usize) -> Result<Json, String> {
        self.expect(b'{')?;
        let mut map = BTreeMap::new();
        self.skip_space();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Json::Object(map));
        }
        loop {
            self.skip_space();
            let key = self.string()?;
            self.skip_space();
            self.expect(b':')?;
            self.skip_space();
            let value = self.value(depth + 1)?;
            if map.insert(key.clone(), value).is_some() {
                return Err(format!("duplicate key {key:?}"));
            }
            self.skip_space();
            match self.bump() {
                Some(b',') => {}
                Some(b'}') => break,
                _ => return Err(format!("expected ',' or '}}' at byte {}", self.at)),
            }
        }
        Ok(Json::Object(map))
    }

    fn array(&mut self, depth: usize) -> Result<Json, String> {
        self.expect(b'[')?;
        let mut values = Vec::new();
        self.skip_space();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Json::Array(values));
        }
        loop {
            self.skip_space();
            values.push(self.value(depth + 1)?);
            self.skip_space();
            match self.bump() {
                Some(b',') => {}
                Some(b']') => break,
                _ => return Err(format!("expected ',' or ']' at byte {}", self.at)),
            }
        }
        Ok(Json::Array(values))
    }
}

fn read_json(path: &Path) -> Result<Json, CliFailure> {
    let bytes = fs::read(path)
        .map_err(|error| CliFailure::Usage(format!("cannot read {}: {error}", path.display())))?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| CliFailure::Usage(format!("{} is not UTF-8", path.display())))?;
    Json::parse(text).map_err(|message| CliFailure::Usage(format!("{}: {message}", path.display())))
}

fn expect_object<'a>(json: &'a Json, file: &str) -> Result<&'a BTreeMap<String, Json>, CliFailure> {
    json.as_object()
        .ok_or_else(|| CliFailure::Usage(format!("{file}: top level must be an object")))
}

fn string_field(
    object: &BTreeMap<String, Json>,
    key: &str,
    file: &str,
) -> Result<String, CliFailure> {
    let value = object
        .get(key)
        .ok_or_else(|| CliFailure::Usage(format!("{file}: missing {key:?}")))?;
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| CliFailure::Usage(format!("{file}: {key:?} must be a string")))
}

fn integer_field<T>(object: &BTreeMap<String, Json>, key: &str, file: &str) -> Result<T, CliFailure>
where
    T: std::str::FromStr,
{
    let value = object
        .get(key)
        .ok_or_else(|| CliFailure::Usage(format!("{file}: missing {key:?}")))?;
    let Json::Number(raw) = value else {
        return Err(CliFailure::Usage(format!(
            "{file}: {key:?} must be an integer"
        )));
    };
    raw.parse::<T>()
        .map_err(|_| CliFailure::Usage(format!("{file}: invalid {key:?} {raw:?}")))
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
///   "generator_version": 2,
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

fn cmd_replay(args: &[String], options: &Options) -> u8 {
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

fn hex_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic transect: `w = 0.5` at and behind the border and a linear
    /// rise to `w = 0.95` at 24 blocks in front of it.
    #[allow(clippy::cast_possible_truncation)]
    fn synthetic_profile(half_width: f64) -> Vec<(f64, f32)> {
        (-64..=64)
            .map(|offset| {
                let distance = f64::from(offset);
                let weight = if distance <= 0.0 {
                    0.5
                } else if distance >= half_width {
                    0.95
                } else {
                    (0.5 + 0.45 * distance / half_width) as f32
                };
                (distance, weight)
            })
            .collect()
    }

    #[test]
    fn transect_width_doubles_the_half_width() {
        let width = transect_width(&synthetic_profile(24.0)).expect("a complete transect measures");
        assert!((width - 48.0).abs() < 1e-9, "measured {width}");

        let width = transect_width(&synthetic_profile(12.0)).expect("a complete transect measures");
        assert!((width - 24.0).abs() < 1e-9, "measured {width}");
    }

    #[test]
    fn transect_width_accepts_a_quantised_border() {
        // The bilinear minimum behind the anchor can sit just above 0.5. The
        // border tolerance bounds that error below one block.
        let mut profile = synthetic_profile(20.0);
        for sample in &mut profile {
            if sample.0 < 0.0 {
                sample.1 = 0.5004;
            }
        }
        let width = transect_width(&profile).expect("a near-border minimum measures");
        assert!((width - 40.0).abs() < 1.0, "measured {width}");
    }

    #[test]
    fn transect_width_rejects_a_saturated_profile() {
        let flat = vec![(-8.0, 0.8_f32), (0.0, 0.85), (8.0, 0.9)];
        assert_eq!(transect_width(&flat), None);
    }

    #[test]
    fn percentiles_interpolate_between_order_statistics() {
        let values: Vec<f64> = (0..=10).map(f64::from).collect();
        assert!((percentile(&values, 0.5) - 5.0).abs() < 1e-12);
        assert!((percentile(&values, 0.1) - 1.0).abs() < 1e-12);
        assert!((percentile(&values, 0.9) - 9.0).abs() < 1e-12);
        assert_eq!(percentile(&[], 0.5), 0.0);
        assert_eq!(percentile(&[7.0], 0.9), 7.0);
    }

    #[test]
    fn pair_summaries_keep_the_unordered_pair_and_its_acceptance() {
        let mut widths = PairWidths::new();
        widths.insert((3, 1), vec![24.0; 32]);
        widths.insert((0, 2), vec![60.0; 8]);
        let summaries = summarise_pairs(&widths);
        assert_eq!(summaries.len(), 2, "one summary per unordered pair");
        assert_eq!((summaries[0].low, summaries[0].high), (0, 2));
        assert_eq!(summaries[0].samples, 8);
        assert!(
            !summaries[0].assessed(),
            "eight transects are reported only"
        );
        assert_eq!((summaries[1].low, summaries[1].high), (1, 3));
        assert!(summaries[1].accepted(), "a 24-block median is accepted");
    }

    #[test]
    fn calibrate_json_is_sorted_and_fixed_precision() {
        let outcome = BlendOutcome {
            seeds: 3,
            from: (0, 0),
            size: 512,
            step: 16,
            pairs: vec![
                PairSummary {
                    low: 0,
                    high: 1,
                    samples: 25,
                    median: 24.0,
                    p10: 10.5,
                    p90: 40.0,
                },
                PairSummary {
                    low: 2,
                    high: 3,
                    samples: 3,
                    median: 48.333,
                    p10: 20.0,
                    p90: 70.0,
                },
            ],
        };
        let json = calibrate_json(&outcome);
        assert_eq!(
            json,
            "{\"subcommand\":\"calibrate-blend\",\"seeds\":3,\"from\":[0,0],\"size\":512,\"step\":16,\"pairs\":[\
             {\"pair\":[0,1],\"samples\":25,\"median_blocks\":24.00,\"p10_blocks\":10.50,\"p90_blocks\":40.00,\"assessed\":true,\"ok\":true},\
             {\"pair\":[2,3],\"samples\":3,\"median_blocks\":48.33,\"p10_blocks\":20.00,\"p90_blocks\":70.00,\"assessed\":false,\"ok\":false}],\"ok\":true}"
        );
    }

    #[test]
    fn anchor_cells_follow_the_global_stride() {
        let cells = anchor_cells((0, 0), 64, 16);
        assert_eq!(cells.len(), 16, "four anchors per axis");
        assert!(cells.iter().all(|(x, z)| x % 4 == 0 && z % 4 == 0));
        assert_eq!(anchor_cells((0, 0), 2, 16).len(), 0);
    }

    #[test]
    fn counter_comparison_fails_above_five_percent() {
        let baseline = BTreeMap::from([("a".to_owned(), 100_u64), ("b".to_owned(), 0)]);
        assert!(compare_counters(&baseline, &BTreeMap::from([("a".to_owned(), 105)])).is_empty());
        let drifts = compare_counters(&baseline, &BTreeMap::from([("a".to_owned(), 110)]));
        assert_eq!(drifts.len(), 1);
        assert_eq!(drifts[0].name, "a");
        assert_eq!((drifts[0].baseline, drifts[0].current), (100, 110));
        assert_eq!(
            compare_counters(
                &baseline,
                &BTreeMap::from([("a".to_owned(), 100), ("b".to_owned(), 1)]),
            )
            .len(),
            1,
            "a zero baseline must stay zero"
        );
    }

    #[test]
    fn blend_weight_matches_the_generation_formula() {
        assert_eq!(blend_weight(0.0), 0.5);
        assert_eq!(blend_weight(255.0), 1.0);
        let mut previous = blend_weight(0.0);
        for q in 1..=255_u8 {
            let weight = blend_weight(f32::from(q));
            assert!(weight >= previous, "w fell at q {q}");
            previous = weight;
        }
    }
}

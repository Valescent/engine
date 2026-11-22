use std::fmt::Write as _;
use std::thread;

use terrain_core::description::{self, HYDRO_TILE, MAX_TERRAIN_Y, SEA};
use terrain_core::error::TerrainError;
use terrain_core::hash::sha256;
use terrain_core::hydro;
use terrain_core::lattice::floor_div;
use terrain_core::pipeline::{AIR, Terrain, WATER};
use terrain_core::section::BlockId;

use crate::cli::{Options, Order, number, parse_pairs, permutation};
use crate::output::{CliFailure, EXIT_INVARIANT, EXIT_OK, hex32, json_str, report};
use crate::{COLUMN, Column, internal};

/// Blocks per sweep region axis.
pub(crate) const SWEEP_WINDOW: i32 = 8;
/// `pr` sweep seed count.
const PR_SEEDS: usize = 16;
/// `nightly` sweep seed count.
const NIGHTLY_SEEDS: usize = 64;

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

// ---------------------------------------------------------------------------
// sweep
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Tier {
    Pr,
    Nightly,
}

impl Tier {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "pr" => Ok(Self::Pr),
            "nightly" => Ok(Self::Nightly),
            other => Err(format!("invalid --tier {other:?}; expected pr or nightly")),
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Pr => "pr",
            Self::Nightly => "nightly",
        }
    }

    pub(crate) const fn default_seeds(self) -> usize {
        match self {
            Self::Pr => PR_SEEDS,
            Self::Nightly => NIGHTLY_SEEDS,
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Region {
    name: &'static str,
    pub(crate) origin: (i32, i32),
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

pub(crate) fn cmd_sweep(args: &[String], options: &Options) -> u8 {
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
pub(crate) fn regions(tier: Tier) -> Vec<Region> {
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

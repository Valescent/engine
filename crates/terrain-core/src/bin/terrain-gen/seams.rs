use std::fmt::Write as _;

use terrain_core::description::TILE_BLOCKS;
use terrain_core::error::TerrainError;
use terrain_core::pipeline::Terrain;

use crate::cli::{Options, number, parse_pairs};
use crate::output::{CliFailure, EXIT_INVARIANT, EXIT_OK, json_str, report};

/// Default `seams` window side.
const DEFAULT_SEAMS_SIDE: i32 = 5;

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

pub(crate) fn cmd_seams(args: &[String], options: &Options) -> u8 {
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

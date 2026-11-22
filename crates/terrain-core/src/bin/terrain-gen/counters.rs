use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::thread;

use terrain_core::error::TerrainError;
use terrain_core::pipeline::Terrain;

use crate::cli::{Cursor, Options, Order, number, permutation};
use crate::output::{
    CliFailure, EXIT_INVARIANT, EXIT_OK, Json, expect_object, integer_field, json_str, read_json,
    report, string_field,
};
use crate::sweep::{Region, SWEEP_WINDOW, Tier, regions};
use crate::{Counts, internal};

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

pub(crate) fn cmd_counters(args: &[String], options: &Options) -> u8 {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}

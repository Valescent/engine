//! Simulated frame loop that pits rayon against micropool on the workload the
//! engine actually has (roadmap v0.0.4, "Async meshing").
//!
//! Every frame does three things, matching the real frame shape:
//!
//! 1. main-thread work that cannot leave the frame,
//! 2. a foreground `join` for a chunk remesh the player just edited (the caller
//!    blocks, so any stall lands in frame time),
//! 3. a burst of background chunk-mesh jobs submitted to the pool, bounded by a
//!    backlog cap so a saturated queue drops requests instead of growing forever.
//!
//! Frames are paced to the 60 fps deadline and the numbers below are frame *work*
//! time, so a backend that spends 12 ms blocked in `join` shows up as a hitched
//! frame. Background load per scenario is expressed as a fraction of the pool's
//! capacity for a 16.67 ms frame, which keeps the two backends comparable on any
//! core count.
//!
//! Run with:
//!
//! ```text
//! cargo bench --bench pool_sim                 # both, sequentially
//! cargo bench --bench pool_sim -- micropool    # one backend per process
//! cargo bench --bench pool_sim -- both --threads 8
//! ```

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use std::hint::{black_box, spin_loop};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

const FRAME_BUDGET_US: u64 = 16_667;
const FRAME_BUDGET: Duration = Duration::from_micros(FRAME_BUDGET_US);
const MAIN_US: u64 = 1_200;
const FOREGROUND_US: u64 = 2_500;
const BACKGROUND_JOB_US: u64 = 4_000;
const WARMUP_FRAMES: usize = 50;
const FRAMES: usize = 300;

/// Dependency-free CPU burn used as the unit of simulated work.
fn burn(units: u64) -> u64 {
    let mut x = units | 1;
    for _ in 0..units {
        x = x
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        x ^= x >> 33;
    }
    black_box(x)
}

/// Calibrates burn units to wall-clock microseconds on this machine.
fn units_per_us() -> u64 {
    const PROBE: u64 = 4_000_000;
    let start = Instant::now();
    burn(PROBE);
    let us = start.elapsed().as_micros().max(1) as u64;
    (PROBE / us).max(1)
}

trait Backend {
    fn label(&self) -> String;
    fn workers(&self) -> usize;
    fn join<A, B, RA, RB>(a: A, b: B) -> (RA, RB)
    where
        A: FnOnce() -> RA + Send,
        B: FnOnce() -> RB + Send,
        RA: Send,
        RB: Send;
    fn spawn(f: impl FnOnce() + Send + 'static);
}

struct Rayon;

impl Backend for Rayon {
    fn label(&self) -> String {
        format!("rayon ({} workers)", self.workers())
    }

    fn workers(&self) -> usize {
        rayon::current_num_threads()
    }

    fn join<A, B, RA, RB>(a: A, b: B) -> (RA, RB)
    where
        A: FnOnce() -> RA + Send,
        B: FnOnce() -> RB + Send,
        RA: Send,
        RB: Send,
    {
        rayon::join(a, b)
    }

    fn spawn(f: impl FnOnce() + Send + 'static) {
        rayon::spawn(f);
    }
}

struct Micropool;

impl Backend for Micropool {
    fn label(&self) -> String {
        format!("micropool ({} workers)", self.workers())
    }

    fn workers(&self) -> usize {
        micropool::num_threads()
    }

    fn join<A, B, RA, RB>(a: A, b: B) -> (RA, RB)
    where
        A: FnOnce() -> RA + Send,
        B: FnOnce() -> RB + Send,
        RA: Send,
        RB: Send,
    {
        micropool::join(a, b)
    }

    fn spawn(f: impl FnOnce() + Send + 'static) {
        drop(micropool::spawn_owned(f));
    }
}

struct Scenario {
    name: &'static str,
    /// Simulated background load as a fraction of total pool capacity.
    load: f64,
    /// Frames recorded after the warmup.
    frames: usize,
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "idle",
        load: 0.0,
        frames: FRAMES,
    },
    Scenario {
        name: "light",
        load: 0.3,
        frames: FRAMES,
    },
    Scenario {
        name: "loaded",
        load: 0.7,
        frames: FRAMES,
    },
    Scenario {
        name: "saturated",
        load: 2.0,
        frames: FRAMES,
    },
];

struct Report {
    backend: String,
    scenario: &'static str,
    jobs_per_frame: usize,
    dropped: usize,
    completed: u64,
    frame_p50: u64,
    frame_p95: u64,
    frame_p99: u64,
    frame_max: u64,
    join_p50: u64,
    join_p99: u64,
    join_max: u64,
    over_budget: usize,
    wall_ms: u64,
    checksum: u64,
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 * p).ceil() as usize).saturating_sub(1);
    sorted[idx.min(sorted.len() - 1)]
}

fn run<B: Backend>(backend: &B, scenario: &Scenario, upu: u64) -> Report {
    let workers = backend.workers();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let jobs_per_frame = ((scenario.load * workers as f64 * FRAME_BUDGET_US as f64)
        / BACKGROUND_JOB_US as f64)
        .round() as usize;
    let backlog_cap = 4 * workers;

    let in_flight = Arc::new(AtomicUsize::new(0));
    let completed = Arc::new(AtomicU64::new(0));
    let dropped = Arc::new(AtomicUsize::new(0));

    let main_units = MAIN_US * upu;
    let foreground_units = (FOREGROUND_US / 2) * upu;
    let background_units = BACKGROUND_JOB_US * upu;

    let mut frames = Vec::with_capacity(scenario.frames);
    let mut joins = Vec::with_capacity(scenario.frames);
    let mut checksum = 0_u64;
    let wall_start = Instant::now();

    for frame in 0..(WARMUP_FRAMES + scenario.frames) {
        let frame_start = Instant::now();

        checksum = checksum.wrapping_add(burn(main_units));

        let join_start = Instant::now();
        let (a, b) = B::join(
            || burn(foreground_units),
            || burn(foreground_units.wrapping_add(7)),
        );
        let join_us = join_start.elapsed().as_micros() as u64;
        checksum = checksum.wrapping_add(a).wrapping_add(b);

        for _ in 0..jobs_per_frame {
            if in_flight.load(Ordering::Relaxed) >= backlog_cap {
                dropped.fetch_add(1, Ordering::Relaxed);
                break;
            }
            in_flight.fetch_add(1, Ordering::Relaxed);
            let in_flight = Arc::clone(&in_flight);
            let completed = Arc::clone(&completed);
            B::spawn(move || {
                burn(background_units);
                completed.fetch_add(1, Ordering::Relaxed);
                in_flight.fetch_sub(1, Ordering::Relaxed);
            });
        }

        let frame_us = frame_start.elapsed().as_micros() as u64;
        if frame >= WARMUP_FRAMES {
            frames.push(frame_us);
            joins.push(join_us);
        }

        // Pace to a 60 fps deadline so both backends get the same wall-clock
        // time per frame for background work, like a vsync-locked engine.
        if let Some(rest) = FRAME_BUDGET.checked_sub(frame_start.elapsed()) {
            std::thread::sleep(rest);
        }
    }

    while in_flight.load(Ordering::Relaxed) > 0 {
        burn(main_units / 4);
        spin_loop();
    }
    let wall_us = wall_start.elapsed().as_micros() as u64;

    frames.sort_unstable();
    joins.sort_unstable();
    let budget = FRAME_BUDGET_US;

    Report {
        backend: backend.label(),
        scenario: scenario.name,
        jobs_per_frame,
        dropped: dropped.load(Ordering::Relaxed),
        completed: completed.load(Ordering::Relaxed),
        frame_p50: percentile(&frames, 0.50),
        frame_p95: percentile(&frames, 0.95),
        frame_p99: percentile(&frames, 0.99),
        frame_max: frames.last().copied().unwrap_or(0),
        join_p50: percentile(&joins, 0.50),
        join_p99: percentile(&joins, 0.99),
        join_max: joins.last().copied().unwrap_or(0),
        over_budget: frames.iter().filter(|&&f| f > budget).count(),
        wall_ms: wall_us / 1000,
        checksum,
    }
}

fn print_header() {
    println!(
        "sim: 60 fps budget, main {:>5.0}us/frame, foreground join {:>5.0}us, background job {:>5.0}us",
        MAIN_US as f64, FOREGROUND_US as f64, BACKGROUND_JOB_US as f64
    );
    println!(
        "     backlog cap 4 jobs/worker, frames paced to the budget, times are frame work time (us)\n"
    );
    println!(
        "{:<10} {:<24} {:>7} {:>7} {:>6} {:>6} {:>6} {:>6} {:>7} {:>6} {:>6} {:>9} {:>6} {:>7}",
        "scenario",
        "backend",
        "jobs/fr",
        "p50",
        "p95",
        "p99",
        "max",
        "over",
        "join50",
        "join99",
        "joinmax",
        "completed",
        "drops",
        "wall ms"
    );
    println!("{}", "-".repeat(122));
}

fn print_row(report: &Report) {
    println!(
        "{:<10} {:<24} {:>7} {:>7} {:>6} {:>6} {:>6} {:>6} {:>7} {:>6} {:>6} {:>9} {:>6} {:>7}",
        report.scenario,
        report.backend,
        report.jobs_per_frame,
        report.frame_p50,
        report.frame_p95,
        report.frame_p99,
        report.frame_max,
        report.over_budget,
        report.join_p50,
        report.join_p99,
        report.join_max,
        report.completed,
        report.dropped,
        report.wall_ms
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut target = "both";
    let mut threads = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "rayon" | "micropool" | "both" => target = &args[i],
            "--threads" => {
                i += 1;
                threads = args.get(i).and_then(|v| v.parse::<usize>().ok());
            }
            // cargo appends its own flags to the harness binary
            "--bench" | "--nocapture" => {}
            other => {
                panic!("unknown argument {other:?} (expected rayon|micropool|both|--threads N)")
            }
        }
        i += 1;
    }

    if let Some(n) = threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global()
            .expect("rayon global pool");
        micropool::ThreadPoolBuilder::default()
            .num_threads(n)
            .build_global();
    }

    let upu = units_per_us();
    let mut checksums: Vec<(&'static str, u64)> = Vec::new();
    let mut reports: Vec<Report> = Vec::new();

    print_header();
    if target == "rayon" || target == "both" {
        let backend = Rayon;
        for scenario in SCENARIOS {
            let report = run(&backend, scenario, upu);
            checksums.push((scenario.name, report.checksum));
            print_row(&report);
            reports.push(report);
        }
    }
    if target == "micropool" || target == "both" {
        let backend = Micropool;
        for scenario in SCENARIOS {
            let report = run(&backend, scenario, upu);
            checksums.push((scenario.name, report.checksum));
            print_row(&report);
            reports.push(report);
        }
    }

    if target == "both" {
        println!();
        let mut ok = true;
        for scenario in SCENARIOS {
            let values: Vec<u64> = checksums
                .iter()
                .filter(|(name, _)| *name == scenario.name)
                .map(|(_, checksum)| *checksum)
                .collect();
            if values.len() == 2 && values[0] != values[1] {
                ok = false;
                println!(
                    "workload mismatch on {:?}: rayon {:#x} vs micropool {:#x}",
                    scenario.name, values[0], values[1]
                );
            }
        }
        println!(
            "workload equivalence: {}",
            if ok { "identical" } else { "MISMATCH" }
        );
    }
    println!("\n{} scenario runs reported", reports.len());
}

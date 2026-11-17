# Benchmark results

This page records local `pool_sim` runs. `pool_sim` simulates the v0.0.5 async-meshing frame
loop to compare rayon with micropool. It is a decision aid, not a regression gate, and it is not
run in CI. The benchmark source is the methodology; this page is the run log.

| Area | Status |
|---|---|
| `pool_sim` benchmark | Current |
| This results log | Current |
| Run in CI | No |
| Real renderer or GPU measurement | No; see [Profiling](profiling.md) |
| Worker-pool decision | [ADR-0009](../decisions/0009-micropool-for-chunk-workers.md) |
| Engine run (v0.0.5) | Current; see [Engine run](#engine-run-v005-release) |

## Run it

From the repository root:

```console
cargo bench --bench pool_sim -- both
cargo bench --bench pool_sim -- rayon
cargo bench --bench pool_sim -- micropool
cargo bench --bench pool_sim -- both --threads 8
```

`both` runs rayon and micropool sequentially in one process, which is the quickest comparison.
Running one backend per process isolates the two pools. `--threads N` forces both pools to the
same worker count for a matched comparison. `cargo bench` appends its own `--bench` flag; the
benchmark ignores it.

## What is simulated

| Parameter | Value |
|---|---:|
| Frame budget | 16,667µs (60 FPS) |
| Main-thread work | 1,200µs per frame |
| Foreground join | 2,500µs |
| Background job | 4,000µs |
| Warmup | 50 frames |
| Recorded frames | 300 per scenario |
| Backlog cap | 4 jobs per worker |

The scenarios are `idle` (0.0 background load), `light` (0.3), `loaded` (0.7), and `saturated`
(2.0) as a fraction of pool capacity. Frames are paced to the 60 FPS deadline, but the reported
times are frame *work* time, not wall-clock frame intervals. The benchmark does not use the real
renderer, world, or GPU.

## Columns

| Column | Meaning |
|---|---|
| `scenario` | `idle`, `light`, `loaded`, or `saturated` |
| `backend` | `rayon` or `micropool`, with the worker count |
| `jobs/fr` | Background jobs submitted per frame |
| `p50`, `p95`, `p99`, `max` | Frame work-time percentiles in microseconds |
| `over` | Frames whose work exceeded the 16,667µs budget |
| `join50`, `join99`, `joinmax` | Foreground `join` latency percentiles in microseconds |
| `completed` | Background jobs completed during the scenario |
| `drops` | Background jobs dropped because the backlog cap was reached |
| `wall ms` | Wall time for the scenario, including warmup and cleanup |

A valid comparison also prints `workload equivalence: identical`. If the checksums differ, the
two backends did different work; discard the comparison.

## Interpreting a run

- Compare like with like: same machine, OS, Rust toolchain, worker count, and command. Do not
  compare absolute numbers across machines.
- Tail metrics matter more than the median for hitching. The `saturated` scenario is the
  decisive pool-contention case; `light` and `loaded` are noisier.
- A single run is not a trend. Run at least twice and record both runs rather than selecting the
  best one.
- `drops` under `saturated` are expected: the design drops background work when the backlog cap
  is reached. They are not a failure by themselves.
- `over = 0` means no recorded frame work exceeded the budget, but p99 and max still show the tail.
- A benchmark-code change is a methodology change. Record the commit and do not compare runs
  across different `pool_sim` versions as if they were the same test.

## Reference run (ADR-0009)

This is the baseline recorded when micropool was selected. The ADR owns the decision and its
reasoning; this page preserves the run for comparison.

- **Machine:** AMD Ryzen 5 7600, 6 cores / 12 logical processors, Windows.
- **Toolchain:** Rust 1.99.1.
- **Command:** `cargo bench --bench pool_sim -- both` (default worker counts; micropool 11,
  rayon 12).

**Budget:** 16,667µs per frame. Times are frame work time in microseconds. All scenarios had
`over = 0` and identical workload checksums.

| Scenario | Backend | p95 | p99 | max | join max |
|---|---|---:|---:|---:|---:|
| idle | rayon | 2,727 | 2,839 | 2,893 | 1,595 |
| idle | micropool | 2,660 | 2,772 | 2,910 | 1,518 |
| light | rayon | 2,809 | 2,895 | 3,077 | 1,602 |
| light | micropool | 2,724 | 2,849 | 2,995 | 1,512 |
| loaded | rayon | 2,808 | 2,911 | 3,318 | 1,582 |
| loaded | micropool | 2,754 | 2,873 | 2,990 | 1,550 |
| saturated | rayon | 2,457 | 2,753 | 2,781 | 1,624 |
| saturated | micropool | 2,471 | 2,662 | 2,830 | 1,508 |

The ADR did not record p50, `completed`, `drops`, or `wall ms` for this run.

### Repeat observations from the ADR

- A second default run kept micropool ahead in the saturated scenario: p95/p99 were
  2,452/2,519µs versus rayon's 2,701/2,728µs.
- That repeat also produced a micropool light-load outlier: p99/max were 3,264/6,497µs versus
  rayon's 2,924/3,048µs. The choice is not a claim that micropool wins every percentile in
  every run.
- The matched 12-worker run confirmed the saturated tail difference: rayon
  p95/p99/join-p99 were 6,827/8,259/6,995µs; micropool's were 3,935/4,666/2,875µs. The
  matched run also had a micropool light p99 outlier.

## 2026-11-16 run

- **Machine:** AMD Ryzen 5 7600, 6 cores / 12 logical processors, Windows 11 Home build 26300.
- **Toolchain:** `rustc 1.99.1 (58cf8f9c3 2026-10-13)`.
- **Commit:** `27c39b1` with docs-only working-tree changes; `benches/pool_sim.rs` is unchanged.
- **Command:** `cargo bench --bench pool_sim -- both` (default worker counts; micropool 11,
  rayon 12).

**Run:** one local, unpinned run with no deliberate CPU pinning or background-process cleanup.
Treat it as a noisy sample, not a regression measurement.

```text
sim: 60 fps budget, main  1200us/frame, foreground join  2500us, background job  4000us
     backlog cap 4 jobs/worker, frames paced to the budget, times are frame work time (us)

scenario   backend                  jobs/fr     p50    p95    p99    max   over  join50 join99 joinmax completed  drops wall ms
--------------------------------------------------------------------------------------------------------------------------
idle       rayon (12 workers)             0    2839   3194   3337   4250      0    1456   1726   2152         0      0    5944
light      rayon (12 workers)            15    2799   3182   3281   3459      0    1356   1726   1902      5250      0    5955
loaded     rayon (12 workers)            35    2790   3113   3225   3644      0    1446   1810   1934     12250      0    5954
saturated  rayon (12 workers)           100    3334   9510  12617  13618      0    2065  11377  12384     15605    350    6051
idle       micropool (11 workers)         0    2736   3142   3201   3309      0    1317   1691   1719         0      0    5951
light      micropool (11 workers)        14    2727   3086   3377   4693      0    1288   1672   1701      4900      0    5948
loaded     micropool (11 workers)        32    2741   3034   3214   4465      0    1361   1701   1738     11200      0    5954
saturated  micropool (11 workers)        92    3004   3830   4108   4358      0    1656   2603   3005     14737    350    5975

workload equivalence: identical

8 scenario runs reported
```

The run had `over = 0` in every scenario and identical workload checksums. The saturated rayon
tail was much worse than the ADR reference in this sample (p99 `12,617` versus `2,753`), while
micropool stayed at `4,108`. Both backends dropped `350` background jobs under saturation. The
gap is consistent with the ADR's tail argument, but this single run is not evidence of a code
regression; repeat it before drawing a trend.

## Engine run (v0.0.5, release)

End-to-end run of the real engine on the v0.0.5 stress scene, plus a headless hot-path
measurement. This is not a CI gate; record a new row when the hot paths change.

### How to run

```console
# End-to-end: stress fixture, vsync off, one-second frame samples
$env:ENGINE_STRESS_SCENE = "1"; $env:ENGINE_NO_VSYNC = "1"; $env:RUST_LOG = "engine=debug"
cargo run --release
```

```console
# Headless hot paths: snapshot preparation, LOD0/LOD1 meshing, and player collision
cargo test --release --lib bench_hot_paths -- --ignored --nocapture
```

`ENGINE_NO_VSYNC=1` starts the surface without vsync so the frame time is not clamped to the
display rate. The engine keeps rendering while unfocused, so the numbers stay valid when the
terminal has focus.

### Measurement

- **Machine:** AMD Ryzen 5 7600 (6C/12T), NVIDIA GeForce RTX 4060, Vulkan backend, Windows 11
  Home, 1280x720 window.
- **Toolchain:** `rustc 1.99.1 (58cf8f9c3 2026-10-13)`, `--release` profile (thin LTO,
  `codegen-units = 1`).
- **Scene:** `ENGINE_STRESS_SCENE=1`, spec spawn `(16.5, 3.0, 16.5)`, yaw `pi/4`, pitch `0`.
- **Sample:** 32 one-second samples after the first two seconds; no other work on the machine.

| Metric | Value |
|---|---:|
| FPS mean / p50 / min / max | 779.6 / 781.2 / 750.6 / 783.1 |
| `frame_ms` mean / min / max | 1.283 / 1.277 / 1.332 |
| Loaded chunks | 64 |
| Visible chunks / draw calls | 8 / 8 |
| Visible vertices | 100,464 |

Hot paths, 64 chunks, release:

| Hot path | Result |
|---|---:|
| Snapshot + border preparation | 0.049-0.050 ms/chunk |
| LOD0 greedy mesh | 0.477-0.488 ms/chunk |
| LOD1 greedy mesh | 0.099-0.101 ms/chunk |
| LOD0 vertices, all 64 chunks | 795,072 |
| LOD1 vertices, all 64 chunks | 81,984 |
| `Player::update` (collision scan) | 1150-1167 us/frame |

CPU/GPU split from a Tracy capture of the same scene (development profile, 349 frames):

| Zone | Mean | Max |
|---|---:|---:|
| `frame` | 16.52 ms (vsync on) | 33.19 ms |
| `player update` | 1.09 ms | 1.44 ms |
| `chunk mesh` (worker, 72 jobs) | 1.54 ms | 3.42 ms |
| `mesh upload` | 22 us | 431 us |
| `encode` | 13 us | 65 us |
| GPU `terrain` | ~5.5 us | — |

### Reading the result

- The frame is **CPU-bound and collision-bound**: `Player::update` is 1.15 ms of the 1.28 ms
  frame. It scans every block of every loaded chunk three times per frame, so it scales with
  world size rather than with the player's surroundings.
- Meshing is off the main thread; the per-chunk cost above is worker time, not frame time.
- GPU time is negligible at this view. The spec spawn faces out of the fixture, so only 8 of 64
  chunks are inside the frustum; a view across the map draws more chunks and is not covered by
  this run.
- These numbers come from one machine and one run. Treat them as a baseline, not a regression
  gate.

## Run log

| Date | Commit | Machine | Rust | Command | Workers | Result |
|---|---|---|---|---|---|---|
| 2026-11-15 (ADR date; exact time not recorded) | Not recorded | Ryzen 5 7600, 6C/12T, Windows | 1.99.1 | `cargo bench --bench pool_sim -- both` | rayon 12, micropool 11 | All `over = 0`; checksums identical; micropool better saturated p99 and join max |
| 2026-11-16 | `27c39b1` | Ryzen 5 7600, 6C/12T, Windows 11 26300 | 1.99.1 | `cargo bench --bench pool_sim -- both` | rayon 12, micropool 11 | All `over = 0`; checksums identical; micropool saturated p99 4,108 vs rayon 12,617; drops 350 each |
| 2026-11-17 | v0.0.5 release build | Ryzen 5 7600, 6C/12T, RTX 4060, Windows 11 | 1.99.1 | `ENGINE_NO_VSYNC=1 cargo run --release` | n/a | 779.6 FPS mean, 1.283 ms frame; `player update` 1.15 ms dominates |

Add new runs below the latest row. Keep the raw output with the entry when possible.

## Record a new run

1. Use the pinned toolchain and confirm it with `rustup show`.
2. Close heavy background work. Note if the machine is busy, thermally throttled, or on battery.
3. Run the benchmark from the repository root. For a matched comparison, use `--threads N` with
   `both`.
4. Run at least twice. Record both runs rather than selecting the best one.
5. Confirm `workload equivalence: identical`; discard the comparison if it is not.
6. Record the date, commit, machine, OS, Rust, command, worker counts, and raw output.
7. Add a row to the run log and keep the detailed output in a section above it.

### Entry template

```text
Date:
Commit:
Machine / CPU / cores / logical:
OS / build:
Rust:
Command:
Workers: rayon N, micropool M
Run number:
Workload equivalence:
Results:
  scenario | backend | jobs/fr | p50 | p95 | p99 | max | over | join50 | join99 | joinmax | completed | drops | wall ms
Notes:
```

## Caveats

- `pool_sim` is a simulation of the frame shape, not the real renderer. It does not measure
  chunk generation, real meshing, GPU upload, or GPU time.
- It is a decision aid, not a regression gate, and it is not run in CI.
- Absolute times are machine-specific. Re-run on the target host before changing the pool or its
  worker count.
- The benchmark is noisy outside saturation. The ADR records light-load outliers in both
  directions.
- A methodology change in `benches/pool_sim.rs` requires a new baseline; do not compare across
  methodology versions.

## References

- [ADR-0009: micropool for chunk workers](../decisions/0009-micropool-for-chunk-workers.md)
- [v0.0.5 spec](../specs/v0.0.5.md)
- [v0.0.5 scope](v0.0.5-scope.md)
- [Debugging: current aids](debugging.md#current-aids)
- [Profiling](profiling.md)
- [Conventions](conventions.md)
- [CI](ci.md)

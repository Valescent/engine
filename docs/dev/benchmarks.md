# Benchmark results

This page records local `pool_sim` runs, engine runs, and the headless harness. `pool_sim` simulates
the v0.0.5 async-meshing frame loop to compare rayon with micropool. It is a decision aid, not a
regression gate, and it is not run in CI. The headless harness is the CI gate. The benchmark source
is the methodology; this page is the run log.

| Area | Status |
|---|---|
| `pool_sim` benchmark | Current |
| This results log | Current |
| Run in CI | No |
| Real renderer or GPU measurement | No; see [Profiling](profiling.md) |
| Worker-pool decision | [ADR-0009](../decisions/0009-micropool-for-chunk-workers.md) |
| Engine run (v0.0.5) | Historical baseline; see [Engine run](#engine-run-v005-release) |
| Engine run (v0.0.8) | Current; see [Engine run](#engine-run-v008-release) |
| Headless run (v0.0.8) | Current; gated by the `Perf` workflow |

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

### 2026-11-17 collision query fix

`World::solid_blocks_in` used to scan every block of every stored chunk. It now visits only the
coordinates inside the requested box, so the cost scales with the player's swept AABB. The run
above is the pre-fix baseline; the same command on the fixed commit gives:

| Metric | Before | After |
|---|---:|---:|
| `Player::update` (walking) | 1150-1167 us/frame | 0.9 us/frame |
| FPS mean / p50 / min / max | 779.6 / 781.2 / 750.6 / 783.1 | 4370 / 4387 / 4088 / 4425 |
| `frame_ms` mean / min / max | 1.283 / 1.277 / 1.332 | 0.229 / 0.226 / 0.245 |
| Visible chunks / draw calls / vertices | 8 / 8 / 100,464 | 8 / 8 / 100,464 |

The frame is no longer collision-bound. What remains at this view is roughly 0.2 ms of
encode/present overhead; the GPU zone was ~5.5 us.

### Where the remaining 0.25 ms goes

Captured with the `tracy` feature in release and vsync off, 11,034 frames in 3.1 s. The `frame`
span averages 246.5 us; nested spans account for about 236 us of it:

| Span | Mean | Share |
|---|---:|---:|
| `acquire` (swapchain image) | 87.1 us | 35% |
| `submit` (encoder finish + `Queue::submit`) | 86.7 us | 35% |
| `present` (`Queue::present`) | 47.6 us | 19% |
| `encode` | 5.3 us | 2% |
| `profiler end` (Tracy GPU readback) | 2.6 us | 1% |
| `mesh request` | 2.9 us | 1% |
| `select active` | 1.5 us | <1% |
| `mesh upload` | 0.85 us | <1% |
| `player update` | 0.81 us | <1% |
| `highlight` | 0.5 us | <1% |
| `frustum` | 0.13 us | <1% |

Inside those spans, wgpu reports `vkQueueSubmit` at 32.8 us per call, `Device::wait` at 40.2 us,
`CommandEncoder::finish` at 22.4 us, `vkQueuePresentKHR` at 24.6 us, and the per-frame camera
`Queue::write_buffer` at 5.4 us. The engine's own logic is under 15 us of the frame; the rest is
driver and present-path bookkeeping that scales with frame count, not with world size.

So the uncapped ceiling is the swapchain/present path on this driver (~0.25 ms per frame), not
mesh work, collision, or the GPU. Vsync hides it completely at 60 FPS.

## Engine run (v0.0.8, release)

Generated-world streaming at distance 24, with the v0.0.7 distance-12 result as the baseline. Same
machine and release profile as above.

### How to run

Put a `config.toml` in the working directory:

```toml
[world]
render_distance_chunks = 24
```

Then run with `ENGINE_NO_VSYNC=1` and `RUST_LOG=engine=debug`, and keep the window foregrounded.
Windows throttles redraws for covered windows, which looks like a frame-rate regression; the CPU
frame time in the log is the comparable number.

### Measurement

| Scene | Chunks loaded | Working set | Frame time | FPS |
|---|---:|---:|---:|---:|
| v0.0.7, distance 12 | 1434 | 338 MB | 16.7 ms (vsync) | 60 |
| v0.0.8, distance 12 | 1434 | 295 MB | 0.8 ms | 1247 |
| v0.0.8, distance 24, still streaming | 3031 | 467 MB | 2.9-3.2 ms | 316-345 |
| v0.0.8 after packing, distance 24, settled | 5289 | 626 MB | 2.2-2.6 ms | 380-450 |

The distance-24 run is sampled while the ring is still filling: generation commits 125-150
columns per second and the level work set keeps the steady-state scan off the frame. At distance
12 the resident light cost falls from about 79 KiB to 39 KiB per chunk. Packed 12-byte vertices
with 16-bit indices cut the vertex buffers by about 75%; the settled working set is dominated by
blocks, light, and allocator overhead, so it moves less than the mesh data.

### Headless run (v0.0.8)

`ENGINE_HEADLESS=1` drives the same generation, streaming, light, and mesh paths without a window
at a fixed 60 Hz step. Reported work excludes the paced sleep, so the numbers are per-frame CPU
cost, not wall time.

| Run (distance 12, seed 42) | Generated columns | Chunks | Ready columns | Mean / p95 / max work | Blocks + light |
|---|---:|---:|---:|---:|---:|
| Static camera, 600 frames | 624 | 1,376 | 625 / 625 | 0.42 / 1.9 / 3.8 ms | 95 MB |
| Walk at 32 m/s, 600 frames | 630 | 1,405 | 631 / 631 | 0.46 / 2.1 / 5.7 ms | 97 MB |

`resident_bytes` covers stored blocks and light fields only; meshes and GPU buffers are not
included. The **Perf** workflow runs both scenes on every `main` push and gates on a generous
budget (625 ready columns, mean under 20 ms, p95 under 50 ms, under 200 MB of blocks and light).
## 2026-11-17 runs

- **Machine:** AMD Ryzen 5 7600, 6 cores / 12 logical processors, Windows 11 Home.
- **Toolchain:** `rustc 1.99.1 (58cf8f9c3 2026-10-13)`.
- **Commit:** `15ae656` (pool and benchmark unchanged from the v0.0.5 release).
- **Command:** `cargo bench --bench pool_sim -- both`, run twice.

Both runs reported `over = 0` in every scenario, identical workload checksums, and 350 dropped
background jobs under saturation. Idle, light, and loaded p95 stayed between 2.9 and 3.3 ms for
both backends; the difference is the saturated tail:

| Run | Backend | p50 | p95 | p99 | max | join p99 | join max |
|---|---|---:|---:|---:|---:|---:|---:|
| 1 | rayon (12) | 2,905 | 3,943 | 4,309 | 4,567 | 3,096 | 3,325 |
| 1 | micropool (11) | 3,594 | 3,856 | 4,249 | 6,107 | 2,892 | 4,650 |
| 2 | rayon (12) | 3,747 | 5,869 | 8,581 | 10,567 | 7,289 | 9,302 |
| 2 | micropool (11) | 3,429 | 3,993 | 4,397 | 4,716 | 3,004 | 3,149 |

Micropool had the better saturated p99 in both runs (`4,249` and `4,397` versus `4,309` and
`8,581`) and the better saturated join tail in the second. Run 1 had micropool slightly worse on
saturated p50 and max, so the result is a tail preference rather than a clean sweep. That is
consistent with [ADR-0009](../decisions/0009-micropool-for-chunk-workers.md), and single runs
remain noisy.

## 2026-11-17 mesh pool A/B (real engine)

`pool_sim` simulates the frame loop. This run compares the pools on the real engine by building
with the optional `rayon-pool` feature and selecting the pool at startup:

```console
cargo build --release --features tracy,rayon-pool
$env:ENGINE_STRESS_SCENE = "1"; $env:RUST_LOG = "engine=debug"
$env:ENGINE_MESH_POOL = "rayon"   # omit for micropool
target\release\engine.exe
```

Each run captures the first six to eight seconds with `tracy-capture`, which covers the startup
burst: 64 urgent LOD0 jobs plus the LOD1 jobs for chunks outside the four-chunk radius, 72 jobs
in total. Frame pacing is vsync-locked at 60 FPS, so this compares worker throughput and startup
behaviour, not steady-state frame rate.

| Run | Pool (workers) | Jobs | Mean job | Max job | Worker total |
|---|---|---:|---:|---:|---:|
| 1 | micropool (11) | 72 | 887 us | 1,952 us | 63.8 ms |
| 2 | micropool (11) | 72 | 970 us | 4,337 us | 69.8 ms |
| 1 | rayon (12) | 72 | 951 us | 2,194 us | 68.5 ms |
| 2 | rayon (12) | 72 | 968 us | 2,121 us | 69.7 ms |

- Per-job time is within noise: micropool averages 887 and 970 us, rayon 951 and 968 us, with
  micropool one worker down.
- Both complete the startup burst inside the first second and reach the same 60 FPS with 8
  visible chunks; `frame` max in both traces is ~33 ms from the first frame, not from meshing.
- Two runs lost their capture window before the startup burst was recorded and were discarded.
  The engine state in those runs still rendered correctly.

**Verdict:** no meaningful difference on the real workload. micropool stays the default under
ADR-0009; the rayon path exists only for A/B runs.

## Run log

| Date | Commit | Machine | Rust | Command | Workers | Result |
|---|---|---|---|---|---|---|
| 2026-11-15 (ADR date; exact time not recorded) | Not recorded | Ryzen 5 7600, 6C/12T, Windows | 1.99.1 | `cargo bench --bench pool_sim -- both` | rayon 12, micropool 11 | All `over = 0`; checksums identical; micropool better saturated p99 and join max |
| 2026-11-16 | `27c39b1` | Ryzen 5 7600, 6C/12T, Windows 11 26300 | 1.99.1 | `cargo bench --bench pool_sim -- both` | rayon 12, micropool 11 | All `over = 0`; checksums identical; micropool saturated p99 4,108 vs rayon 12,617; drops 350 each |
| 2026-11-17 | v0.0.5 release build | Ryzen 5 7600, 6C/12T, RTX 4060, Windows 11 | 1.99.1 | `ENGINE_NO_VSYNC=1 cargo run --release` | n/a | 779.6 FPS mean, 1.283 ms frame; `player update` 1.15 ms dominates |
| 2026-11-17 | collision query fix | Ryzen 5 7600, 6C/12T, RTX 4060, Windows 11 | 1.99.1 | `ENGINE_NO_VSYNC=1 cargo run --release` | n/a | 4370 FPS mean, 0.229 ms frame; `player update` 0.9 us |
| 2026-11-17 | `15ae656` | Ryzen 5 7600, 6C/12T, Windows 11 | 1.99.1 | `cargo bench --bench pool_sim -- both` (x2) | rayon 12, micropool 11 | `over = 0`; checksums identical; micropool saturated p99 4,249 / 4,397 vs rayon 4,309 / 8,581 |
| 2026-11-17 | v0.0.5 + `rayon-pool` | Ryzen 5 7600, 6C/12T, RTX 4060, Windows 11 | 1.99.1 | `ENGINE_MESH_POOL=rayon` release captures | micropool 11 / rayon 12 | 72 startup jobs; per-job 887-970 us (micropool) vs 951-968 us (rayon); no frame difference |

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

## 2026-11-19 far-view baseline (v0.0.9 slice 1)

Machine and toolchain as above (Ryzen 5 7600, 6C/12T, RTX 4060, Windows 11 Home, `rustc 1.99.1`,
release profile). No other work on the machine.

### Mesh build

```console
cargo test --release --lib bench_mesh_build -- --ignored --nocapture
```

Each row meshes one 32^3 chunk of a 3x3 column patch with its light field: the seed 42 spawn
surface, the spawn ground layer, a flat slab, and an all-air chunk that isolates the fixed scan
cost. Thirty samples after two warmups. The table is the second run; the first was 25-40% slower,
so treat the numbers as a range rather than precise constants.

| Fixture | LOD0 | LOD1 | LOD2 | LOD0 quads | LOD0 indices |
|---|---:|---:|---:|---:|---:|
| Spawn surface | 0.81 ms | 0.25 ms | 0.07 ms | 2,005 | 12,030 |
| Spawn ground | 1.39 ms | 0.45 ms | 0.12 ms | 2,273 | 13,638 |
| Flat slab | 0.90 ms | 0.23 ms | 0.07 ms | 2 | 12 |
| All air (scan only) | 0.33 ms | 0.15 ms | 0.05 ms | 0 | 0 |

The water pass costs 0.36-0.51 ms per chunk and is built for every chunk, including 0.36 ms when it
emits nothing.

Reading it:

- A real LOD0 surface chunk costs 0.8-1.4 ms, not the recorded 0.48 ms. That figure came from the
  v0.0.5 stress scene, which has changed since; `bench_hot_paths` on the current scene gives
  2.3 ms/chunk on the same machine.
- About 0.33 ms of every LOD0 build is fixed scan cost: six faces x 32 slices x 32 x 32 cells,
  independent of output. That is roughly 3 s of worker time for the 9,285 chunks at distance 32
  before any geometry is emitted, and the water pass adds about 3.3 s more.
- A surface chunk emits about 2,000 quads, ten times the 188 implied by the "11 KiB per chunk"
  average. Near LOD0 chunks dominate geometry; coarse and underground chunks pull the average down.

### Streaming baseline

```console
$env:ENGINE_HEADLESS = "1"; $env:ENGINE_HEADLESS_FRAMES = "120"; $env:ENGINE_SEED = "42"
target\release\engine.exe
```

Fresh world in a temporary directory, `render_distance_chunks` set per run. Settle frames are
counted at the 1 ms step.

| Distance | Chunks | Blocks + light | Settle frames | Wall | Work p95 / max |
|---:|---:|---:|---:|---:|---:|
| 24 | 5,289 | 381.2 MB | 5,765 | 16.0 s | 2.9 / 3.9 ms |
| 32 | 9,285 | 669.2 MB | 9,749 | 32.1 s | 3.2 / 19.1 ms |

After four-bit block packing (same session, same world):

| Distance | Chunks | Blocks + light | Settle frames | Wall | Work p95 / max |
|---:|---:|---:|---:|---:|---:|
| 24 | 5,289 | 294.5 MB | 5,861 | 15.8 s | 2.3 / 8.4 ms |
| 32 | 9,285 | 517.1 MB | 9,936 | 34.8 s | 2.3 / 8.0 ms |

`resident_bytes` is blocks plus light only. The drop is exactly 16 KiB per chunk: 86.6 MB at
distance 24 and 152.1 MB at distance 32. Settle time is unchanged within noise.

Meshing after packing measured 0.97 ms (spawn surface), 1.53 ms (spawn ground), and 0.42 ms
(all-air scan) per LOD0 chunk, against 0.81, 1.39, and 0.33 before. The nibble decode costs about
15-25% and runs on workers.

The other half is light. A `LightField` stores one nibble per cell for sky and block light, and the
block-light plane is now omitted when the 3x3x3 neighbourhood contains no lamp. Generated terrain
places no lamps, so almost every chunk keeps only the sky plane, 19.2 KiB instead of 39 KiB:

| Distance | Chunks | Blocks + light | Settle frames | Wall | Work p95 / max |
|---:|---:|---:|---:|---:|---:|
| 24 | 5,289 | 190.6 MB | 5,964 | 15.1 s | 2.0 / 2.8 ms |
| 32 | 9,285 | 334.6 MB | 10,323 | 29.1 s | 2.6 / 4.4 ms |

The sparsity is per neighbourhood, not per chunk: a lamp keeps the block plane for the 27 chunks
around it, adding 19.2 KiB each. The distance-32 figure is under the 350 MB definition of done with
no lamps placed, and a lamp costs 0.6% of that budget.

### Cheap wins after the baseline

Three worker-side and renderer-side cleanups landed after the numbers above. None changes output.

Mesher (`bench_mesh_build`):

| Fixture | Before | After | Note |
|---|---:|---:|---|
| Water pass, dry chunk | 0.37-0.38 ms | 0.0 ms | no scan when the chunk has no water |
| Surface LOD0 | 0.97 ms | 0.91 ms | mask allocation hoisted out of the slice loop |
| Ground LOD0 | 1.53 ms | 1.49 ms | same |
| All-air LOD0 | 0.42 ms | 0.38 ms | same |

Headless (`ENGINE_HEADLESS=1`, 120 frames):

| Distance | Settle frames | Wall | Work p95 |
|---:|---:|---:|---:|
| 24 before | 5,964 | 15.1 s | 2.04 ms |
| 24 after | 5,145 | 12.2 s | 1.59 ms |
| 32 before | 10,323 | 29.1 s | 2.60 ms |
| 32 after | 8,717 | 24.4 s | 1.52 ms |

Windowed:

| Distance | Before | After | Draws | Geometry |
|---:|---:|---:|---:|---:|
| 24 | 2.24-2.39 ms | 2.34 ms | 1,934 | 89.4 MB |
| 32 | 9.06 ms | 4.86-5.01 ms | 3,409-3,507 | 125.2 MB, 9,430 chunks |

The distance-32 windowed run drifted (the player slid several metres), so it loaded 145 extra chunks
and is not a perfectly static comparison; the frame time is still the settled figure for that run.
One interaction with the cheaper traversal: the distance-24 frame is unchanged because draw
submission dominates there, while distance 32 loses the duplicate BTreeMap walks and frustum tests.

A single release run aborted with no output while streaming at distance 32; it did not reproduce in
a debug build or in a later release run with the same configuration. Treat it as unresolved until it
recurs. The same silent exit happened twice more in this session and then stopped; later runs with
the same binary completed. It recurred again during the eight-upload run, ending that process after
about six seconds. This is the top open risk for benchmarking and needs a crash handler or a device-loss
diagnostic before the next long run.

### Predictable CPU work

Two structural changes make the main-thread work bounded and ordered rather than bursty:

- `level_work` is a `BTreeSet<ChunkCoord>` drained under a 2 ms per-frame budget, so chunks are
  processed in a stable order and the request phase cannot run long. A chunk leaves the set while its
  light or mesh job is in flight and is re-inserted when the result is accepted, so the set is a queue
  of actionable work instead of a set of blocked chunks.
- Active-LOD selection moved from a per-frame scan of every loaded chunk (9,285 at distance 32) to the
  upload of the chunk that just changed, and stale LOD pruning happens there.

A `slow level request` debug line reports any single chunk that takes 2 ms or more; none appeared in
the settled runs.

| Metric | Before | After |
|---|---:|---:|
| Settled windowed frame, distance 32 | 4.86-5.01 ms | **4.30-4.35 ms** |
| Settled windowed frame, distance 24 | 2.34 ms | 2.34 ms |
| Headless wall, distance 24 | 12.2 s | **11.7 s** |
| Headless wall, distance 32 | 24.4 s | **21.6 s** |
| Headless work p95 / max, distance 24 | 1.59 / 2.46 ms | **1.36 / 1.53 ms** |
| Headless work p95 / max, distance 32 | 1.52 / 2.08 ms | 1.47 / 2.54 ms |
| `slow phases` logs, 79 s at distance 32 | present while streaming | **none** |

The distance-32 maximum is noisy and slightly worse in this sample; the p95 and wall time both
improved. Streaming to 9,285 chunks still takes about 70 s at 190-230 FPS, because generation and
light throughput set the pace once requests are bounded.

### Shared index buffer

Every quad uses the same four-vertex, six-index pattern, so one static `u16` buffer replaces the
per-chunk index buffers; a chunk draws `0..6 * quads` from it. Chunks above 65,535 vertices keep a
per-chunk `u32` buffer.

| Metric at distance 32 | Before | After |
|---|---:|---:|
| Resident geometry | 125.9 MB | **100.9 MB** |
| Visible geometry | 37.6 MB | **30.1 MB** |
| Draw calls | 3,409 | 3,409 |

The run overlapped prototype compilation, so its frame sample (4.45-6.00 ms) is not used as a
performance result; the geometry totals are exact. This removes 12 of the 60 bytes per quad and is a
step toward ADR-0015's 12-byte quads, which remove the remaining 48.

### Upload cap

`MAX_UPLOADS_PER_FRAME` was the last open slice-1 measurement. Windowed distance 32, fresh world,
time from the first frame log to the first log with 9,285 chunks and `rebuild_ms = 0`:

| Uploads per frame | Settle time | Max one-second sample |
|---:|---:|---:|
| 2 | 33 s | 6.76 ms |
| 64 | 22 s | 6.97 ms |
| 8 | 23 s | 10.35 ms |

The cap was limiting first-load streaming: raising it to 64 recovers about 11 s with no measurable
frame regression. `8` reproduces the 64 result on a retry (23 s) and keeps a single frame's upload
burst lower than 64, so it is the chosen value. The 10.35 ms maximum is one noisy sample; the other
runs peaked at 6.76-6.97 ms.

### Quad record emission

The mesher now writes ADR-0015's 12-byte record for every quad alongside the existing vertices and
indices. `bench_mesh_build` reports the count, and every row has `records == quads`:

| Fixture | LOD0 | Records | Water pass |
|---|---:|---:|---:|
| Spawn surface | 1.03 ms | 2,005 | 0.44 ms |
| Spawn ground | 1.51 ms | 2,273 | 0.46 ms |
| Flat slab | 0.98 ms | 2 | 0.0 ms (dry) |
| All air | 0.37 ms | 0 | 0.0 ms (dry) |

Against the pre-record numbers this is within run noise, with the surface fixture 5-8% slower and the
others flat. The records add 12 bytes per quad to the transient worker output (about 24 KB for a
2,000-quad chunk); nothing is uploaded to the GPU until the pool lands.

### Quad pool (ADR-0015)

Settled distance 32, seed 42, after the quad path replaced the per-chunk vertex and index buffers:

| Metric | Shared-index path | Quad pool |
|---|---:|---:|
| Draw calls | 3,409 | **2** |
| Resident geometry | 100.9 MB | **37.6 MiB** |
| Visible geometry | 30.1 MB | **7.52 MiB** |
| Frame CPU | 4.30-4.35 ms | **1.84-1.87 ms** |
| Visible quads | 656,817 | 656,891 |

- The pool reserves 38.4 MB (3.2M quads at 12 bytes), plus a 48-byte chunk table and indirect
  arguments; `geometry_mb` reports the reservation, not the allocated fraction.
- No mesh was skipped by a full pool in the settled run. If it happens, the result is not marked
  built and the level pass requests it again after two-frame frees retire.
- The visible quad count matches the old path within 0.01%, so LOD selection and water are intact.
- The stress scene screenshot (64 chunks) rendered the expected terrain and sky; draw calls fell
  from 8 to 1. The per-chunk vertex/index path remains as the fallback for adapters without the
  required features.
- `ENGINE_LEGACY_RENDER=1` forces the fallback. On the stress scene it draws 8 calls in 0.31 ms
  against 1 call in 0.22 ms for the pool; the small scene hides the submission win that distance 32
  shows.

### Neighbour light deferral

Loading a column changes the sky exposure of the eight surrounding columns, because light treats a
missing chunk as open sky. The first fix invalidated all eight neighbour columns on every generated
column, which made the settle path compute the same light fields repeatedly. Deferring a light job
until its 3x3 column neighbourhood is either loaded or outside the render distance (where it will
never arrive) keeps one light pass per chunk.

| Run | Before deferral | After deferral | v0.0.8 baseline |
|---|---:|---:|---:|
| Headless wall, distance 24 | 26.4 s | **11.1 s** | 11.7 s |
| Headless wall, distance 32 | 47.4 s | **20.4 s** | 21.6 s |
| Headless mean frame, distance 32 | 2.75 ms | **0.99 ms** | — |

The after-run reports 2,400 generated columns and 5,289 chunks at distance 24, and 4,224 columns and
9,285 chunks at distance 32, with `ready_columns == loaded_columns` and no pending work. Windowed
distance 32 settles at 2.51-2.59 ms, 2 draw calls, 38.18 MiB resident and 3.64 M visible vertices.
The visible vertex count is higher than the pre-fix 2.90 M because the LOD fixes keep resident LOD0
meshes selected instead of leaving the chunk on a pruned coarse level.

A deferred chunk is re-admitted when a generation result lands, when the player crosses a column, or
when an unload removes its blocking neighbour. Chunks at the edge of the wanted area are not deferred:
the missing side will never be generated from that camera position, so open sky is the final answer.

### Streaming candidate cache

`stream_columns` used to rebuild and sort the wanted-column list, test all loaded columns for
readiness, and scan for unloadable columns every frame. The wanted and unloadable lists are now
cached until the player crosses a column, and a column is marked ready from mesh acceptance instead
of a world scan.

| Run | Before | After |
|---|---:|---:|
| Windowed distance-32 frame | 2.51-2.59 ms | **0.66-0.68 ms** |
| Headless wall, distance 24 | 11.1 s | **7.6-7.9 s** |
| Headless wall, distance 32 | 20.4 s | **12.2-12.8 s** |

- The settled frame is unchanged visually: 2,435 visible chunks, 3.64 M visible vertices, 2 draws,
  38.18 MiB resident and 6.94 MiB visible.
- The headless mean rises (0.89 to 1.43 ms at distance 32) because the same 120 fixed steps now pack
  more generation work; wall time and settle frames are the comparable metrics.
- A moving run (`ENGINE_HEADLESS_SPEED=12`, distance 16, 400 frames) settles with
  `wanted_columns = 0`, `ready_columns == loaded_columns`, and no pending jobs.
- Rotating in place does not reorder the candidate list until the player crosses a column; the
  forward-first preference is a scheduling nicety, not a correctness rule.
- A failed unload save is retried from a rebuilt list on the next frame instead of every frame.

### Saved-chunk index

Streaming probed the save directory for every generated chunk, a failed `fs::read` for each
coordinate that had never been edited. The chunk directory is now listed once at startup, and the
per-chunk read only runs when the listing contains the coordinate. Unloads and `Ctrl+S` add the
chunks they write.

| Run | Probe per chunk | Indexed |
|---|---:|---:|
| Headless wall, distance 24 | 7.8-7.9 s | 7.8 s |
| Headless wall, distance 32 | 12.6-12.8 s | 12.6 s |
| Headless mean / p95, distance 32 | 1.50 / 2.53 ms | **1.18 / 2.23 ms** |

Wall time is unchanged on this machine, where the failed reads are cheap and cached. The index
removes about 9,300 filesystem calls per distance-32 load and the stall risk behind the occasional
settle outlier. A missing directory yields an empty index; a listed coordinate still goes through the
normal per-chunk loader, so corrupt or mismatched files report the same errors.

### Pool high-water

The frame log now reports `allocated_geometry_mb` (quad-pool pages currently reserved) and
`peak_geometry_mb` (session high-water), alongside the fixed 38.18 MiB reservation.

| Distance-32 settled | Value |
|---|---:|
| Peak allocated quads | 2.36 M |
| Peak allocated records | 28.32 MB |
| Pool reservation | 2.36 M x 1.35 = 3.19 M, matching the 3.2 M cap |

The current cap is the measured peak plus the planned 35% slack, so it is not over-provisioned and
does not need resizing. A smaller pool would trade a few MB of VRAM for pool-full risk on other seeds
or views.

### Renderer traversal

The quad path stored slots in a `HashMap<ChunkCoord, Slot>` and allocated two indirect-argument
`Vec`s per frame. Slots are now a dense `Vec` with a chunk-to-position map (swap-remove on unload),
and the argument lists are reused. The frame log now reports `stream_ms`, `levels_ms`, and
`render_ms` separately.

| Distance-32 settled frame | Value |
|---|---:|
| Whole frame | 0.588-0.602 ms |
| `render_ms` | 0.47-0.60 ms |
| `stream_ms` / `levels_ms` | <0.001 ms |

- The dense slots are neutral at this scale: the frame is dominated by surface acquire/present and
  the fixed submission path, not the slot traversal. They remove two per-frame allocations and make
  the traversal sequential; they are kept for the larger hierarchy node sets.
- One 80 s run logged 0.65 ms and a 54 ms stall in an otherwise 0.59 ms run; frame-time outliers are
  dominated by the window/surface state, so compare ranges, not single samples.

### Frame capture

`ENGINE_CAPTURE=<path.bmp>` and `ENGINE_CAPTURE_DELAY=<seconds>` (default 20) copy the presented
surface to a readback buffer and write a 24-bit BMP once. It does not need window focus, so it is the
repeatable visual gate for horizon and hierarchy work.

The horizon A/B at the spawn view:

- **Off:** the full ridge and its trees are visible.
- **On, before the fix:** the far part of the ridge beyond the near crest was removed, leaving a
  vertical cut at a chunk boundary.
- **On, after the fix:** the capture matches the off view on seed 42, seed 7, and a strafe
  view. Occluders now use the chunk's **minimum** column top (`Chunk::terrain_min`) instead of its
  maximum, so a crest cannot act as a wall across a slope; the occluder span also uses the chunk's
  16 m half-width rather than 24 m. Trees are excluded from both tops.
- The conservative occluder reduces the cull: 819 visible chunks instead of 166, 1.84 M vertices,
  and a 0.139 ms GPU scene against 0.245-0.253 ms with horizon off.
- Horizon is now **on by default**; `ENGINE_HORIZON_CULL=0` disables it. `ENGINE_AUTO_STRAFE=1` is
  the dev hook used for the moving capture.

### Hierarchy selection and the 2 km gate

`ENGINE_HIERARCHY=1` stops L0 at `ENGINE_DETAIL_CHUNKS` (default 16, 512 m). Level-2 parents cover
768-2,048 m and refine to their eight level-1 children when a projected-size threshold is met and all
children are ready; otherwise the parent stays drawn. Level-1 sections also fill the near ring.
Refine/collapse use 72/62 px at 900 px/rad, with the parent retained until the child swap. Uploaded
children stay in the pool but hidden until the swap; one target rebuild runs per 250 ms.

| Settled 2 km view (west, spawn) | Value |
|---|---:|
| Target sections | 1,486 |
| Visible nodes | 1,045 |
| Visible vertices | 3.58 M |
| Draw calls | 2 |
| Frame | 0.466 ms |
| GPU scene | 0.235 ms |
| Allocated quads | 30.94 MB |
| Geometry reservation | 39.31 MB |

- Headless L0 at detail 16: 2,383 chunks, 85.9 MB resident, 4.3 s settle. With the pool reservation
  the 2 km working set is roughly 125 MB, well inside 400 MB.
- The west capture matches the L0-only control for the visible island and caves; the hierarchy fills
  the ring beyond the 1,024 m L0 edge. The earlier dark coarse-water seam was fixed by using the
  highest solid sample as the coarse sky top, so water cells above the seabed stay lit.
- Coarse cliffs still use per-column sky light (0 below the top), so unlit side faces read dark next
  to L0 BFS light. That is the design's first-slice lighting; coarse BFS is the next lighting step.
- Caves and trees above L0 are intentionally absent; the far terrain is blockier and smoother.
- Edit invalidation marks the overlapping level-1/2 sections dirty, removes the stale upload, and
  rebuilds from the modified/saved chunk overlay before the normal queue.

### Section foundation (level 1)

The first hierarchy slice adds `src/section.rs`: `SectionKey` math, a level-1 coarse generator using
the conservative `2 × 2` footprint maximum, a modified-chunk overlay, per-column coarse sky light,
and a sample-space greedy mesh through `build_section_mesh`. It is not wired into streaming or the
renderer yet. `bench_section` compares one level-1 section with the eight LOD1 chunk meshes covering
the same `64³` region at seed 42's spawn:

| Metric | One level-1 section | Eight LOD1 chunks |
|---|---:|---:|
| Generation | 629 µs (p95 868) | not timed |
| Mesh | 1,053 µs (p95 1,184) | 2,360 µs |
| Quads | 2,193 | 3,613 |
| Vertices | 8,772 | 14,452 |
| Samples / storage | 32,768 B | 131,072 B |
| Light | 19,652 B | 157,216 B |
| Draw calls | 1 | 8 |

- The section covers the same volume with ~39% fewer quads and ~18% of the sample + light bytes.
  Its mesh is cheaper than the eight LOD1 meshes because the coarse surface has no caves or trees.
- The comparison uses LOD1 chunk meshes from freshly generated L0 chunks; the section uses the
  surface-only seed rule, so the visual result is a comparison, not a pixel match.
- Missing neighbour sections emit boundary faces (the mesher gets an empty border), which is the
  deliberate no-crack rule for the first slice.

### GPU timing and indexed draws

`wgpu-profiler` now runs unconditionally; the `tracy` feature only adds its Tracy client. The frame
log reports `gpu_ms` for the scene pass. The quad pipeline draws four vertices per quad through a
shared `u16` index pattern; meshes above 16,383 quads keep the six-vertex non-indexed path.

| Distance-32 settled | Non-indexed | Indexed |
|---|---:|---:|
| GPU scene | 0.38-0.59 ms | **0.253 ms** |
| Frame | 0.62-0.65 ms | **0.479-0.482 ms** |
| Draw calls | 2 | 2 |
| Visible chunks / vertices | 2,435 / 3.64 M | 2,435 / 3.64 M |
| Geometry reservation | 38.18 MiB | 39.31 MiB |

- The index buffer costs 187.5 KiB and the four-list argument buffer about 0.9 MiB; the reservation
  stays under the 40 MiB target. The pool peak is unchanged at 28.32 MB.
- The pattern is `[4q, 4q+1, 4q+2, 4q, 4q+2, 4q+3]` with `base_vertex = first_quad * 4`, so the
  shader derives the pool quad from `vertex_index / 4` and the corner from `vertex_index % 4`. A unit
  test pins the pattern against the CPU corner order `[0,1,2,0,2,3]`.
- `visible_vertices` keeps the six-per-quad meaning so the metric stays comparable; the GPU now runs
  four vertex invocations per quad.
- `Depth24Plus` instead of `Depth32Float`, and `textureSampleLevel(..., 0.0)` instead of
  `textureSample`, were measured together against the settled indexed frame. GPU time stayed at
  0.248-0.249 ms against the 0.245-0.253 ms baseline, within noise, so both were reverted.

### Light job

The job used to scan the full `96³` propagation grid three times: once to fill blocks and column
ceilings, once to mark sky and collect lamps, and once more to seed the sky flood fill. Lamp
discovery moved into the block fill, and sky seeding now runs only above each column's ceiling and
tests the boundary in the same pass. The snapshot also shares the chunk storage behind an `Arc`
instead of copying each packed chunk.

| Fixture (`bench_light`, 20 samples) | Before | After |
|---|---:|---:|
| Surface compute | 5,164 µs | **4,790 µs** |
| Ground compute | 4,370 µs | **3,552 µs** |
| Snapshot build | ~6.5 µs (copy) | **1.7 µs** (Arc clone) |
| Headless d32 wall | 12.1-12.8 s | **11.9 s** |

- The merge removes one full-grid pass and skips every cell below its column ceiling; the ground
  chunk gains more because more of its grid is shadowed.
- The light benchmark is noisy outside saturation; one after-run measured 5.5 ms. The before/after
  pair above was taken from the same binary with the implementation stashed.
- `Chunk` now stores its packed cells in `Arc<Vec<u8>>`; an edit clones the 16 KiB buffer once
  (`Arc::make_mut`) and worker snapshots only bump the refcount.

### Corner neighbourhood gather

`corner_values` re-read the outside cell and its edge/diagonal neighbours for each of the four
corners, and re-read the same cells again to test light candidates. It now gathers the 3x3 ring once
per exposed cell and derives AO and light from that. Output is unchanged; the mesher tests compare
the exact records. `bench_mesh_build`, release, 30 samples:

| Fixture | Before | After | Change |
|---|---:|---:|---:|
| Spawn surface LOD0 | 1,131 µs | **955 µs** | -16% |
| Spawn ground LOD0 | 1,715 µs | **1,134 µs** | -34% |
| Flat slab LOD0 | 1,188 µs | **859 µs** | -28% |
| Spawn surface LOD1 | 299 µs | 317 µs | +6% |
| Spawn ground LOD1 | 534 µs | **278 µs** | -48% |
| Flat slab LOD1 | 295 µs | **142 µs** | -52% |
| Spawn ground LOD2 | 139 µs | **74 µs** | -47% |
| Water, surface | 512 µs | 519 µs | +1% |
| Water, ground | 518 µs | **460 µs** | -11% |

The gather helps most on nearly flat chunks: the top face exposes 1,024 cells that merge into one
quad, so the old per-cell corner reads dominated the merge. Water uses the same solidity ring but
keeps the water cell's uniform light, so it is unchanged within noise.

### Binary greedy attempt (reverted)

A bitmask greedy mesher with the strict light merge key was implemented and measured against the
current mesher. Output matched exactly (248 tests), but it was slower:

| Fixture | Current | Bitmask |
|---|---:|---:|
| Spawn surface LOD0 | 1.03 ms | 1.02 ms |
| Spawn ground LOD0 | 1.51 ms | 1.53 ms |
| Flat slab LOD0 | 0.98 ms | 1.08 ms |
| All air LOD0 | 0.37 ms | 0.48 ms |
| Water, surface | 0.44 ms | 0.64 ms |
| Water, ground | 0.46 ms | 0.68 ms |

The bitmask build evaluates passability for every padded cell; the current scan short-circuits the
exposure test for non-matching cells, and the per-cell corner/light key dominates the merge. Reverted.
The next experiment is the relaxed merge key described in [Meshing research](research/meshing.md).

### Horizon culling

Horizon culling is on by default; `ENGINE_HORIZON_CULL=0` disables it. It builds a 128-bin horizon
from the lowest terrain/water column top in each chunk, patched across each chunk's azimuth span, and
culls only when nearer terrain hides the chunk's top in every direction its footprint covers.

| Metric at distance 32, settled | Off | On |
|---|---:|---:|
| Visible chunks | 2,435 | 819 |
| Visible vertices | 3.64 M | 1.84 M |
| Visible geometry | 6.94 MiB | 3.50 MiB |
| GPU scene | 0.245-0.253 ms | **0.139 ms** |
| Frame | 0.479-0.498 ms | 0.516-0.821 ms |

- Culling removes two-thirds of the visible set on the spawn view and about half of the visible
  vertices.
- The measured GPU scene time falls by 45%; the frame is still acquire/present-bound, so frame time
  does not improve. The periodic horizon build and run-to-run variance dominate the frame samples.
- A full horizon build costs about 2.4 ms, so it is cached and rebuilt at most every 250 ms.
- The threshold and margins make it deliberately conservative: chunks under 256 m are never culled,
  empty bins keep a chunk, and the elevation margin is about 1.7 degrees. Occluders use the lowest
  terrain column top, candidates use `max(terrain top, water top)`, and trees are excluded from both.
- In-engine captures on seeds 42 and 7 and on a strafe view match the horizon-off image. The feature
  is on by default; `ENGINE_HORIZON_CULL=0` disables it.

### Windowed baseline

Same world and camera, 30 s (distance 24) and 60 s (distance 32) after startup, `ENGINE_NO_VSYNC=1`,
`RUST_LOG=engine=debug`, window minimised. The player stands at the seed 42 spawn `(3.5, 36.0, 0.5)`
and does not move.

| Distance | Chunks | Visible chunks | Draws | Indices drawn | Geometry | Visible geometry | Frame CPU |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 24 | 5,289 | 1,436 | 1,934 | 2.98 M | 89.3 MB | 28.5 MB | 2.24-2.39 ms |
| 32 | 9,285 | 2,435 | 3,409 | 3.94 M | 125.9 MB | 37.6 MB | 9.06 ms |

- `geometry_mb` counts every retained vertex and index buffer: all built LODs plus water.
  `visible_geometry_mb` counts only the buffers reached by that frame's draws. Both are new
  metrics from this slice.
- Indices divided by six give drawn quads: 497,000 at distance 24 and 657,000 at distance 32.
  At four 12-byte vertices and six 16-bit indices per quad that is 60 bytes per quad, which matches
  `visible_geometry_mb` to within rounding.
- Distance 32 draws 77% more calls and costs 3.8x the frame CPU of distance 24. One sample during
  streaming logged 18.4 ms; the settled figure above is the comparable number.
- Resident geometry is 89 MB at distance 24 and 126 MB at distance 32, not the 60-80 MB estimated
  after the v0.0.8 packing. Retained LODs, not the visible set, dominate it: 1.48 M and 2.10 M quads
  respectively.
- A fixed 12-byte quad would put the same geometry in 18-25 MB; an 8-byte quad in 12-17 MB. That is
  the budget the v0.0.9 page format is measured against.

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

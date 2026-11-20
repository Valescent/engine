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
| Terrain v1 demo | Current; see [Terrain v1 demo](#terrain-v1-demo-2026-11-20) |
| Terrain blend calibration | Current; fails the v1 target; see [Terrain blend calibration](#terrain-blend-calibration-2026-11-20) |
| Terrain PR counters | Current; gated by the `CI` terrain job; see [Terrain PR counters](#terrain-pr-counters-2026-11-20) |
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
## Terrain blend calibration (2026-11-20)

`terrain-gen calibrate-blend` measures `W_blocks` from `w = 0.5` to `w = 0.95`, doubled, per
unordered biome pair. The definition is in [Terrain tools](terrain-tools.md#calibrate-blend).
This run used the whole 2048-block world window (`--from -1024 -1024 --size 2048`) and the
default 16-block anchor stride on the shipped v1 `blend_width = 0.05`:

```console
cargo run --locked --quiet --no-default-features --bin terrain-gen -- calibrate-blend --seeds 3
```

| Pair | Transects | Median | p10 | p90 | Assessment |
|---|---:|---:|---:|---:|---|
| 0-1 (plains/forest) | 5,219 | 48.76 | 27.12 | 172.60 | fails median and p90 |
| 0-2 (plains/desert) | 869 | 43.50 | 25.06 | 164.02 | fails median and p90 |
| 0-3 (plains/tundra) | 1,118 | 71.52 | 33.08 | 278.26 | fails median and p90 |
| 1-3 (forest/tundra) | 707 | 38.01 | 20.38 | 159.61 | fails median and p90 |
| 2-3 (desert/tundra) | 5 | 47.71 | 30.45 | 78.53 | reported only (<20 transects) |

The command exits `1`. The measured medians are about twice the `20-28` target, which is
consistent with `blend_width = 0.05` being the documented **starting point**, not a measured
value: halving `W_blocks` needs roughly `blend_width = 0.1`. The p90 spread is wider than the
`<= 64` gate even after that scale change. The interview's `E3.6` answer records the intended
follow-up: fix the data (biome centres and weights) first, and add the gradient-normalising term
in a later version bump. Changing `blend_width` or the biome table changes generated blocks, so
it is a separate, generator-versioned change with fresh goldens, sweeps, and counter baselines.

The tool is therefore local-only for now and is not part of the `Terrain gates` job.

## Terrain PR counters (2026-11-20)

`counters --tier pr` pins the deterministic part of the `Q1.1` metric list for the 16 PR seeds.
The checked-in baseline is `tests/golden/terrain-counters.json`; the selection rules are in
[Terrain tools](terrain-tools.md#counters). The first baseline:

| Counter | Value |
|---|---:|
| `blocks_air` | 579,022 |
| `blocks_solid` | 182,722 |
| `blocks_water` | 24,688 |
| `columns` | 2,048 |
| `lod0_columns` | 0 |
| `queued_columns` | 0 |
| `ready_columns` | 512 |
| `resident_columns` | 512 |

Wall-clock latencies, per-second rates, and runtime-only residency and worker-health counters are
excluded because they are not deterministic for the headless workload. The baseline moved once
for this change; later movements require `counters --tier pr --update` and a reason in the commit.

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

## Terrain v1 demo (2026-11-20)

Release build, 12 logical cores, `TERRAIN_DEMO=1`, 128x128 one-tile patch. Measured by
`demo_build_phase_split`, `demo_rebuild_cost_split`, and the startup log.

| Phase | Time |
|---|---:|
| `Terrain::new` | ~0 ms |
| Field tile | 18 ms |
| Column generation (12 workers) | 815 ms |
| Column light | 31 ms |
| Fine meshing | 31 ms |
| Coarse LOD meshing | 45 ms |
| **Startup build** | **~0.89 s** |

With world-mode hydrology (generator version 2) the demo solves hydrology once and shares it:

| Phase | Time |
|---|---:|
| Sample 262,144 macro heights | 66 ms |
| Priority flood, D8, accumulation, lakes | 98 ms |
| **One shared solve** | **~165 ms** |
| **Startup build** | **~0.97 s** |

| Edit rebuild (surface break) | Before | After |
|---|---:|---:|
| Sections remeshed | 27 | **1** |
| Light | 49–56 ms | **22.1 ms** |
| Mesh | 26–37 ms | **1.8 ms** |
| **Total** | ~55 ms | **~24 ms** |

- Spawn active set: 29 sections / 66,293 quads; far corner drawn at LOD1, 64 blocks away.
- Working set: 350 MB with the demo vs 295 MB for the legacy stress scene; tile streaming (active
  + three cached tiles) adds ~55 MB.
- Startup before the tile alignment and worker fan-out was 5.14 s; generation was 3.54 s serial.
- `stream` prefetch builds one tile (~0.9 s) at a time while the frame loop stays at ~60 FPS.



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

# Performance backlog

Work items from the v0.0.9 8 km profile. This page is a living list: move an item to **done** only
after it has a measurement and a test or benchmark. The source profile is a 40 s, 8 km spawn run
captured with `--features tracy`:

- 26,647 frames, 1.99 M zones, 56.3 MB trace.
- The instrumented frame is about 1.5 ms; the normal build is 0.58-0.69 ms. Use the ranking, not
  the absolute times.
- Self-time leaders: `section build` 34.2 s (5,532 jobs, 6.18 ms each), `chunk light` 20.8 s,
  `encode` 534 µs/frame, `frame` self 283 µs/frame, `chunk generate` 6.7 s, `chunk mesh` 3.0 s,
  `mesh request` 88.5 µs/frame.

Baseline at `0b65acc`:

| Gate | Value |
|---|---:|
| 8 km visible nodes | 1,259 |
| 8 km horizon-culled sections | 646 of 1,330 in-frustum (`bench_horizon_sections`) |
| 8 km visible vertices | 5.15 M |
| 8 km steady frame / GPU | 0.58-0.69 / 0.323 ms |
| 8 km hierarchy settle | 6.4-6.9 s |
| `section build` | ~6.2 ms/job under Tracy |
| `chunk light` | ~8.7 ms/job under Tracy |

The windowed frame log also prints a 652-of-1,911 horizon counter, but that sample covers a
different in-frustum set; the benchmark's 646 of 1,330 is the baseline. After the review fixes the
counts are 552 of 1,330 offline and 553 of 1,911 in the windowed run; the conservative full-bin
coverage rule intentionally culls less.

## Steady frame

| ID | Item | Expected | Effort | Status |
|---|---|---|---|---|
| SF1 | Cache horizon occlusion at the 250 ms rebuild; make `occludes` / `occludes_section` hash lookups | High | S | done |
| SF2 | Iterate occupied quad-pool slots instead of all 20,480 | High | S-M | done |
| SF3 | Add scopes to `stream_columns`, `request_levels`, `ensure_hierarchy`, `record_frame`; attribute the 283 µs `frame` self time | Enables SF1-SF2 follow-up | S | done |
| SF4 | Reuse the per-frame texture view and resolve GPU timestamp queries every N frames | Low-medium | S | deferred; the view is the acquired swapchain texture and timestamp batching is unsafe in `wgpu-profiler` 0.28 |
| SF5 | Cross-section greedy merging for coplanar top faces | Medium (GPU) | M-H | deferred |
| SF6 | Hi-Z or software occlusion beyond the heightmap horizon | Low for the far heightmap | M-H | deferred |

## Hierarchy load

| ID | Item | Expected | Effort | Status |
|---|---|---|---|---|
| HL1 | Bounds-limit the section mesher to the solid/water y range | High (2-4x mesh) | M | done |
| HL2 | Fill generated columns in bulk instead of per-sample `set_sample` | High (2-3x generate) | M | done |
| HL3 | Add `section generate` / `section bounds` / `section light` / `section mesh` scopes | Enables HL1-HL2 | S | done |
| HL4 | Tune `MAX_SECTION_JOBS` and section priority on the shared pool | Medium (settle) | S | done; 8 -> 11 jobs, 6.53 s -> 5.69 s with separate upload budgets; the shared 16-result budget settles in 7.8 s |
| HL5 | Optimize the chunk-light BFS (bitsets or a flatter queue) | Medium (load) | M | todo |
| HL6 | Reduce the L0 detail radius | High, but lowers near detail | S | rejected |

## Current results

Measured after the review fixes, on the same spawn view:

| Gate | Before | Current |
|---|---:|---:|
| 8 km steady frame / GPU | 0.58-0.69 / 0.323 ms | **0.62-0.63 / 0.365 ms** |
| 8 km hierarchy settle | 6.4-6.9 s (22.9 s before the pool) | **7.8 s** with the shared 16-result budget |
| Visible nodes / vertices | 1,259 / 5.15 M | **1,358 / 5.86 M** |
| Horizon-culled sections | 646 of 1,330 | **552 of 1,330 (778 visible)** |
| L3 far section generation | 1,793 µs | **1,135 µs** |
| L3 far opaque mesh | 1,784 µs | **642 µs** |
| L3 far water mesh | 1,139 µs | **248 µs** |
| Horizon rebuild | ~0.74 ms | ~2.58 ms every 250 ms; zero per-frame bin scans |

The steady frame and visible geometry are higher than the pre-fix 0.55-0.56 ms / 5.15 M because the
horizon now marks a bin covered only when the occluder interval fully contains it. That conservative
rule is quality-safe but draws 94 more sections. The section-job cap test used two settled runs:
`MAX_SECTION_JOBS=8` gave 6.53 s and 11 gave 5.69 s with separate 8 + 8 upload budgets. Chunk and
section results now share one budget; 8 slowed the settle to 8.9-10.3 s, while the shipped 16 restores
7.8 s. The far-section mesher remains the main load win because far sections hold only a few
solid/water layers; near level-1 sections span almost the whole grid and improve less.

## Rules

- Every item keeps the visible image identical unless the row says otherwise.
- Measure the steady frame with a settled 8 km spawn run; measure load from
  `generated world ready` to `hierarchy target target=4908 queue=0`.
- Keep frame and headless benchmarks isolated from background builds and agents.
- Do not relax light/AO merge keys, LOD thresholds, or the horizon margins for a speed number.

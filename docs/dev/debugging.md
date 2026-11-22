# Debugging

Debugging tools and workflows, current and planned. Sections marked **planned** are not
implemented. Environment variables are defined in [Configuration](configuration.md), and
input bindings are in [Controls](controls.md). For a symptom-first index, see
[FAQ and troubleshooting](../faq.md#troubleshooting-index).

| Area | Status |
|---|---|
| stderr logging | Current |
| Per-run log file | Planned, no milestone |
| Typed startup errors and validation | Current |
| Frame-timing log | Current |
| FPS/frame-time overlay (`F3`) | Current |
| `pool_sim` benchmark | Current |
| Tracy CPU/GPU profiling | Current behind the `tracy` feature |
| Milestone metrics | v0.0.3, v0.0.5, v0.0.7, and v0.0.8 current |
| Debug overlay and crash reporting | FPS/frame-time overlay current; crash reporting not implemented |

## Logging

The engine uses `tracing` and writes human-readable text to stderr. `RUST_LOG` is an
`EnvFilter`; unset or invalid falls back to `info`.

- `RUST_LOG=engine=debug` adds frame timing, input, and voxel events.
- `RUST_LOG=engine=debug,wgpu=warn` adds engine detail without the full `wgpu` trace.
- `RUST_LOG=off` silences the engine.

The engine emits `error`, `warn`, `info`, and `debug` events. It does not emit `trace` events
or spans.

| Level | Module | Event |
|---|---|---|
| `info` | `engine::renderer` | Selected GPU adapter, with adapter name and backend. |
| `info` | `engine::renderer` | Renderer ready, with surface size and vertex count. |
| `info` | `engine` | Window ready, with physical size. |
| `debug` | `engine` | Frame timing (`fps`, `frame_ms`, chunk counts, vertices, draw calls, rebuild/upload/generation times, generated columns) about once per second while the window is visible. |
| `debug` | `engine` | Toggled voxel, with the new voxel count. |
| `debug` | `engine` | Cursor captured or released. |
| `debug` | `engine` | Locked cursor grab unavailable; trying a confined grab. |
| `warn` | `engine` | Cursor capture or release failed. |
| `error` | `engine` | Engine startup failed. |
| `error` | `engine::renderer` | Surface acquisition hit a validation error. |
| `error` | `engine::renderer` | Failed to recreate the window surface. |

At the default `info` level, startup events and warnings or errors are visible. Frame timing,
input, and voxel events require `engine=debug`.

### Planned log file

No log file is written today. The decided shape is:

- **Path:** `logs/engine-YYYYMMDD-HHMMSS.log` under the process working directory.
- **Frequency:** one file per run, always written alongside stderr in debug and release
  builds.
- **Format:** the same `RUST_LOG` filter and human-readable `tracing` text as stderr.
- **Retention:** no automatic rotation or deletion; old files are removed manually.
- **Git:** `/logs` is ignored by `.gitignore`.

No milestone is assigned to the log file. Set `RUST_LOG=engine=debug` to include frame timing
in both stderr and the file.

## Errors and panics

- Startup failures are logged at `error`, stored on `App`, and returned as `RunError` after the
  event loop exits.
- Errors are typed with `thiserror`: `RunError` covers event-loop, window, and renderer
  failures; `RendererError` covers surface, adapter, device, unsupported-surface, and validation
  failures.
- Pipeline creation uses a validation error scope. A shader or pipeline validation error becomes
  `RendererError::Validation`, which fails startup instead of being swallowed. See
  [Shader authoring](shaders.md#validate).
- World save/load errors are typed `SaveError`s; see
  [World management](world-management.md#corruption-recovery).
- Terrain v1 persistence errors are typed `TerrainError`s on `Stage::Persistence`. Run
  `RUST_LOG=engine=debug` with `TERRAIN_DEMO=1` to log `terrain rebuild applied` (light/mesh/upload
  times, patch count), `terrain stream tile build started/ready/swapped`, and repair/migration
  outcomes. Saves live in `world/terrain-demo/`; see [Terrain v1 runtime](terrain-v1.md).
- Surface acquisition that reports validation logs an error and skips that frame.
- There is no panic hook, crash reporter, or minidump writer.
- Set `RUST_BACKTRACE=1` or `RUST_BACKTRACE=full` to print a standard Rust backtrace on panic.
- Panics are reserved for unreachable invariants; user input and disk data return errors. See
  [Conventions](conventions.md).

## Profiling

### Current aids

- The frame-timing log and the `F3` overlay are the built-in runtime measurements. The overlay
  refreshes once per second; run with `RUST_LOG=engine=debug` to also log `fps` and `frame_ms`.
- `cargo bench --bench pool_sim` compares rayon with micropool on the v0.0.5 async-meshing frame
  loop. It is a decision aid, not a regression gate: keep both backends in one run on the same
  machine, and do not add it to CI. Recorded runs and the entry template are in
  [Benchmark results](benchmarks.md).
- Tracy CPU, memory, and GPU profiling is on by default in debug builds, which also save a trace
  under `reports/tracy/`. See [Profiling](profiling.md). The terrain path adds
  `terrain_sections_uploaded` and logs its startup phase split.

### Tracy

[ADR 0006](../decisions/0006-tracy-for-profiling.md) selects Tracy, instrumented through
`tracy-client`, with `wgpu-profiler` reporting GPU timer queries into the same timeline.

The setup, pinned versions, frame wiring, scopes, plots, and troubleshooting are in
[Profiling](profiling.md). The feature is local-only because it builds Tracy's C++ client, so CI
does not compile it.

## Metrics

| Milestone | Output |
|---|---|
| v0.0.3 | After an edit: dirty chunk count, total vertices, and rebuild time. Implemented. |
| v0.0.5 | Per frame at debug level: frame time, visible and total chunks, vertices, draw calls, rebuild time, and upload time. Implemented. |
| v0.0.7 | Per frame at debug level: generated columns and their elapsed generation time, plus queue depth, visible and total chunks, vertices, and frame time. Implemented; `generated_columns` and `generation_ms` are also Tracy plots. |
| v0.0.8 | Headless runs print frame work percentiles, ready columns, pending jobs, and resident bytes. Implemented; the `Perf` workflow gates on them. |

## External tools

[ADR 0006](../decisions/0006-tracy-for-profiling.md) keeps RenderDoc, Nsight, and PIX as
complementary frame debuggers for pipeline and shader inspection. They are external tools; the
engine has no capture integration or setup instructions for them. Tracy does not replace them.

## Not implemented

| Facility | Status |
|---|---|
| Debug overlay or HUD | FPS/frame-time overlay on `F3`; no other HUD. |
| Debug console, commands, or CVars | Not implemented. |
| Chunk-border, frustum, or other debug views | Not implemented. |
| Crash reporter or minidump writer | Not implemented. |
| Engine-side GPU capture integration | Not implemented; use external frame debuggers. |
| Automated GPU tests | Not yet; rendering is checked manually. |
| Log file | Planned, no milestone. |
| Tracy integration | Current behind the `tracy` feature. |

## Capturing a report

- Run with `RUST_LOG=engine=debug,wgpu=warn` and save stderr. The planned log file will capture
  the same output automatically.
- Include the adapter name and backend from the `selected GPU adapter` log, the window size, the
  OS and GPU driver version, and the reproduction steps.
- For backend-specific problems, compare `WGPU_BACKEND=dx12`, `vulkan`, or `metal` before
  assuming an engine bug.
- For a panic, include the `RUST_BACKTRACE=1` output.
- For rendering artifacts, attach a frame capture from RenderDoc, Nsight, or PIX and name the
  tool and version.

See [Testing](testing.md) for the manual GPU acceptance procedure.

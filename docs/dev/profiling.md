# Profiling

Tracy shows a live, per-frame CPU timeline with per-thread lanes. `wgpu-profiler` adds GPU timer
queries to the same view, so a slow frame can be attributed to CPU work or GPU work instead of
guessed at. [ADR-0006](../decisions/0006-tracy-for-profiling.md) selects this pair.

> **Status: current, behind the `tracy` feature.** `cargo run --features tracy` starts the Tracy
> client, registers the frame marks, spans, and plots, and creates a `wgpu-profiler` GPU context
> when the adapter supports timestamp queries. The default build has no profiling code enabled
> and no CPU cost beyond a few no-op macros.

| Area | Status |
|---|---|
| Tracy CPU timeline | Current (`tracy` feature) |
| `wgpu-profiler` GPU zones | Current (`tracy` feature, when timestamps are supported) |
| `tracy` Cargo feature | Current; local builds only, not in CI |
| Timestamp-query device features | Requested only with `tracy`, intersected with adapter support |
| Memory allocations | Current; `ProfiledAllocator` in `src/main.rs` |
| Plots | Current; `frame_ms`, `gpu_ms`, `visible_chunks`, `draw_calls`, `visible_vertices`, `rebuild_ms`, `upload_ms`, `mesh_jobs`, `generated_columns`, `generation_ms` |
| Frame-timing log | Current; see [Debugging](debugging.md#logging) |
| `pool_sim` benchmark | Current; see [Benchmark results](benchmarks.md) |
| Frame debuggers (RenderDoc, Nsight, PIX) | Complementary external tools; see ADR-0006 |

## Constraints

Both gates from the original plan are decided.

- **CI does not build the feature.** The `CI` workflow runs Clippy, tests, and docs without
  `--all-features`; do not add it back. The `tracy` feature compiles Tracy's C++ client, and
  ADR-0006 keeps profiling builds local. Run the instrumented checks yourself with
  `cargo clippy --all-targets --features tracy -- -D warnings`.
- **Broadcast remains compiled in.** `wgpu-profiler` and `profiling` keep `tracy-client`'s
  default features, so `broadcast` stays enabled even with `only-localhost`. The client binds to
  loopback and broadcasts to `127.255.255.255`, so traffic stays on the machine. That meets the
  intent of ADR-0006 but not its literal "disable broadcast" wording. Revisit it if the
  dependency set changes.

Traces can contain source paths, symbols, and machine code. Treat a saved trace as sensitive and
do not attach one to a public issue without checking it.

## What each tool shows

| Tool | Shows | Does not show |
|---|---|---|
| Tracy | Instrumented CPU spans, frame marks, per-thread lanes, plots, messages, memory events | Uninstrumented code |
| `wgpu-profiler` | GPU timer queries around encoders and passes | GPU work outside the submitted scopes |
| Combined | CPU and GPU duration on one timeline | Why an uninstrumented path is slow |

The combination answers the first question of a slow frame: is the CPU still meshing while the
GPU is idle, or is the GPU saturated while the CPU waits? A gap in the timeline is a missing
span, not a finding.

## Version set

Use this exact set. The versions are coupled by protocol, not just by semver.

| Component | Pin | Reason |
|---|---|---|
| `tracy-client` | `=0.18.4` | `wgpu-profiler` 0.28 and `profiling` 1.0.18 both use `tracy-client` 0.18. `0.18.5` does not compile against `tracy-client-sys` 0.28.0; it expects a newer sys API. |
| `tracy-client-sys` | `=0.28.0` | Bundles Tracy 0.13.1 and fixes the client protocol. |
| `wgpu-profiler` | `=0.28.0` | Current release with `wgpu` 30 support. |
| `profiling` | `=1.0.18` | CPU macro layer; `profile-with-tracy` uses `tracy-client` 0.18. Already in the graph through `wgpu`. |
| Tracy GUI | `0.13.1` | Matches the client bundled by `tracy-client-sys` 0.28.0. |

Do not add `tracing-tracy` 0.12. It requires `tracy-client >=0.19,<0.20`, which would link a
second client and two protocols into the process. Do not let `tracy-client` float on `0.18`:
`0.18.5` plus `tracy-client-sys` 0.28.0 is a verified compile failure. Upgrade the client, sys
crate, `wgpu-profiler`, `profiling`, and GUI as one set and reconnect the GUI after the upgrade.

## Prerequisites

- A C++ compiler. `tracy-client-sys` compiles `TracyClient.cpp` with `cc`. On Windows, the
  **Desktop development with C++** workload from [Setup](setup.md#platform-prerequisites)
  supplies it; on Debian/Ubuntu `build-essential` does; on macOS the Xcode command-line tools
  do.
- Tracy GUI 0.13.1. The Windows build is the `windows-0.13.1.zip` asset on the
  [v0.13.1 release](https://github.com/wolfpld/tracy/releases/tag/v0.13.1); it contains
  `tracy-profiler.exe`. The GUI is not vendored in this repository.
- A local build. Profiling is not part of CI, the release workflow, or a shipped binary.
- The first run may trigger a firewall prompt. Do not expose the port publicly.

## Cargo setup

`wgpu` already depends on `profiling` without a backend, so keeping `profiling` as a normal
dependency adds no new crate. The instrumented call sites then compile to no-ops when Tracy is
off, instead of needing a `#[cfg(feature = "tracy")]` on every span.

```toml
[features]
tracy = [
    "profiling/profile-with-tracy",
    "dep:tracy-client",
    "dep:tracy-client-sys",
    "dep:wgpu-profiler",
]

[dependencies]
profiling = { version = "=1.0.18", default-features = false }
tracy-client = { version = "=0.18.4", optional = true, default-features = false, features = [
    "enable",
    "ondemand",
    "only-localhost",
] }
tracy-client-sys = { version = "=0.28.0", optional = true, default-features = false }
wgpu-profiler = { version = "=0.28.0", optional = true, features = ["tracy"] }
```

`ondemand` starts collecting when the GUI connects, not at process start. `enable` is required;
without it the instrumentation is compiled but the client does not run. `tracy-client-sys` is
listed only to hold the protocol version; the code uses `tracy-client` and `wgpu-profiler`.

Enabling `profiling/profile-with-tracy` also turns on the `profiling` scopes inside `wgpu`, so
internal calls such as `Queue::write_buffer_with` appear in the timeline.

## Start the client

The client starts in `run()` and stays alive for the whole process. Start it before the GPU
profiler and before the first instrumented call: a `profiling::scope!` with the Tracy backend
panics if no client is running, and `new_with_tracy_client` returns
`CreationError::TracyClientNotRunning`.

```rust
#[cfg(feature = "tracy")]
let _tracy_client = tracy_client::Client::start();
```

`src/main.rs` also installs the memory profiler, which must live in the binary crate:

```rust
#[cfg(feature = "tracy")]
#[global_allocator]
static GLOBAL: tracy_client::ProfiledAllocator<std::alloc::System> =
    tracy_client::ProfiledAllocator::new(std::alloc::System, 0);
```

Callstack depth `0` keeps allocation overhead low; raise it only for a targeted allocation hunt.

## Request timestamp queries

`wgpu-profiler` needs `wgpu::Features::TIMESTAMP_QUERY` for its GPU-to-CPU timestamp readback.

| Feature | Enables |
|---|---|
| `TIMESTAMP_QUERY` | Pass-level scopes created with `scoped_render_pass` or `scoped_compute_pass`. Required by `new_with_tracy_client`. |
| `TIMESTAMP_QUERY_INSIDE_ENCODERS` | Scopes opened on a `CommandEncoder`. |
| `TIMESTAMP_QUERY_INSIDE_PASSES` | Scopes opened at arbitrary points inside a pass. |

Only the intersection with the adapter's features is requested, because requesting an
unsupported feature fails device creation. If `TIMESTAMP_QUERY` is missing, the GPU profiler is
skipped and CPU spans still work.

```rust
#[cfg(feature = "tracy")]
fn timer_features(adapter: &wgpu::Adapter) -> wgpu::Features {
    let requested = wgpu_profiler::GpuProfiler::ALL_WGPU_TIMER_FEATURES & adapter.features();
    if requested.contains(wgpu::Features::TIMESTAMP_QUERY) {
        requested
    } else {
        wgpu::Features::empty()
    }
}
```

## Create the GPU profiler

The profiler is created after the device and queue exist. The creation call blocks once to read
the initial GPU timestamp for Tracy's GPU context, so it belongs in startup, not the frame loop.

```rust
let profiler = wgpu_profiler::GpuProfiler::new_with_tracy_client(
    wgpu_profiler::GpuProfilerSettings::default(),
    adapter.get_info().backend,
    &device,
    &queue,
);
```

A failure logs a warning and disables GPU scopes; it never blocks startup. The default settings
keep three profiler frames in flight.

## Wire a frame

Order matters:

1. Open the frame scope.
2. Open GPU scopes and record passes with `scoped_render_pass`.
3. Call `resolve_queries` after the last scope and before `encoder.finish()`.
4. Submit and present.
5. Mark the CPU frame with `profiling::finish_frame!`.
6. Call `end_frame`.
7. Call `process_finished_frame` to drain the oldest completed GPU frame.

`scoped_render_pass` owns the pass; drop it before `resolve_queries`. `end_frame` returns an
error if a scope is still open or queries were not resolved; log it instead of ignoring it. GPU
results lag the CPU by a few frames because the readback must complete. Call
`process_finished_frame` every frame even if the result is unused; otherwise the pending-frame
cap fills and newer frames are dropped. The returned per-scope times are in seconds and feed the
`gpu_ms` plot.

## CPU scopes

| Section | Thread | Covers |
|---|---|---|
| `frame` | main | Redraw handler through present |
| `player update` | main | Gravity, walking, and the swept AABB collision query |
| `highlight` | main | Targeted-block raycast and outline rebuild |
| `mesh request` | main | LOD selection, snapshots, and job submission |
| `mesh upload` | main | Draining finished meshes and creating vertex buffers |
| `select active` | main | Picking the resident LOD mesh per chunk |
| `frustum` | main | Extracting the six view planes |
| `acquire` | main | Waiting for a swapchain image |
| `encode` | main | Command encoding and the render pass |
| `submit` | main | Encoder finish and `Queue::submit` |
| `present` | main | `Queue::present` |
| `profiler end` | main | `wgpu-profiler` frame end and timestamp readback |
| `chunk mesh` | worker | One greedy mesh from a chunk snapshot |
| `chunk generate` | worker | One generation job for a column |
| `chunk light` | worker | One light propagation job |

Rules:

- Do not put spans inside per-voxel or per-block loops. The observer effect and the timeline
  noise are both worse than the detail is worth.
- Nest scopes instead of repeating a name at the same level.
- Keep the per-frame span count bounded. A frame with hundreds of peer spans is unreadable.
- Worker lanes are named `chunk worker` once per pool thread so the lanes are labelled.

## Plots

`profiling` 1.0.18 has spans, thread names, and frame marks but no plot macro. `src/metrics.rs`
wraps `tracy_client` directly and is a no-op without the feature:

```rust
pub(crate) fn frame_ms(value: f64) {
    if let Some(client) = tracy_client::Client::running() {
        client.plot(tracy_client::plot_name!("frame_ms"), value);
    }
}
```

| Plot | Source |
|---|---|
| `frame_ms` | Wall time between redraws |
| `gpu_ms` | Summed GPU timer-query durations for the finished frame |
| `visible_chunks` | Frustum-culled chunks drawn this frame |
| `draw_calls` | Chunk draws this frame |
| `visible_vertices` | Vertices submitted this frame |
| `rebuild_ms` | Worker mesh time received this frame |
| `upload_ms` | Main-thread vertex-buffer upload time this frame |
| `mesh_jobs` | Mesh jobs currently in flight |
| `generated_columns` | Columns committed to the world this frame |
| `generation_ms` | Worker generation time received this frame |

Plots are emitted every frame. Do not add a plot per chunk.

## Run Tracy

```console
cargo run --features tracy
cargo run --release --features tracy
```

Start the engine, then `tracy-profiler.exe`. With `ondemand`, collection begins when the GUI
connects and the client connects automatically over loopback. The GUI then shows:

- the CPU timeline, including `frame`, `player update`, `mesh request`, `mesh upload`, `encode`,
  and the `chunk worker` lanes;
- the frame graph from the `frame` marks and the ten plots above;
- the memory view from the global allocator: live allocations, frees, and allocation counts;
- GPU zones `world` and `terrain` once a few frames of timestamp readback have completed.

Save a trace from the GUI if you need to compare runs; keep in mind that it contains source
paths and symbols. A missing GPU row means the adapter rejected `TIMESTAMP_QUERY`; check the
startup log for the warning.

Use a development build for source-level work. The repository's dev profile optimizes the
engine at `opt-level = 1` and dependencies at `3`, so the profiled code is not a debug build in
the usual sense. The release profile has thin LTO and line tables only; use it for realistic
frame times, not for detailed source attribution.

To compare backends, force one with `WGPU_BACKEND=dx12` or `WGPU_BACKEND=vulkan`. GPU timestamp
support and timer resolution can differ between them.

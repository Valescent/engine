# Profiling

Tracy shows a live, per-frame CPU timeline with per-thread lanes. `wgpu-profiler` adds GPU timer
queries to the same view, so a slow frame can be attributed to CPU work or GPU work instead of
guessed at. [ADR-0006](../decisions/0006-tracy-for-profiling.md) selects this pair.

> **Status: planned.** Tracy is not integrated. There is no `tracy` feature, no profiling
> dependency, no span, plot, or GPU zone in the tree. `src/renderer.rs` still requests
> `wgpu::Features::empty()`. The pinned set and snippets below were compiled in a scratch crate
> against `wgpu` 30; this repository does not compile them yet.

| Area | Status |
|---|---|
| Tracy CPU timeline | Planned (ADR-0006), not integrated |
| `wgpu-profiler` GPU zones | Planned (ADR-0006), not integrated |
| `tracy` Cargo feature | Not present |
| Timestamp-query device features | Not requested |
| Frame-timing log | Current; see [Debugging](debugging.md#logging) |
| `pool_sim` benchmark | Current; see [Debugging](debugging.md#current-aids) and [Benchmark results](benchmarks.md) |
| Frame debuggers (RenderDoc, Nsight, PIX) | Complementary external tools; see ADR-0006 |

## Before enabling this

Two constraints need a decision before the feature lands.

- **Broadcast is still compiled in.** `wgpu-profiler` and `profiling` request `tracy-client`'s
  default features. Cargo unions features, so adding `default-features = false` to this crate
  cannot remove `broadcast`, even though it does enable `only-localhost`. With `only-localhost`,
  the client binds to loopback and broadcasts to `127.255.255.255`, so traffic stays on the
  machine. That meets the intent of ADR-0006 but not its literal "disable broadcast" wording. If
  the literal wording is required, patch or vendor the dependencies, or revise the ADR.
- **CI runs `--all-features`.** The `CI` workflow runs `cargo clippy --all-targets --all-features`
  and `cargo test --all-features` (see [CI](ci.md#ci-workflow)). Adding a `tracy` feature makes
  CI build Tracy's C++ client on all three runners. ADR-0006 says profiling builds stay local
  and do not gate CI, so change the CI commands to an explicit feature list before adding the
  feature, or accept the extra build.

Traces can contain source paths, symbols, and machine code. Treat a saved trace as sensitive and
do not attach one to a public issue without checking it.

## What each tool shows

| Tool | Shows | Does not show |
|---|---|---|
| Tracy | Instrumented CPU spans, frame marks, per-thread lanes, plots, messages | Uninstrumented code |
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
| `profiling` | `=1.0.18` | CPU macro layer; `profile-with-tracy` uses `tracy-client` 0.18. |
| Tracy GUI | `0.13.1` | Matches the client bundled by `tracy-client-sys` 0.28.0. |

Do not add `tracing-tracy` 0.12. It requires `tracy-client >=0.19,<0.20`, which would link a
second client and two protocols into the process. Do not let `tracy-client` float on `0.18`:
`0.18.5` plus `tracy-client-sys` 0.28.0 is a verified compile failure. Upgrade the client,
sys crate, `wgpu-profiler`, `profiling`, and GUI as one set and reconnect the GUI after the
upgrade.

## Prerequisites

- A C++ compiler. `tracy-client-sys` compiles `TracyClient.cpp` with `cc`. On Windows, the
  **Desktop development with C++** workload from [Setup](setup.md#platform-prerequisites)
  supplies it; on Debian/Ubuntu `build-essential` does; on macOS the Xcode command-line tools
  do.
- Tracy GUI 0.13.1 from the upstream Tracy releases. The GUI is not vendored in this
  repository.
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

Start the client before creating the GPU profiler and before the first instrumented call. A
`profiling::scope!` with the Tracy backend panics if no client is running, and
`new_with_tracy_client` returns `CreationError::TracyClientNotRunning`.

```rust
pub fn run() -> Result<(), RunError> {
    #[cfg(feature = "tracy")]
    let _tracy_client = tracy_client::Client::start();

    // ... existing setup ...
}
```

Keep the returned `Client` alive for the run. The `ondemand` feature still needs the client
started; it only defers data collection until a GUI connects.

## Request timestamp queries

`wgpu-profiler` needs `wgpu::Features::TIMESTAMP_QUERY` for its initial GPU-to-CPU timestamp
readback. Two further features control which scopes produce timings:

| Feature | Enables |
|---|---|
| `TIMESTAMP_QUERY` | Pass-level scopes created with `scoped_render_pass` or `scoped_compute_pass`. Required by `new_with_tracy_client`. |
| `TIMESTAMP_QUERY_INSIDE_ENCODERS` | Scopes opened on a `CommandEncoder`, such as the outer frame scope. |
| `TIMESTAMP_QUERY_INSIDE_PASSES` | Scopes opened at arbitrary points inside a pass. |

Request only the intersection with the adapter's features. Requesting an unsupported feature
fails device creation.

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

#[cfg(not(feature = "tracy"))]
fn timer_features(_adapter: &wgpu::Adapter) -> wgpu::Features {
    wgpu::Features::empty()
}
```

Pass the result to `wgpu::DeviceDescriptor::required_features`; keep the rest of the current
descriptor unchanged. If `TIMESTAMP_QUERY` is missing, skip the GPU profiler and keep CPU
spans. Do not create a profiler that cannot report valid GPU data.

## Create the GPU profiler

Create the profiler after the device and queue exist. The creation call blocks once to read the
initial GPU timestamp for Tracy's GPU context, so it belongs in startup, not in the frame loop.

```rust
let supports_gpu_timers =
    required_features.contains(wgpu::Features::TIMESTAMP_QUERY);

let profiler = if supports_gpu_timers {
    match wgpu_profiler::GpuProfiler::new_with_tracy_client(
        wgpu_profiler::GpuProfilerSettings::default(),
        adapter.get_info().backend,
        &device,
        &queue,
    ) {
        Ok(profiler) => Some(profiler),
        Err(error) => {
            tracing::warn!(%error, "GPU profiling disabled");
            None
        }
    }
} else {
    tracing::warn!("no timestamp-query support; profiling the CPU only");
    None
};
```

Store the `Option<GpuProfiler>` on the renderer. The default settings enable timer queries and
debug groups and keep three profiler frames in flight. Use the defaults until there is a reason
to change them.

## Wire a frame

Order matters:

1. Open the frame scope.
2. Open GPU scopes and record passes.
3. Call `resolve_queries` after the last scope and before `encoder.finish()`.
4. Submit and present.
5. Mark the CPU frame with `profiling::finish_frame!`.
6. Call `end_frame`.
7. Call `process_finished_frame` to drain the oldest completed GPU frame.

```rust
profiling::scope!("frame");

if let Some(profiler) = profiler.as_ref() {
    let mut scope = profiler.scope("world", &mut encoder);
    let _pass = scope.scope("terrain");
}

if let Some(profiler) = profiler.as_mut() {
    profiler.resolve_queries(&mut encoder);
}

queue.submit(Some(encoder.finish()));
// present ...

profiling::finish_frame!();

if let Some(profiler) = profiler.as_mut() {
    profiler.end_frame().expect("profiler frame was not resolved");
    let _finished = profiler.process_finished_frame(queue.get_timestamp_period());
}
```

`end_frame` returns an error if a scope is still open or if queries were not resolved. Fix the
ordering; do not ignore the error. GPU results lag the CPU by a few frames because the readback
must complete, so the GPU row will not match the current CPU frame instantly. Call
`process_finished_frame` every frame even if the result is unused; otherwise the pending-frame
cap fills and newer frames are dropped.

## Add CPU scopes

Instrument stable boundaries, not inner loops. Use literal names where possible; the literal
form of `profiling::scope!` uses a static span location, while a runtime string allocates a
span location.

| Scope | Thread | Covers |
|---|---|---|
| `frame` | main | Redraw handler through present |
| `stream` | main | Streaming update and commit |
| `chunk generate` | worker | One generation job |
| `chunk light` | worker | One light propagation job |
| `chunk mesh` | worker | One meshing job |
| `mesh upload` | main | Buffer upload for one chunk batch |
| `encode` | main | Command encoding |
| `pass: opaque`, `pass: water` | main | One render pass |
| `present` | main | Present call |

Rules:

- Do not put spans inside per-voxel or per-block loops. The observer effect and the timeline
  noise are both worse than the detail is worth.
- Nest scopes instead of repeating a name at the same level.
- Keep the per-frame span count bounded. A frame with hundreds of peer spans is unreadable.
- Name worker threads with `profiling::register_thread!("chunk worker")` at worker startup so
  their lanes are labelled.

For GPU scopes, create the pass through the profiler scope so it owns the pass:

```rust
let mut scope = profiler.scope("world", &mut encoder);
let mut pass = scope.scoped_render_pass(
    "opaque",
    wgpu::RenderPassDescriptor {
        label: Some("opaque"),
        color_attachments: &[Some(color_attachment)],
        depth_stencil_attachment: Some(depth_attachment),
        ..Default::default()
    },
);
// set pipeline, bind groups, buffers, draw ...
```

The pass and scope close when dropped. `scope` borrows the encoder, so drop it before
`resolve_queries`.

## Add plots

`profiling` 1.0.18 has spans, thread names, and frame marks but no plot macro. Use
`tracy_client` directly:

```rust
if let Some(client) = tracy_client::Client::running() {
    client.plot(tracy_client::plot_name!("frame_ms"), frame_ms);
}
```

Plot frame time, queued chunks, visible chunks, vertices uploaded per frame, and draw calls.
Check `Client::running()` instead of unwrapping so a missing client disables the plot rather
than crashing the frame. Do not add a plot per chunk.

## Run Tracy

```console
cargo run --features tracy
cargo run --release --features tracy
```

Start the GUI before or after the engine; with `ondemand`, collection begins when it connects.
The GUI lists the client automatically while broadcast is on. Save a trace from the GUI if you
need to compare runs; keep in mind that it contains source paths and symbols.

Use a development build for source-level work. The repository's dev profile optimizes the
engine at `opt-level = 1` and dependencies at `3`, so the profiled code is not a debug build in
the usual sense. The release profile has thin LTO and line tables only; use it for realistic
frame times, not for detailed source attribution.

To compare backends, force one with `WGPU_BACKEND=dx12` or `WGPU_BACKEND=vulkan`. GPU timestamp
support and timer resolution can differ between them.

## Read the timeline

- The `frame` scope and the frame marks delimit frames. A frame without the scope is a redraw
  path that skipped instrumentation.
- CPU lanes show one row per thread. Worker lanes should show generation, lighting, and meshing
  separately; a worker that is idle while the queue is full points at a lock or a scheduling
  problem.
- The GPU context row is named `wgpu`. Its child zones are the pass scopes. Compare the CPU
  `frame` duration with the GPU row: CPU-bound frames have a short GPU row and long CPU spans;
  GPU-bound frames have the opposite.
- Plots sit next to the timeline. A frame-time spike that tracks queued chunks is a streaming
  problem, not a rendering problem.
- The statistics view gives mean, median, and maximum per zone. The maximum is usually the
  hitch that matters; the mean hides it.

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| GUI lists no client | Client not started, feature off, GUI protocol mismatch, or firewall | Start the client before setup, build with `--features tracy`, use GUI 0.13.1, allow loopback traffic |
| `TracyClientNotRunning` at startup | `new_with_tracy_client` ran before `Client::start` | Move `Client::start` to the top of `run` |
| No GPU row | Adapter lacks `TIMESTAMP_QUERY`, or the feature was not requested | Log adapter features, request the intersection, fall back to CPU only |
| GPU row exists but no pass scopes | `TIMESTAMP_QUERY_INSIDE_PASSES` is missing | Request it when the adapter supports it |
| GPU row exists but the encoder scope is missing | `TIMESTAMP_QUERY_INSIDE_ENCODERS` is missing | Request it when the adapter supports it |
| `end_frame` reports `UnclosedQueries` | A `Scope` outlived the encoder | Drop scopes before `resolve_queries` |
| `end_frame` reports `UnresolvedQueries` | `resolve_queries` was skipped or called after finish | Call it after the last scope, before `encoder.finish()` |
| GPU results never appear | `process_finished_frame` is not called every frame | Drain one finished frame per frame |
| Frame times are worse than normal | Instrumentation overhead | Reduce spans and plots, then compare with the feature off |
| C++ build or link failure | No C++ compiler or wrong toolchain | Install the C++ workload or `build-essential`; check `cl.exe`, `g++`, or `clang++` |
| Two `tracy-client` versions | `tracing-tracy` or another crate pulled 0.19 | Remove it; run `cargo tree -i tracy-client` |
| `tracy-client 0.18.5` fails to compile | Incompatible with `tracy-client-sys` 0.28.0 | Pin `tracy-client = "=0.18.4"` and `tracy-client-sys = "=0.28.0"` |
| Symbols show as addresses | No debug info, or code transfer is disabled | Build with debug info; do not disable the client's `code-transfer` feature |

## CI and releases

- Do not add `tracy` to default features and do not enable it for release artifacts.
- Do not add a profiling build to CI. If the feature exists, `--all-features` will build it; see
  [CI](ci.md#ci-workflow) for the current commands.
- Keep the Tracy GUI, client, and sys crate upgrades together. A GUI that cannot connect after
  an upgrade is usually a protocol mismatch, not an engine bug.
- Profiling builds are local developer builds. A missing C++ toolchain is a local setup problem,
  not a reason to gate CI on the feature.

## References

- [ADR-0006: Tracy for profiling](../decisions/0006-tracy-for-profiling.md)
- [Debugging](debugging.md)
- [CI](ci.md)
- [Setup](setup.md)
- [Testing](testing.md)

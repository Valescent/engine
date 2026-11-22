# Why I Chose Tracy for Profiling

**Document type:** Architecture Decision Record / Devlog Entry  
**Status:** Accepted  
**Date:** 2026-11-13  
**Project:** Valescent Engine  
**Owner:** Project Owner  
**Supersedes:** None  
**Scope:** The profiler used to measure CPU and GPU time during development. Frame-graph tooling (RenderDoc, Nsight, PIX), microbenchmarking, and production telemetry are out of scope and may use other tools.

**Amendment (2026-11-19):** `tracy` is now a default Cargo feature so debug builds profile without
a flag. Automatic capture and the profiled allocator are gated on `debug_assertions`, traces are
written to the git-ignored `reports/tracy/`, and every CI cargo command passes
`--no-default-features`. Statements below describing profiling as opt-in apply to release and CI
builds.

---

## Summary

This document records why the engine is profiled with Tracy. It is both an official decision record and a devlog entry: the decision is final for the current scope, and I am publishing the reasoning so future decisions can be traced back to it.

The short version: a voxel engine's performance problems are frame-shaped and intermittent — a chunk rebuild hitches, a frame is CPU-bound in meshing instead of GPU-bound, a worker thread stalls on a lock. Tracy shows a live, per-frame, per-thread timeline with exact instrumented durations, and `wgpu-profiler`'s Tracy integration puts GPU timer queries on the same timeline. The tradeoff is that Tracy is instrumentation-based, so it only shows what I instrument, it adds a C++ build dependency to profiling builds, and its client/server versions must be kept in sync.

---

## 1. Context

### 1.1 The project

Performance work in this engine has specific characteristics:

- **Frame-shaped problems.** The unit of interest is a frame: 16.6 ms at 60 Hz, with a budget split between meshing, upload, and rendering. Averages hide the hitches that matter.
- **Intermittent stalls.** Breaking a block rebuilds one chunk; if that rebuild lands on the main thread, a single spike is what breaks the experience. Statistical sampling over a long run smooths exactly that spike away.
- **Two costs to separate.** Is a frame slow because the CPU is meshing and uploading, or because the GPU is fill-rate or draw-call bound? Only a tool that shows both timelines answers that.
- **Worker threads.** Chunk generation and meshing move to worker threads in roadmap v0.0.5. Stalls caused by contention or queueing do not appear on a single main-thread profile.
- **Invisible abstractions.** The wgpu decision (ADR-0002) promises to measure draw-call and abstraction overhead rather than assume it. That requires instrumentation at the encoder and pass level, not just a frame counter.
- **Fast iteration.** I need to profile the build I am currently running, without recompiling with special flags, restarting, or replaying a capture.

These requirements point toward an instrumentation profiler with a live timeline, per-thread lanes, and a GPU path that works with wgpu — not a sampling profiler and not a post-hoc trace writer.

### 1.2 The constraints

My choice is constrained by the following:

- It must attach to a running engine and show a live frame timeline without a capture/replay cycle.
- It must correlate CPU work with GPU work on the same timeline.
- It must show per-thread activity, since meshing and generation are parallel.
- It must cost nothing in builds where profiling is disabled.
- It must be usable from Rust without writing C++ instrumentation by hand.
- It must be free and usable offline, with no per-seat license or hosted service.

---

## 2. Decision

**Tracy is the profiler for the engine, instrumented through `tracy-client`, with `wgpu-profiler` reporting GPU timer queries into the same timeline.**

This covers CPU spans around frame phases, chunk generation, meshing, and buffer uploads; plots for frame time and queue depth; and GPU scopes around render passes. Instrumentation is compiled in only behind an opt-in feature, so normal and release builds contain no client code. Frame debuggers (RenderDoc, Nsight, PIX) remain acceptable and complementary tools for pipeline and shader inspection; they are not the profiler.

See the [profiling guide](../dev/profiling.md) for the intended dependency set, engine wiring, and the current gaps against this decision.

I accept this decision for the current scope. I will revisit it only if the assumptions in this document stop holding.

---

## 3. Why I Chose Tracy

### 3.1 A live frame timeline instead of a post-hoc report

Tracy connects to the running process and streams profiling data continuously. I can fly around the world, break blocks, and watch the frame timeline in real time, then pause and inspect the frame that felt bad. Sampling profilers and trace writers require me to reproduce a stall inside a capture window and analyze it afterward; Tracy lets me profile the session I am actually playing.

### 3.2 Exact durations for the work I care about

Tracy is instrumentation-based, so a zone reports the actual time spent in that scope, not a statistical estimate. For meshing and upload code, that is the right model: I want to know that one chunk's meshing took 1.8 ms, not that "meshing-related frames" appeared in 4% of samples. It also makes regressions unambiguous — the same scope either got slower or it did not.

### 3.3 CPU and GPU on one timeline

With the `tracy` feature enabled, `wgpu-profiler` issues wgpu timestamp queries around passes and reports them into the same view as the CPU spans. That answers the first question of every slow frame: is the CPU still meshing when the GPU is idle, or is the GPU saturated while the CPU waits? This directly serves ADR-0002, where the abstraction layer's cost was accepted on the condition that it would be measured rather than assumed.

### 3.4 Per-thread lanes and contention visibility

Tracy gives each thread its own lane and can trace locks and context switches. When chunk meshing moves to worker threads, a frame stall caused by a worker waiting on a lock or by a queue handing off work shows up as a visible gap with an explanation. A main-thread-only profiler would show a stall with no cause attached.

### 3.5 Plots for engine state, not just time

Tracy's plots take arbitrary numeric series over time: frame time, chunks in the meshing queue, vertices uploaded per frame, draw calls per frame. Plotting engine state next to the timeline turns "performance is inconsistent" into "frame time tracks queued chunks," which is a diagnosis rather than a complaint.

### 3.6 Safe Rust bindings, no C++ instrumentation

`tracy-client` is a safe wrapper over Tracy's C++ client: `span!` scopes, frame marks, plots, thread names, and an allocator wrapper are all available as Rust APIs. I do not hand-write Tracy macros into C++, and span locations are captured statically, so instrumentation does not allocate per call. If the engine later adopts `tracing`, `tracing-tracy` can bridge spans into the same timeline.

### 3.7 Zero cost when disabled

The client is gated behind the crate's `enable` feature. A build without it compiles the instrumentation call sites into no-ops and links no client library, so shipped and everyday builds carry no runtime overhead and no network surface. `ondemand` mode additionally starts collecting only when a profiler connects, which keeps the cost off during normal play.

### 3.8 Allocation tracking for the upload path

`ProfiledAllocator` reports allocations to Tracy. For an engine that rebuilds chunk meshes constantly, seeing allocation rate and transient spikes next to frame time is a direct way to catch per-frame allocations that should have been reused buffers.

### 3.9 Why not puffin, superluminal, or a sampling profiler?

- **puffin** is the strongest Rust-native alternative: light, pure Rust, and it also integrates with `wgpu-profiler`. It is narrower, though — its viewer is embedded UI, its analysis tools are simpler, and it does less for long-session, cross-thread, and memory investigation. I want the deeper analysis without embedding a viewer.
- **Superluminal** is an excellent Windows profiler, but it is commercial and Windows-only, which conflicts with a cross-platform engine.
- **`perf`, VTune, and Instruments** are platform-specific, need elevated setup, and know nothing about frames or GPU passes.
- **Sampling profilers and flamegraph tools** give good steady-state CPU pictures but specifically erase the short, intermittent spikes this engine needs to eliminate.
- **Chrome trace writers** produce offline JSON after the fact with no live view and no GPU correlation out of the box.

---

## 4. Alternatives Considered

| Alternative | Assessment | Outcome |
|---|---|---|
| **Tracy** | Live timeline, exact instrumented durations, per-thread lanes, lock and allocation tracing, GPU zones via `wgpu-profiler`, safe Rust bindings, zero cost when disabled. | **Selected** |
| **puffin** | Pure Rust, lightweight, embedded viewer, `wgpu-profiler` support; simpler analysis, less suited to long sessions and memory work. | Not selected; viable fallback |
| **Superluminal** | Strong Windows profiler; commercial and single-platform. | Not selected |
| **`perf` / VTune / Instruments** | Powerful system-level sampling; platform-specific, heavy setup, no frame or GPU-pass semantics. | Not selected |
| **Sampling / flamegraph tools** | Good for steady-state CPU cost; averages away short hitches, which are the primary problem here. | Not selected as the primary tool |
| **Chrome trace / `tracing-chrome`** | Free and simple; post-hoc JSON, no live view, weak GPU correlation. | Not selected |
| **RenderDoc / Nsight / PIX** | Best tools for pipeline and shader inspection; frame-capture debuggers, not continuous profilers. | Retained as complementary tools |

---

## 5. Counterarguments and Disposition

This decision has real weaknesses. I am recording them here rather than omitting them, so the record stays honest.

| Counterargument | Disposition |
|---|---|
| **Instrumentation only shows what I instrument.** Uninstrumented hot code is invisible, unlike in a sampling profiler. | Accepted. I will instrument at stable boundaries — frame, chunk generation, meshing, upload, encoder, pass — and treat missing spans as a gap to fix rather than a finding. A sampling profiler remains a complement for hunting unknown hot spots. |
| **It is an observer effect.** Instrumentation and the client's buffering cost time, so the measured build is not the shipped build. | Accepted. I will keep span counts per frame modest, measure with profiling disabled for baseline numbers, and treat Tracy's absolute frame times as slightly pessimistic. |
| **Rust bindings lag the C++ client, and the GUI must match the client protocol.** A version mismatch can simply fail to connect. | Accepted. I will pin `tracy-client`, `tracing-tracy`, and `wgpu-profiler` together, keep a known-good Tracy GUI build documented, and upgrade them as a set. |
| **`wgpu-profiler` pins an older `tracy-client` than the latest release.** Two versions in the tree is possible and confusing. | Accepted. Version alignment is part of the upgrade procedure, not a per-crate decision. The dependency set is upgraded together and tested by connecting the GUI. |
| **GPU timing needs timestamp query support.** Some backends and adapters do not provide it, or provide it coarsely. | Accepted. I will check the adapter's features at startup and degrade to CPU-only profiling with a clear message rather than reporting bogus GPU spans. |
| **Tracy broadcasts on the local network by default, and traces can include source and machine code.** | Accepted. Profiling builds will enable `only-localhost` and disable broadcast; the client feature stays off by default; I will not profile on untrusted networks. |
| **It compiles Tracy's C++ client.** That adds a C++ toolchain requirement and cold-build time to profiling builds. | Accepted. It is behind an opt-in feature, so ordinary builds never pay it. Build failures on CI are a reason to keep profiling builds local. |
| **It is a development tool, not observability.** No aggregation, no dashboards, no history across users. | Accepted. That is the intended scope. Production metrics would be a separate decision with separate tooling. |

---

## 6. Consequences

### 6.1 Positive

- Slow frames can be diagnosed live, without capture/replay cycles.
- CPU and GPU cost appear side by side, separating meshing cost from rendering cost.
- Worker-thread stalls and lock contention become visible.
- Engine state plots give context to timing, turning symptoms into causes.
- Instrumentation is safe Rust with statically captured locations.
- Disabled builds carry no client, no overhead, and no network surface.
- The tool is free, cross-platform, and works offline.

### 6.2 Negative

- Uninstrumented code is invisible, so profiling quality depends on span hygiene.
- Profiling builds need a C++ toolchain and a matching Tracy GUI.
- The client, bridge, and GPU profiler crates must be version-aligned.
- GPU spans are unavailable on adapters without timestamp query support.
- The observer effect makes absolute timings slightly pessimistic.
- A networked profiler is a data-exposure surface if misconfigured.

### 6.3 Neutral

- Frame debuggers remain the tool for shader and pipeline inspection; Tracy does not replace them.
- `tracing-tracy` is available if the engine adopts `tracing` for logging, but it is not required.
- Microbenchmarks remain a separate concern from frame profiling.

---

## 7. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| Version skew between client, bridge, GPU profiler, and GUI prevents connection. | Pin the profiling dependency set; document the matching GUI version; upgrade and verify the connection as one step. |
| Missing spans leave a hot path unexplained. | Instrument stable engine boundaries first; review instrumentation when a frame report has an unexplained gap; keep a sampling profiler available for discovery. |
| Instrumentation overhead distorts the measurement. | Keep per-frame span counts bounded; compare against a profiling-disabled baseline; avoid spans inside per-voxel loops. |
| GPU timestamps unavailable on a target. | Query adapter features at startup; fall back to CPU-only with a warning; do not report invalid GPU data. |
| The profiler is accidentally enabled in a release build. | Cargo has no profile-conditional features, so `tracy` is on for release too; capture and the profiled allocator are gated on `debug_assertions`, and CI/release builds pass `--no-default-features`. |
| Tracing data leaks onto the network or the machine. | Enable `only-localhost`, disable broadcast, gate automatic capture on debug builds, write traces only to the git-ignored `reports/`, and profile only on trusted networks. |
| Profiling builds break because of the C++ toolchain. | Treat profiling as a local developer build; do not gate CI on it; document the toolchain requirement. |
| Too many zones make the timeline unreadable. | Use a consistent, small set of named scopes; nest rather than repeat; keep plot count deliberate. |

---

## 8. Conclusion

Tracy is my deliberate, measurement-driven, frame-aware choice for profiling the engine. I am selecting it because frame hitches and CPU/GPU attribution are the actual performance questions in a voxel engine, because a live per-frame timeline with per-thread lanes answers them, and because `wgpu-profiler` puts GPU timer queries on the same view as the CPU spans — which is exactly the evidence ADR-0002 committed to produce. I acknowledge that it is instrumentation-based, so it sees only what I instrument, that profiling builds require a C++ toolchain and a version-matched GUI, and that its Rust integration must be upgraded as a set. I manage those through deliberate instrumentation boundaries, pinned versions, timestamp-query fallbacks, local-only networking, and a feature gate that keeps the client out of every other build.

---

*This entry is part of the Valescent Engine decision log. Future entries should reference this document when they depend on the profiling setup or revise its assumptions.*

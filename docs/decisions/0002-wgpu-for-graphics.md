# Why I Chose wgpu as the Graphics API

**Document type:** Architecture Decision Record / Devlog Entry  
**Status:** Accepted  
**Date:** 2026-11-13  
**Project:** Valescent Engine  
**Owner:** Project Owner  
**Supersedes:** None  
**Scope:** The GPU abstraction layer the engine renders through. Window creation, input, the math library, and asset formats are out of scope and decided separately.

---

## Summary

This document records why I am building the engine's renderer on wgpu. It is both an official decision record and a devlog entry: the decision is final for the current scope, and I am publishing the reasoning so future decisions can be traced back to it.

The short version: I already committed to Rust, and wgpu is the Rust-native graphics API that gives me Vulkan-class capability on Windows, macOS, and the web from one codebase without writing unsafe driver code. The tradeoff is that wgpu is an abstraction, so it hides vendor-specific features and its API still changes between releases. I accept that because a voxel engine needs correct, portable buffer and pipeline management far more than it needs cutting-edge GPU extensions.

---

## 1. Context

### 1.1 The project

The engine is a voxel renderer, and voxel rendering stresses a graphics API in specific ways:

- **Constant buffer churn.** Chunk meshes are rebuilt and re-uploaded whenever terrain changes. Buffer creation, resizing, and lifetime management are hot paths, not one-time setup.
- **Many small-to-medium draw calls.** One draw per chunk, hundreds to thousands per frame. Per-draw CPU overhead matters as much as per-pixel cost.
- **Instanced or batched geometry.** Greedy meshing and LOD produce variable vertex counts that must be uploaded dynamically.
- **Texture atlases and bind groups.** Block textures live in a single atlas, so bind group layout is stable and shared.
- **GPU compute is optional but plausible.** The stretch goals include SVDAG compression and GPU raymarching, which need compute support.
- **Frequent shader iteration.** Lighting, fog, water, and ambient occlusion all arrive as shader work in later milestones (roadmap v0.0.5).
- **Portability is uncertain.** I want a native build now and a browser build is plausible later; I do not want to rewrite the renderer to get it.

These requirements favor an API with explicit resource management, multi-backend support, and strong validation, rather than the lowest-level platform API on a single OS.

### 1.2 The constraints

My API choice is constrained by the following:

- It must be usable from Rust without a large unsafe FFI surface or a C++ shim.
- It must support more than one platform without a separate renderer per OS.
- It must give explicit control over buffers, textures, pipelines, and shader resources.
- It must fail loudly during development instead of crashing the GPU or producing silent corruption.
- It must not lock me into a full engine whose rendering architecture I cannot control.

---

## 2. Decision

**wgpu is the graphics API for the engine renderer, with WGSL as the shader language.**

This covers device and adapter setup, buffers, textures, samplers, pipelines, bind groups, and compute work. Window creation and input remain the responsibility of a separate windowing library. Raw platform APIs remain acceptable as a debugging reference or as an escape hatch; I am not selecting them as the implementation. I am not building the renderer on OpenGL, on a C++ engine, or inside Bevy's renderer.

I accept this decision for the current scope. I will revisit it only if the assumptions in this document stop holding.

---

## 3. Why I Chose wgpu

### 3.1 One API across backends and platforms

wgpu targets Vulkan, Metal, DirectX 12, and OpenGL, and compiles to WebGPU in the browser. On Windows it can run on either Vulkan or DX12 without changing engine code. That matters because I do not want to maintain a Vulkan path, a Metal path, and a DX12 path in a solo project. The roadmap's later milestones (greedy meshing, LOD, compute raymarching) are architectural work I want to spend time on, not platform plumbing.

### 3.2 Rust-native, safety-checked resource management

wgpu is written in Rust and exposes Rust types. It tracks buffer and texture lifetimes internally and validates usage against the WebGPU rules, so using a destroyed buffer or a mismatched bind group is reported as a validation error rather than a driver crash or silent corruption. For a renderer that re-uploads chunk meshes constantly, that class of bug is exactly what I need the API to catch for me.

### 3.3 Consistency with the Rust decision

I chose Rust for the engine core to get strict typing, compile-time safety, and good tooling. wgpu keeps that property at the GPU boundary instead of dropping into unsafe C bindings. The surrounding ecosystem fits the same mold: `wgpu` for rendering, `winit` for windows, `bytemuck` for vertex layouts, and `naga` for shader validation and translation. One language, one build system, one type system, one debugger.

### 3.4 Explicit control without raw driver code

wgpu is not a scene graph or a renderer. It exposes queues, command encoders, render passes, pipelines, and bind groups directly. That means I decide the draw order, the buffer layout, the atlas strategy, and the chunk upload path. I get explicit control over the parts of rendering that actually affect a voxel engine's performance, without hand-writing Vulkan boilerplate or platform-specific descriptor setup.

### 3.5 WebGPU alignment and a plausible browser target

wgpu is the reference Rust implementation of the WebGPU standard. Designing against WebGPU limits pushes me toward conservative, portable resource usage: explicit buffer sizes, explicit bind group layouts, no reliance on driver-side magic. If the engine ever ships to the browser, the renderer already speaks the right language. If it never does, the constraints still make the native renderer more predictable.

### 3.6 Debuggability during development

Validation errors, `wgpu`'s error scopes, and `naga`'s shader validation give me readable failures during development. When a pipeline is wrong, I get a message about the pipeline, not a black screen with no explanation. That is worth a measurable amount of iteration speed in a project where I am learning the API as I go.

### 3.7 Avoidance of heavier alternatives

I am deliberately not adopting a full engine (Bevy, Fyrox) as the renderer, because their rendering architectures are opinions I would inherit and fight later. I am also not adopting the raw platform APIs as the implementation, because the boilerplate and unsafe surface scale badly for one developer. wgpu sits at the level I want: below an engine, above a driver.

### 3.8 Why not Vulkan, OpenGL, or Bevy?

- **Vulkan** is the most capable and the most verbose. It would give me access to everything, at the cost of thousands of lines of unsafe setup, per-platform branching, and manual lifetime management before I render a single cube. That cost is not justified at this stage.
- **OpenGL** is simpler, but it is legacy, deprecated on macOS, inconsistent across drivers, and a poor fit for compute-heavy or modern pipeline work. It would also make a browser path harder, not easier.
- **Bevy** would give me a working renderer immediately, but I would be building on top of its ECS-driven rendering model rather than understanding and owning my own pipeline. The roadmap is explicitly about building the engine, so adopting another engine's renderer defeats the purpose.

---

## 4. Alternatives Considered

| Alternative | Assessment | Outcome |
|---|---|---|
| **wgpu** | Rust-native, multi-backend, explicit resource control, validation, WebGPU-aligned, usable for compute. | **Selected** |
| **Vulkan (raw)** | Maximum control and feature access; extreme boilerplate, unsafe, platform-specific, slow to first frame. | Not selected; retained as a conceptual reference and possible escape hatch via `wgpu-hal` |
| **Metal / DirectX 12 (raw)** | Best per-platform performance and features; requires separate implementations and platform-specific code. | Not selected |
| **OpenGL / GLES** | Widely supported and simple; legacy design, deprecated on macOS, driver-dependent behavior, weak compute story. | Not selected |
| **`ash` / `vulkano`** | Rust bindings to Vulkan; still Vulkan-shaped, unsafe or heavyweight, single-backend. | Not selected |
| **Bevy / Fyrox renderer** | Working renderer out of the box; engine lock-in and an architecture I do not own. | Not selected |

---

## 5. Counterarguments and Disposition

This decision has real weaknesses. I am recording them here rather than omitting them, so the record stays honest.

| Counterargument | Disposition |
|---|---|
| **wgpu is an abstraction, so it hides vendor-specific features.** Mesh shaders, bindless resources, and some ray-tracing extensions are not reachable through the portable API. | Accepted. My rendering plan (chunk meshes, atlas textures, simple lighting, optional compute raymarching) fits inside WebGPU's feature set. If a genuinely necessary feature is missing, `wgpu-hal` or a targeted raw backend is the escape hatch, and that would be a new decision. |
| **wgpu's API still changes between releases.** Breaking changes across versions are common, and upgrading is real work. | Accepted. I will pin versions, upgrade deliberately, and keep wgpu usage concentrated in a small renderer module so upgrades touch one layer instead of the whole engine. |
| **It is not the fastest possible path to pixels.** Validation, abstraction, and bind group management cost something compared with a hand-tuned Vulkan renderer. | Accepted with qualification. For voxel workloads the bottleneck is mesh size, draw call count, and CPU-side meshing, not the abstraction layer. I will measure frame time and draw call overhead rather than assume. |
| **Documentation and examples are thinner than OpenGL's or Vulkan's.** There are fewer tutorials and fewer Stack Overflow answers. | Accepted. The `wgpu` examples repository and the WebGPU specification are the primary sources, and the WebGPU spec is precise. This is a real learning cost I am choosing to pay. |
| **WGSL is a younger shader language.** GLSL and HLSL have more tooling and more existing material. | Accepted with qualification. WGSL is small, well-specified, and validated by `naga`. The shaders this engine needs are simple enough that language maturity is not a blocker. |
| **wgpu is not a renderer.** I still have to design the mesh upload path, atlas, pipeline layout, and draw ordering myself. | Accepted. That is the point: I want to own those decisions. The cost is that v0.0.1 takes longer than it would in Bevy. |
| **Backend differences are real.** The OpenGL backend supports less, and device limits vary across adapters. | Accepted. I will target a conservative feature set, query limits at startup, and test on more than one backend on Windows. |

---

## 6. Consequences

### 6.1 Positive

- One renderer implementation covers Windows, macOS, Linux, and potentially the browser.
- Rust type safety and wgpu validation catch resource and pipeline mistakes early.
- The whole engine stays in one language and one build system.
- Explicit control over buffers and pipelines supports chunk-focused optimizations directly.
- Compute support is available for the roadmap's later rendering experiments.
- The engine is not locked into another engine's rendering architecture.

### 6.2 Negative

- Some GPU-specific features and extensions are unreachable through the portable API.
- wgpu version upgrades will require deliberate migration work.
- Learning wgpu and WGSL is a real cost on top of learning Rust and voxel rendering.
- Fewer tutorials and community answers than the older graphics APIs have.
- Backend-specific bugs and limit differences still have to be handled.
- First-cube time is slower than adopting an existing engine.

### 6.3 Neutral

- Window creation is delegated to `winit`, so the windowing decision stays separate from the graphics decision.
- Shaders are written in WGSL; `naga` can translate if a different shader source is ever needed.
- The renderer stays behind a small engine-side module boundary, so the rest of the engine does not depend on wgpu types directly.

---

## 7. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| wgpu API churn breaks the renderer on upgrade. | Pin the wgpu version; upgrade on purpose between milestones; keep all wgpu usage inside the renderer module; read the changelog before upgrading. |
| A needed rendering technique is not expressible in wgpu. | Prototype the technique early; if it fails, evaluate `wgpu-hal` or a targeted backend as a separate decision rather than silently bending the design. |
| Performance is capped by the abstraction layer. | Measure draw call overhead and frame time with real chunk counts; optimize mesh size and batching first, since those dominate in voxel rendering. |
| Validation errors are noisy and slow iteration. | Use error scopes around pipeline creation; keep shader logic small and incremental; rely on `naga` errors during shader work. |
| Backend differences cause behavior that only appears on one platform. | Query adapter limits and features at startup; fail with a clear message; test the Vulkan and DX12 paths on Windows. |
| WebGPU/`wgpu` documentation is insufficient when stuck. | Read the wgpu examples repository and the WebGPU specification first; keep a small reproduction case and test it against the examples. |
| Scope creep into building a general renderer. | Keep the renderer scoped to what the roadmap needs; defer post-processing, shadows, and advanced features until their milestone. |

---

## 8. Conclusion

wgpu is my deliberate, requirement-driven, portability-aware choice for the engine's graphics API. I am selecting it because I chose Rust, I need one renderer across multiple backends, and I want explicit control over resources with validation instead of unsafe driver code. I acknowledge that wgpu is an abstraction that hides vendor-specific features and that its API still changes between releases. I manage that through pinned versions, a narrow renderer boundary, conservative feature use, and the escape hatches recorded above. I therefore accept this decision as the graphics API for the engine, with raw platform APIs retained only as a reference and emergency escape hatch, and with its limits recorded above.
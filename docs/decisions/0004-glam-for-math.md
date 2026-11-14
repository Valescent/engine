# Why I Chose glam for Math

**Document type:** Architecture Decision Record / Devlog Entry  
**Status:** Accepted  
**Date:** 2026-11-13  
**Project:** Valescent Engine  
**Owner:** Project Owner  
**Supersedes:** None  
**Scope:** Vector, matrix, and quaternion math for the engine. Serialization of GPU data and vertex layout (`bytemuck`) is a separate decision; physics, collision queries, and geometry algorithms are out of scope.

---

## Summary

This document records why the engine uses glam for linear algebra. It is both an official decision record and a devlog entry: the decision is final for the current scope, and I am publishing the reasoning so future decisions can be traced back to it.

The short version: glam is a game-oriented math crate with the exact types a renderer needs (`Vec3`, `Mat4`, `Quat`, `IVec3`), SIMD acceleration without allocation, and conventions that line up with wgpu and WGSL. It also exposes `bytemuck` support, so matrices can go into a uniform buffer without conversion code. The tradeoff is that glam is `f32`-first, so very large world coordinates will need camera-relative rendering, and it is not a geometry library — AABB tests and voxel raycasts are mine to write.

---

## 1. Context

### 1.1 The project

Math sits on both the hot path and the correctness path of a voxel engine:

- **Per-voxel transforms during meshing.** Greedy meshing walks millions of voxels and emits quads. Vector math in that loop has to be branch-light, allocation-free, and inlined.
- **Camera and projection.** A first-person camera needs view matrices, perspective projection matching the renderer's clip-space conventions, and quaternion or Euler look rotation.
- **Integer voxel coordinates.** Voxel positions are integers (`IVec3` is already the natural key for the roadmap's `HashSet<IVec3>` storage), and they must convert cleanly into world-space floats for meshing.
- **GPU uniform layout.** Model, view, and projection matrices are uploaded every frame, so the memory layout must match WGSL without manual packing or padding bugs.
- **Large worlds.** Chunk coordinates grow with view distance, which eventually stresses `f32` precision.
- **Collision and picking.** The roadmap needs swept AABB collision against terrain and DDA voxel traversal for block selection.

These requirements favor a compact, game-oriented math crate with integer vectors, graphics conventions, and byte-level compatibility — not a general-purpose numerical linear algebra library.

### 1.2 The constraints

My math choice is constrained by the following:

- It must be fast enough for per-voxel use inside meshing loops, with no heap allocation.
- It must provide the graphics types directly: 3D vectors, 4×4 matrices, quaternions, and integer vectors.
- It must match wgpu and WGSL conventions for handedness, matrix layout, and clip-space depth.
- Its types must be trivially uploadable to GPU buffers, ideally with `bytemuck`.
- It must not drag in a huge generic type system that slows compiles or complicates engine code.

---

## 2. Decision

**glam is the math library for the engine, using its `f32` types as the default.**

This covers vectors, matrices, quaternions, integer coordinate types, and the transform helpers used by the camera and the mesher. `bytemuck` integration is enabled through glam's feature flag so math types can be written into GPU buffers directly. Collision, raycasting, and geometric intersection tests are engine code built on top of glam, not part of this decision.

I accept this decision for the current scope. I will revisit it only if the assumptions in this document stop holding.

---

## 3. Why I Chose glam

### 3.1 Types that match the problem

glam ships `Vec3`, `Vec4`, `Mat4`, `Quat`, `IVec3`, `UVec3`, and the rest of the set a renderer actually uses. `IVec3` matters directly: voxel coordinates are integers, the roadmap stores them in a `HashSet<IVec3>`, and chunk-local coordinates are naturally `UVec3`. Having those types in the same crate as the float math removes conversion boilerplate at the boundary between voxel data and rendering.

### 3.2 Conventions that line up with wgpu and WGSL

glam is column-major, right-handed, and provides `Mat4::perspective_rh` for a 0-to-1 depth range — the clip-space convention wgpu uses. That is not a detail I want to rediscover per project. Choosing a math library whose defaults match the renderer's conventions removes a whole category of bugs where the image is mirrored, depth is inverted, or the projection assumes OpenGL's −1-to-1 range.

### 3.3 SIMD performance without allocation

glam is built for speed: SIMD paths on x86_64 (SSE2 baseline, AVX2 with a feature), NEON on ARM, and `wasm32` SIMD when enabled. All of it is inline and allocation-free, which is what the meshing loop needs. Math in a voxel mesher is not a place where I want virtual dispatch, `Box`, or a trait-object abstraction layer.

### 3.4 Direct GPU upload through bytemuck

With glam's `bytemuck` feature, `Mat4`, `Vec3`, and `Quat` satisfy `Pod` and can be cast straight into a buffer for wgpu. No manual `[f32; 16]` conversion, no hand-rolled packing, no chance of transposing a matrix while copying it. Since the same crates handle both the CPU-side math and the GPU-side layout, the uniform struct in Rust and the `struct` in WGSL can be kept visibly identical.

### 3.5 Const-friendly and copy-friendly

Most glam constructors and operations are `const fn`, and the types are `Copy`. That means constants like unit axes and identity transforms are compile-time values, and passing math types around costs nothing. For an engine with many small transform computations per frame, this keeps the code direct.

### 3.6 Ecosystem alignment and documentation

glam is the math layer under Bevy and is widely used across the Rust graphics ecosystem, which means the answers, examples, and wgpu sample code I find are already written in glam types. It is small enough to read, documented thoroughly, and tested against its SIMD backends. That reduces both learning cost and the odds of hitting an obscure, unmaintained corner.

### 3.7 Honest about what it is not

glam does not include collision, raycasting, spatial partitioning, or geometry generation. It is a math crate, not a geometry engine. I am choosing that deliberately: the roadmap's AABB sweep and DDA voxel traversal are a few hundred lines of engine code that I want to own and tune, not a dependency I inherit and fight.

### 3.8 Why not nalgebra, cgmath, or ultraviolet?

- **nalgebra** is an excellent general-purpose linear algebra library, but it is generic over scalar and dimension, which means heavier compile times, more type noise, and machinery aimed at numerical and robotics workloads rather than a game loop. It also does less to match graphics conventions by default.
- **cgmath** was the older game-math standard, but it is effectively unmaintained compared with glam, has weaker SIMD coverage, and lacks the integer-vector story I need for voxels.
- **ultraviolet** is well designed and SoA-friendly, which suits ECS-style batch math, but that is not my access pattern. I compute per-object transforms in a straight loop, and ultraviolet's structure adds friction for that. It also has a smaller ecosystem.
- **euclid** is strong for 2D and typed-unit geometry (it came out of Servo), but it is not aimed at 3D game rendering.
- **Writing my own** would mean reimplementing tested SIMD paths and projection matrices for no benefit.

---

## 4. Alternatives Considered

| Alternative | Assessment | Outcome |
|---|---|---|
| **glam** | Game-focused, SIMD, allocation-free, `Mat4`/`Quat`/`IVec3`, wgpu-friendly conventions, `bytemuck` support, small API. | **Selected** |
| **nalgebra** | Powerful and general; generic scalar/dimension parameters, heavier compiles, less graphics-oriented. | Not selected |
| **cgmath** | Familiar API; effectively unmaintained, weaker SIMD, poor integer-vector support. | Not selected |
| **ultraviolet** | SoA-oriented and fast for batch math; mismatched with per-object usage, smaller ecosystem. | Not selected |
| **euclid** | Excellent typed 2D geometry; not a 3D rendering math library. | Not selected |
| **Hand-rolled math** | Total control; reimplements tested SIMD and projection code, adds maintenance burden. | Not selected |

---

## 5. Counterarguments and Disposition

This decision has real weaknesses. I am recording them here rather than omitting them, so the record stays honest.

| Counterargument | Disposition |
|---|---|
| **glam is `f32`-first, so large worlds lose precision.** At long view distances, chunk-relative coordinates become visibly jittery. | Accepted. This is real for a voxel engine with large worlds. I will use camera-relative rendering and chunk-local origins, and convert to world space only where needed. If that is insufficient, a dedicated `f64` world-position type is a follow-up decision, not a reason to change the math library now. |
| **SIMD abstraction hides what the code costs.** `Vec3A` versus `Vec3`, alignment, and feature-dependent paths can produce surprises. | Accepted with qualification. I will prefer plain `Vec3` in vertex data and only reach for `Vec3A` inside hot math loops where alignment is safe. I will benchmark before assuming SIMD is helping. |
| **Convention mismatches still happen.** Handedness, column-major layout, and the 0-to-1 depth range differ from OpenGL tutorials and from other crates. | Accepted. I will document the engine's conventions once and route all projection setup through a single camera module so the assumption lives in one place. |
| **glam's API has changed across 0.x releases.** Breaking changes between versions require migration work. | Accepted. I will pin the version and upgrade deliberately, keeping glam usage concentrated in engine math modules rather than spread through every file. |
| **It is not a geometry library.** No AABB tests, no ray intersections, no frustum extraction. | Accepted. Those are engine code by design. Frustum culling and DDA traversal are on the roadmap and will be written against glam types. |
| **`bytemuck` layout is not automatic.** A `Vec3A` in a vertex struct wastes four bytes per vertex and can break a tight layout. | Accepted. Vertex formats will use `Vec3` with explicit `#[repr(C)]` structs, and I will verify sizes and offsets against the WGSL declarations rather than assume. |
| **Swizzles and method duplication hurt discoverability.** The API is large and easy to get lost in. | Accepted. This is a documentation problem more than a design problem. The types I need are few, and the documentation is good. |

---

## 6. Consequences

### 6.1 Positive

- The camera, mesher, and renderer share one set of math types with no conversion layer.
- Matrix layout and depth conventions match wgpu by default, reducing sign and depth bugs.
- Uniform data uploads through `bytemuck` without manual packing.
- Integer voxel coordinates are a first-class type instead of a cast site.
- Meshing loops stay allocation-free and inline.
- The engine can read wgpu and ecosystem examples directly, since they use the same crate.

### 6.2 Negative

- Precision is limited to `f32`, so large worlds require camera-relative rendering.
- Collision, raycasting, and frustum logic must be written in the engine.
- SIMD behavior depends on target features, so results must be validated per platform rather than assumed.
- glam version upgrades carry migration cost.
- The API is large and includes SIMD-aligned variants that can be misused in vertex layouts.

### 6.3 Neutral

- `bytemuck` is used for GPU data layout and will be recorded as its own decision.
- Math types remain engine-internal; no engine module needs to expose glam in public interfaces beyond the renderer and mesher.
- `f64` math remains possible for isolated cases, but it is not the engine default.

---

## 7. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| Floating-point jitter at large world coordinates. | Render relative to the camera; keep chunk geometry in chunk-local space; measure jitter at target view distances before adding complexity. |
| Convention bugs (flipped axes, inverted depth, wrong handedness). | Centralize camera and projection setup in one module; document the conventions; verify the first cube against a known-good reference. |
| Misuse of `Vec3A` or SIMD-aligned types in GPU structs. | Use `Vec3` in vertex and uniform structs; assert `size_of` and `align_of` for GPU layouts; compare offsets with the WGSL declarations. |
| glam upgrade breaks engine code. | Pin the version; upgrade between milestones; keep glam usage behind math modules; read the changelog first. |
| SIMD not available or slower on a target. | Use the crate's scalar fallback feature for verification; benchmark on each target; do not assume SIMD wins for small vector counts. |
| Missing geometry helpers cause duplicated or wrong collision code. | Write AABB, ray, and frustum helpers once in an engine math module with unit tests against known cases. |
| Over-reliance on glam spreads it into serialization and public APIs. | Keep glam out of persistence formats; serialize plain floats or integers; convert at the boundary. |

---

## 8. Conclusion

glam is my deliberate, requirement-driven, performance-aware choice for engine math. I am selecting it because it provides the exact vector, matrix, quaternion, and integer-coordinate types a voxel renderer needs; because it matches wgpu's matrix layout and clip-space conventions by default; and because its SIMD implementation is allocation-free and its types upload through `bytemuck` without conversion. I acknowledge that it is `f32`-first, so large worlds depend on camera-relative rendering, and that it is a math crate rather than a geometry library, leaving collision and raycasting to engine code. I manage those through the risks and mitigations above. I therefore accept this decision as the engine's math library, with `bytemuck` handled as a separate GPU-layout decision and with its limits recorded above.

---

*This entry is part of the Valescent Engine decision log. Future entries should reference this document when they depend on the engine's math conventions or revise its assumptions.*

# Why I Chose bytemuck for Byte-Level Casting and GPU Layout

**Document type:** Architecture Decision Record / Devlog Entry  
**Status:** Accepted  
**Date:** 2026-11-13  
**Project:** Valescent Engine  
**Owner:** Project Owner  
**Supersedes:** None  
**Scope:** Casting typed data to and from raw bytes, and the memory layout of vertex, index, and uniform data sent to the GPU. Serialization formats for save files and asset pipelines are out of scope and will be decided separately.

---

## Summary

This document records why the engine uses bytemuck for byte-level casting. It is both an official decision record and a devlog entry: the decision is final for the current scope, and I am publishing the reasoning so future decisions can be traced back to it.

The short version: every frame the mesher produces vertex and index data that has to reach the GPU as bytes. bytemuck lets me hand a `&[Vertex]` straight to wgpu as `&[u8]` with a safe, trait-checked cast instead of scattering `unsafe` transmutes through the renderer. Its `Pod` and `NoUninit` bounds catch padding and invalid-bit-pattern mistakes, and its `derive` feature validates my `#[repr(C)]` structs at compile time. The tradeoff is that bytemuck does not know WGSL's uniform alignment rules — it guarantees plain bytes, not a correct GPU layout — so I still have to order and pad uniform fields deliberately.

---

## 1. Context

### 1.1 The project

The renderer is defined by how much data crosses the CPU/GPU boundary:

- **Per-chunk vertex and index data.** Meshing produces `Vec<Vertex>` and `Vec<u32>` per chunk, rebuilt whenever terrain changes. That data must be uploaded as raw bytes, often dozens of times per second while editing.
- **Per-frame uniforms.** View, projection, and model matrices are written every frame. Layout mistakes here produce silently wrong rendering rather than a crash.
- **Strict layout requirements.** GPU vertex formats and WGSL `struct` layouts are byte-level contracts. A missing padding field or a `Vec3` in the wrong place in a uniform block produces corruption or a validation error.
- **No tolerance for unsafe everywhere.** A renderer that casts pointers at every upload site is a renderer where one wrong cast is an invisible memory bug.
- **Validation-heavy development.** wgpu rejects buffers containing uninitialized padding bytes and misaligned writes, so the types I upload must be genuinely plain data, not merely `repr(C)`.
- **Voxel data on the way back.** Loading chunk data from disk eventually means reading typed structures out of byte slices, which is the inverse cast.

These requirements favor a crate whose entire purpose is proving that a type is safe to reinterpret as bytes — and whose guarantees are checked at compile time, not discovered at runtime.

### 1.2 The constraints

My choice is constrained by the following:

- It must remove hand-written `unsafe` pointer casts from buffer upload paths.
- It must express "this type has no padding and no invalid bit patterns" as a trait bound the compiler enforces.
- It must work with the math types I already chose (glam) without a conversion layer.
- It must support slice-level casting, not just single values, since meshes are arrays.
- It must not impose a serialization format or a runtime layout system I do not control.

---

## 2. Decision

**bytemuck is the byte-level casting and GPU data layout crate for the engine, with the `derive` and `must_cast` features enabled.**

This covers casting vertex, index, and uniform data to bytes for wgpu uploads, reading typed data back out of byte slices, zero-initialization of buffers, and the `#[repr(C)]` structs that define my GPU layouts. glam's `bytemuck` feature is enabled so its math types satisfy the same traits. Serialization formats, compression, and asset file layouts are not covered by this decision.

I accept this decision for the current scope. I will revisit it only if the assumptions in this document stop holding.

---

## 3. Why I Chose bytemuck

### 3.1 Uploads become a cast, not a copy

With bytemuck, uploading a chunk mesh is `cast_slice(&vertices)` and handing the result to wgpu. No manual `Vec<u8>` accumulation, no per-vertex `extend_from_slice`, no `unsafe { std::slice::from_raw_parts(...) }` at each call site. Since meshing already produces contiguous typed vectors, the upload path becomes one safe function call, and the same slice can be measured, logged, and reused without conversion.

### 3.2 Soundness encoded in trait bounds

bytemuck's guarantees are exactly the ones a GPU upload needs: `Pod` for types that are plain old data, `NoUninit` for types with no uninitialized or padding bytes, and `AnyBitPattern` for types valid under any bit pattern. Those bounds are checked by the compiler, so a struct that fails to meet them cannot reach the upload function at all. That is a real improvement over passing bags of bytes around with comments claiming they are safe.

### 3.3 The derive macro enforces the layout rules

Deriving `Pod`, `Zeroable`, and `NoUninit` on a `#[repr(C)]` vertex struct makes the compiler verify the properties instead of me. If I add a field that introduces padding, the derive fails rather than silently uploading misaligned data. Combined with the `must_cast` feature, casts that cannot be statically proven valid become compile errors, which is the strongest place to catch a layout bug.

### 3.4 It slots directly into glam and wgpu

glam exposes a `bytemuck` feature that implements the traits for `Vec3`, `Vec4`, `Mat4`, and `Quat`. That means my uniform struct can contain glam types directly and still be uploaded as bytes. wgpu's own examples use the same combination, so the upload path I write is the documented one rather than an adaptation of it.

### 3.5 Slice-oriented, which matches how meshes are shaped

`cast_slice` and `cast_slice_mut` handle arrays natively, and `pod_read_unaligned` handles reading a struct out of a byte slice at an arbitrary offset. Meshing, index buffers, and chunk deserialization all operate on arrays, so a crate built around slices fits the access pattern better than a value-level conversion helper would.

### 3.6 Small surface, no runtime cost

bytemuck is a traits-and-functions crate. There is no runtime layout engine, no reflection, no registration step, and no allocation. The casts compile down to pointer reinterpretation, which is what an upload path should be. It also builds without `std`, so it will not block a constrained target later.

### 3.7 Zero-initialization where the GPU needs it

`Zeroable`, `zeroed`, and `fill_zeroes` handle the cases where wgpu wants a buffer region cleared — uniform padding, storage buffers, or scratch memory. Having that in the same crate as the casting story means one consistent model for "what does this memory contain" across the renderer.

### 3.8 Why not `encase`, `zerocopy`, or raw `unsafe`?

- **`encase`** is a strong crate, and it solves a problem bytemuck does not: automatic WGSL `std140`/`std430` uniform layout for arbitrary structs. I am not adopting it as the base because it is a layout engine built on top of byte casting, and my structs are small enough to order and pad by hand with explicit assertions. If uniform layout work becomes a recurring source of bugs, `encase` is a complementary addition, not a replacement.
- **`zerocopy`** is a well-maintained alternative with a broader feature set and its own derive macros. It would work, but bytemuck is the de facto standard in Rust graphics, glam ships support for it, and the examples and ecosystem code I learn from use it. Choosing the crate the rest of my stack already speaks to is worth more than a marginally wider API.
- **Hand-written `unsafe`** would remove the dependency, at the cost of an unreviewed `from_raw_parts` at every upload site and no compile-time guarantee that the type is actually plain data. That is the exact failure mode I am trying to eliminate.
- **`serde` or `bincode`** answer a different question. Serialization is about a stable, versioned file format, not about reinterpreting in-memory structs for the GPU. Using them for buffer uploads would add encoding overhead and couple my GPU layout to a file format.

---

## 4. Alternatives Considered

| Alternative | Assessment | Outcome |
|---|---|---|
| **bytemuck** | Standard in Rust graphics, trait-checked soundness, derive validation, slice casting, glam and wgpu support, zero cost. | **Selected** |
| **`encase`** | Solves automatic WGSL uniform alignment; a layout layer above casting, macro-driven, extra abstraction. | Not selected as the base; retained as a possible complement |
| **`zerocopy`** | Broader API and good maintenance; different trait model, less used in graphics, no glam-first ecosystem story. | Not selected |
| **Hand-written `unsafe` casts** | No dependency; unsound-by-accident, repeats unsafe at every call site, no compile-time proof. | Not selected |
| **`serde` / `bincode`** | Correct for versioned persistence; wrong tool for GPU buffer layout, adds encoding cost. | Not selected for this scope; still a candidate for save formats |

---

## 5. Counterarguments and Disposition

This decision has real weaknesses. I am recording them here rather than omitting them, so the record stays honest.

| Counterargument | Disposition |
|---|---|
| **bytemuck does not know WGSL's uniform layout rules.** A `Vec3` followed by an `f32` in a uniform struct will not match WGSL's 16-byte alignment, and a cast will succeed while the shader reads garbage. | Accepted. This is the real limitation. I will order uniform fields explicitly, add padding fields where required, and assert `size_of` against the WGSL declaration. If this becomes a recurring bug source, `encase` is the documented next step. |
| **`Pod` is a blunt instrument.** It forbids padding and invalid bit patterns, which is more than some types need. | Accepted with qualification. That is why `NoUninit` and `AnyBitPattern` exist. I will use `NoUninit` for write-only upload types and keep `Pod` for types that genuinely round-trip. |
| **Manual `impl Pod` is unsafe and easy to get wrong.** An incorrect implementation is undefined behavior with no compiler help. | Accepted. Engine code will use the derive macros exclusively; manual implementations require a written justification and a test. |
| **Derive errors are hard to read.** Adding a padded field produces a wall of trait-bound errors. | Accepted. This is friction during layout changes, but it is friction that fires at compile time instead of corrupting a frame. |
| **Padding bytes themselves are a hazard.** wgpu rejects buffers containing uninitialized padding, and `NoUninit` prevents that only if the type is declared correctly. | Accepted. Vertex and uniform structs will be padding-free by construction, and any intentional padding will be an explicit field that is written, not implicit alignment. |
| **It adds a dependency to every crate that touches a buffer.** | Accepted. It is tiny, has no transitive dependencies, and is already in the dependency tree through glam and wgpu. |
| **It does nothing for endianness or compression.** | Accepted. Save formats are a separate decision. `pack1` and `half` exist if packed or half-float formats are ever needed. |
| **It can be misused for persistence.** Casting chunk data straight to disk ties the save format to the in-memory layout. | Accepted. Persistence will use an explicit format with versioning, not a raw cast. This is recorded as a separate decision. |

---

## 6. Consequences

### 6.1 Positive

- Buffer uploads are a safe `cast_slice` call instead of per-site `unsafe`.
- Layout requirements are expressed as trait bounds and enforced by the derive macro.
- Vertex structs cannot silently acquire padding or uninitialized bytes.
- glam types upload without conversion because both crates share the same trait.
- Meshes can be measured and reused as both typed data and raw bytes.
- Zero-initialization has one consistent story across the renderer.

### 6.2 Negative

- WGSL uniform alignment still requires manual field ordering and explicit padding.
- Layout changes produce verbose derive errors.
- Intentional padding must be represented as real fields rather than implicit alignment.
- Manual trait implementations remain an unsafe footgun that must be avoided by discipline.
- A second crate defines the rules for how GPU-facing structs are declared, so engine code has one more convention to follow.

### 6.3 Neutral

- Serialization, compression, and asset formats remain separate decisions.
- If uniform layout proves error-prone, `encase` can layer on top without replacing bytemuck.
- The crate is usable without `std`, which leaves constrained targets open.

---

## 7. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| A uniform struct silently mismatches WGSL layout. | Order fields by decreasing alignment; use explicit padding fields; assert `size_of::<T>()` and field offsets against the WGSL declaration; keep one struct per shader block. |
| Vertex data contains padding, producing validation errors or corrupt geometry. | Derive `Pod`/`NoUninit` so padding fails at compile time; keep vertex formats as ordered `f32` and `u32` fields; verify with `size_of` assertions in tests. |
| Someone writes a manual `impl Pod` incorrectly. | Restrict manual implementations to reviewed cases with a test; prefer derive everywhere; document the rule in the engine's layout module. |
| Casts panic at runtime on misalignment. | Use `cast_slice` only on naturally aligned typed slices; use `pod_read_unaligned` for byte buffers; keep casts centralized in upload helpers. |
| bytemuck's trait requirements change in an upgrade. | Pin the version; upgrade deliberately; keep the dependency surface behind upload helpers so a migration is local. |
| Over-casting turns the engine into an unreadable byte soup. | Keep typed structs as the source of truth; cast only at the GPU boundary; never expose `&[u8]` as an engine-level API. |
| Using casts for persistence ties save files to in-memory layout. | Write an explicit, versioned serialization format and convert at the boundary. |

---

## 8. Conclusion

bytemuck is my deliberate, safety-driven, layout-aware choice for byte-level casting in the engine. I am selecting it because it turns GPU uploads into a trait-checked cast instead of scattered `unsafe`, because its derive macro enforces the padding and bit-pattern rules that wgpu actually cares about, and because it composes with the glam math types I already chose. I acknowledge that it guarantees plain bytes rather than correct WGSL uniform alignment, and that hand-written trait implementations remain a footgun. I manage those through explicit field ordering, padding fields, layout assertions, validation-enabled testing, and a documented path to `encase` if uniform layout becomes a recurring source of defects. I therefore accept this decision as the engine's byte-casting and GPU layout crate, with serialization formats left as a separate decision and with its limits recorded above.

---

*This entry is part of the Valescent Engine decision log. Future entries should reference this document when they depend on the engine's GPU data layout or revise its assumptions.*

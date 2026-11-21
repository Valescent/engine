# Public API

The library does not have a stable public surface yet. This page records the current surface, why
a full reference is not published, and the gate for adding one. It is a status page, not the
reference.

| Area | Status |
|---|---|
| Public API stability | None before `1.0` |
| `1.0.0` target | Not set |
| Package published | No; `publish = false` |
| Published rustdoc or docs.rs | No |
| Local rustdoc | Builds with `cargo doc --no-deps` |
| Missing-docs lint | Not enabled |
| Public API diff or semver check | Not run |
| Public items | `run`, `RunError`, `RendererError`, `SaveError`, `ChunkCoord`, plus the transitional `terrain_core` and `v0` module re-exports |

## Current surface

The crate is `engine`. The complete public surface is defined by `src/lib.rs` and its
re-exports:

```rust
pub fn run() -> Result<(), RunError>;

pub enum RunError {
    EventLoop(winit::error::EventLoopError),
    Window(winit::error::OsError),
    Renderer(RendererError),
    Save(SaveError),
}

pub enum RendererError {
    CreateSurface(wgpu::CreateSurfaceError),
    RequestAdapter(wgpu::RequestAdapterError),
    RequestDevice(wgpu::RequestDeviceError),
    UnsupportedSurface,
    Validation(String),
}

pub enum SaveError {
    Io(std::io::Error),
    InvalidFileName(String),
    InvalidMagic,
    UnsupportedVersion(u16),
    InvalidChunkCoordinate(IVec3),
    InvalidBlockCount(u32),
    InvalidLocalCoordinate(IVec3),
    InvalidBlockId(u8),
    DuplicateRecord(IVec3),
    DuplicateChunk(ChunkCoord),
    ChunkMismatch { file: ChunkCoord, header: ChunkCoord },
    Truncated,
    TrailingBytes,
}

pub struct ChunkCoord(IVec3);
```

- `run` initializes tracing, loads or seeds the world, creates the event loop and application, and
  returns when the window closes or startup fails. It is the only public function.
- `RunError`, `RendererError`, and `SaveError` derive `Debug` and `thiserror::Error`. Their variants
  are public. `RunError::Save` reports world load and save failures.
- `ChunkCoord` is re-exported so `SaveError` variants can be matched. It has no public constructor
  or accessor; the world-facing methods are `pub(crate)`.
- `RendererError` is defined in the private `renderer` module, `SaveError` in the private
  `v0::world` module; their re-exports are public API.
- **Transitional re-exports (not part of the intended surface):** `src/lib.rs` currently does
  `pub use terrain_core::{...}` for the 28 headless terrain modules (`pipeline`, `section`,
  `content`, `save`, and the rest) and `pub use v0::{generation, light, mesh, mesher, streaming,
  voxel, world};`. They exist so the tests and tools compile across the crate split. The structure
  plan narrows them to one `engine::terrain` facade; see [Modular structure](../design/modular-structure.md)
  §4.2 and §8, decision 2.
- There are no other public traits, constants, or macros. The crate has two optional Cargo
  features, `tracy` and `rayon-pool`, and both are development aids rather than API surface.
- None of the error enums or `ChunkCoord` is `#[non_exhaustive]`. Adding a variant is a breaking
  change under the current definition.
- The public errors expose `winit`, `wgpu`, and `std::io` error types directly. Their stability is
  coupled to those dependencies until a decision says otherwise.

This surface has no semver compatibility promise. Any `0.0.x` release may change it.

## What is not public

- `App`, `Camera`, `Player`, `Aabb`, `Block`, `Renderer`, `Vertex`, `World`, `RaycastHit`, `Lod`,
  `build_mesh`, `MeshQueue`, `WorkerPool`, `Frustum`, and renderer helpers are private or `pub(crate)`.
- The `block`, `camera`, `config`, `frustum`, `generation`, `headless`, `light`, `mesh`, `mesher`,
  `metrics`, `overlay`, `player`, `raycast`, `renderer`, `sky`, `streaming`, `voxel`, and `world`
  modules are private. Paths such as `engine::Camera` or `engine::Renderer` do not exist.
- `src/main.rs` is the only in-repo consumer. It calls `engine::run()` and returns
  `engine::RunError`.
- The shader, mesh layout, texture pipeline, world state, and configuration are not public API.

## Current reference

- Run `cargo doc --no-deps --open` to generate the current rustdoc. The output lands at
  `target/doc/engine/index.html`.
- The rustdoc build succeeds today, but only `run` and `SaveError` have item doc comments. The
  error variants and `ChunkCoord` have no rustdoc descriptions.
- There is no docs.rs page because the package is not published.
- Local rustdoc describes the current tree. It is not a stability promise.

## Why there is no full reference yet

- [Versioning](versioning.md#compatibility) says there is no stability promise before `1.0.0`,
  there is no `1.0.0` target, and the package is unpublished.
- The public surface is currently small and will change as the engine grows.
- A full reference would imply a compatibility contract the project has not made.
- The error enums currently expose `winit` and `wgpu` error types and a `String`; their evolution
  needs a decision before a stable reference can describe them.
- No `missing_docs`, API-diff, or semver-check gate exists yet, so a generated reference would
  document an incomplete and unchecked surface.

## Gate for a stable reference

| Gate | Current | Needed before a full reference |
|---|---|---|
| Stability decision | None | An ADR or versioning update that declares a stable line |
| Version target | `0.0.x`, no `1.0.0` target | Decide when or whether to leave `0.0.x` |
| Publication | `publish = false` | Decide docs.rs or site-hosted, versioned rustdoc |
| Item documentation | `run` only | Every public item and error variant documented |
| Missing-docs lint | Not enabled | `#![warn(missing_docs)]` or `#![deny(missing_docs)]` |
| Examples and doctests | None for the public API | Runnable examples for `run` and its errors; `cargo test --doc` |
| Error evolution | Exhaustive enums | Decide `#[non_exhaustive]` or a documented variant policy |
| API diff or semver check | Not run | Decide whether the policy needs a check, then add it |
| Reference hosting | Local rustdoc only | Choose a stable URL and versioning scheme |

## Definition of done for this page

When the gate is met:

1. Record the stability decision in an ADR or in [Versioning](versioning.md).
2. Audit the public surface and decide whether to keep or wrap the `winit` and `wgpu` error types.
3. Document every public item and variant, then enable the missing-docs lint.
4. Add compiled examples and doctests, and run `cargo test --doc`.
5. Publish or host the rustdoc at a stable, versioned URL.
6. Replace this page with the reference, or with a landing page that links to it.
7. Link the reference from `SUMMARY.md`, the README, and the documentation index.
8. Update CI if an API-diff or semver check becomes part of the policy.
9. Keep the reference and the versioning compatibility policy in sync.

## Do not

- Do not treat this page or local rustdoc as a stable reference.
- Do not expose `pub(crate)` internals just to fill a reference.
- Do not publish docs.rs or a versioned reference before the stability decision.
- Do not claim semver compatibility for `0.0.x`.
- Do not document planned APIs as if they already exist.
- Do not change the public surface only to make the reference look complete.

## Related

- [Versioning](versioning.md)
- [Architecture](architecture.md)
- [Configuration](configuration.md)
- [Testing](testing.md)
- [CI](ci.md)
- [Decision index](../decisions/index.md)

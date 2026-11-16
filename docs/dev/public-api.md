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
| Public items | `run`, `RunError`, `RendererError` |

## Current surface

The crate is `engine`. The complete public surface is defined by `src/lib.rs` and its one
re-export:

```rust
pub fn run() -> Result<(), RunError>;

pub enum RunError {
    EventLoop(winit::error::EventLoopError),
    Window(winit::error::OsError),
    Renderer(RendererError),
}

pub enum RendererError {
    CreateSurface(wgpu::CreateSurfaceError),
    RequestAdapter(wgpu::RequestAdapterError),
    RequestDevice(wgpu::RequestDeviceError),
    UnsupportedSurface,
    Validation(String),
}
```

- `run` initializes tracing, creates the event loop and application, and returns when the window
  closes or startup fails. It is the only public function. Its rustdoc says it returns an error if
  the event loop, window, or GPU initialization fails.
- `RunError` and `RendererError` derive `Debug` and `thiserror::Error`. Their variants are public.
- `RendererError` is defined in the private `renderer` module and re-exported at the crate root.
  That re-export is the only `pub use`.
- There are no public modules, structs, traits, constants, macros, or Cargo features.
  `--all-features` currently selects nothing.
- Neither error enum is `#[non_exhaustive]`. Adding a variant is a breaking change under the
  current definition.
- The public errors expose `winit` and `wgpu` error types directly. Their stability is coupled to
  those dependencies until a decision says otherwise.

This surface has no semver compatibility promise. Any `0.0.x` release may change it.

## What is not public

- `App`, `Camera`, `Renderer`, `Vertex`, `build_mesh`, and renderer helpers are private or
  `pub(crate)`.
- The `camera`, `renderer`, and `voxel` modules are private. Paths such as `engine::Camera` or
  `engine::Renderer` do not exist.
- `src/main.rs` is the only in-repo consumer. It calls `engine::run()` and returns
  `engine::RunError`.
- The shader, mesh layout, texture pipeline, world state, and configuration are not public API.

## Current reference

- Run `cargo doc --no-deps --open` to generate the current rustdoc. The output lands at
  `target/doc/engine/index.html`.
- The rustdoc build succeeds today, but only `run` has an item doc comment. The error types and
  their variants have no rustdoc descriptions.
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

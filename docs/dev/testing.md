# Testing

The default test suite is headless and deterministic. Rendering and window behaviour are verified manually or behind an opt-in feature, never in the default `cargo test` run.

## Test layers

| Layer | Purpose | Runs in CI |
|---|---|---|
| Unit tests | Pure logic: camera math, meshing, storage, collision, raycasts, save/load, lighting. | Yes |
| Integration tests | Public API behaviour across modules. | Yes |
| Manual GPU checks | Window, surface, pipeline, input, resize, and visual correctness. | No |
| Opt-in GPU tests | Automated rendering checks behind a feature or environment variable. | Not yet |

## Commands

```console
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features --locked
cargo doc --no-deps --all-features
typos
mdbook build
```

CI also runs dependency licence and advisory checks, commit message checks, and the documentation build. The `pool_sim` benchmark is not part of CI.

## Writing tests

- Put unit tests beside the code in `#[cfg(test)] mod tests`.
- Put integration tests in `tests/` and use only the public API.
- Use fixed inputs and fixed outputs. No wall clock, network, or unseeded randomness.
- Prefer pure functions for collision, raycasting, serialization, light propagation, and generation so they stay testable without a GPU.
- Test edge cases explicitly: negative chunk coordinates, chunk borders, empty meshes, malformed save files, and stale async revisions.
- World save/load and corruption checks are in [World management](world-management.md#validate).
- Do not make the default suite depend on a window, GPU, display server, or user home directory.
- GPU tests must be gated behind a feature or environment variable and must not run on CI by default.
- Use temporary directories for persistence tests and clean them up.

## Current coverage

The headless suite covers:

- Camera look direction, yaw/pitch clamping, and angle wrapping.
- Mouse-look gating on capture and focus.
- Block IDs, solidity, face-layer mapping, and hotbar selection.
- Chunk coordinate conversion including negative worlds, block storage idempotence, and dirty-chunk sets for interior, border, corner, and last-block edits.
- Missing and empty chunk meshes, cross-chunk face culling, exact face order, UV order, texture layers, and deterministic chunk meshing.
- Player AABB, gravity, jumping, ground detection, axis order, collision response, and tunnelling limits.
- DDA ray hits, misses, range limits, tie-break order, and face normals.
- Placement rejection for occupied cells and player intersection.
- Procedural texture data size and determinism.
- `VCHK` round-trip, negative chunks, local `0`/`31`, stale-file removal, missing versus empty directories, and every malformed-file error path.

## Manual GPU acceptance

Run the engine and verify the milestone's controls and visuals:

```console
cargo run
```

Set `RUST_LOG=engine=debug` first to see frame timing and input logs:

```console
# PowerShell
$env:RUST_LOG = "engine=debug"; cargo run
```

```console
# POSIX
RUST_LOG=engine=debug cargo run
```

Check:

- The window opens and shows the demo world with grass, dirt, stone, wood, and leaves.
- WASD walks using yaw; `Space` jumps; the player lands on solid blocks and cannot walk through walls.
- Mouse look works after clicking to capture; `Escape` releases the cursor without closing.
- The wireframe outline follows the targeted block and disappears when nothing is in range.
- Left click breaks the targeted block; right click places the selected block.
- Number keys `1`–`5` change the placed block type.
- Placement inside the player or an occupied cell is rejected.
- Block faces show the expected texture layers.
- `Ctrl+S` and closing the window save `world/chunks/`; reopening restores the edits.
- `V` toggles vsync without stopping rendering and logs the resulting state.
- `F3` toggles the FPS/frame-time overlay; values update once per second.
- A malformed `.vchk` file produces a typed startup error rather than a panic.
- Resizing and minimizing/restoring keep the projection correct.
- Frame-time logs show a stable rate while moving.
- The output contains no `wgpu` validation errors or panics.

Useful environment variables:

```console
$env:RUST_LOG = "engine=debug"   # engine logs and frame timing
$env:WGPU_BACKEND = "dx12"       # force a backend on Windows
```

On Linux and macOS, set the same variables with the shell's normal syntax.

## Rendering failures

- Run with `RUST_LOG=engine=debug,wgpu=warn` to see validation warnings without the full `wgpu` trace.
- Shader and pipeline compile errors fail startup as `RendererError::Validation`; see
  [Shader authoring](shaders.md#validate).
- Texture layer, format, sampler, and upload checks are in
  [Texture and asset pipeline](textures.md#validate).
- Compare backends with `WGPU_BACKEND` before assuming an engine bug.
- Update the GPU driver before filing a rendering issue.
- Include the adapter name, backend, window size, and a reproduction step in bug reports.
- Never hide shader or pipeline validation errors; surface them as typed startup errors.

See [Debugging](debugging.md) for log filters, error reporting, and profiling tools.

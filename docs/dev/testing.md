# Testing

The default test suite is headless and deterministic. Rendering and window behaviour are verified manually or behind an opt-in feature, never in the default `cargo test` run.

## Test layers

| Layer | Purpose | Runs in CI |
|---|---|---|
| Unit tests | Pure logic: camera math, meshing, storage, collision, raycasts, save/load, lighting. | Yes |
| Integration tests | Public API behaviour across modules. | Yes |
| Manual GPU checks | Window, surface, pipeline, input, resize, and visual correctness. | No |
| Opt-in GPU tests | Automated rendering checks behind a feature or environment variable. | Not yet |

## Headless runs

`ENGINE_HEADLESS=1` runs the normal generation, streaming, light, and mesh paths without a window,
at a fixed 60 Hz step, then prints one machine-readable line:

```console
ENGINE_HEADLESS=1 ENGINE_HEADLESS_FRAMES=600 ENGINE_HEADLESS_SPEED=32 ENGINE_SEED=42 cargo run --release
```

The line reports frame work (`mean_ms`, `p50_ms`, `p95_ms`, `max_ms`), generated columns, chunks,
loaded and ready columns, pending jobs, and resident block and light bytes. The **Perf** workflow
runs it for a static camera and a walk on every `main` push and gates on a generous budget. See
[Benchmark results](benchmarks.md#headless-run-v008).

## Commands

```console
cargo fmt --all -- --check
cargo clippy --all-targets --locked --no-default-features -- -D warnings
cargo test --locked --no-default-features
cargo run --locked --quiet --no-default-features --bin terrain-gen -- golden check
cargo tree -p terrain-core -e normal --no-default-features
cargo doc --no-deps --no-default-features
sh scripts/size-budgets.sh
sh scripts/size-budgets.test.sh
typos
mdbook build
```

`size-budgets.sh` fails when a counted Rust file exceeds 1,000 lines and reports the 600-line
advisory tier without failing; the self-test checks its annotations and counts against fixed
fixtures. See [CI](ci.md#size-budgets).

CI also runs dependency licence and advisory checks, commit message checks, and the documentation build. The `pool_sim` benchmark is not part of CI. `--no-default-features` is required in CI because `tracy` is a default feature that builds a C++ client; use `cargo test --locked` locally to test the profiled build.

`terrain-core` has no `wgpu`, `winit`, `wgpu-profiler`, or `tracy-client` in its normal dependency tree; the CI crate-graph step fails with a clear message if one appears. Run `cargo tree -p terrain-core -e normal --no-default-features` and look for those four names locally.

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

The headless suite covers **882 library tests** across `engine` and `terrain-core` (471 engine +
411 terrain-core, 7 ignored), **1** `facade`, **3** `light_preview`, **4** `blend`, **30**
`terrain_gen`, and **3** `visibility` integration tests. The terrain integration test and its
goldens live under `crates/terrain-core/tests/`; `light_preview`, `blend`, and `visibility` stay
with `engine`.

- Camera look direction, yaw/pitch clamping, and angle wrapping.
- Mouse-look gating on capture and focus.
- Block IDs, solidity, face-layer mapping, and hotbar selection.
- Chunk coordinate conversion including negative worlds, block storage idempotence, and dirty-chunk sets for interior, border, corner, and last-block edits.
- Missing and empty chunk meshes, cross-chunk face culling, exact greedy quads, block-unit UVs, texture layers, LOD boundary faces, face winding for back-face culling, packed 12-byte vertices, indexed quads, and deterministic chunk meshing.
- LOD1 and LOD2 downsampling, boundary faces, thresholds and hysteresis across three levels, and greedy reduction against the v0.0.4 mesher.
- Frustum plane extraction and conservative AABB rejection for inside, outside, and straddling boxes.
- Light storage and BFS: open columns, overhang falloff, dark caves, lamp falloff, water transmission, and cross-chunk borders.
- Per-corner AO, packed light/AO, greedy splitting at light/AO changes, and LOD1 representative sampling.
- Water meshing: air-only faces, no opaque faces, layer 9, and the water cell's own light.
- Light snapshots, edit invalidation (3x3x3 plus columns below), and light/mesh queue ordering: an edit waits for the accepted light field before remeshing.
- Generation determinism, biome thresholds, sea fill, cave carving, tree structure, streaming ring order, unload hysteresis, and the readiness guard, including air-only chunks and rejection of movement into an unready column; `config.toml` defaults, ranges, and unknown keys.
- Modified-only saves, seed creation and parsing, the missing-seed error, saved chunks applied over generated blocks, empty saved chunks, and unload saving and dropping a column; a late generation result is discarded after its column was cancelled.
- Mesh revisions, stale-result rejection, queue priority, background dropping, the two-upload cap, and admission checks before snapshot work.
- v1 terrain: description/header/record round trips, generation goldens and seams, column light equality and fuzz, greedy meshing face coverage, LOD downsample, section encode/decode, persistence round trips, backup rotation, torn-tail recovery, journal replay and prefix trimming, repair actions, migration and reports, corruption fuzzing, repeated crash/save/reopen, incremental light/mesh equality, LOD selection, and tile-stream swap decisions.
- The deterministic 8x8 stress fixture and the stress-mode world path.
- Player AABB, gravity, jumping, ground detection, axis order, collision response, and tunnelling limits.
- DDA ray hits, misses, range limits, tie-break order, and face normals.
- Placement rejection for occupied cells and player intersection.
- Gameplay core: sorted item ids, every placeable block has an item, tool stacks, liquid exclusion, inventory merge/fill/overflow/split/move, tool durability breakage, hotbar clamping, break time and tier gates, wrong-class drop rejection, content drop resolution, item gravity and landing, pickup and full-inventory behaviour, despawn, spawn merging, entity record round trips, and the `VPLY` round trip, CRC, corruption, version, oversized-count, and quarantine paths.
- The scripted gameplay loop: spawn a stone drop from the held tool, pick it up, select the hotbar slot, consume one placeable item, save, reopen, and assert the player and inventory survived.
- UI: the HUD produces egui shapes headlessly with an empty inventory and a target, and the current capture policy is pinned to non-interactive.
- Procedural texture data size and determinism.
- `VCHK` round-trip, negative chunks, local `0`/`31`, unmodified files left in place, empty modified chunks, missing versus empty directories, and every malformed-file error path.

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

- The window opens and shows the generated world with grass, dirt, stone, wood, leaves, and sand.
- Lighting matches the time of day: open surfaces are lit, caves are dark, and lamps glow.
- WASD walks using yaw; `Space` jumps; the player lands on solid blocks and cannot walk through walls.
- Mouse look works after clicking to capture; `Escape` releases the cursor without closing.
- The wireframe outline follows the targeted block and disappears when nothing is in range.
- Left click breaks the targeted block; right click places the selected block.
- Number keys `1`–`5` change the placed block type.
- Placement inside the player or an occupied cell is rejected.

For the 0.0.10 demo (`TERRAIN_DEMO=1`):

```console
# PowerShell
$env:TERRAIN_DEMO = "1"
$env:ENGINE_STRESS_SCENE = "1"   # only when world/seed is absent
cargo run --bin engine
```

- Hold left mouse on wood: the outline fills and the block breaks after the hardness time.
- Walk over the drop; the item count increases, and a full inventory leaves the drop on the ground.
- `1`-`9` select hotbar slots. Slot 1 starts with the temporary wooden pickaxe.
- Stone with the wooden pickaxe drops stone; stone by hand breaks without a drop.
- Right click places the selected block, rejects the player's cell, and consumes one item only on a
  successful edit.
- `Ctrl+S`, close the window, reopen, and confirm the position, view, inventory, selected slot,
  dropped items, and block edits survived.
- Confirm the HUD: crosshair at centre, nine hotbar slots at the bottom, a white selected border,
  the temporary pickaxe icon and durability bar, and the hold/place prompt when a block is targeted.
- Click and drag over the HUD; the camera must still capture and the block actions must still work,
  because the HUD is non-interactive.
- Block faces show the expected texture layers.
- `Ctrl+S` and closing the window save modified chunks under `world/chunks/`; reopening restores the edits over the generated terrain.
- `V` toggles vsync without stopping rendering and logs the resulting state.
- `F3` toggles the FPS/frame-time overlay; values update once per second.
- A malformed `.vchk` file produces a typed startup error rather than a panic.
- Resizing and minimizing/restoring keep the projection correct.
- Frame-time logs show a stable rate while moving, plus visible chunks, total chunks, visible vertices, draw calls, rebuild time, and upload time.
- The output contains no `wgpu` validation errors or panics.

Run the stress fixture with:

```console
$env:ENGINE_STRESS_SCENE = "1"; $env:RUST_LOG = "engine=debug"; cargo run
```

It never reads or writes `world/chunks/`. Check that all 64 chunks load, distant chunks switch LOD
without holes or flicker, draw calls drop when looking away, breaking blocks does not hitch, and the
frame rate holds at 60 FPS in the default window.

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

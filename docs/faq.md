# FAQ and troubleshooting

Short answers for the current pre-alpha engine and a symptom-first index into the detailed pages.
Only v0.0.1 through v0.0.5 are released; v0.0.6-v0.0.7 are planned. A planned feature is not a bug.

| I need to... | Start here |
|---|---|
| Find out what works today | [Project and status](#project-and-status) |
| Find out what is planned | [Roadmap](roadmap.md) |
| Fix a build or run problem | [Troubleshooting index](#troubleshooting-index) |
| Understand a control | [Controls](dev/controls.md) |
| Report a bug or security issue | [Reporting a problem](#reporting-a-problem) |
| Contribute a change | [Contributing](CONTRIBUTING.md) |

## Project and status

The current release is **v0.0.5, pre-alpha**. The build has a
window, a depth-buffered `wgpu` renderer, chunked greedy meshes with two LODs, frustum culling, asynchronous remeshing, walking and collision,
block breaking and placing, procedural block textures, and `VCHK` chunk saves. It does not have
lighting, fog, water, or terrain generation.

| Question | Answer |
|---|---|
| What is Valescent Engine? | A voxel engine written in Rust on `wgpu` and `winit`. It is source-available, not open source. See [the landing page](index.md) and [Architecture](dev/architecture.md). |
| What is the current version? | `0.0.5` is released. v0.0.6-v0.0.7 are planned. See [Roadmap](roadmap.md) and [Changelog](https://github.com/Valescent/engine/blob/main/CHANGELOG.md). |
| Is it stable? | No. It is pre-alpha and there is no public API stability before 1.0. See [Versioning](dev/versioning.md). |
| Why is the version `0.0.x`? | Each roadmap milestone ships as the next `0.0.x` patch. There is no `0.1.0` or `1.0.0` target yet. See [Versioning](dev/versioning.md#semantic-versioning). |
| What is next? | v0.0.6 for lighting, fog, and water. See [the roadmap](roadmap.md). |
| Why is the world one mesh? | It is not. Since v0.0.3 each non-empty chunk owns its mesh and GPU buffer. See [Architecture](dev/architecture.md#rendering-resources). |
| Can I walk, fall, jump, or collide? | Yes. WASD walks, `Space` jumps, and swept AABB collision stops the player at solid blocks. See [Controls](dev/controls.md#bindings-by-milestone). |
| Can I break or place blocks? | Yes, after the cursor is captured. Left click breaks and right click places the selected block. See [Interaction](dev/controls.md#interaction-current-v004). |
| Why are there no light, fog, or water? | Lighting, AO, fog, and water are planned for v0.0.6. Procedural block textures ship in v0.0.4. See the [Roadmap](roadmap.md). |
| Is there multiplayer, a server, modding, or scripting? | No. They are out of scope through v0.0.7. See the non-goals in the [Roadmap](roadmap.md). |
| What platforms are supported? | Windows, Linux, and macOS through `winit`/`wgpu`, with Vulkan, Direct3D 12, or Metal. CI tests all three. See [Setup](dev/setup.md#platform-prerequisites) and [CI](dev/ci.md#ci-workflow). |
| Are there release binaries? | No. The release workflow publishes changelog notes, not binaries. See [CI](dev/ci.md#release-workflow) and [Release checklist](dev/release-checklist.md). |
| What licence is it under? | PolyForm Shield License 1.0.0. It is source-available, not OSI open source. See [the licence](https://github.com/Valescent/engine/blob/main/LICENSE). |
| Which Rust version do I need? | The repository pins Rust `1.99.1` in `rust-toolchain.toml`; `rustup` installs it automatically. A broader MSRV policy is out of scope. See [Setup](dev/setup.md#toolchain) and [Versioning](dev/versioning.md). |
| Where are the architecture decisions? | The [decision index](decisions/index.md) lists all accepted ADRs and the template. |
| Where is the public API reference? | Not published. The library is pre-alpha with no stability promise; see [Public API](dev/public-api.md). |

## Setup and build

| Question or symptom | Answer |
|---|---|
| `cargo` uses the wrong toolchain | Run `rustup show` in the repository. `rust-toolchain.toml` pins Rust 1.99.1. See [Setup](dev/setup.md#toolchain). |
| `link.exe` is missing on Windows | Install Visual Studio Build Tools with the **Desktop development with C++** workload. See [Setup](dev/setup.md#platform-prerequisites). |
| Linux build errors mention `xkbcommon`, Wayland, or X11 | Install the windowing development packages for your distribution. See [Setup](dev/setup.md#platform-prerequisites). |
| macOS build errors | Install the Xcode command-line tools with `xcode-select --install`. See [Setup](dev/setup.md#platform-prerequisites). |
| `cargo run` fails before the window opens, or finds no adapter | Update the GPU driver and check `WGPU_BACKEND`. See [Setup](dev/setup.md#graphics-backend) and [Rendering failures](dev/testing.md#rendering-failures). |
| Which backend is being used? | The engine logs the selected adapter and backend at `info`. To force one, set `WGPU_BACKEND=dx12`, `vulkan`, or `metal`. See [Setup](dev/setup.md#graphics-backend). |
| `cargo fmt` or `cargo clippy` is missing | Use the pinned toolchain, or add the components with `rustup component add rustfmt clippy`. See [Setup](dev/setup.md#toolchain). |
| `mdbook` is missing or the docs build fails | Install mdBook `0.5.4` to match CI and check that the page is in `SUMMARY.md`. See [CI](dev/ci.md#docs-workflow). |
| `typos` fails | CI uses `typos` `1.52.2`. Real project terms belong in `_typos.toml`; otherwise fix the spelling. See [CI](dev/ci.md#ci-workflow). |
| `cargo deny` fails | The policy is in `deny.toml`; see the dependency job in [CI](dev/ci.md#ci-workflow). |
| How do I run the same checks as CI? | See [Testing commands](dev/testing.md#commands) and [Local equivalents](dev/ci.md#local-equivalents). |
| Do tests need a GPU or a window? | No. The default suite is headless and deterministic; rendering is checked manually. See [Test layers](dev/testing.md#test-layers). |
| How do I get more logs? | Set `RUST_LOG=engine=debug` or `RUST_LOG=engine=debug,wgpu=warn`. See [Logging](dev/debugging.md#logging). |
| How do I get a backtrace? | Set `RUST_BACKTRACE=1` or `RUST_BACKTRACE=full`. See [Errors and panics](dev/debugging.md#errors-and-panics). |

## Controls and camera

| Question or symptom | Answer |
|---|---|
| Mouse look does nothing | Click the window to capture the cursor. Mouse look also requires window focus. See [Current capture and focus](dev/controls.md#current-capture-and-focus-v004). |
| The cursor is stuck or hidden | Press `Escape` to release it. A failed grab logs a warning. See [Current capture and focus](dev/controls.md#current-capture-and-focus-v004). |
| WASD does nothing | The window must be focused. WASD walks the player; capture is not required for movement. See [Controls](dev/controls.md#bindings-by-milestone). |
| Why does `Space` jump instead of toggling a voxel? | v0.0.4 replaced the demo toggle; `Space` is jump. See [Controls](dev/controls.md#bindings-by-milestone). |
| Can I jump or fall? | Yes. Gravity, jumping, and swept collision are implemented. |
| Can I break or place blocks? | Yes. The first left click captures; later left clicks break and right clicks place. See [Interaction](dev/controls.md#interaction-current-v004). |
| Can I remap keys? | Not yet. Bindings are hardcoded physical key codes. An action map is planned with no milestone. See [Keybindings](dev/controls.md#keybindings). |
| How fast does the player walk, and how sensitive is the mouse? | Walk speed is `4.5 m/s`, jump velocity `8 m/s`, and mouse sensitivity `0.0025 rad/px`. See [Tuning](dev/controls.md#tuning). |
| Is there a crosshair, HUD, hotbar, or pause menu? | No UI. Number keys select the block and the targeted block is outlined. `Escape` only releases the cursor. See [Controls](dev/controls.md) and [Not implemented](dev/debugging.md#not-implemented). |
| Why did input stop when I alt-tabbed? | Losing focus releases capture and clears held keys by design. See [Current capture and focus](dev/controls.md#current-capture-and-focus-v004). |

## Rendering, shaders, and textures

| Question or symptom | Answer |
|---|---|
| The window opens but the screen is black | Check the adapter/backend log, pipeline validation, vertex count, and depth state. See [Rendering failures](dev/testing.md#rendering-failures) and [Shader debugging](dev/shaders.md#debug). |
| Startup fails with a shader or pipeline validation error | The error is `RendererError::Validation` and names the problem. See [Validate shaders](dev/shaders.md#validate) and [Errors and panics](dev/debugging.md#errors-and-panics). |
| Colours look washed out or too dark | The shader works in linear colour; the surface format usually encodes sRGB. Do not add gamma by hand. See [Coordinates, depth, and colour](dev/shaders.md#coordinates-depth-and-color). |
| Faces are missing or inside-out | Check winding, front face, and cull mode. See [Shader debugging](dev/shaders.md#debug). |
| Resizing or minimising breaks the image | The renderer reconfigures the surface and clamps zero-sized dimensions. See [Rendering resources](dev/architecture.md#rendering-resources). |
| Why do blocks show noise patterns? | The current tiles are deterministic procedural placeholders, not final art. See [Texture pipeline](dev/textures.md). |
| Where are the game textures or assets? | There are no image assets yet. v0.0.4 generates eight procedural tiles; `docs/assets/` is documentation-site material. See [Current texture state](dev/textures.md#current-state). |
| Can I use my own textures or a texture pack? | Not yet. The external asset pipeline and texture-pack format are deferred. See [External assets](dev/textures.md#external-assets-and-texture-packs). |
| Why is there no lighting, fog, or water? | They are planned for v0.0.6. See the [Roadmap](roadmap.md) and [v0.0.6 spec](specs/v0.0.6.md). |
| How do I add or change a shader? | Follow [Shader authoring](dev/shaders.md). A shader edit needs a rebuild; there is no hot reload. |
| Can I disable vsync? | Press `V` to toggle between FIFO and immediate or mailbox presentation. See [Controls](dev/controls.md#bindings-by-milestone). |
| Can I see the frame rate? | Press `F3` to toggle the FPS/frame-time overlay. It refreshes once per second. See [Debugging](dev/debugging.md#profiling). |
| Can I inspect a frame in RenderDoc, Nsight, or PIX? | Yes, as external frame debuggers; the engine has no capture integration. See [External tools](dev/debugging.md#external-tools). |

## Performance and profiling

| Question or symptom | Answer |
|---|---|
| How do I measure FPS and frame time? | Run with `RUST_LOG=engine=debug` and read `fps` and `frame_ms`. See [Logging](dev/debugging.md#logging). |
| Why is performance low? | The world uses one draw per visible chunk, greedy meshes, two LODs, frustum culling, and asynchronous remeshing. Missing are occlusion culling, streaming, lighting, and terrain generation. See [Current limitations](dev/architecture.md#current-limitations) and the [Roadmap](roadmap.md). |
| Is Tracy integrated? | Yes, behind the `tracy` feature: `cargo run --features tracy` adds CPU spans, memory events, GPU timer zones, and metric plots. See [Profiling](dev/profiling.md). |
| Why are there no GPU timings? | `wgpu-profiler` and timestamp queries are not integrated. See [Profiling](dev/profiling.md). |
| What is `pool_sim`? | A benchmark that compares rayon with micropool on the v0.0.5 async-meshing frame loop. It is a decision aid, not a CI gate. See [Current aids](dev/debugging.md#current-aids) and [Benchmark results](dev/benchmarks.md). |
| Why does the window keep redrawing? | Continuous redraw runs even while unfocused so profilers keep receiving frames; only occlusion pauses it. Input stays focus-gated. See [Frame flow](dev/architecture.md#frame-flow). |
| Should I profile a debug or release build? | Use dev for symbols and release for realistic frame times. See [Profiling](dev/profiling.md). |
| A frame is stuttering | There is no async meshing or LOD yet, and frame delta is clamped to `100 ms`. See [Architecture](dev/architecture.md) and [Profiling](dev/profiling.md). |

## World, saves, and files

| Question or symptom | Answer |
|---|---|
| Where are my saves? | Under `world/chunks/`, one `VCHK` file per non-empty chunk. v0.0.7 adds `world/seed` and modified-only saves. See [World management](dev/world-management.md). |
| Closing the window lost my edits | The engine saves on a clean window close and on `Ctrl+S`. A crash or forced kill loses edits since the last save. See [World management](dev/world-management.md). |
| How do I back up, restore, recover, or reset a world? | See [Backups](dev/world-management.md#backups), [Restore](dev/world-management.md#restore), [Corruption recovery](dev/world-management.md#corruption-recovery), and [Reset](dev/world-management.md#reset). |
| Why is `world/` missing? | It is created on the first save. A missing `world/chunks/` on startup seeds the demo world. See [World directory](dev/world-management.md#world-directory). |
| What is `world/seed`? | A v0.0.7 file containing one decimal `u64`, used to regenerate unmodified chunks. See [World directory](dev/world-management.md#world-directory). |
| Why is `world/` ignored by Git? | It is local save data, not source. Back it up outside the repository. See [Paths](dev/configuration.md#paths) and [World management](dev/world-management.md). |
| Can I import or export a world? | No importer or exporter exists. The format is `VCHK` v1, one file per saved chunk. See [World management](dev/world-management.md) and [ADR-0008](decisions/0008-chunk-save-format.md). |
| Where is `config.toml`? | It is not read yet. Current inputs are environment variables and compile-time constants. See [Configuration](dev/configuration.md). |
| Where are the logs? | They go to stderr today. A per-run `logs/` file is planned with no milestone. See [Logging](dev/debugging.md#logging). |
| What is the stress scene? | `ENGINE_STRESS_SCENE=1` loads a deterministic 64-chunk in-memory scene that bypasses `world/chunks/` and saving. See [Configuration](dev/configuration.md) and the [v0.0.5 spec](specs/v0.0.5.md). |
| Can I choose the world seed? | v0.0.7 will read `world/seed`; see [Choose a seed](dev/world-management.md#choose-a-seed). |
| Can I change the render distance? | It is planned as `world.render_distance_chunks` in v0.0.7. See [Configuration](dev/configuration.md). |

## Docs, CI, and contributing

| Question or symptom | Answer |
|---|---|
| My page does not appear on the documentation site | Add it to `SUMMARY.md`. A page outside the summary is not published. See [Documentation conventions](README.md#conventions). |
| A documentation link is broken | Use relative links such as `dev/setup.md`, not `/docs/dev/setup.md`. See [Documentation conventions](README.md#conventions). |
| Does CI check documentation links? | No. It builds mdBook but does not run a link checker. See [CI](dev/ci.md#ci-workflow). |
| Which mdBook and typos versions does CI use? | mdBook `0.5.4` and typos `1.52.2`. See [CI](dev/ci.md#docs-workflow). |
| Why does CI not use `--all-features`? | The optional `tracy` feature builds a C++ client and stays local, so CI uses explicit commands. See [CI](dev/ci.md#ci-workflow) and [Profiling](dev/profiling.md#constraints). |
| Why are benchmarks, GPU tests, coverage, and link checks not in CI? | They are current CI gaps. See [CI](dev/ci.md). |
| My commit was rejected | Commits follow [Conventional Commits](https://www.conventionalcommits.org). See [Commits](CONTRIBUTING.md#commits). |
| CI says the changelog is missing | `feat`, `fix`, `perf`, `refactor`, and `revert` commits need an `[Unreleased]` entry. See [Changelog](CONTRIBUTING.md#changelog) and [Commits workflow](dev/ci.md#commits-workflow). |
| Do I need an ADR? | Architectural changes do. Start from the [decision index](decisions/index.md#conventions) and the template. |
| Do I need to sign the CLA? | Yes. See [Licensing](CONTRIBUTING.md#licensing) and [`CLA.md`](CLA.md). |
| Is there a code of conduct? | Yes: [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md). |
| How do I report a bug? | Use the [bug report form](https://github.com/Valescent/engine/issues/new/choose). Include the commit, Rust version, OS/GPU/driver, reproduction, and logs. |
| How do I report a security issue? | Do not open a public issue. Use private vulnerability reporting as described in [`SECURITY.md`](SECURITY.md). |
| How do I request a feature? | Check the roadmap's non-goals first, then use the [feature request form](https://github.com/Valescent/engine/issues/new/choose). See [Contributing](CONTRIBUTING.md). |

## Troubleshooting index

| Symptom | Start here |
|---|---|
| `cargo run` fails before the window opens | [Platform prerequisites](dev/setup.md#platform-prerequisites), [Toolchain](dev/setup.md#toolchain) |
| No window, no adapter, or a driver error | [Graphics backend](dev/setup.md#graphics-backend), [Rendering failures](dev/testing.md#rendering-failures) |
| Window opens but the scene is black | [Rendering failures](dev/testing.md#rendering-failures), [Shader debugging](dev/shaders.md#debug) |
| Startup fails with a validation error | [Validate shaders](dev/shaders.md#validate), [Errors and panics](dev/debugging.md#errors-and-panics) |
| Mouse look does not work | [Current capture and focus](dev/controls.md#current-capture-and-focus-v002) |
| Cursor is stuck or hidden | [Current capture and focus](dev/controls.md#current-capture-and-focus-v002) |
| WASD or `Space` does something unexpected | [Bindings by milestone](dev/controls.md#bindings-by-milestone) |
| Low FPS, stutter, or no frame-time log | [Logging](dev/debugging.md#logging), [Profiling](dev/profiling.md) |
| Tracy or GPU timings are missing | [Profiling](dev/profiling.md#run-tracy) |
| Texture, asset, or shader question | [Texture pipeline](dev/textures.md), [Shader authoring](dev/shaders.md) |
| Save, backup, corruption, or reset question | [World management](dev/world-management.md) |
| `world/` or `world/seed` is missing | [World directory](dev/world-management.md#world-directory) |
| Documentation page is missing from the site | [`SUMMARY.md`](SUMMARY.md), [Documentation conventions](README.md#conventions) |
| mdBook, typos, or CI check fails | [Local equivalents](dev/ci.md#local-equivalents), [Docs workflow](dev/ci.md#docs-workflow) |
| Commit or changelog check fails | [Commits](CONTRIBUTING.md#commits), [Commits workflow](dev/ci.md#commits-workflow) |
| Bug or security report | [Bug form](https://github.com/Valescent/engine/issues/new/choose), [`SECURITY.md`](SECURITY.md) |
| Feature request | [Roadmap](roadmap.md), [Contributing](CONTRIBUTING.md) |

## Reporting a problem

1. Check the linked page above first; many "bugs" are milestone boundaries or expected
   placeholders.
2. Reproduce the problem with a clean `cargo run` and note the exact steps.
3. Capture logs with `RUST_LOG=engine=debug,wgpu=warn`. Add `RUST_BACKTRACE=1` for a panic.
4. Collect the commit (`git rev-parse HEAD`), `rustc --version`, OS, GPU, driver version, window
   size, and any validation error text.
5. Use the bug report form. A reproduction matters more than a tidy description.
6. Report security issues privately through [`SECURITY.md`](SECURITY.md), never in a public issue.

## Source pages

- [Setup](dev/setup.md)
- [Controls](dev/controls.md)
- [Debugging](dev/debugging.md)
- [Testing](dev/testing.md)
- [Profiling](dev/profiling.md)
- [Shader authoring](dev/shaders.md)
- [Texture pipeline](dev/textures.md)
- [World management](dev/world-management.md)
- [Configuration](dev/configuration.md)
- [CI](dev/ci.md)
- [Roadmap](roadmap.md)
- [Decision index](decisions/index.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)
- [Code of conduct](CODE_OF_CONDUCT.md)
- [Contributor licence agreement](CLA.md)

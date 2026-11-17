# Configuration

The project has two configuration layers: runtime inputs for the engine and repository tooling
files for builds, checks, docs, and releases. Runtime settings are not read from a file yet.

| Area | Status |
|---|---|
| Runtime `config.toml` | Planned, no milestone |
| Remappable bindings | Planned, no milestone |
| Render distance | Planned (v0.0.7) |
| Environment variables | Current |
| Compile-time constants | Current |
| Repository tooling | Current |

## Runtime

### Current inputs

- No runtime config file is read.
- No command-line flags are parsed; `src/main.rs` calls `engine::run()` directly.
- Current settings are environment variables and compile-time constants.

#### Environment variables

| Variable | Read by | Effect |
|---|---|---|
| `RUST_LOG` | Engine (`tracing-subscriber` `EnvFilter`) | Sets the log filter. Unset or invalid falls back to `info`. Examples: `engine=debug`, `engine=debug,wgpu=warn`. |
| `ENGINE_STRESS_SCENE` | Engine (`run`) | `1` builds the deterministic 8x8 stress scene in memory and disables world load and save. Any other value or unset uses `world/chunks/`. |
| `WGPU_BACKEND` | `wgpu` | Forces a backend such as `dx12`, `vulkan`, or `metal`, depending on the platform. Unset lets `wgpu` choose. The engine does not parse it. |

See [Testing](testing.md) for manual-run examples, [Setup](setup.md) for backend selection, and
[Debugging](debugging.md) for log workflows and the planned log file.

The stress scene is selected with `ENGINE_STRESS_SCENE=1`; it never reads or writes `world/chunks/`.
See the [v0.0.5 spec](../specs/v0.0.5.md).

#### Compile-time constants

| Setting | Location | Value |
|---|---|---|
| Package version | `Cargo.toml` | Embedded in the window title. |
| Window title | `src/lib.rs` | `Valescent Engine <package version>`. |
| Initial window size | `src/lib.rs` | `1280 × 720` logical pixels. |
| Frame delta clamp | `src/lib.rs` | `100 ms`. |
| Frame log interval | `src/lib.rs` | `1 s`. |
| Camera tuning | `src/camera.rs` | Field of view, near/far planes, speed, and sensitivity. See [Controls](controls.md) and [Coordinates](coordinates.md). |

Changing one of these requires a code change and rebuild. They are not user settings.

### Planned config file

No `config.toml` is read today. The decided shape is:

- **Path:** `config.toml` in the process working directory, next to the planned `world/` data
  directory. This is the engine's file, not Cargo's `.cargo/config.toml`.
- **Format:** TOML, read once at startup.
- **Defaults:** a missing file uses the built-in defaults.
- **Precedence:** built-in defaults, then `config.toml`. There are no environment or
  command-line overrides for file settings. `RUST_LOG` and `WGPU_BACKEND` remain separate
  diagnostic inputs.
- **Validation:** unknown keys and invalid values are rejected with a typed error naming the
  setting.
- **Settings:** the remappable action map (no milestone) and render distance in chunks
  (v0.0.7).
- **v0.0.7 key:** `world.render_distance_chunks = 12`, range `8..=16`, default `12` when
  missing. See the [v0.0.7 spec](../specs/v0.0.7.md).
- **Schema:** the exact keys and nesting are not fixed. The file will not configure logging,
  the GPU backend, the window, camera tuning, or frame timing; those stay environment
  variables or compile-time constants.

No separate milestone is assigned to the config file itself.

### Paths

| Path | Status |
|---|---|
| `config.toml` | Planned; working directory. |
| `world/` | Current (v0.0.4); working directory. |
| `world/seed` | Planned (v0.0.7); decimal world seed. |
| `logs/` | Planned; working directory; per-run log files. |

Both are resolved against the process working directory. `.gitignore` ignores `/target`,
`/.idea`, `/book`, `/logs`, and `/world`. See [World management](world-management.md) for backups,
corruption recovery, and reset.

## Repository tooling

These files configure the repository, not the running engine.

### Toolchain and build

| File | Purpose |
|---|---|
| `rust-toolchain.toml` | Pins Rust `1.99.1`, includes clippy and rustfmt, and uses the minimal profile. |
| `Cargo.toml` | Package metadata, dependencies, lint levels, profiles, and the `pool_sim` bench. The MSRV is `1.99.1`. |
| `Cargo.lock` | Committed lockfile for reproducible builds. |
| `clippy.toml` | Sets the Clippy MSRV to `1.99.1`. |
| `deny.toml` | cargo-deny policy for licences, advisories, duplicate crates, wildcards, and sources. |

Cargo profiles:

| Profile | Settings |
|---|---|
| `dev` | `opt-level = 1`; dependencies use `opt-level = 3`. |
| `release` | Thin LTO, one codegen unit, and `line-tables-only` debug info. |

Lints:

- Rust: `unsafe_code` is denied; `unreachable_pub` and `missing_debug_implementations` are
  warned.
- Clippy: `all` and `pedantic` are warned, with allowances for module name repetitions,
  missing `must_use` candidates, and missing error and panic docs. The Clippy job turns
  warnings into errors.
- No optional Cargo features are defined today. `--all-features` currently selects nothing. The
  planned Tracy feature in [Debugging](debugging.md) is not defined yet.

cargo-deny allows the licences listed in `deny.toml`, ignores the private PolyForm Shield
licence, denies wildcard requirements and unknown registries or Git sources, and warns on
duplicate crate versions.

### Formatting and spelling

| File | Configuration |
|---|---|
| `rustfmt.toml` | Edition 2024, 100-column maximum, Unix newlines, field-init and try shorthand. |
| `.editorconfig` | UTF-8, LF, final newline, trimmed trailing whitespace, four-space indent. Markdown and YAML/JSON use two spaces; Markdown keeps trailing whitespace. |
| `.gitattributes` | LF normalization, Rust and WGSL diff drivers, and binary asset types. |
| `_typos.toml` | `// spellchecker:disable-line` support, project names such as `Valescent`, `wgpu`, and `winit`, and the deliberate mdBook CSS variable misspelling. |

### Documentation and releases

| File | Purpose |
|---|---|
| `book.toml` | mdBook source (`docs/`), build directory (`book/`), site URL, themes, custom CSS and JS, and repository and edit links. |
| `cog.toml` | Cocogitto: releases from `main`, merge commits ignored, `v` tag prefix, and changelog omission for `style`, `test`, `build`, `ci`, and `chore`. |
| `.github/workflows/ci.yml` | Format, Clippy, test, docs, cargo-deny, and typos checks across the CI matrix. |
| `.github/workflows/docs.yml` | Builds with mdBook `0.5.4` and deploys to GitHub Pages. |
| `.github/workflows/commits.yml` | Conventional Commit check and the changelog-entry gate for user-facing commits. |
| `.github/workflows/release.yml` | Tag-triggered or manual release. Manual runs call `scripts/release.sh`; tag pushes publish the GitHub release. |
| `.github/dependabot.yml` | Weekly grouped Cargo and GitHub Actions updates. |

See [CI](ci.md) for the full workflow map, local equivalents, and current gaps.

`docs/SUMMARY.md` defines the documentation navigation. A page that is not listed there is
not published.

CI details:

- The test matrix runs on Ubuntu, Windows, and macOS.
- Formatting and documentation run only on Ubuntu.
- Clippy runs with `--all-targets --all-features --locked` and `-D warnings`.
- Tests run with `--all-features --locked`.
- `RUSTDOCFLAGS=-D warnings` fails documentation builds on warnings.
- Typos uses `crate-ci/typos@v1.52.2`; cargo-deny uses
  `EmbarkStudios/cargo-deny-action@v2`.
- Docs CI currently pins mdBook `0.5.4`. Install the same version locally when checking the
  site.
- The Commits workflow requires a `CHANGELOG.md` change when a PR or push contains `feat`,
  `fix`, `perf`, `refactor`, or `revert` commits.
- The Release workflow checks that the `vX.Y.Z` tag matches the `Cargo.toml` version.

See [Release checklist](release-checklist.md) for the full release procedure.

# CI

GitHub Actions workflows for checks, documentation, commits, and releases. Branch protection
and required checks are repository settings outside this repository; this page does not assert
them.

| Area | Status |
|---|---|
| `CI` workflow | Current |
| `Docs` workflow | Current |
| `Commits` workflow | Current |
| `Release` workflow | Current |
| `Perf` workflow | Current; headless and blend calibration run on `main` and nightly |
| Dependabot | Current |
| Release binaries | Not built |
| Coverage, GPU tests, pool benchmark, link checks | Not covered |

## Workflow map

| Workflow | File | Triggers | Jobs |
|---|---|---|---|
| `CI` | `.github/workflows/ci.yml` | Push to `main`, pull request | Test matrix, dependencies, spelling, terrain gates |
| `Docs` | `.github/workflows/docs.yml` | Push to `main`, pull request, manual dispatch | mdBook build, GitHub Pages deploy |
| `Commits` | `.github/workflows/commits.yml` | Push to `main`, pull request | Conventional Commits, changelog gate |
| `Release` | `.github/workflows/release.yml` | `v*` tag push, manual dispatch | Version bump, GitHub release |
| `Perf` | `.github/workflows/perf.yml` | Push to `main`, nightly schedule, manual dispatch | Headless static and walking runs, budget gate, blend calibration |
| Dependabot | `.github/dependabot.yml` | Weekly schedule | Grouped Cargo and Actions updates |

`Docs` builds on pull requests but deploys only on non-pull-request events. `Release` does not
run on pull requests. `Perf` does not run on pull requests; it runs on `main` pushes and on a
nightly schedule (03:17 UTC).

## CI workflow

`CI` declares `contents: read` permissions, sets `CARGO_TERM_COLOR=always`, and cancels a
superseded run in the same ref group. The `test` job uses `fail-fast: false` so every operating
system reports a result.

| OS | Format | Clippy | Test | Rustdoc |
|---|---|---|---|---|
| Ubuntu | Yes | Yes | Yes | Yes |
| Windows | No | Yes | Yes | No |
| macOS | No | Yes | Yes | No |

Each matrix entry:

1. Checks out the repository.
2. Runs `rustup show`, which picks up `rust-toolchain.toml`.
3. Restores the `Swatinem/rust-cache@v2` cache.
4. Runs the platform-specific checks:

```console
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

Formatting and rustdoc run only on Ubuntu. Clippy and tests run on all three systems. Clippy
treats warnings as errors; tests do not.

The `dependencies` job runs `EmbarkStudios/cargo-deny-action@v2`. The policy in `deny.toml`
covers advisories, licences, duplicate and wildcard bans, and allowed sources.

The `spelling` job runs `crate-ci/typos@v1.52.2`.

## Docs workflow

`Docs` declares `contents: read`, `pages: write`, and `id-token: write`. The `pages` concurrency
group does not cancel an in-progress deployment.

- **Build:** installs mdBook `0.5.4` with `peaceiris/actions-mdbook@v2`, runs `mdbook build`, and
  uploads `book` as the Pages artifact.
- **Deploy:** runs only for non-pull-request events, uses the `github-pages` environment, and
  deploys with `actions/deploy-pages@v5`.

Install mdBook `0.5.4` locally to match the CI build. A page that is not in `docs/SUMMARY.md`
is not published.

## Commits workflow

`Commits` declares `contents: read` and cancels superseded runs.

- **Conventional Commits:** `cocogitto/cocogitto-action@v4` runs `check` on the PR head or the
  pushed commit range.
- **Changelog gate:** compares the base to `HEAD`. If the range contains a `feat`, `fix`,
  `perf`, `refactor`, or `revert` commit, `CHANGELOG.md` must also change. Otherwise the job
  passes.

The changelog job checks that the file changed, not that the entry is relevant or well written.

## Release workflow

`Release` declares `contents: write` and uses a single `release` concurrency group without
cancellation. It has two jobs:

- **Bump:** runs only for manual dispatches. It checks out `main`, configures the GitHub Actions
  bot identity, and runs `scripts/release.sh <version> [--dry-run]`. The script rolls
  `[Unreleased]`, bumps `Cargo.toml`, refreshes `Cargo.lock`, commits
  `chore(release): vX.Y.Z`, and tags and pushes unless it is a dry run.
- **Publish:** runs for tag pushes or a successful non-dry-run manual dispatch. It checks out
  the tag, verifies that `vX.Y.Z` matches the `Cargo.toml` version, extracts the matching
  `CHANGELOG.md` section, fails if that section is empty, and creates the GitHub release with
  `gh release create --verify-tag`.

Manual runs take a required `version` input and a `dry_run` boolean. A dry run commits the
version bump but does not tag, push, or publish.

The workflow builds no binaries and attaches no artifacts. A GitHub release contains the
changelog notes only. See [Versioning](versioning.md) for the version and tag policy, and
[Release checklist](release-checklist.md) for the full procedure.

## Perf workflow

`Perf` declares `contents: read`, cancels superseded runs in the same ref group, and runs on
`ubuntu-latest` with a 20-minute timeout. It runs on `main` pushes, a nightly `17 3 * * *`
schedule, and manual dispatch; it never runs on pull requests. It builds with
`cargo build --release --locked`, then runs the headless harness twice with `ENGINE_HEADLESS=1`,
`ENGINE_HEADLESS_FRAMES=600`, and `ENGINE_SEED=42`:

- a static camera at distance 12 (`ENGINE_HEADLESS_SPEED=0`);
- a walk at 32 m/s (`ENGINE_HEADLESS_SPEED=32`).

Each `HEADLESS` summary line is appended to the job summary, and the budget step gates on 625
ready columns, mean frame work under 20 ms, p95 under 50 ms, and resident blocks and light under
200 MB. Both raw outputs are uploaded as the `headless-results` artifact. The budgets are
deliberately generous because GitHub runners are shared; they catch order-of-magnitude
regressions, not small drift. See [Benchmark results](benchmarks.md#headless-run-v008).

The job also runs `terrain-gen calibrate-blend --seeds 16`, appends the table to the job summary,
and uploads it with the headless outputs. It is deliberately **not** in the PR `Terrain gates`
job: the measured medians sit near the `20..=28` window and pair 2-3 has 36 transects with p90
63.32 against the 64-block ceiling, so a platform-level floating-point difference could fail a
pull request that did not change the blend. Running it nightly on one known Linux runner keeps
the data-change signal without adding PR noise. The per-PR coverage floor is still enforced by
`biome_coverage_holds_for_pr_seeds` over the same 16 seeds.

## Dependency updates

Dependabot runs weekly for Cargo and GitHub Actions, both rooted at `/`. Updates are grouped
with patterns `*` and use a `chore` commit prefix with scope. Dependabot pull requests are
checked by `CI`, `Commits`, and `Docs`; the changelog gate ignores `chore` commits.

Workflows reference actions by version tags such as `@v7`, `@v2`, and `@v1.52.2`, not by commit
SHA. Dependabot updates those tags.

## Terrain gates

The `Terrain gates` job runs the headless generator on `ubuntu-latest` after the test job. It is
the only job that treats generated block output as an assertion:

```console
cargo tree -p terrain-core -e normal --no-default-features
cargo run --locked --quiet --no-default-features --bin terrain-gen -- golden check
cargo run --locked --quiet --no-default-features --bin terrain-gen -- seams --window 5x5 --seeds 3
cargo run --locked --quiet --no-default-features --bin terrain-gen -- sweep --tier pr
cargo run --locked --quiet --no-default-features --bin terrain-gen -- counters --tier pr
```

The crate-graph step greps the normal dependency tree of `terrain-core` and fails with a clear
message when it resolves `wgpu`, `winit`, `wgpu-profiler`, or `tracy-client`. Because
`terrain-gen` is a binary of that crate, it inherits the same guarantee: no window, GPU, audio, or
renderer code.

The job also runs `gen --threads 1 --order forward` against `--threads 4 --order shuffled` and
fails when the two digests differ. Goldens change only through `golden update`, which is
deliberate and must be accompanied by a generator version bump; a pull request that moves a golden
without one fails review, not CI. The golden and counter baselines live under
`crates/terrain-core/tests/golden/`.

`counters --tier pr` compares the deterministic counter snapshot against
`crates/terrain-core/tests/golden/terrain-counters.json` and fails when any counter drifts by more
than 5%; a baseline of zero must stay zero. The checked-in baseline is rewritten only with
`counters --tier pr --update`, and the commit must say why. The selected counters, the excluded
wall-clock counters, and the comparison rule are defined in
[Terrain tools](terrain-tools.md#counters).

### Size budgets

The `Terrain gates` job also runs the file-size checker, which scans tracked `*.rs` files and
ignores `target/`, `book/`, and `world/`:

```console
sh scripts/size-budgets.sh
```

The thresholds come from [Modular structure](../design/modular-structure.md#56-size-budgets)
§5.6. Every file over **1,000 lines** is printed as a GitHub warning annotation
(`::warning file=...,line=1::`), and files over **600 lines** are counted as the advisory tier.
The script always exits 0 on findings, and the CI step sets `continue-on-error: true`: §8 records
the decision that size budgets are warnings, not failures, until the refactor brings the tree
under the limits. Remove `continue-on-error` and make the checker exit non-zero once no tracked
Rust file exceeds 1,000 lines.

On 2026-11-20 the merged tree has **21 files over 1,000 lines** and **29 files over 600 lines**
out of 77 tracked Rust files (the 600-line count includes the 1,000-line offenders; 8 files sit
between 601 and 1,000). The five largest are:

| Lines | File |
|---:|---|
| 6,896 | `src/lib.rs` |
| 2,688 | `crates/terrain-core/src/mesher2.rs` |
| 2,681 | `src/renderer.rs` |
| 2,129 | `crates/terrain-core/src/light2.rs` |
| 2,089 | `crates/terrain-core/src/pipeline.rs` |

The `terrain-gen` tool now lives under `crates/terrain-core/src/bin/terrain-gen/`, one module
per subcommand; its largest module is 969 lines, so the split removed the last 4,000-line
offender from this list.

`sh scripts/size-budgets.test.sh` builds a throwaway Git work tree with exactly 600-line and
1,001-line fixtures and asserts the annotation, both counts, the 600-line boundary, and the zero
exit. It runs as a separate, blocking step before the checker, because a broken checker should
fail even while its findings stay advisory.

`calibrate-blend` is not part of this PR job: it passes at 16 seeds against the shipped generator
version 4, but the pair 2-3 margin is too thin to gate every pull request (p90 57.68 against 64,
36 transects). The `Perf` workflow runs it nightly instead; the per-PR guard is the
`biome_coverage_holds_for_pr_seeds` test, which samples all 16 PR seeds. See
[Terrain tools](terrain-tools.md#current-margins-16-seeds) and
[Benchmark results](benchmarks.md#terrain-blend-calibration-2026-11-20). Run the calibration
locally after any change to climate fields, `BLEND_W`, or biome centres:

```console
cargo run --locked --quiet --no-default-features --bin terrain-gen -- calibrate-blend --seeds 16
```

## Local equivalents

Run the same checks locally:

```console
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
cargo tree -p terrain-core -e normal --no-default-features
typos
mdbook build
```

Rustdoc needs `RUSTDOCFLAGS`:

```console
# POSIX
RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
```

```console
# PowerShell
$env:RUSTDOCFLAGS = "-D warnings"; cargo doc --no-deps
```

`cargo deny check` reproduces the dependency job when cargo-deny is installed. CI uses typos
`1.52.2` and mdBook `0.5.4`; use the same versions for parity. The formatting and rustdoc steps
are platform-independent even though CI runs them only on Ubuntu.

The default test suite is headless, so no GPU is needed. Manual rendering checks remain in
[Testing](testing.md). The [Release checklist](release-checklist.md) runs the same fmt, Clippy,
test, rustdoc, typos, and mdBook checks before a release.

## Conventions

- Declare explicit workflow permissions: read-only for `CI` and `Commits`, `contents: write`
  for `Release`, and Pages permissions for `Docs`.
- Cancel superseded `CI`, `Commits`, and `Perf` runs. Do not cancel `Docs` deployments or
  `Release` runs.
- Use `--locked` for Clippy and tests. Pass `--no-default-features`: `tracy` is a default
  feature but builds a C++ client and stays local (ADR-0006).
- Reference actions by version tags and let Dependabot update them.
- Do not add `pool_sim` to CI. It is a local decision aid. Record local runs in
  [Benchmark results](benchmarks.md).
- Keep GPU tests opt-in and out of the default CI run.

## Not covered

| Area | Status |
|---|---|
| Release binaries | No build or upload step; releases contain changelog notes only. |
| GPU and window tests | Not run; rendering is checked manually. |
| Benchmarks | `pool_sim` is not run in CI. `Perf` runs the headless harness, not `pool_sim`. |
| Public API diff or semver check | Not run; the API is unstable and unpublished. See [Public API](public-api.md). |
| Coverage | Not measured. |
| mdBook link checking | Not run. |
| Shell script linting | Not run. |
| Workflow linting | No actionlint job. |
| Separate MSRV job | Not run; CI uses the pinned `1.99.1` toolchain. |
| Formatting and rustdoc on Windows or macOS | Not run; both steps are Ubuntu-only. |
| Code scanning | No CodeQL or similar; `cargo-deny` covers dependency policy. |

Branch protection, required checks, merge queue, and repository rulesets are GitHub settings.
They are not stored in this repository, and this page does not assert them.

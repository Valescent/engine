# Valescent Engine documentation

Source of the documentation site and the folder index on GitHub.

The site is built with mdBook. `book.toml` configures it, `SUMMARY.md` defines the navigation, and a page that is not listed there does not appear on the site. Push to `main` to deploy, or run `mdbook build` locally.

## Layout

| Path | Contents |
|---|---|
| `index.md` | Landing page for the site. |
| `roadmap.md` | Versioned milestones and their status. |
| `faq.md` | FAQ and symptom-first troubleshooting index. |
| `dev/` | Development setup, conventions, architecture, testing, release process, milestone scopes, and templates. |
| `specs/` | Normative technical contracts for milestones and formats. |
| `design/` | Long-range design directions and research notes. Not milestone contracts. |
| `decisions/` | Architecture decision records, indexed in [`decisions/index.md`](decisions/index.md). |
| `assets/` | Fonts and the client-side script copied into the built site. |
| `CONTRIBUTING.md`, `CODE_OF_CONDUCT.md`, `SECURITY.md`, `CLA.md` | Project policy. |

Add new top-level folders only when a real document needs them. Do not create empty placeholders.

## Source of truth

When a fact appears in more than one place, update its source of truth first, then any summaries.

| Topic | Source of truth |
|---|---|
| Milestone status and Definition of Done | [`roadmap.md`](roadmap.md) |
| Milestone deliverables and acceptance criteria | `dev/vX.Y.Z-scope.md` |
| Milestone technical contracts | `specs/vX.Y.Z.md` |
| Architecture, module ownership, and frame flow | [`dev/architecture.md`](dev/architecture.md) |
| WGSL shader conventions, interfaces, and validation | [`dev/shaders.md`](dev/shaders.md) |
| Texture pipeline, procedural tiles, sampler, and asset workflow | [`dev/textures.md`](dev/textures.md) |
| Coordinates, units, and coordinate conversions | [`dev/coordinates.md`](dev/coordinates.md) |
| Input bindings, cursor capture, and interaction rules | [`dev/controls.md`](dev/controls.md) |
| Runtime inputs, config file, and repository tooling | [`dev/configuration.md`](dev/configuration.md) |
| World directory, backups, corruption recovery, and reset | [`dev/world-management.md`](dev/world-management.md) |
| Logging, error reporting, and log files | [`dev/debugging.md`](dev/debugging.md) |
| Profiling setup, Tracy, and wgpu-profiler | [`dev/profiling.md`](dev/profiling.md) |
| Benchmark methodology | `benches/pool_sim.rs` |
| Benchmark runs and results | [`dev/benchmarks.md`](dev/benchmarks.md) |
| CI workflows, local checks, and CI gaps | [`dev/ci.md`](dev/ci.md) |
| Test strategy, commands, and manual GPU checks | [`dev/testing.md`](dev/testing.md) |
| Versioning policy: tags, changelog, and compatibility | [`dev/versioning.md`](dev/versioning.md) |
| Public API surface and reference gate | [`dev/public-api.md`](dev/public-api.md) |
| Terminology and preferred terms | [`dev/glossary.md`](dev/glossary.md) |
| Release procedure and verification | [`dev/release-checklist.md`](dev/release-checklist.md) |
| Architecture decisions and rejected alternatives | [`decisions/`](decisions/index.md) |
| Development environment and toolchain setup | [`dev/setup.md`](dev/setup.md) |
| Code, test, logging, and naming conventions | [`dev/conventions.md`](dev/conventions.md) |
| Milestone scope template and lifecycle | [`dev/milestone-template.md`](dev/milestone-template.md) |
| Documentation navigation | [`SUMMARY.md`](SUMMARY.md) |
| FAQ and troubleshooting entry points | [`faq.md`](faq.md) |
| Documentation conventions | this page |
| Project policy | [`CONTRIBUTING.md`](CONTRIBUTING.md), [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md), [`SECURITY.md`](SECURITY.md), [`CLA.md`](CLA.md) |
| Landing page copy | [`index.md`](index.md) |
| Package, dependency, and toolchain versions | `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, `clippy.toml` |
| User-visible release notes | `CHANGELOG.md` |
| Runtime behaviour | `src/` |
| CI execution | `.github/workflows/` |
| Site build, theme, and assets | `book.toml`, `theme/`, `docs/assets/` |

Scope pages expand roadmap entries but do not change their status. Specs define the technical
contract while the scope keeps deliverables and acceptance criteria. ADRs record decisions
rather than current behaviour. The executable source remains authoritative for what runs.

## Conventions

- **Links are relative.** Use `dev/setup.md`, not `/docs/dev/setup.md`, so the same link works on GitHub and in the book.
- **One title per page.** The `#` heading is the title; do not repeat the page name in a subtitle.
- **Roadmap stays flat.** Milestones live in one `roadmap.md`; per-version files are not worth the navigation cost until the file becomes unmanageable. A scope page may expand one milestone's deliverables, but the roadmap remains the status source of truth.

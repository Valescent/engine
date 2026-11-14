# Contributing

Thanks for looking. This is a pre-alpha engine, so the highest-value contributions right now are bug reports with reproductions, small focused fixes, and documentation corrections.

## Before you start

- **Open an issue first** for anything larger than a typo. It saves you from building something that conflicts with the [roadmap](docs/roadmap.md).
- **Architectural changes need an ADR.** Follow the process in [`docs/README.md`](docs/README.md): copy `docs/decisions/0000-template.md`, fill it in, and link it from the pull request.
- **Check the roadmap's Non-Goals** for the version you are touching. Deferred work is deferred on purpose.

## Setup

```console
git clone https://github.com/Valescent/engine
cd engine
git config core.hooksPath .githooks
cargo run
```

`rust-toolchain.toml` pins the compiler; `rustup` installs it automatically. Platform details are in [`docs/dev/setup.md`](docs/dev/setup.md).

The hook validates commit messages locally. It uses [cocogitto](https://docs.cocogitto.io) when installed and falls back to a built-in check otherwise:

```console
cargo install cocogitto --locked
```

## Commits

Commits follow [Conventional Commits](https://www.conventionalcommits.org). CI rejects anything else.

```
<type>(<scope>): <description>
```

Allowed types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`, `revert`.

- `feat:` and `fix:` drive version bumps; use them only for user-visible behaviour.
- Add a scope when it helps: `feat(voxel): ...`, `fix(render): ...`.
- Mark breaking changes with `!` after the type or scope, plus a `BREAKING CHANGE:` footer.
- Keep the subject imperative and under ~72 characters. Body wraps at 72.

## Changelog

`CHANGELOG.md` is written by hand, under `## [Unreleased]`, using [Keep a Changelog](https://keepachangelog.com) sections: Added, Changed, Deprecated, Removed, Fixed, Security.

CI requires a `CHANGELOG.md` change in any push or pull request containing `feat`, `fix`, `perf`, `refactor`, or `revert` commits. Write for a reader who uses the engine, not for a reader of the diff.

## Pull requests

- One topic per pull request; keep unrelated refactors out.
- CI must be green: format, clippy, tests, docs, dependency licences, spelling, and the commit checks.
- Fill in the pull request template. Link the issue it closes.
- Squash merges only. The pull request title becomes the commit message, so it must be conventional too.
- Review is best-effort. There is no response-time guarantee.

## Licensing

Contributions are accepted under the [PolyForm Shield License 1.0.0](LICENSE) and require accepting [`CLA.md`](CLA.md). The CLA preserves the project's ability to relicense later; it does not take ownership of your work. Until the signing bot is configured, a maintainer will ask you to confirm acceptance in the pull request.

## Code

Read [`docs/dev/conventions.md`](docs/dev/conventions.md) before writing engine code. In short: no `unsafe`, no panics in library paths, no global mutable state, and measure before optimizing.

# Versioning

Release versioning for the engine package. Save-format and Rust compiler support versions are
out of scope; see [Out of scope](#out-of-scope).

| Area | Status |
|---|---|
| Semantic Versioning 2.0.0 | Current |
| Keep a Changelog 1.1.0 | Current |
| `0.0.x` milestone releases | Decided policy |
| Four-part planning checkpoints | Internal only; not versions or tags |
| Prerelease and build suffixes | Not supported |
| Public API stability | None before 1.0 |
| `1.0.0` target | Not set |
| Save-format versioning | Out of scope |
| MSRV policy | Out of scope |

## Version source of truth

`Cargo.toml` is the package version source. The release script updates it and refreshes
`Cargo.lock`; the Release workflow rejects a tag that does not match the manifest version. The
window title embeds the version at compile time through `CARGO_PKG_VERSION`.

- Git tags are `vX.Y.Z`. The `v` prefix comes from `cog.toml`.
- `v0.0.1` through `v0.0.7` are the current release tags. `v0.0.8` is implemented and awaiting
  release.
- Release from the script rather than tagging by hand, so the tag and manifest stay in sync.
- The package has `publish = false`. No crates.io releases are part of the versioning scheme.
- Version numbers in the roadmap, scope pages, and README are updated manually during release;
  they are not derived from the manifest.

## Semantic Versioning

The project adheres to [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html).
Release versions use only three numeric components: `MAJOR.MINOR.PATCH`.

- The current line is `0.0.x`.
- Each roadmap milestone ships as the next `0.0.x` patch release.
- Breaking changes may occur in any `0.0.x` release.
- There is no `0.1.0` target yet. Stay on `0.0.x` until a separate decision changes the line.
- There is no `1.0.0` target. Completing the roadmap does not automatically trigger `1.0.0`;
  that requires a separate decision.

The release script accepts any three-component numeric version. The `0.0.x` policy is a review
convention, not an automated check.

## Prereleases

Prerelease and build-metadata versions are not supported. `scripts/release.sh` accepts only
`X.Y.Z`; it rejects versions such as `0.0.3-alpha`, `0.0.3-rc.1`, and `0.0.3+build`. Adding
prerelease support requires changing the release script and its version validation.

## Planning checkpoints

A release plan may split one release into reviewable checkpoints labelled with four components,
such as `0.0.10.1` and `0.0.10.2`; see the
[0.0.10–0.0.19 roadmap](roadmap-0.0.10-0.0.19.md). These are internal planning and review markers,
not versions.

- Releases remain `MAJOR.MINOR.PATCH`; the current line stays `0.0.x`.
- A four-part identifier is never a published version or a Git tag. Tags remain `vX.Y.Z`.
- Checkpoints do not change `Cargo.toml`, the release script, changelog headings, or Git tags.
- A release ships only when all of its checkpoints are complete or explicitly deferred in its
  scope page and the roadmap.

## Tags and release commits

- Tags use annotated tags created by `git tag -a`; the script does not sign them.
- The release commit is `chore(release): vX.Y.Z`.
- The script refuses to run with a dirty working tree or an existing tag.
- Treat tags as immutable. Do not move or force-push a release tag unless a maintainer
  explicitly decides to.
- Releases are cut from `main`. No maintenance branches or backport process are documented.

## Changelog

`CHANGELOG.md` uses [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/). It declares
Semantic Versioning and keeps `## [Unreleased]` at the top.

- User-facing changes are written under `[Unreleased]`, using the standard headings such as
  `Added`, `Changed`, `Fixed`, `Deprecated`, `Removed`, and `Security`.
- The changelog is written by hand. It is not generated from commit messages.
- On release, the script fails if `[Unreleased]` is empty. It writes
  `## [X.Y.Z] - YYYY-MM-DD` using the UTC date and leaves an empty `[Unreleased]` section above
  it.
- Release notes are extracted only from the matching version section, so entries should be
  written for users rather than as a commit list.
- CI requires a `CHANGELOG.md` change when a commit range contains `feat`, `fix`, `perf`,
  `refactor`, or `revert`. It checks that the file changed, not the entry's content.

See [CI](ci.md) for the changelog gate and [Release checklist](release-checklist.md) for the
release procedure.

## Compatibility

There is no stability promise before `1.0.0`. Public API, behaviour, and on-disk formats may
change in any `0.0.x` release. The changelog records user-visible changes; it does not imply that
older APIs or files remain supported.

The package is not published, so downstream crates have no published compatibility contract.
Treat the library API as internal until a later decision says otherwise. The current public
surface and the gate for a stable reference are in [Public API](public-api.md).

## Out of scope

- **Save formats.** The `VCHK` format has its own format-version field. That policy is
  separate from the package version; see the [v0.0.4 scope](v0.0.4-scope.md) and
  [ADR 0008](../decisions/0008-chunk-save-format.md).
- **MSRV.** The workspace pins Rust `1.99.1` in `rust-toolchain.toml`, `Cargo.toml`, and
  `clippy.toml`. No support window or versioning policy for older compilers is defined here; see
  [Configuration](configuration.md).
- **Documentation site.** The site is not versioned per release. It deploys from `main`.

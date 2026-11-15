# Release checklist

Releases are cut from `main` by `scripts/release.sh`, usually through the **Release** GitHub Actions workflow. The script rolls `[Unreleased]` into a version, bumps `Cargo.toml`, commits, tags, and pushes. The tag then publishes a GitHub release.

## Before the release

- [ ] The target version is decided.
- [ ] `main` is clean and synced with `origin/main`.
- [ ] CI, Commits, and Docs are green on the latest commit.
- [ ] `CHANGELOG.md` has the user-visible entries under `## [Unreleased]`, not under an older version.
- [ ] The roadmap marks the milestone complete.
- [ ] The matching `docs/dev/vX.Y.Z-scope.md` is marked complete.
- [ ] The README controls and status are current.
- [ ] No tag already exists for the target version.

Run the local checks:

```console
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-features --locked
cargo doc --no-deps --all-features
typos
mdbook build
```

## Cutting the release

From Git Bash or a POSIX shell:

```console
sh scripts/release.sh 0.0.3
```

Or run **Actions → Release → Run workflow** with the version. The script:

1. Requires a clean working tree.
2. Rejects an existing tag.
3. Reads the `[Unreleased]` entries.
4. Writes `## [X.Y.Z] - YYYY-MM-DD` into `CHANGELOG.md`.
5. Bumps `Cargo.toml` and refreshes `Cargo.lock`.
6. Commits `chore(release): vX.Y.Z`.
7. Creates the annotated tag and pushes `main` and the tag.

The tag push triggers the **Release** workflow. Its `Publish` job checks that the tag matches `Cargo.toml`, extracts the changelog section, and creates the GitHub release.

## Verify

- [ ] The tag points at the release commit.
- [ ] The Release workflow completed successfully.
- [ ] `gh release view vX.Y.Z` shows the right notes and is not a draft.
- [ ] The Docs workflow deployed.
- [ ] CI is green on the release commit.
- [ ] `main` is clean and synced.

## Troubleshooting

- **`[Unreleased]` has no entries** — the changelog entry was added under the previous release. Move it under `[Unreleased]`, commit, and rerun.
- **Tag already exists** — choose the next version. Do not move or force-push an existing tag unless a maintainer explicitly decides to.
- **`tag does not match Cargo.toml`** — the tag must match the manifest version; release from the script rather than tagging by hand.
- **Release workflow skipped** — the `publish` job must use `always()` because the dispatch-only bump job is skipped for tag pushes.
- **Docs did not deploy** — rerun the Docs workflow for the release commit.

## Notes

- `--dry-run` still creates the release commit; it only skips tagging, pushing, and publishing. Use it deliberately.
- Release notes come only from the matching `CHANGELOG.md` section, so write for users rather than for the diff.
- Do not commit generated `book/` output or local world saves.

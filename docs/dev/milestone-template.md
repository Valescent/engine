# Milestone scope template

Copy this into `docs/dev/vX.Y.Z-scope.md`, fill it in, link it from the roadmap section, and add it to `docs/SUMMARY.md`. Scope the milestone before writing code; update status and checkboxes as the work lands.

## Scoping questions

- What is the smallest demonstrable outcome that proves the milestone is done?
- What existing behaviour or architecture does this replace?
- What is explicitly deferred, and to which later milestone?
- Which decisions are durable enough to need an ADR?
- What can be tested headlessly?
- Which README, roadmap, changelog, and scope changes are part of the milestone?

## Template

```markdown
# vX.Y.Z scope: NAME

**Status:** Scoped; implementation not started.

**Goal:** One sentence describing the player-visible or engine-visible outcome.

This page expands the [vX.Y.Z roadmap entry](../roadmap.md) into concrete
deliverables. The roadmap remains the source of truth for milestone status and
the Definition of Done.

## Definition of Done

- Observable outcome one.
- Observable outcome two.
- Observable outcome three.

## In scope

- Concrete subsystem or behaviour.
- Concrete tests and documentation.
- Measurements or debug facilities needed to verify the milestone.

## Out of scope

- Work that belongs to a later milestone.
- Tempting refactors that are not required for the definition of done.

## Technical shape

### Data and ownership

- Which type owns what.
- Which existing structure is replaced.
- Invariants and coordinate conventions.

### Algorithms and flow

- The core algorithm or data flow.
- Edge cases such as negative coordinates, borders, empty data, or stale work.

### Rendering, storage, or generation

- GPU resource changes.
- File formats, versioning, and corruption behaviour.
- Worker-thread boundaries and main-thread responsibilities.

### Code shape

- New or changed modules.
- Public versus internal API.
- Where tests live.

## Implementation slices

1. [ ] First independently reviewable slice.
2. [ ] Second slice.
3. [ ] Tests, measurements, and edge cases.
4. [ ] Documentation, changelog, and acceptance run.

## Acceptance checklist

- [ ] The definition-of-done behaviour works manually.
- [ ] The edge cases have automated tests.
- [ ] `cargo fmt --all -- --check`, `cargo clippy --all-targets --locked -- -D warnings`,
      `cargo test --locked`, `cargo doc --no-deps`, `typos`, and `mdbook build` pass. If the
      change touches the `tracy` feature, also run the Clippy and test commands with
      `--features tracy`.
- [ ] The README and roadmap are current and `CHANGELOG.md` has the user-visible entries.

## Deliberate simplifications

| Simplification | Revisit when |
|---|---|
| Temporary shortcut | The later milestone or measured need arrives. |

## Relationship to later milestones

- What the next milestone builds on this.
- Anything this milestone intentionally leaves behind.
```

## Lifecycle

- **Scoped:** decisions and acceptance criteria are written; no implementation yet.
- **In progress:** mark the roadmap status `[~]` and check off slices as they land.
- **Implemented:** all slices and acceptance items are checked except the release tag.
- **Complete:** the release is tagged and published; the roadmap status is `[x]`.

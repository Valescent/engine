# Plan: 0.0.10.4 to 0.0.11

**Status:** Planning. `0.0.10.4`, `0.0.10.5`, `0.0.10.6`, and `0.0.11` are not implemented yet.
This page turns the remaining `0.0.10` scope and the `0.0.11` outline into review-sized pull
requests. It does not change the release policy: `0.0.10.4`, `0.0.10.5`, and `0.0.10.6` are internal
checkpoints; `v0.0.10` is the next three-part release.

**Baseline:** `0.0.10.1` items and save, `0.0.10.2` HUD and inventory, and `0.0.10.3` crafting
Phase 1 are on `main`. The package is still `0.0.9` and no `v0.0.10` tag exists. The current headless
suite is 536 engine and 411 terrain-core library tests.

**Progress:** 4.1, 4.2, and 4.3 are implemented. 5.1 onward remain.

See [v0.0.10 scope](v0.0.10-scope.md), [the 0.0.10-0.0.19 roadmap](roadmap-0.0.10-0.0.19.md), and
[the release roadmap](../roadmap.md) for the commitments this plan expands.

## Recommended sequencing change

Move the `VPLY` version 2 bump into `0.0.10.4`, not `0.0.10.6`.

Health, stamina, and the spawn point should survive quit and reopen as soon as they exist. A late
save bump would create a transit window where the new state is not persisted and would make the
last checkpoint a large format change. With the bump in `0.0.10.4`, `0.0.10.6` becomes save
hardening, the scripted acceptance run, documentation, and the release.

The `0.0.10.5` roadmap section also needs a rewrite. The UI layer and first HUD already landed in
`0.0.10.2`; the remaining checkpoint is audio, settings, keybinds, and the pause/death overlays.
The table below is the authoritative sequence for this page.

## 0.0.10.4 - Survival

**Goal:** the player can take damage, run out of stamina, die, respawn safely, and keep that state
across a reopen.

| PR | Branch | Scope | Tests | Documentation |
|---|---|---|---|---|
| 4.1 | `feat/survival-state` | Headless `Survival` state: health, stamina, dead flag, spawn point, fall and drown timers. `GameState` owns it. `VPLY` version 2 adds these fields, with `decode_v1` and bounds checks. | Damage/clamp/death, stamina drain and regeneration, v1 to v2 load, v2 round-trip, corrupt field rejection | [Save format](save-format.md), [Gameplay core](gameplay.md), [scope](v0.0.10-scope.md) |
| 4.2 | `feat/survival-damage` | `EnvironmentProbe` trait for solid, water, and lava queries, implemented by the demo and legacy paths. Fall-distance tracking, fall damage, drowning interval, lava/fire hooks, death event, and safe-spawn search. | Fall thresholds, drowning cadence, lava hook, respawn search, no spawn inside a solid or flooded cell | [Gameplay core](gameplay.md), [Testing](testing.md), [scope](v0.0.10-scope.md) |
| 4.3 | `feat/ui-survival` | Health and stamina bars, damage flash, death overlay, and pause overlay. `Escape` opens pause when no screen is open. Pause stops player, game, and sky ticks; streaming may continue. Capture hooks `ENGINE_UI_PAUSE=1` and `ENGINE_UI_DEATH=1`. | UI transitions, pause tick behavior, death/respawn button, HUD draw | [UI](ui.md), [Controls](controls.md), [Configuration](configuration.md) |
| 4.4 | `fix/survival-playtest` | Only if the manual pass finds issues. Keep each fix separate from feature work. | Regression test per fix | Changelog only |

### Decisions for 4.1

- Health is a `u16` with a maximum of 20; stamina is a `u16` with a maximum of 100.
- Death keeps the inventory for `0.0.10`. Drop-on-death is later.
- Respawn uses the world spawn point for now. A bed or spawn item is deferred.
- Pause freezes player movement, the fixed tick, and the sky, but leaves mesh streaming and upload
  running so background work can finish.

### Exit criteria

- Damage, death, and respawn work end to end.
- Spawn safety is tested against solid and flooded cells.
- Health, stamina, and the spawn point survive a reopen.
- The HUD and the pause/death screens pass a windowed manual check.

## 0.0.10.5 - Audio and settings

**Goal:** the game has sound, a settings screen, persisted volumes, and remappable keys.

| PR | Branch | Scope | Tests | Documentation |
|---|---|---|---|---|
| 5.1 | `feat/audio-backend` | Add kira behind an app-side `Audio` service. Synthesized placeholder buffers, master/sfx/ambience buses, and a null backend for headless and CI. Events for break, place, pickup, craft, hit, death, and footstep. | Event mapping, volume clamps, null backend, no device required | [Architecture](architecture.md), [Controls](controls.md), [ADR-0019](../decisions/0019-kira-for-game-audio.md) status |
| 5.2 | `feat/settings-screen` | Settings model and screen: render distance, FOV, sensitivity, master/music/sfx, and vsync. Apply volumes, FOV, sensitivity, and vsync immediately; apply render distance on resume to avoid a mid-frame streaming rebuild. | Defaults, range validation, parse and serialize round-trip, UI state | [Configuration](configuration.md), [UI](ui.md), [Controls](controls.md) |
| 5.3 | `feat/keybinds` | Action map, default bindings, key-name parser, conflict detection, reset defaults, rebind UI, and persistence. Replace hard-coded `KeyCode` lookups in input handling. | Parse and serialize, duplicate detection, default lookup, rebind state | [Controls](controls.md), [Configuration](configuration.md) |
| 5.4 | `feat/audio-ambience` | Optional split: surface footsteps, wind and water ambience, basic positional block sounds, and crossfades. | Event mapping and volume behavior; manual audio pass | [Controls](controls.md), changelog |

### Decisions for 5.1 and 5.2

- Settings use one `settings.toml` parsed with `serde` and `toml` and `deny_unknown_fields`, rather
  than growing the hand-rolled parser. `config.toml` stays the world-settings file or migrates in
  one documented step.
- The Ubuntu CI job installs `libasound2-dev` if kira/cpal needs ALSA headers. The null backend
  remains the default for tests and headless runs.
- Audio scope is basic positional block sounds plus non-positional UI and ambience. Full mixing,
  occlusion, and streaming music are later.
- Keybinds are fully rebindable in `0.0.10.5`, not just persisted defaults.

### Exit criteria

- Sound plays on a real device; `ENGINE_AUDIO=null` keeps tests headless.
- Settings persist across restart.
- Rebinds survive restart and detect conflicts.
- The settings screen is reachable from pause.

## 0.0.10.6 - Acceptance and release

**Goal:** no new features. Harden the save, prove the full loop headlessly, run the manual pass, and
cut `v0.0.10`.

| PR | Branch | Scope | Gates |
|---|---|---|---|
| 6.1 | `test/save-hardening` | Truncation and fuzz cases, invalid health/stamina/spawn bounds, unknown item, oversized entity count, v1 migration, and quarantine behavior. | Test-only; full CI |
| 6.2 | `test/acceptance-loop` | Scripted gather, craft, place, damage, die, respawn, save, and reopen run over a deterministic synthetic world. | Test-only; full CI |
| 6.3 | `docs/close-0.0.10` | Final roadmap, scope, test, control, configuration, and save-format updates; record manual acceptance results and captures. | Docs-only; Rust jobs skip |
| 6.4 | `chore/release-0.0.10` | Run the release script, bump `Cargo.toml` and the lockfile, date the changelog, tag `v0.0.10`, and verify the release workflow and docs deploy. | Full pipeline; Perf on `main` |

### Release gate

- The full manual loop passes with `TERRAIN_DEMO=1`.
- Reopen preserves health, stamina, spawn point, inventory, drops, and edits.
- Settings, keybinds, and audio survive restart.
- `cargo fmt`, Clippy with `-D warnings`, the full test suite, `typos`, `mdbook build`, and the size
  budgets pass.
- The `v0.0.10` tag matches `Cargo.toml`; docs and release notes publish.

## 0.0.11 - Look and scale

This milestone needs its own detailed plan after the visual target and the scale ADR. The proposed
order is:

1. `docs/visual-target` - reference frames, palette, camera, budgets, and non-goals.
2. `feat/capture-harness` - fixed camera, deterministic seed, `ENGINE_CAPTURE`, diff threshold, and
   a CI artifact.
3. `docs/adr-voxel-scale` - measure 1 m, 0.5 m, and dual-scale options; record memory, mesh, frame,
   collision, and save costs, then choose the supported scale and migration policy.
4. `feat/hero-pond-scene` - curated deterministic scene and `ENGINE_SCENE=pond`.
5. `feat/instanced-props` - instance buffer, transforms, wind phase, LOD, and culling. Split
   placement and rendering if the PR grows.
6. `feat/materials` - replace placeholder tiles, palette and tint controls, and the validation tool.
7. `feat/look-baseline` - shadows, AO, height fog, light shafts, bloom, tonemapping, and water.
   Split lighting, water, and post into separate PRs.
8. `test/visual-gate` - screenshot diff, frame time, GPU time, and memory.

Do not commit to the prop or lighting PR count until the hero scene and scale ADR are settled.

## Cross-cutting rules

- **PR size.** One subsystem per PR. Keep new files under the 1,000-line gate and split survival,
  audio, settings, and UI into submodules early.
- **Headless first.** Gameplay logic lives in `src/game` and names no `wgpu`, `winit`, or audio type.
  App glue lives in `src/app`. Tests use traits and null or recording backends.
- **Save discipline.** Stable item names only. Every format change gets a decoder path, bounds
  checks, and corruption tests.
- **Documentation per PR.** Update the owning design or reference page and `CHANGELOG.md` in the
  same PR. Keep the final closeout and release as separate PRs.
- **Manual QA hooks.** Add environment switches for pause, death, and settings captures, mirroring
  `ENGINE_UI_INVENTORY` and `ENGINE_UI_WORKBENCH`.
- **CI.** Code PRs run the full matrix. Docs-only PRs skip the Rust jobs. The release PR changes
  `Cargo.toml` and must run everything.

## Adopted decisions

These choices are the recommended defaults for the plan. Change them before the owning PR starts if
the implementation evidence points elsewhere.

1. Move the `VPLY` version 2 bump into `0.0.10.4`.
2. Keep the inventory on death for `0.0.10`.
3. Use a real TOML and serde settings file.
4. Add ALSA development packages to Ubuntu CI for kira/cpal, with a null backend fallback.
5. Pause gameplay and the sky but keep mesh streaming running.
6. Implement full keybind rebinding in `0.0.10.5`.
7. Defer beds and drop-on-death.
8. Commit to `0.0.11` as the target-look and terrain-scale milestone; defer its prop and lighting
   detail until the visual brief and scale ADR land.

## First three steps

1. `4.1` survival state plus `VPLY` version 2.
2. `4.2` damage, death, and respawn.
3. `4.3` survival HUD and pause/death screens.

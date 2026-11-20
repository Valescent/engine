# Terrain tools

`terrain-gen` is the headless acceptance harness for the terrain pipeline. This page defines
the two measurement tools that do not generate blocks of their own: `calibrate-blend` measures
the biome blend width from the generated climate field, and `counters` pins the deterministic
subset of the `Q1.1` counters for the PR tier.

Both commands honour the global options. Output is identical for every `--threads` value and
every `--order` value, `--json` prints exactly one JSON object on stdout, and diagnostics go to
stderr. Seeds are always `1..=n`.

## calibrate-blend

`docs/specs/terrain.md` §8.4 and `E3.6` require the biome blend to be measured rather than
labelled: `BLEND_BLOCKS = 24` is a target, and the tool reports the measured `W_blocks` per
unordered biome pair.

```console
cargo run --locked --quiet --no-default-features --bin terrain-gen -- \
  calibrate-blend --seeds 3
```

Options:

| Option | Default | Meaning |
|---|---|---|
| `--seeds <n>` | required | Measure seeds `1..=n` |
| `--from <x> <z>` | `-1024 -1024` | Low corner of the block window |
| `--size <n>` | `2048` | Window side in blocks |
| `--step <n>` | `16` | Anchor stride in blocks; a positive multiple of `MACRO_CELL` |

The default window is the whole finite world (`WORLD_HALF_EXTENT = 1024`). The window must lie
inside it.

### Measurement definition

1. **Anchor grid.** The window is sampled on a fixed grid, not randomly. Climate cells are
   `MACRO_CELL = 4` blocks wide, so cell `c` has its centre at block `4c + 2`. The anchors are
   the cells whose centre lies in `[from, from + size)` and whose cell index is a multiple of
   `step / 4`. The stride is global: moving `--from` changes which anchors are inside the
   window, never the phase of the grid.
2. **Blend cells only.** An anchor is kept when `best != second` and `w(q) < 0.95`, where
   `w(q) = 0.5 + 0.5 * smoothstep01(q / 255)` is the exact expression `biome_pick` evaluates.
   Hysteresis is not used and the generated climate is not changed. `q` is the quantised
   **block distance** to the pair's border, not a raw `d2` margin: the climate stage computes
   `d = m / |grad m|` and then `q = clamp(floor(d / ramp * 255 + 0.5), 0, 255)`, with
   `ramp = BLEND_BLOCKS / (2 * 0.8042)`. The tool reads the stored `q`, so it measures exactly
   the value `biome_pick` reconstructs.
3. **Direction.** The margin `q` is fitted with a least-squares plane that has an intercept,
   over the cells of a `5 x 5` window that share the anchor's **ordered** `(best, second)`
   pair and are not saturated (`q < 255`). The window widens to `7 x 7` then `9 x 9` until four
   cells qualify. `q` is a V across a border, so a plane through both sides has a crease in it;
   keeping one side makes the fitted gradient normal to the border instead of nearly
   tangential.
4. **Sampling.** A block position maps to the cell lattice as `(block - 2) / 4`. `q` is
   bilinearly interpolated over the cell centres there, then `w` is evaluated with the exact
   formula above. Samples are one block apart from `-march` to `+march`. `march` is 256 blocks,
   shortened when the world edge is nearer, and transects that cannot reach 128 blocks are
   discarded.
5. **Border crossing.** Walking from the anchor opposite the gradient, the first local minimum
   of `w` is the border (`w = 0.5`): the walk stops at the first rise after a descent, and at
   the first rise after the anchor when the anchor is already the minimum. It never scans past
   that rise to a deeper, unrelated minimum. Because `q` is quantised, bilinear `w` can sit a
   fraction of a block above `0.5`; a minimum at `w <= 0.51` (`q <= 17`) is accepted and the
   residual error is below one block. A profile with no such minimum before the rise is
   discarded.
6. **End crossing.** Continuing in the gradient direction, the first sample at or beyond
   `w = 0.95` is linearly interpolated with its predecessor.
7. **Width.** `W_blocks = 2 * (d_end - d_border)`: the distance from `w = 0.5` to `w = 0.95`,
   doubled, as §8.4 requires.
8. **Bucketing.** Every width is bucketed by the unordered pair `(min(best, second),
   max(best, second))`, pooled over every measured seed. Results are merged in canonical seed
   order, so threads and processing order cannot change them.
9. **Percentiles.** Widths are sorted and read with linear interpolation between order
   statistics: `index = p * (n - 1)`. The table reports the median, p10, and p90 with two
   decimals.
10. **Acceptance.** A pair with fewer than 20 transects is reported only (`n/a`). Every
    assessed pair must satisfy median `20..=28`, p10 `>= 8`, and p90 `<= 64`. The command exits
    `1` when an assessed pair fails or when no pair is assessed, `0` otherwise.

### Output

Text is a pinned table; the first line is the run configuration, then one row per observed
pair. `--json` is one object with sorted pairs and fixed two-decimal values:

```json
{"subcommand":"calibrate-blend","seeds":3,"from":[-1024,-1024],"size":2048,"step":16,"pairs":[{"pair":[0,1],"samples":2213,"median_blocks":23.50,"p10_blocks":18.98,"p90_blocks":33.45,"assessed":true,"ok":true}],"ok":true}
```

Current v1 status is recorded in [Benchmark results](benchmarks.md#terrain-blend-calibration-2026-11-20).

## counters

`counters` snapshots the deterministic part of the pinned `Q1.1` metrics for a sweep tier. It
runs the same seeds and regions as `sweep` once per region, without the sweep's duplicate cold
run, and sums the counter values over the independent per-seed generator instances.

```console
cargo run --locked --quiet --no-default-features --bin terrain-gen -- counters --tier pr
cargo run --locked --quiet --no-default-features --bin terrain-gen -- counters --tier pr --json
cargo run --locked --quiet --no-default-features --bin terrain-gen -- counters --tier pr --update
```

| Option | Default | Meaning |
|---|---|---|
| `--tier pr\|nightly` | required | The sweep region set and default seed count |
| `--seeds <n>` | tier default (`16` / `64`) | Seeds `1..=n` |
| `--file <path>` | `crates/terrain-core/tests/golden/terrain-counters.json` | Baseline to check or rewrite; required for `nightly` |
| `--update` | off | Rewrite the baseline and exit `0` |

### Pinned counters

Four pinned counters survive the "no wall clock, thread count, or order" rule:

| Counter | Why it is deterministic |
|---|---|
| `resident_columns` | Published from the field-tile cache after each column; a function of the tiles the fixed column set touches |
| `ready_columns` | Published beside `resident_columns`; this pipeline caches no eviction, so it follows the same set |
| `queued_columns` | Structurally `0`; the generator owns no admission queue. Pinned so a future queue cannot appear unnoticed |
| `lod0_columns` | Distinct LOD0 columns with a generated section; `0` here because this workload stops at columns |

The tool also pins four counters derived from the generated block stream:

| Counter | Meaning |
|---|---|
| `blocks_air` | Air blocks across every generated column |
| `blocks_solid` | Every id that is neither air nor water |
| `blocks_water` | Water blocks |
| `columns` | Generated columns |

`blocks_*` and `columns` are pure functions of the output bytes, so they catch a generation change
that a tile-cache gauge would not.

Excluded deliberately:

- every `_p50`/`_p95` latency and `upload_bytes_per_s` - wall-clock dependent, and sensitive to
  thread count and scheduling;
- `queue_depth`, `in_flight_jobs`, `panics_caught`, `stuck_threads`, `dropped_before_start`,
  `discarded_stale`, and `applied` - runtime residency and worker-health state that this headless
  workload never records, so a baseline would pin zeros and catch nothing;
- `upload_bytes`, `meshes`, `quads`, `light_repairs`, `lod1_columns`, and `lod2_columns` - written
  only by the render, mesh, light, and LOD paths that `terrain-gen` never runs.

### Baseline and drift gate

The baseline is `crates/terrain-core/tests/golden/terrain-counters.json`:

```json
{
  "schema": "valescent/terrain-counters@1",
  "tier": "pr",
  "seeds": 16,
  "counters": {
    "blocks_air": 579022,
    "blocks_solid": 182722,
    "blocks_water": 24688,
    "columns": 2048,
    "lod0_columns": 0,
    "queued_columns": 0,
    "ready_columns": 512,
    "resident_columns": 512
  }
}
```

A check fails when any counter moves by **more than 5%** relative to the baseline. A baseline of
zero must stay exactly zero. A counter present on only one side is also a drift: removing or
adding a pinned name is a deliberate baseline change. The tier and seed count in the file must
match the run, so a check against a different workload is a usage error rather than a silent
comparison.

Rewrite the baseline only with `--update`, and say why in the commit. Changing the pinned metric
names, the region sets, the seed list, or the block classification is a baseline change, not a
noise fluctuation.

The `Terrain gates` job in `.github/workflows/ci.yml` runs `counters --tier pr` on every push and
pull request. The requirement is the `Q1.8`/`R1.8` PR tier: deterministic counters only, fail at
5%; the nightly tier owns wall-clock measurements.

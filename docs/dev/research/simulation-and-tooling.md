# Simulation and tooling research

The engine has no entities and a single player, so simulation is small. The gaps that matter are
determinism, collision cost as speed grows, and the ability to catch performance regressions before a
player notices them.

## Current state

- **Movement:** axis-separated swept AABB, gravity, jump, and a column-readiness guard. Delta time
  is clamped, so a long frame cannot tunnel, but integration still depends on frame rate.
- **Collision query:** `World::solid_blocks_in` scans the integer volume of the swept box. It is
  cheap at walking speed and grows with speed and body size.
- **No step-up:** a half-block lip stops the player, making stairs and rough terrain feel sticky.
- **Water** has no physics; it is a visual and light medium only.
- **Tooling:** Tracy spans and plots behind a feature, `pool_sim` for the worker pool,
  `bench_hot_paths` for snapshot, meshing, and collision. No headless run mode, no performance gate
  in CI, no crash log, no replay.
- **Public API:** `engine::run()` starts the window loop; there is no library entry for a headless
  world.

## Simulation options

### 1. Fixed timestep with interpolation

Accumulate real time and step simulation at a fixed rate (for example 120 Hz), interpolating the
camera between the last two states. Movement, jumping, and the guard become independent of frame
rate, replays become reproducible, and tests can assert exact positions after N steps. This is the
standard fix-your-timestep shape and a prerequisite for trustworthy replay tests.

### 2. Collision acceleration

Replace the volume scan with a traversal that only visits cells the box can touch:

- A DDA over the voxel grid for the leading face, or
- A per-chunk `solid` bitmask/height range to skip empty regions quickly.

For multiple entities later, add a spatial hash or chunk occupancy list as a broadphase. With one
player, the win is smaller but still measurable at high speeds and large bodies.

### 3. Step-up and ground handling

Allow a small automatic step (0.5-0.6 blocks) when the horizontal move is blocked but the space one
step up and forward is free. This is a quality change with a small physics cost and a big feel
change.

### 4. Sub-stepping for fast movement

Split a step whose displacement exceeds a threshold into sub-steps inside one tick. The readiness
guard and collision stay conservative, and future projectiles or vehicles do not need a second code
path.

## Tooling options

### 1. Headless runner

A `--headless` flag or a library entry that builds a world, runs a fixed camera for N frames or
seconds, and prints the metric line. Uses the normal generation, light, and mesh paths without a
surface or event loop. This is the foundation for CI performance runs and for scripted acceptance.

### 2. Performance gate in CI

Run the headless benchmark on a fixed scene and seed, compare against a checked-in baseline with a
generous tolerance (for example 20%), and fail only on large regressions. Store the raw numbers as
an artifact. Machines vary, so gate on relative changes, not absolute times.

### 3. Criterion benches

Add benches for column generation, light computation, LOD0/1/2 meshing, and snapshot preparation,
next to `pool_sim`. Keep the existing ignored `bench_hot_paths` test for release measurements.

### 4. Memory accounting

Count resident bytes per subsystem (blocks, light, meshes, scratch) and expose them in the debug
overlay and Tracy. A per-chunk byte total makes memory regressions obvious in review.

### 5. Replay recording

Record the seed, spawn, input events with timestamps, and frame deltas. Replaying with a fixed
timestep reproduces a session for bug reports and regression tests. This pairs with the fixed
timestep above.

### 6. Crash and error reporting

Install a panic hook that writes a crash report with the backtrace, last metric line, seed, and
config next to the world. Add the planned log file so a user can attach one file.

### 7. Offscreen GPU tests

Render a fixed camera to a texture and compare a hash or a small pixel sample. Behind a feature and
not in the default suite. This is the only automated way to catch shader or winding regressions like
the back-face change.

## Recommendation

1. Headless runner, then the CI performance gate. This is what protects every other optimization.
2. Fixed timestep with interpolation and replay recording.
3. Collision traversal (DDA or chunk bitmask), then step-up.
4. Memory counters and the crash report.
5. Offscreen GPU smoke test behind a feature.

## Measurement

- Determinism: the same replay produces the same final position and chunk set, bit for bit.
- Performance gate: headless scene at distances 12 and 24, wall time and frame mean.
- Collision: `Player::update` time at walking, sprinting, and a synthetic 64 m/s speed.

## References

- [Gaffer on Games: Fix Your Timestep](https://gafferongames.com/post/fix_your_timestep/) — fixed-step integration and interpolation.
- [voxel-aabb-sweep](https://github.com/fenomas/voxel-aabb-sweep) — swept AABB against voxels.
- [Spatial acceleration structures](https://research.ncl.ac.uk/game/mastersdegree/gametechnologies/physicstutorials/6accelerationstructures/Physics%20-%20Spatial%20Acceleration%20Structures.pdf) — broadphase and narrow phase.
- [Profiling](../profiling.md) and [Benchmark results](../benchmarks.md) — the current tooling and run log.

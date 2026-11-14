# Code conventions

These are defaults, not dogma. Deviating is fine when the code says why.

## Layout

- Engine code lives in the `engine` library (`src/lib.rs`); `src/main.rs` stays a thin entry point.
- One concept per module. A module owns its data and the operations on it.
- Export the minimum. `pub(crate)` before `pub`, `pub` before `pub use` trees.
- No global mutable state. Pass what a subsystem needs.

## Errors

- Library code returns typed errors with [`thiserror`](https://docs.rs/thiserror); `anyhow` is for the binary edge only.
- Never `panic!`, `unwrap`, or index blindly on data derived from user input or disk. Return an error.
- `expect` is acceptable only for programming errors that cannot be reached, with a message explaining the invariant.
- Validation errors from `wgpu` and shader compilation must surface as errors, not be swallowed.

## Logging

- Use [`tracing`](https://docs.rs/tracing) for engine diagnostics; `println!` belongs in the binary only.
- Log at the level of the reader: `error` for broken invariants, `warn` for recoverable surprises, `info` for lifecycle events, `debug`/`trace` for per-frame detail.
- Never log per-frame data at `info`.

## Safety

- `unsafe` is denied at the crate root. If a real need appears, write an ADR, then add the narrowest possible `#[allow(unsafe_code)]` with a `SAFETY:` comment stating the invariant.
- Data crossing into the GPU is plain-old-data, laid out with `bytemuck`. Never reinterpret arbitrary structs.

## Determinism

- Simulation must not depend on `HashMap` iteration order. Sort keys or use an ordered map.
- Randomness comes from a seeded RNG owned by the simulation.
- Time-dependent behaviour reads a frame delta passed in, not a clock read from inside the system.

## Performance

- Measure before optimizing. A profiling run or a benchmark, not a hunch.
- Keep the per-frame path allocation-free where practical, but correctness first.
- Document any hot loop that relies on a non-obvious layout or ordering.

## Tests

- Unit tests live beside the code in `#[cfg(test)] mod tests`.
- Integration tests go in `tests/` and use the public API.
- The default test suite must not require a GPU or a window; gate those tests behind a feature or an environment variable.
- Fixed inputs, fixed outputs. No wall-clock, no network, no unseeded randomness.

## Documentation

- Public items get rustdoc. Explain invariants and units, not the signature.
- Decisions with tradeoffs get an ADR in `docs/decisions/`, following `docs/README.md`.
- Update `CHANGELOG.md` for user-facing changes.

## Naming

- Spell words out: `position`, not `pos`; `descriptor`, not `desc`.
- Units belong in the name when they are not obvious: `_meters`, `_millis`.
- Types are nouns, functions are verbs, booleans read as assertions (`is_visible`, `has_neighbour`).

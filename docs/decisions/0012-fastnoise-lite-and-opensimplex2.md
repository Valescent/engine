# Why I Chose fastnoise-lite and OpenSimplex2 for World Generation

**Document type:** Architecture Decision Record / Devlog Entry  
**Status:** Accepted  
**Date:** 2026-11-15  
**Project:** Valescent Engine  
**Owner:** Project Owner  
**Supersedes:** None  
**Scope:** The deterministic noise dependency and algorithm behind the five v0.0.7 generation fields, their seed derivation, frequencies, fractal settings, normalization, and the deterministic tree-placement hash. Height, biome, cave, and tree rules remain in the v0.0.7 spec.

---

## Summary

This document records why the engine uses `fastnoise-lite` 1.1.1 with OpenSimplex2 for procedural world generation. It is both an official decision record and a devlog entry: the decision is final for the current scope, and I am publishing the reasoning so future decisions can be traced back to it.

The short version: v0.0.7 needs five deterministic fields in 2D and 3D, seeded from one world seed, with smooth low-frequency shapes and a small dependency. `fastnoise-lite` provides OpenSimplex2 in both dimensions, deterministic seeding, and fractal controls behind a compact API. OpenSimplex2 avoids the stronger grid-direction artifacts of classic Perlin at the scales the biome and continentalness fields use. The tradeoff is a smaller Rust port rather than a large Rust-native noise framework, pinned initial frequencies that must be tuned deliberately, and f32 coordinates.

---

## 1. Context

### 1.1 The project

v0.0.7 generates a four-chunk-tall world from a seed:

- **Continentalness and erosion** shape the heightmap at low frequency.
- **Temperature and humidity** select plains, hills, or desert.
- **Cave** is a 3D field that carves tunnels and overhangs.
- **Trees** use a deterministic per-chunk hash, not global RNG.
- **Streaming** generates chunks independently and in any order.
- **Determinism** requires every field to be a function of `(seed, position)`.
- **Workers** run generation off the main thread.
- **Modified-only saves** mean unmodified chunks are regenerated from the seed.
- **Finite render distance** keeps coordinates local enough for f32 noise in the current scope.

The noise layer must be deterministic, seedable, available in 2D and 3D, and small enough to wrap behind an engine-owned interface.

### 1.2 The constraints

My choice is constrained by the following:

- Five independent fields with different frequencies and fractal settings.
- OpenSimplex2-like smooth noise in 2D and 3D.
- A seed per field, derived from one world seed.
- No global RNG call order; no dependence on chunk load order.
- Normalized output in `[0, 1]` for the height and biome formulas.
- Deterministic output for the same seed and coordinates on the same target.
- A small, permissively licensed dependency with no C or unsafe build step.
- CPU generation on the v0.0.5 worker pool.
- No requirement for domain warp, cellular noise, or coloured noise in v0.0.7.
- The generator values must be pinned tightly enough to make unmodified chunks reproducible.

---

## 2. Decision

**World generation uses `fastnoise-lite` 1.1.1 with `NoiseType::OpenSimplex2`.**

The dependency is pinned as `fastnoise-lite = "1.1.1"` with default features. The engine uses
the crate's default `f32` coordinate path.

Frequencies are in inverse blocks; sampling uses world block coordinates. All five fields use
`OpenSimplex2` and `FractalType::FBm`:

| Field | Dimensions | Frequency | Octaves | Lacunarity | Gain |
|---|---|---:|---:|---:|---:|
| `continentalness` | 2D | `0.0015` | 4 | `2.0` | `0.5` |
| `erosion` | 2D | `0.0030` | 4 | `2.0` | `0.5` |
| `temperature` | 2D | `0.0010` | 3 | `2.0` | `0.5` |
| `humidity` | 2D | `0.0012` | 3 | `2.0` | `0.5` |
| `cave` | 3D | `0.0120` | 3 | `2.0` | `0.5` |

Seed derivation:

- Fold the world seed and a field tag through SplitMix64.
- Use the lower 32 bits as the `i32` seed for that field's `FastNoiseLite`.
- Field tags are explicit constants, not enum order or hash-map order.

```text
field_seed(seed, tag) = (splitmix64(seed ^ tag) as u32) as i32
```

SplitMix64:

```text
fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E3779B97F4A7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}
```

Field tags:

```text
continentalness = 0x01
erosion         = 0x02
temperature     = 0x03
humidity        = 0x04
cave            = 0x05
tree            = 0x06
```

Normalization:

- `FastNoiseLite` output is approximately `[-1, 1]`.
- Every field is normalized to `[0, 1]` with `clamp((raw + 1.0) * 0.5, 0.0, 1.0)`.
- The height and biome formulas consume only the normalized values.

Tree hash:

- Tree placement does not use the noise library's RNG.
- It uses SplitMix64 over `(seed, chunk x, chunk z, candidate index, tree tag)`:

```text
tree_hash(seed, cx, cz, index) =
    splitmix64(seed ^ 0x06 ^ (cx as u64) ^ ((cz as u64) << 32)
               ^ (index as u64).wrapping_mul(0x9E3779B97F4A7C15))
```

- Candidate `x` is `hash & 31`, candidate `z` is `(hash >> 8) & 31`, and trunk height is
  `4 + ((hash >> 16) % 3)` within the v0.0.7 tree spec.
- Two candidate indices are evaluated; a duplicate candidate is skipped.

The exact height, biome, cave, and tree rules remain in the
[v0.0.7 spec](../specs/v0.0.7.md). The frequencies and fractal settings above are the pinned
initial generator parameters; changing them changes unmodified terrain and requires a new
decision.

I accept this decision for the current scope. I will revisit it only if the assumptions in this
document stop holding.

---

## 3. Why I Chose fastnoise-lite and OpenSimplex2

### 3.1 OpenSimplex2 has the right shape for terrain

OpenSimplex2 produces smooth, isotropic-looking fields without the stronger axis alignment of
classic Perlin. Continentalness, erosion, temperature, and humidity are sampled at very low
frequencies, where directional artifacts become broad and visible. OpenSimplex2 is the better
default for those shapes, and the same algorithm has a 3D form for caves.

### 3.2 One algorithm covers 2D and 3D

The same noise type handles the four surface fields and the cave field. That keeps the
generation code to one configuration path, one seed rule, and one normalization rule. A mix of
Perlin, cellular, and Worley fields would need more tuning and more failure modes.

### 3.3 The library is small and portable

`fastnoise-lite` is a compact Rust port with a small API, no C build step, and an MIT licence that
passes the dependency policy. It provides exactly the controls the generator needs: noise type,
seed, frequency, fractal type, octaves, lacunarity, and gain. It has no renderer or world-model
baggage.

### 3.4 Seeding is explicit

Each field gets its own seed derived from the world seed and a stable tag. There is no global
RNG, no call-order dependency, and no shared state between chunks. This directly satisfies the
v0.0.7 determinism constraint and makes chunk generation independent of worker order.

### 3.5 The tree path is separate from noise

Trees should not consume a sequence from a shared noise object. A SplitMix64 hash of the seed,
chunk coordinates, and candidate index is deterministic, local, and independent of how many
other features were generated. The noise library is only for the five continuous fields.

### 3.6 The frequencies and octaves are pinned

The ADR fixes the initial frequencies and fractal settings so the same seed produces the same
terrain. Tuning is still expected against the stress/generation fixture, but a tuning change is
an explicit generator change, not a silent drift. Modified chunks remain saved; unmodified
chunks will regenerate with the new parameters.

### 3.7 Why not the `noise` crate?

`noise` 0.9.0 is a capable Rust-native library with a trait-based API and more noise families.
It was not selected because it is a larger API surface than the generator needs, its OpenSimplex
path is not OpenSimplex2, and the project benefits from the smaller, direct FastNoiseLite port.
The `noise` crate remains a viable fallback if `fastnoise-lite` becomes unmaintained.

### 3.8 Why not Perlin, cellular, or an in-engine implementation?

- **Perlin** is familiar but has stronger directional artifacts at low frequencies and no clear
  advantage for this terrain.
- **Cellular/Worley** is useful for cell-like caves or biomes, but it is a different look and
  more tuning than the scope requires.
- **An in-engine OpenSimplex2 implementation** would remove the dependency but add a large,
  subtle, test-heavy numerical component that is not the milestone's point.

---

## 4. Alternatives Considered

| Alternative | Assessment | Outcome |
|---|---|---|
| **`fastnoise-lite` + OpenSimplex2** | OpenSimplex2 2D/3D, small API, fractal controls, MIT, no C build. | **Selected** |
| **`noise` + OpenSimplex** | Larger Rust-native library, trait API, more noise families; not OpenSimplex2, more surface than needed. | Not selected; viable fallback |
| **`noise` + Perlin** | Familiar and simple; stronger directional artifacts at low frequencies. | Not selected |
| **`fastnoise-lite` + Perlin** | Same small dependency; Perlin artifacts remain. | Not selected |
| **Cellular / Worley fields** | Distinct cell shapes; different look and more tuning; not required. | Not selected |
| **In-engine OpenSimplex2** | No dependency; large numerical implementation and test burden. | Not selected |
| **Multiple algorithms per field** | Could tune each field independently; more configuration, determinism surface, and tuning work. | Not selected |
| **Domain warp** | More organic shapes; extra parameters and cost not needed in v0.0.7. | Deferred |

---

## 5. Counterarguments and Disposition

This decision has real weaknesses. I am recording them here rather than omitting them, so the
record stays honest.

| Counterargument | Disposition |
|---|---|
| **`fastnoise-lite` is a smaller Rust port than the `noise` ecosystem.** | Accepted. It is a direct port of the maintained FastNoiseLite algorithm, pinned at 1.1.1. A maintenance problem triggers a new ADR and the `noise` fallback. |
| **The crate had no new crates.io release after 1.1.1 at the time of this decision.** | Accepted. The API is small and stable, and the port is wrapped behind engine code. Maintenance is a monitored risk, not a reason to write noise from scratch now. |
| **The frequencies are initial tuning values.** | Accepted. They are pinned in this ADR so the generator is reproducible; retuning is a deliberate generator change. |
| **The default `f32` path loses precision far from the origin.** | Accepted. The finite render distance keeps sampled coordinates local enough for this milestone. If distant streaming needs more precision, the crate's `f64` path or a new noise decision can follow. |
| **Cross-platform floating-point output can differ at the last bits.** | Accepted. The generator targets deterministic same-target output; the milestone tests pin values on the acceptance target. Threshold margins and tests catch visible boundary changes. |
| **OpenSimplex2 is not Perlin.** | Accepted. That is the point: fewer axis-aligned artifacts at low frequencies. |
| **Two fixed candidate trees per chunk is simple.** | Accepted. It is deterministic and fits one-chunk trees; richer placement is a later feature decision. |
| **No domain warp means less organic terrain.** | Accepted. Domain warp adds parameters and cost; the current noise/fractal mix is the starting point. |
| **The tree hash is separate from the noise library.** | Accepted. Trees are discrete features; a stable hash is simpler and avoids shared RNG state. |
| **A generator parameter change invalidates unmodified terrain.** | Accepted. Modified chunks remain saved; unmodified chunks intentionally regenerate. A parameter change is a new decision, not a runtime tweak. |

---

## 6. Consequences

### 6.1 Positive

- One small dependency provides OpenSimplex2 in 2D and 3D.
- The five fields share one noise type, normalization, and seed rule.
- No global RNG or call-order dependency.
- Fractal controls shape continentalness, erosion, temperature, humidity, and caves.
- The tree hash is deterministic and independent of generation order.
- The MIT licence fits the dependency policy.
- The initial frequencies and octaves are reproducible rather than implicit.
- The noise dependency sits behind an engine-owned generation wrapper.

### 6.2 Negative

- A smaller Rust port is the maintenance surface.
- The default `f32` path limits coordinate precision at extreme distances.
- The pinned frequencies need deliberate tuning.
- Cross-platform bit-identical output is not the contract.
- Domain warp and cellular noise are not used.
- A parameter change changes unmodified terrain.
- The dependency adds a crate and a version to pin.

### 6.3 Neutral

- Height, biome, cave, and tree rules remain in the v0.0.7 spec.
- Modified chunks persist independently of noise parameter changes.
- The `noise` crate remains a fallback if the port becomes a problem.
- More noise families or domain warp can be added by a later decision.

---

## 7. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| `fastnoise-lite` becomes unmaintained. | Pin 1.1.1, keep it behind a small noise wrapper, and retain the `noise` crate as a documented fallback. |
| A dependency change alters field output. | Pin the version, lock the dependency, and test fixed seed/coordinate vectors. |
| Frequencies produce poor terrain. | Tune against the generation/stress fixture; record a new decision for changed parameters. |
| Seed derivation collides between fields. | Use distinct field tags through SplitMix64 and test each field's first values. |
| f32 coordinates lose precision far from the origin. | Keep render distance finite; evaluate the crate's `f64` path or a new decision if large-world streaming needs it. |
| Cross-platform floating-point differences shift a threshold. | Test fixed inputs on the acceptance target; keep biome/cave thresholds away from exact boundary values where practical. |
| A shared noise object leaks state between fields. | Construct one configured `FastNoiseLite` per field; never share mutable noise state across workers. |
| Tree placement depends on load order. | Use the SplitMix64 seed/chunk/candidate hash only; test shuffled generation order. |
| Normalized output exceeds `[0, 1]`. | Clamp after mapping; test extremes. |
| The noise wrapper leaks into world code. | Keep a narrow engine-owned interface; callers receive normalized fields, not `FastNoiseLite` values. |

---

## 8. Conclusion

`fastnoise-lite` with OpenSimplex2 is my deliberate, deterministic choice for v0.0.7 world
generation. I am selecting it because it provides the 2D and 3D smooth fields the generator
needs in a small, permissively licensed dependency, because OpenSimplex2 avoids the stronger
low-frequency axis artifacts of Perlin, and because explicit per-field seeds and a separate
SplitMix64 tree hash eliminate global RNG and load-order dependence. I acknowledge that it is a
smaller Rust port, that the frequencies are pinned tuning values, that the default path is
`f32`, and that a parameter change alters unmodified terrain. I manage those through a pinned
version, a narrow wrapper, fixed test vectors, explicit seed tags, and a documented `noise`
fallback. I therefore accept this decision as the engine's noise library and algorithm, with the
height/biome/cave/tree rules left to the v0.0.7 spec and with its limits recorded above.

---

*This entry is part of the Valescent Engine decision log. Future entries should reference this document when they depend on noise generation, seed derivation, or world-generation parameter changes.*

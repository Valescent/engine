# Why I Chose VCHK for Chunk Persistence

**Document type:** Architecture Decision Record / Devlog Entry  
**Status:** Accepted  
**Date:** 2026-11-15  
**Project:** Valescent Engine  
**Owner:** Project Owner  
**Supersedes:** None  
**Scope:** The on-disk chunk file format and save policy: `VCHK` v1–v3, one file per saved chunk, block IDs, modified-only writes, and empty modified chunks. The world seed file, config file, compression, checksums, encryption, region files, atomic-write protocols, backups, and networking are out of scope.

---

## Summary

This document records why chunk persistence uses `VCHK`, a small versioned binary format with one file per chunk. It is both an official decision record and a devlog entry: the decision is final for the current scope, and I am publishing the reasoning so future decisions can be traced back to it.

The short version: chunks are saved and loaded independently as the player streams through the world. A compact binary file with a magic value, format version, chunk coordinates, block count, and sparse block records gives per-chunk isolation, explicit versioning, and no dependency on the in-memory layout. v3 adds a `block_count = 0` case so a fully cleared modified chunk does not regenerate from the seed. The tradeoff is manual format code and many small files, with no checksum, compression, or atomic write in v1–v3.

---

## 1. Context

### 1.1 The project

Persistence has to fit the world model rather than wrap it:

- **Chunked, streamed world.** Chunks load and unload independently as the player moves. Saving a chunk must not require rewriting the whole world.
- **Modified-only persistence.** v0.0.7 generates unmodified chunks from the seed. Only player-modified chunks are stored; unmodified chunks are regenerated.
- **Sparse block data.** A 32×32×32 chunk contains at most 32,768 blocks, and most of them are air. Saving every cell wastes space.
- **Derived light.** Sky and block light are computed from blocks and are not part of the save. They must never become stale on disk.
- **Evolving blocks.** Block IDs grew from `1..=5` to `1..=7` and then `1..=8`. The format needs an explicit version and stable IDs.
- **Single-player, single-writer.** There is no server, sync protocol, or concurrent writer. The save path can stay simple.
- **Corruption isolation.** A bad chunk file must not take down the whole world or panic.

These requirements favour a small, explicit, per-chunk binary format over a serialization framework or one monolithic world file.

### 1.2 The constraints

My choice is constrained by the following:

- One chunk must be written and read independently.
- A save file must be versioned so block additions remain loadable.
- Byte order and record layout must be explicit and platform-independent.
- The format must not depend on Rust's in-memory layout, padding, or `unsafe` casts.
- Derived light must not be serialized.
- The format must support sparse non-air records.
- A fully cleared modified chunk must be able to persist as empty.
- A malformed file must produce a typed error, not a panic or a partial world.
- The save path must not require `serde`, a database, or another large dependency.
- Save-on-unload, save-on-exit, and `Ctrl+S` must all use the same format.

---

## 2. Decision

**Chunk saves use one `VCHK` binary file per saved chunk under `world/chunks/`, named `chunk_{x}_{y}_{z}.vchk`.**

The file contains:

- Magic bytes `VCHK`.
- A little-endian format version.
- Little-endian chunk coordinates.
- A little-endian block count.
- Sparse `(local x, local y, local z, block ID)` records.

Block IDs are explicit and stable. Light is never written. Records are sorted on save and
rejected when duplicated on load. The full byte tables are owned by the v0.0.4, v0.0.6, and
v0.0.7 specs.

Versions:

- v1: block IDs `1..=5`.
- v2: block IDs `1..=7` for Lamp and Water.
- v3: block IDs `1..=8` for Sand, plus `block_count = 0` for an explicitly emptied modified
  chunk.

The loader accepts v1, v2, and v3. The writer writes the latest version. v0.0.4 and v0.0.6 save
every non-empty chunk; v0.0.7 saves only modified chunks. Empty modified chunks are tracked in
a separate modified-coordinate set and saved as v3 files with zero records.

The exact header and record layout remain in the milestone specs. This ADR records the format
choice and save policy, not a second copy of the byte table.

I accept this decision for the current scope. I will revisit it only if the assumptions in this
document stop holding.

---

## 3. Why I Chose VCHK

### 3.1 Per-chunk streaming fits the world model

The engine loads and unloads chunks independently. One file per chunk lets unload save exactly
the chunk being dropped and lets startup load only the files that are needed. A single world
file would force a full rewrite on every unload and make modified-only persistence awkward.

### 3.2 Versioning is explicit

The magic, version, and stable block IDs make old files loadable after new blocks arrive. v1,
v2, and v3 can coexist in the loader; the writer moves forward. The version is in the file,
not inferred from the engine build or the presence of a field.

### 3.3 Sparse records keep chunks small

Chunks are mostly air. Storing only non-air blocks avoids a dense 32,768-cell dump, and the
record layout gives each block a local coordinate plus a block ID. The fixed 32×32×32 chunk
makes the local coordinate range small and predictable.

### 3.4 Derived light stays derived

The save contains blocks only. Sky light, block light, and AO are recomputed by the v0.0.6
lighting pipeline after generation or load. That keeps the file smaller and prevents stale
lighting from outliving its block data.

### 3.5 Modified-only persistence needs an empty case

v0.0.7 regenerates unmodified chunks from the seed. A modified chunk with every block removed
must still persist; otherwise generation would restore the terrain. v3 allows `block_count = 0`
for exactly that case, and the world tracks modified empty chunk coordinates separately from
the non-empty chunk map.

### 3.6 No in-memory layout coupling

The format is written and read field by field. It does not cast a `Chunk` struct to bytes, does
not depend on `HashMap` iteration order, and does not inherit Rust padding or endianness. This
follows the warning in ADR-0005: persistence is an explicit format, not a raw cast.

### 3.7 Deterministic files

Records are sorted by local `(x, y, z)` on save, and chunk files are written in ascending
`ChunkCoord` order. The same chunk with the same blocks produces the same bytes, which makes
tests and debugging straightforward.

### 3.8 Small code and no heavy dependency

The format needs only `std::fs`, byte conversion, and a compact writer/reader. It does not pull
in `serde`, `bincode`, or a database. That keeps the dependency surface small and the save
format under direct control.

### 3.9 Why not a text format, a framework, or a single file?

- **JSON or TOML** is readable but larger and slower, introduces parsing and schema
  dependencies, and is a poor fit for many small binary chunk records.
- **`serde` plus `bincode`** removes some manual code, but still needs an explicit schema and
  version policy; the format would be only slightly less hand-written while adding a
  dependency and a second representation.
- **A raw memory dump** is fast to write but couples the disk format to in-memory layout,
  padding, endianness, and `HashMap` order. ADR-0005 already rejects this for persistence.
- **One world file** makes per-chunk unload and modified-only saves inefficient and makes one
  corruption affect everything.
- **Region files** reduce file count for very large worlds, but add an index, allocation, and
  corruption-recovery layer that the current world does not need.
- **A database** would provide transactions and queries, but it is far more machinery than a
  single-player chunk save requires.

---

## 4. Alternatives Considered

| Alternative | Assessment | Outcome |
|---|---|---|
| **VCHK binary, one file per chunk** | Per-chunk isolation, versioned, sparse, deterministic, no heavy dependency, empty-modified support in v3. | **Selected** |
| **JSON / TOML text** | Human-readable and easy to inspect; larger, slower, parser and schema overhead, poor for many block records. | Not selected |
| **`serde` + `bincode` / RON** | Less manual code and a familiar model; still needs version policy, adds a dependency, and hides the byte contract. | Not selected |
| **Raw memory dump** | Fastest write path; ties disk to memory layout, padding, endianness, and hash order. | Not selected |
| **Single world file** | Simple file count and easy whole-world copy; full rewrites, poor streaming, one corruption affects all. | Not selected |
| **Region files** | Fewer files and better large-world behavior; index/allocation/recovery complexity is not needed yet. | Not selected |
| **SQLite / embedded database** | Transactions and queries; heavyweight dependency and runtime for a simple chunk store. | Not selected |
| **Compression** | Smaller files; extra dependency and CPU cost for sparse 32³ chunks. | Not selected for v1–v3; revisit if disk use matters |
| **Checksums** | Detects random corruption; extra format field and validation. | Not selected for v1–v3; revisit with a format version |

---

## 5. Counterarguments and Disposition

This decision has real weaknesses. I am recording them here rather than omitting them, so the
record stays honest.

| Counterargument | Disposition |
|---|---|
| **Many small files can stress the filesystem.** | Accepted. The current world is small and per-chunk streaming benefits from independent files. A region file is the documented next step if file count becomes a measured problem. |
| **There is no checksum.** Random bit flips may survive structural validation. | Accepted. The loader validates magic, version, chunk coordinates, block count, local coordinates, block IDs, duplicates, and trailing bytes, but not file integrity. A checksum can be added in a later format version. |
| **There is no compression.** Disk use is higher than a packed format. | Accepted. Sparse records already avoid dense chunk dumps, and compression adds CPU/dependency cost. Revisit when measured disk use justifies it. |
| **There is no atomic-write protocol.** A crash during a write can leave a truncated file. | Accepted. The loader rejects truncated or trailing data. A future format revision can write a temporary file and rename it; that is not part of v1–v3. |
| **There is no cross-chunk transaction.** A crash can save some modified chunks and not others. | Accepted. Unmodified chunks regenerate, and modified chunks are independent. Full-world snapshots are out of scope. |
| **The format is manual code.** Parser and writer bugs are possible. | Accepted. The byte layout is small, versioned, and covered by round-trip and malformed-input tests; typed errors keep failures visible. |
| **A zero block count is a special case.** | Accepted. v3 uses it only for an explicitly empty modified chunk; v1/v2 still require at least one block. The loader skips generation for that case. |
| **Block IDs must never drift.** Reordering the enum would corrupt saves. | Accepted. Block IDs are explicit and tested; the enum order is not the format contract. |
| **Little-endian is fixed.** | Accepted. Conversion is explicit at read/write boundaries, and every supported target can read little-endian data. |
| **The seed file is separate.** A missing seed with saved chunks is unrecoverable. | Accepted. The seed is a world-level input, not a chunk record. A missing seed with chunk files is a typed error rather than a silent new world. |
| **Files can be inspected only with tooling.** | Accepted. A small format is easy to hex-dump; there is no editor or human-readable schema. |

---

## 6. Consequences

### 6.1 Positive

- One chunk can be saved or loaded without touching the rest of the world.
- Unmodified chunks can be regenerated from the seed and never written.
- A fully cleared modified chunk persists instead of regenerating.
- Block additions are versioned and old files remain loadable.
- Derived light never becomes stale on disk.
- The format is independent of Rust layout, padding, and hash order.
- Sparse records keep chunks compact without a compression dependency.
- Sorted records and chunks produce deterministic files for tests.
- Structural corruption produces typed errors instead of panics.

### 6.2 Negative

- A world contains many small files.
- v1–v3 have no checksum, compression, or atomic-write protocol.
- There is no cross-chunk transaction or backup system.
- The parser and writer are manual code with a version matrix.
- Empty modified chunks require separate bookkeeping outside the non-empty chunk map.
- A missing seed file with saved chunks is an error, not a recoverable world without user input.

### 6.3 Neutral

- The exact byte tables live in the milestone specs; this ADR records the format choice.
- The seed file and config file remain separate formats.
- Region files, compression, checksums, and atomic writes can be added by a later versioned
  decision.
- The format stores blocks only; lighting, meshes, and stream state are runtime data.

---

## 7. Risks and Mitigations

| Risk | Mitigation |
|---|---|
| A chunk file is malformed or truncated. | Validate magic, version, coordinates, count, records, duplicates, and trailing bytes; return a typed error and abort loading without a partial world. |
| A crash interrupts a write. | The loader rejects a partial file. A future format version can use temp-file-and-rename; document the current limitation. |
| A fully cleared chunk regenerates. | Track modified empty chunk coordinates and write a v3 file with `block_count = 0`; test the round-trip. |
| Block IDs drift. | Keep explicit stable IDs, never derive them from enum order, and test every block round-trip. |
| Light is accidentally serialized. | The format has no light fields; light is recomputed after load; test that v1–v3 loaders do not expect light. |
| The seed is missing with saved chunks. | Return a typed error instead of choosing a new seed silently. |
| File count becomes a problem. | Measure; if needed, introduce region files in a later format decision. |
| Disk use becomes a problem. | Measure; add compression in a later format version if the sparse format is insufficient. |
| The parser and writer diverge. | Keep one small writer/reader pair, round-trip every block and version, and reject unknown input in tests. |
| Unknown version or block ID. | Typed error; the writer is the only forward path; old loaders are not required to read future versions. |

---

## 8. Conclusion

`VCHK` is my deliberate, streaming-aware choice for chunk persistence. I am selecting it because
one file per chunk matches unload and modified-only saves, because an explicit magic/version/
block-ID contract keeps old worlds loadable, and because sparse little-endian records avoid
both dense chunk dumps and coupling to Rust's memory layout. I acknowledge that the format has
no checksum, compression, atomic write, or cross-chunk transaction, and that many small files
are a real cost. I manage those through structural validation, typed errors, deterministic
order, versioned loading, and a documented path to region files, compression, or checksums if
measurements justify them. I therefore accept this decision as the engine's chunk save format,
with the exact byte layout owned by the milestone specs and with its limits recorded above.

---

*This entry is part of the Valescent Engine decision log. Future entries should reference this document when they depend on chunk persistence or change the save-format version.*

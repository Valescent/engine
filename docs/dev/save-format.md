# Save format

**Status:** Format complete; implementation in progress. Header (version 2 with `migration_state`),
journal framing, the v2 section record, the section/journal writer, the IO thread, the
`TERRAIN_DEMO` open/save path, journal replay, prefix trimming, `world-repair`, the in-app repair
prompt, and the migration journal/resume are in code. Migration can upgrade v1 headers and
canonicalise section records; a decoder for a genuine older record format does not exist yet. See
"Implementation status" at the end.

**Normative summary:** [Terrain spec](../specs/terrain.md) §16. If this page and the code disagree,
the code wins and this page is a bug.

## Directory

```text
world/
  header.bin            fixed layout, verified first
  description.toml      canonical generation parameters
  sections/             column files (layout Open)
  journal/              edit frames, append-only
  migration.journal     only while a migration is in progress
  migration-report-<date>.json
```

All integers are little-endian. All strings are UTF-8.

## header.bin

Written with write-temp → fsync → rename → fsync-directory. Never rewritten in place.

| Offset | Width | Field |
|---|---|---|
| `0..4` | 4 | magic `"TWLD"` |
| `4..6` | 2 | `format_version: u16` |
| `6..8` | 2 | `generator_version: u16` |
| `8..40` | 32 | `description_hash: [u8; 32]` (SHA-256) |
| `40..41` | 1 | `hash_algorithm` length, then that many bytes (`"sha256/v1"`) |
| … | 8 | `seed: u64` |
| … | 1 + n | `platform_tag` length, then bytes |
| … | 1 + n | `last_saved_platform_tag` length, then bytes |
| … | 16 | `world_id: [u8; 16]`, never reused; a Fork assigns a new one |
| … | 1 + n | `name` length, then bytes (display only) |
| … | 8 | `created: i64` (UTC Unix seconds) |
| … | 8 | `last_saved: i64` (UTC Unix seconds) |
| … | 8 | `revision: u64` |
| … | 2 | `schema_version: u16` |
| .. | 1 + 0/12 | `migration_state` (version 2): tag `0` = none, tag `1` = `from_format` `to_format` `started` |
| … | 4 | `crc32` over every preceding byte |

Length-prefixed text is capped at 255 bytes; a longer value is refused at write time rather than
truncated. `created` and `last_saved` are diagnostics and never ordering. The header stores no
derived state, runtime config, or per-section data.

### Load order

1. Read the header, verify magic and `crc32`.
2. `format_version` must be one this build reads.
3. `description.toml` must exist and parse.
4. Recompute `description_hash` and compare.
5. `generator_version` must be shipped (or the world opens only through the migration dialog). This
   build ships version 4 (world-mode hydrology, the per-column biome dither, and the retuned desert centre); a version 1, 2, or 3
   world is refused, because its water levels and biome borders would change.
6. Validate the content pack against the pinned records. Textures are excluded from
   `record_hash`, so a re-texture never fails this check (`ADR-0015`).
7. Recover the journal (below).

Any failure aborts the load and leaves the directory byte-identical.

## Edit journal

One frame per committed batch. A frame is:

```text
len: u32          payload length in bytes
revision: u64     batch revision
payload: [u8; len]
crc32: u32        crc32(revision bytes) continued over payload
```

Payload:

```text
author: u32       batch author (0 = system)
count: u32        record count
count × record
```

Record:

```text
x: i32, y: i32, z: i32     world block coordinates
seq: u32                   position in the batch
author: u32
batch: u32
old: name                  u32 length + UTF-8
new: name                  u32 length + UTF-8
```

Names, never runtime ids. A record with `old == new` is not written. Undo is a new batch with a new
revision; it is never a rewrite.

### Recovery

Scan frames in order. A frame that ends past the end of the file is a **torn tail**: keep the prefix
up to the last complete frame, truncate the file there, and continue. A *complete* frame whose crc
fails is an error (`ErrorKind::JournalTornTail` for the notice, or a framing error), never silently
dropped together with everything after it.

Frames are applied only for revisions greater than the last committed section save, once, in total
order. The edit log is an audit and undo trail; it is never replayed over saved sections. A section
save advances the header `revision` to the greatest revision its blocks include and then drops the
journal frames at or below it; a flush alone never advances the revision, so a frame that no section
covers stays available for replay.

### Writer contract

`JournalWriter` implements group commit; the IO thread is the caller's responsibility. It MUST:

- pass a monotonic millisecond counter whose origin is writer creation (the writer starts at 0;
  backward values are clamped);
- recover and truncate a torn tail, and position the sink at the recovered end, **before**
  constructing the writer — the writer never opens, seeks, or truncates;
- treat a failed `write_all` as session-fatal and recover on reopen. `File::write_all` can append
  partially before erroring, so the journal is not blindly retried in place;
- call `flush()` on pause, quit, and before any section save. The 10 s barrier is evaluated during a
  flush, so an idle tick returns `Idle` without syncing;
- never rely on `into_sink` to flush: pending frames are discarded by design.

A sink error becomes `TerrainError` with `Stage::Persistence` and `ErrorKind::IoError`, naming
`write_all` or `sync_data`. A write failure leaves the buffer and `pending_batches` intact; a sync
failure leaves the bytes written and `pending_batches` cleared.

## Section record

One record per section, magic `TSEC`, **version 2**. Little-endian throughout; `crc32` is the IEEE
reflected polynomial from `src/header.rs`.

| Offset | Width | Field |
|---|---|---|
| `0..4` | 4 | magic `"TSEC"` |
| `4..6` | 2 | record version `u16 = 2` |
| `6..10` | 4 | column `col_x: i32` |
| `10..14` | 4 | column `col_z: i32` |
| `14..15` | 1 | section slot `sy: u8` (`0..12`) |
| `15..16` | 1 | flags: bit 0 = `edited`; every other bit MUST be zero |
| `16..48` | 32 | `baseline_hash: [u8; 32]` |
| `48..49` | 1 | kind: `0 = Uniform`, `1 = Paletted` |
| `49..53` | 4 | solid count `u32` (cells whose id is not `core:air`) |
| `53..len-4` | … | body |
| `len-4..len` | 4 | `crc32` over every preceding byte |

Body:

- **Uniform** — `u16` name byte length, then UTF-8. No index payload.
- **Paletted** — `u8 bits` (`1/2/4/8/16`), `u32 count`, then `count ×` (`u16` length + UTF-8 name)
  **sorted by name**, then the index words as `u64` LE. The word count is derived from `bits`
  (`512/256/128/64/32` for `1/2/4/8/16`) and is never stored.

Names, never runtime ids (`A5.1`, `T1.6`). Index lanes are repacked on encode so a lane is the
position in the sorted name list: two sections with equal content encode identically regardless of
runtime palette order, and the bit width is the smallest that fits.

`baseline_hash` is SHA-256 over the decoded ids as `u16` LE in section index order, taken at first
generation and never rewritten (`J4.1`, `J4.2`). The save writer and `terrain-gen verify` MUST use
this one derivation.

Unknown-name policy: a name missing from the pack resolves to a placeholder from
`content::UNKNOWN_BASE`, one per distinct name, and is reported (`SavedSection::unknown_names`);
exhausting that range is `ManifestIncompatible`. A missing `core:air`, `core:bedrock`, or
`core:water` is `ManifestIncompatible`.

**A world that loaded any unknown name is read-only**: no edits, no journal, no section saves until
the content is restored or aliased. This is what keeps the round trip lossless — the original names
are never re-encoded as `?unknown:<id>`, and no name-override map is needed.

Hard errors: bad magic/version/flags/slot/kind, zero or oversized palette, non-minimal or impossible
`bits`, invalid or duplicate name, out-of-range index, truncated or trailing bytes, bad CRC. All are
`CorruptSection` with the file path (`S1.5`), never a silent repair.

A saved section is never regenerated; one rolling backup generation exists per section.

### Backup rotation

Per section, exactly one previous generation is kept:

| File | Meaning |
|---|---|
| `c.<cx>.<cz>.<sy>.sect` | Current, authoritative |
| `c.<cx>.<cz>.<sy>.sect.bak` | Previous generation, verified |
| `c.<cx>.<cz>.<sy>.sect.tmp` | In-flight write, never trusted directly |
| `c.<cx>.<cz>.<sy>.sect.corrupt` | Damaged current moved aside by `world-repair`; never read |

Save sequence: write `.tmp` → fsync → re-read `.tmp` and verify its CRC → rename current to `.bak`
(replacing) → rename `.tmp` to current → fsync the directory. The journal prefix covered by the
section is dropped only after the last step.

Recovery on open: if the current file is missing and `.tmp` verifies, promote `.tmp`; if the current
fails its CRC and `.bak` verifies, offer `world-repair` (restore `.bak`, quarantine, or salvage) and
refuse the normal load. A stale `.tmp` is deleted on a successful open. Only one generation is kept;
older backups are overwritten by the next successful save.

## Migration

While migrating, the header carries `migration_state = InProgress { from_format, to_format, started }`
and the world opens only to the resume dialog. Sections are transformed in sorted key order, each
with temp → fsync → rename plus a `migration.journal` record. A section already in either format is
accepted by crc, so resume is idempotent. The original is byte-identical on Fork and Pin.

`migration.journal` is append-only. Each record is 22 bytes:

| Offset | Width | Field |
|---|---|---|
| `0..4` | 4 | `col_x: i32` |
| `4..8` | 4 | `col_z: i32` |
| `8..9` | 1 | `sy: u8` |
| `9..10` | 1 | reserved, zero |
| `10..14` | 4 | `old_crc: u32` |
| `14..18` | 4 | `new_crc: u32` |
| `18..22` | 4 | `crc32` over the preceding 18 bytes |

A torn tail is truncated at the last complete record. On resume, a section whose current CRC equals
`new_crc` is skipped; one whose CRC equals `old_crc` is still pending. The journal is deleted only
after the header's `migration_state` is cleared.

## Implementation status

The format above is complete; this table is about code, not design. No open format questions remain.

| Area | Implemented | Missing |
|---|---|---|
| `header.bin` | Read/write, CRC, atomic replace, load order | — |
| Edit journal framing | `encode_batch`, `decode_batches`, torn-tail truncation, `CommitPolicy`, `JournalSink`, `JournalWriter`; `save::open_journal` creates the file and recovers the tail; `world_save::SaveWorker` drives it on an IO thread; a section save trims the covered prefix | Migration-aware replay |
| Section record v2 | `encode_record`, `decode_record`, `decode_record_at`; `save::SectionStore` names files and writes/reads records; `section::baseline_hash` is the one derivation, shared with `terrain-gen verify` | — |
| Backup rotation | Design above; implemented in `save::SectionStore::write` and `read` | — |
| Engine wiring | `TERRAIN_DEMO=1` opens `world/terrain-demo`, layers saved sections over generation by `baseline_hash`, replays recovered journal frames newer than the header revision, saves those sections, then advances the header | Streaming/LOD world integration |
| Migration | Header version 1 upgrades to version 2 on open; `MigrationState::InProgress`, `migration.journal` records/resume, section canonicalisation, `terrain-gen migrate --world <dir>`, and the `migration-report-<date>.json` bundle are implemented | A decoder for a genuine older section record format |
| `world-repair` | `SectionStore::repair` implements restore-backup, quarantine, and salvage; `terrain-gen repair --world <dir> --section <cx,cz,sy> --action restore\|quarantine\|salvage` is the CLI; `TERRAIN_DEMO=1` shows an in-app prompt with `R`/`Q`/`S` or left/middle/right click and resumes saves after the choice | — |

`save::SectionStore` writes and reads section files with write-temp, fsync, verify, rotation to
`.bak`, and a directory fsync where the platform supports one (Windows has none). Opening promotes a
verified `.tmp` when the current file is missing, deletes a stale `.tmp`, and refuses a corrupt
current rather than silently replacing it. `world_save::WorldSave` applies the `A1.5` load order and
`world_save::SaveWorker` runs the writes on one IO thread; `TERRAIN_DEMO=1` opens and saves
`world/terrain-demo`, replays recovered frames newer than the header revision, and trims the journal
prefix covered by each section save. Unit tests cover section round trips, one-generation rotation,
temp promotion and refusal, stale-temp cleanup, journal reopen, torn-tail truncation, a corrupt
complete frame, worker ordering, the world load order, revision advancement, journal replay over
generation and over a saved section, prefix trimming, the three `world-repair` outcomes, a version 1
header upgrading to version 2, migration records/resume, section canonicalisation, and a demo edit
round-tripping through the journal and a section file. Corruption fuzzing covers section records,
headers, and journal truncation at every byte. Migration reports and a repeated crash/save/reopen loop are covered; migration fixtures for older record formats are not.

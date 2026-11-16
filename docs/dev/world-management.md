# World management

The engine has no world persistence yet. v0.0.2 keeps its demo voxels in memory, closing the
window loses them, and no `world/` directory is read or written. This page is the intended
operating procedure for the v0.0.4 save path and the v0.0.7 seed and modified-only changes. The
file layout and commands below do not apply until persistence lands.

| Area | Status |
|---|---|
| `world/` save directory | Planned (v0.0.4) |
| `VCHK` v1 chunks | Planned (v0.0.4) |
| `VCHK` v2 chunks | Planned (v0.0.6) |
| `VCHK` v3, `world/seed`, modified-only saves | Planned (v0.0.7) |
| Built-in backups | None; copy the world directory manually |
| Atomic writes, checksums, compression | None in v1-v3; see [ADR-0008](../decisions/0008-chunk-save-format.md) |
| Built-in repair tool | None |
| Autosave | None; saves happen on exit, `Ctrl+S`, and v0.0.7 chunk unload |
| `world/` in Git | Ignored |

## World directory

The planned layout is:

```text
world/
  seed
  chunks/
    chunk_{x}_{y}_{z}.vchk
```

- `world/` is resolved against the process working directory. Running `cargo run` from the
  repository root puts it at the repository root; launching the binary elsewhere uses that
  directory instead.
- `world/seed` exists from v0.0.7. It contains one decimal `u64` and an optional trailing newline.
  It contains no other bytes or text. The seed file must exist before the first chunk save.
- Saving creates `world/` and `world/chunks/` when they are missing.
- `world/chunks/` holds one `VCHK` file per saved chunk. Coordinates are decimal `i32` values;
  negative values keep their leading `-`.
- The v0.0.4-v0.0.6 writer saves every non-empty chunk. The v0.0.7 writer saves only modified
  chunks. A v0.0.7 world can therefore contain far fewer files than its loaded chunk count.
- An empty modified chunk in v0.0.7 is a valid 22-byte `VCHK` v3 file with `block_count = 0`. It is
  not a truncated file.
- `.gitignore` ignores `/world`. A backup folder created in the repository root is not ignored.
- There is no game `assets/` or `saves/` directory. The world is data, not an asset.

### Missing directory versus empty directory

The loader treats a missing directory and an empty directory differently:

| State | v0.0.4-v0.0.6 | v0.0.7 |
|---|---|---|
| `world/chunks/` missing | Load an empty world, then seed the demo scene | No saved chunks; generate from the seed |
| `world/chunks/` exists and is empty | Load an empty world; do not seed the demo | No saved chunks; generate from the seed |
| `world/seed` missing with `.vchk` files | Not applicable | Typed error; the chunks need a seed |
| `world/seed` missing without `.vchk` files | Not applicable | Generate and write a random seed from OS entropy |

This difference matters for reset: deleting the whole `world/` is a new demo world in v0.0.4, while
leaving an empty `world/chunks/` is an empty world.

## Save behavior

| Milestone | What is saved | When |
|---|---|---|
| v0.0.4-v0.0.6 | Every non-empty chunk | Clean exit and `Ctrl+S` |
| v0.0.7 | Every modified loaded chunk, including empty modified chunks | Clean exit, `Ctrl+S`, and modified-chunk unload during streaming |

- There is no periodic autosave. A crash loses edits made since the last save.
- v0.0.4-v0.0.6 remove `.vchk` files for chunks that are no longer non-empty, so a deleted chunk
  does not reload.
- v0.0.7 saves a modified chunk before dropping it during streaming. If that save fails, the error
  is logged and the chunk stays in memory until the next save. If the process is killed before a
  later save, the edits are lost.
- Save errors are typed and logged; they do not panic. A `Ctrl+S` error does not terminate the
  engine. An exit save error is logged before the process exits.
- v1-v3 have no atomic-write protocol. A crash during a save can leave a truncated file. The
  loader rejects it; it cannot repair it.
- v1-v3 have no cross-chunk transaction. A crash can save some chunks and not others. A backup is
  the clean snapshot.
- v1-v3 have no checksum or compression. Structural validation catches bad magic, version,
  coordinates, counts, records, duplicates, truncation, and trailing bytes. A bit flip that leaves
  a structurally valid file is not detected.

See [ADR-0008](../decisions/0008-chunk-save-format.md) for the format and save-policy decision.

## Backups

There is no built-in backup command or rotation. A backup is a copy of `world/` made while the
engine is stopped.

### What to copy

Copy the whole `world/` directory, not just `world/chunks/`.

- In v0.0.7, `world/seed` is required to regenerate unmodified chunks. A chunks-only backup is
  incomplete; a missing seed with saved chunks is a typed error, not a new world.
- In v0.0.4-v0.0.6, there is no seed file; `world/chunks/` is the whole world.
- Copying the whole directory also preserves any future world-level metadata.

Do not put backups inside `world/chunks/`. The loader scans `.vchk` files there, and a copied chunk
can produce a duplicate-chunk or filename/header mismatch. Keep backups outside `world/` and
outside the repository so they do not appear as untracked files.

### Stop first

Stop the engine before copying. The save path rewrites files, and v1-v3 do not write atomically;
copying during a save or an unload can capture a partial file. If you must copy while the engine is
running, press `Ctrl+S`, wait for the save to finish, accept the race with unload saves, and verify
the copy.

Cloud-synced folders, antivirus scanners, and a second process can lock or rewrite files. Keep the
world on a local disk and let sync finish before backing up.

### PowerShell

```powershell
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$backupRoot = Join-Path $env:USERPROFILE 'ValescentBackups'
$destination = Join-Path $backupRoot "world-$stamp"

New-Item -ItemType Directory -Force -Path $backupRoot | Out-Null
Copy-Item -LiteralPath .\world -Destination $destination -Recurse
```

Run this from the directory that contains `world/`, normally the repository root.

### POSIX

```sh
stamp=$(date +%Y%m%d-%H%M%S)
mkdir -p "$HOME/ValescentBackups"
cp -a world "$HOME/ValescentBackups/world-$stamp"
```

### Verify the backup

```powershell
$backup = Join-Path $env:USERPROFILE 'ValescentBackups\world-20260101-120000'
(Get-ChildItem (Join-Path $backup 'chunks') -Filter *.vchk -File).Count
Test-Path (Join-Path $backup 'seed')
Get-Content (Join-Path $backup 'seed')
```

```sh
backup="$HOME/ValescentBackups/world-20260101-120000"
find "$backup/chunks" -name '*.vchk' | wc -l
test -f "$backup/seed" && cat "$backup/seed"
```

`world/seed` is absent before v0.0.7. A v0.0.7 backup should contain it. A low `.vchk` count is
normal in v0.0.7 because only modified chunks are stored.

For an external integrity record, hash the backup:

```powershell
Get-ChildItem $backup -Recurse -File | ForEach-Object {
    Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256
} | Out-File "$backup.sha256"
```

```sh
find "$backup" -type f -exec sha256sum {} + > "$backup.sha256"
# macOS: use `shasum -a 256` instead of `sha256sum`.
```

The hash manifest is external; `VCHK` has no checksum field. It detects a changed backup, not
corruption inside the running world.

### When to back up

- Before resetting or restoring anything.
- Before upgrading the engine.
- Before changing block definitions, textures, or generation parameters.
- Before a long building session.
- Before testing a new build, a stress scene, or a save/load change.
- Before manual filesystem work.

Keep backups until the restored or reset world has been verified. There is no automatic rotation;
manage retention yourself.

## Restore

1. Stop the engine.
2. Move the current `world/` aside. Do not overwrite the only copy.
3. Copy the backup to `world/`.
4. Verify `world/chunks/` is present and, for v0.0.7, `world/seed` is present.
5. Start the engine and check stderr for save/load errors.
6. Verify the edits and terrain you expected.
7. Keep the old world folder until the restore is confirmed.

### PowerShell

```powershell
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$backupRoot = Join-Path $env:USERPROFILE 'ValescentBackups'
New-Item -ItemType Directory -Force -Path $backupRoot | Out-Null

Move-Item -LiteralPath .\world -Destination (Join-Path $backupRoot "world-before-restore-$stamp")
Copy-Item -LiteralPath (Join-Path $backupRoot 'world-20260101-120000') -Destination .\world -Recurse
```

### POSIX

```sh
stamp=$(date +%Y%m%d-%H%M%S)
mkdir -p "$HOME/ValescentBackups"
mv world "$HOME/ValescentBackups/world-before-restore-$stamp"
cp -a "$HOME/ValescentBackups/world-20260101-120000" world
```

### Restore a single chunk

Because chunks are independent files, you can restore one `.vchk` file:

```powershell
$backup = Join-Path $env:USERPROFILE 'ValescentBackups\world-20260101-120000'
Copy-Item -LiteralPath "$backup\chunks\chunk_0_0_0.vchk" `
    -Destination .\world\chunks\chunk_0_0_0.vchk
```

```sh
backup="$HOME/ValescentBackups/world-20260101-120000"
cp "$backup/chunks/chunk_0_0_0.vchk" world/chunks/chunk_0_0_0.vchk
```

The filename must match the chunk coordinates in the file header. Do not leave two files with the
same header coordinate in `world/chunks/`; the loader rejects the duplicate. A chunk restored from a
different world or seed is structurally valid but may produce seams or inconsistent terrain.
Restore chunks from the same world.

### Restore and versions

- v0.0.7 loads `VCHK` v1, v2, and v3 chunk files. Older chunk files are format-compatible; see the
  seed note below.
- An older build may not load v3. Do not edit the version byte to force a load.
- v0.0.4-v0.0.6 worlds have no seed file. v0.0.7 requires one when `.vchk` files exist. There is
  no automatic migration. To carry an old world forward, add a seed deliberately; saved chunks
  apply over generation with that seed, so unmodified terrain may change. If exact preservation
  matters, keep using the old build.
- A saved chunk loaded by v0.0.7 is marked modified; a v1 or v2 file may be rewritten as v3 on the
  next save.

## Corruption recovery

The loader aborts the world load on a malformed, unsupported, or unreadable chunk file. It does not
partially load the world. In v0.0.4-v0.0.6 this happens at startup; in v0.0.7 it can also happen
when a chunk enters the streaming load set. One bad file can block the world.

### Preserve first

1. Stop the engine.
2. Copy the entire `world/` to a quarantine location before touching it. A corrupt world is still
   evidence, and a failed repair should not destroy the last copy.
3. Read the error message. It names the file and the reason.

The typed errors are:

| Error | Meaning | Safe action |
|---|---|---|
| `InvalidFileName` | A `.vchk` file does not match `chunk_{x}_{y}_{z}.vchk` | Rename it to the header coordinates if the header is valid; otherwise move it out |
| `InvalidMagic` | The file does not start with `VCHK` | Move it out; restore it from backup if it was a real chunk |
| `UnsupportedVersion` | The file version is newer than this build | Upgrade the engine, or restore an older backup; do not edit the version byte |
| `InvalidChunkCoordinate` | Header coordinates do not fit the world model | Move it out; restore from backup |
| `InvalidBlockCount` | Count is outside the version's allowed range | Move it out; restore from backup |
| `InvalidLocalCoordinate` | A record has a local coordinate outside `0..=31` | Move it out; restore from backup |
| `InvalidBlockId` | A block ID is unknown for the file version | Upgrade the engine, or restore a compatible backup |
| `DuplicateRecord` | Two records use the same local coordinate | Move it out; restore from backup |
| `DuplicateChunk` | Two files have the same header chunk coordinate | Move the less trusted file out, then retry |
| `ChunkMismatch` | The filename and header coordinates differ | If the filename is the mistake, rename it to the header coordinates; otherwise restore from backup or move it out |
| `Truncated` | The file ends before the declared records | Move it out; restore from backup |
| `TrailingBytes` | The file has bytes after the declared records | Move it out; restore from backup |
| `Io` | Permissions, a lock, a deleted file during load, or a disk error | Fix the filesystem problem; this is not a format repair |

### Quarantine

Create a quarantine folder outside `world/`, for example:

```text
world-quarantine-20260101-120000/
  chunk_0_0_0.vchk
```

Move the bad file there, preserving its name. Do not rename it to another `.vchk` inside
`world/chunks/`; the loader would still see it. Then start the engine.

What happens to the missing chunk:

- v0.0.4-v0.0.6: the chunk loads as air/empty. Its edits are lost, but the rest of the world loads.
- v0.0.7: the chunk regenerates from the seed. Its edits are lost; the generated terrain returns.

Restore the file from backup if you have one. Otherwise accept the loss of that chunk. If more than
one file is bad, repeat the process; the loader names one file at a time.

### Inspect a chunk header

The `VCHK` header is small. For diagnosis, inspect the first 32 bytes:

```powershell
Format-Hex .\world\chunks\chunk_0_0_0.vchk -Count 32
```

```sh
xxd -l 32 world/chunks/chunk_0_0_0.vchk
```

| Offset | Size | Field |
|---:|---:|---|
| 0 | 4 | Magic `VCHK` |
| 4 | 2 | Version, little-endian (`1`, `2`, or `3`) |
| 6 | 4 | Chunk X, little-endian `i32` |
| 10 | 4 | Chunk Y, little-endian `i32` |
| 14 | 4 | Chunk Z, little-endian `i32` |
| 18 | 4 | Block count, little-endian `u32` |
| 22 | `4 * block_count` | Records: local X, Y, Z (each `u8`), block ID (`u8`) |

The expected file size is `22 + 4 * block_count`. A v3 empty modified chunk has count `0` and is
exactly 22 bytes. v1 and v2 require at least one record. A file shorter or longer than the declared
size is truncated or has trailing bytes.

Do not hand-edit block records, local coordinates, block IDs, or version bytes. There is no repair
tool, and a structurally valid edit can still corrupt the world silently. Restore from backup or
quarantine the file.

### Seed problems

`world/seed` is required from v0.0.7 when `.vchk` files exist. It contains one decimal `u64` and an
optional trailing newline, with no other bytes.

- Missing seed with `.vchk` files: restore `world/seed` from backup. Do not delete the chunks to
  make the error disappear unless you intend to lose the world.
- Malformed seed: restore it from backup. A text editor may add a BOM, spaces, or a Windows CRLF;
  write exactly the decimal digits and at most one `\n`.
- If the original seed is lost and there is no backup, it cannot be derived from the chunk files.
  Restoring a full backup is the only way to recover the same world. Creating a new seed is a
  salvage operation: edited chunks still load, but unmodified terrain is generated from the new
  seed and may not match the old world.
- Do not delete `world/seed` alone while keeping chunks. That is the missing-seed error, not a
  reset.

### Structurally valid but wrong files

A file from another world or seed can pass validation and still produce seams or unexpected
terrain. That is not a format error. Restore the correct chunk from the matching world backup.
Likewise, a bit flip that leaves a valid magic, version, coordinates, count, and record set is not
detected. Backups and external hashes are the only defence.

### When there is no backup

- Quarantine the bad chunk and accept that chunk's loss.
- If the seed is lost, restore a full backup; otherwise a new seed is a salvage, not a recovery.
- Do not edit the binary file unless you can parse and verify the complete format and have a copy.
  The supported recovery path is restore or quarantine.

## Reset

Reset is a filesystem operation. Stop the engine and back up `world/` first.

| Goal | Action | Result |
|---|---|---|
| New world with a new seed (v0.0.7) | Move `world/` aside | New random seed; generated terrain; no edits |
| Same seed, discard all edits (v0.0.7) | Move `world/chunks/` aside; keep `world/seed` | Same generated terrain; all edits gone |
| Discard one chunk's edits (v0.0.7) | Move that `.vchk` file aside | That chunk regenerates; other edits remain |
| New demo world (v0.0.4-v0.0.6) | Move `world/` aside so `world/chunks/` is missing | Demo scene is seeded on first run |
| Empty world (v0.0.4-v0.0.6) | Keep an empty `world/chunks/` directory | Empty world; no demo seed |
| Clear one v0.0.4-v0.0.6 chunk | Move that `.vchk` file aside | The chunk becomes air; the rest loads |

### New world

Move the whole directory aside; do not delete the only copy:

```powershell
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$backupRoot = Join-Path $env:USERPROFILE 'ValescentBackups'
New-Item -ItemType Directory -Force -Path $backupRoot | Out-Null
Move-Item -LiteralPath .\world -Destination (Join-Path $backupRoot "world-reset-$stamp")
```

```sh
stamp=$(date +%Y%m%d-%H%M%S)
mkdir -p "$HOME/ValescentBackups"
mv world "$HOME/ValescentBackups/world-reset-$stamp"
```

Start the engine. v0.0.7 creates a new `world/seed` from OS entropy and generates a new world.
v0.0.4-v0.0.6 seed the demo scene because `world/chunks/` is missing. Once the new world is
verified, delete the moved folder if you no longer need it.

### Same seed, discard edits

In v0.0.7, move `world/chunks/` aside and keep `world/seed`:

```powershell
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$backupRoot = Join-Path $env:USERPROFILE 'ValescentBackups'
New-Item -ItemType Directory -Force -Path $backupRoot | Out-Null
Move-Item -LiteralPath .\world\chunks -Destination (Join-Path $backupRoot "chunks-reset-$stamp")
```

```sh
stamp=$(date +%Y%m%d-%H%M%S)
mkdir -p "$HOME/ValescentBackups"
mv world/chunks "$HOME/ValescentBackups/chunks-reset-$stamp"
```

The next launch sees no saved chunks and regenerates the same terrain from the seed. All edits are
gone. Do not delete `world/seed`.

### Discard one chunk

Move the `.vchk` file out of `world/chunks/`. In v0.0.7 the chunk regenerates from the seed; in
v0.0.4-v0.0.6 it becomes air. Other chunks are untouched.

### Choose a seed

v0.0.7 creates a random seed from OS entropy when `world/seed` is missing and no `.vchk` files
exist. To choose a seed for a new world, create the file before first launch:

```powershell
New-Item -ItemType Directory -Force -Path .\world | Out-Null
[System.IO.File]::WriteAllText(
    (Join-Path $PWD 'world\seed'),
    "1234567890123456789`n",
    [System.Text.Encoding]::ASCII
)
```

```sh
mkdir -p world
printf '1234567890123456789\n' > world/seed
```

Do not add a BOM, spaces, comments, or Windows CRLF; the file must contain one decimal `u64` and an
optional trailing newline. Changing the seed of an existing world changes the unmodified terrain
and can produce seams where saved chunks meet. Do not edit the seed unless you intend that.

### Reset and Git

`world/` is ignored by Git. `git status` does not show it, and `git clean -fd` does not remove it
because it is ignored. Do not use Git to manage worlds. Do not run `git clean -fdx` to reset a
world; it also removes other ignored files. Use the backup and move procedure above.

### Stress mode

`ENGINE_STRESS_SCENE=1` (planned v0.0.5) selects an in-memory stress world. It must not read or
modify `world/chunks/`, and it disables save on exit and `Ctrl+S`. Do not use stress mode to test
backups, restores, or resets. Unset it to use the normal world.

## Version compatibility

| Format | Introduced | Block IDs | Notes |
|---|---|---|---|
| v1 | v0.0.4 | `1..=5` | Every non-empty chunk saved; no seed file |
| v2 | v0.0.6 | `1..=7` | Lamp and Water; no seed file |
| v3 | v0.0.7 | `1..=8` | Sand, empty modified chunks, modified-only saves, `world/seed` |

- The v0.0.7 loader accepts v1, v2, and v3. Older builds accept only the versions they know. The
  writer writes the latest version supported by that build.
- Older chunk files load in newer builds. Newer chunk files may not load in older builds.
- Do not edit the version byte or unknown block IDs to force a load. Upgrade the engine or restore
  a compatible backup.
- A pre-v0.0.7 world has no seed. v0.0.7 requires one when chunks exist. There is no automatic
  migration; adding a seed changes unmodified terrain. Keep the old build if exact preservation
  matters.
- Back up before an upgrade and keep the pre-upgrade backup until the new build has loaded and
  saved the world successfully.

## Validate

After a backup:

- The destination contains `world/chunks/` and, for v0.0.7, `world/seed`.
- The `.vchk` count and total size are plausible. A v0.0.7 world with few files is normal; only
  modified chunks are stored.
- The optional hash manifest matches.

After a restore or quarantine:

- The engine starts without a save/load error.
- The affected chunk is air/regenerated as expected.
- The rest of the world and the edits you expected are present.
- `RUST_LOG=engine=debug,wgpu=warn` shows no save errors.

After a reset:

- `world/seed` exists and contains one decimal `u64` (v0.0.7).
- No old `.vchk` files remain.
- The expected demo, empty, or generated world appears.
- The first save creates the expected files.

Useful checks:

```powershell
(Get-ChildItem .\world\chunks -Filter *.vchk -File).Count
Get-Content .\world\seed
Format-Hex .\world\chunks\chunk_0_0_0.vchk -Count 32
```

```sh
find world/chunks -name '*.vchk' | wc -l
cat world/seed
xxd -l 32 world/chunks/chunk_0_0_0.vchk
```

The v0.0.4 acceptance check is: save, close, and reopen restores the edited world; a malformed
chunk file produces a clear error instead of a panic. The v0.0.7 checks add modified-only saves,
empty modified chunks, seed creation, and the missing-seed error. See [Testing](testing.md) and
the [v0.0.4](../specs/v0.0.4.md) and [v0.0.7](../specs/v0.0.7.md) specs.

## Operating checklist

- Stop the engine before backing up, restoring, or resetting.
- Back up the whole `world/`, including `world/seed` in v0.0.7.
- Store backups outside the repository and outside `world/chunks/`.
- Verify the backup before a destructive action.
- Move or quarantine files instead of deleting the only copy.
- Restore the seed together with the chunks.
- Do not hand-edit version bytes, block IDs, or seed files unless the change is intentional.
- Check stderr for save/load errors after every restore or reset.
- Keep the old world or backup until the new state is verified.
- Do not commit `world/` or backups.

## References

- [ADR-0008: chunk save format](../decisions/0008-chunk-save-format.md)
- [v0.0.4 spec](../specs/v0.0.4.md)
- [v0.0.5 spec](../specs/v0.0.5.md)
- [v0.0.6 spec](../specs/v0.0.6.md)
- [v0.0.7 spec](../specs/v0.0.7.md)
- [Configuration](configuration.md)
- [Debugging](debugging.md)
- [Testing](testing.md)
- [Conventions](conventions.md)

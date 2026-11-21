//! Fixtures and helpers shared by the `terrain-gen` integration tests.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use terrain_core::content;
use terrain_core::description;
use terrain_core::hash::sha256;
use terrain_core::header::{self, HASH_ALGORITHM, HEADER_FORMAT_VERSION, Header, MigrationState};
use terrain_core::pipeline::Terrain;
use terrain_core::section::{self, Section, SectionKey};
use terrain_core::world_save::GENERATOR_VERSION;
pub(crate) const BIN: &str = env!("CARGO_BIN_EXE_terrain-gen");
/// The vertical section index a fixture uses: it contains the sea-level surface.
pub(crate) const FIXTURE_SECTION_Y: i32 = 4;

pub(crate) fn run(args: &[&str]) -> Output {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    Command::new(BIN)
        .args(args)
        .current_dir(root)
        .output()
        .expect("the terrain-gen binary must run")
}

pub(crate) fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8")
}

pub(crate) fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr is UTF-8")
}

pub(crate) fn assert_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context} failed with {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        stdout(output),
        stderr(output)
    );
}

pub(crate) fn temp_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().join(format!(
        "engine-terrain-gen-{name}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).expect("the temporary directory is created");
    directory
}

pub(crate) fn text(path: &Path) -> &str {
    path.to_str().expect("the temporary path is valid UTF-8")
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

// ---------------------------------------------------------------------------
// World fixtures
// ---------------------------------------------------------------------------

/// The pinned baseline derivation, mirrored from the bin: `SHA-256` over the
/// decoded block ids, two little-endian bytes per cell, `section_index` order.
pub(crate) fn baseline_hash(section: &Section) -> [u8; 32] {
    let mut bytes = Vec::with_capacity(description::SECTION_VOLUME * 2);
    for index in 0..description::SECTION_VOLUME {
        bytes.extend_from_slice(&section.get_index(index).to_le_bytes());
    }
    sha256(&bytes)
}

/// `[x, y, z]` display coordinates to the canonical on-disk key.
pub(crate) fn section_key(coord: [i32; 3]) -> SectionKey {
    SectionKey {
        column: [coord[0], coord[2]],
        section: u8::try_from(coord[1]).expect("a fixture section slot fits u8"),
    }
}

/// `sections/c.<column x>.<column z>.<section y>.sect`.
pub(crate) fn section_file(directory: &Path, coord: [i32; 3]) -> PathBuf {
    directory.join(format!(
        "sections/c.{}.{}.{}.sect",
        coord[0], coord[2], coord[1]
    ))
}

/// Recomputes a record's trailing checksum after a deliberate field edit.
pub(crate) fn reseal(mut bytes: Vec<u8>) -> Vec<u8> {
    let crc_at = bytes.len() - 4;
    let crc = header::crc32(&bytes[..crc_at]);
    bytes[crc_at..].copy_from_slice(&crc.to_le_bytes());
    bytes
}

pub(crate) fn write_header(directory: &Path, seed: u64) {
    let digest = description::default_v1()
        .description_hash()
        .expect("the v1 description hashes");
    let header = Header {
        format_version: HEADER_FORMAT_VERSION,
        generator_version: GENERATOR_VERSION,
        description_hash: digest,
        hash_algorithm: HASH_ALGORITHM.to_owned(),
        seed,
        platform_tag: "test-triple".to_owned(),
        last_saved_platform_tag: "test-triple".to_owned(),
        world_id: [0x42; 16],
        name: "terrain-gen fixture".to_owned(),
        created: 0,
        last_saved: 0,
        revision: 0,
        schema_version: 1,
        migration_state: MigrationState::None,
    };
    header
        .write(&directory.join(header::HEADER_FILE))
        .expect("the fixture header is written");
}

/// Writes a fixture world: `header.bin` plus one canonical section record per
/// `[x, y, z]` section coordinate.
pub(crate) fn write_world(name: &str, seed: u64, coords: &[[i32; 3]]) -> PathBuf {
    let directory = temp_dir(name);
    write_header(&directory, seed);

    let sections = directory.join("sections");
    fs::create_dir_all(&sections).expect("the sections directory is created");
    let pack = content::v1_default();
    let terrain = Terrain::new(seed).expect("the fixture terrain builds");
    for &[sx, sy, sz] in coords {
        let section = terrain
            .section(sx, sy, sz)
            .expect("the fixture section generates");
        let bytes = section::encode_record(
            &pack,
            section_key([sx, sy, sz]),
            baseline_hash(&section),
            false,
            &section,
        );
        fs::write(section_file(&directory, [sx, sy, sz]), bytes)
            .expect("the fixture section is written");
    }
    directory
}

/// Recursively hashes every path and byte below `root`.
pub(crate) fn directory_hash(root: &Path) -> [u8; 32] {
    let mut files = Vec::new();
    collect_files(root, root, &mut files);
    files.sort();
    let mut bytes = Vec::new();
    for relative in files {
        bytes.extend_from_slice(relative.to_string_lossy().as_bytes());
        bytes.push(0);
        let contents = fs::read(root.join(&relative)).expect("the fixture file is readable");
        bytes.extend_from_slice(&contents);
    }
    sha256(&bytes)
}

pub(crate) fn collect_files(root: &Path, directory: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = fs::read_dir(directory)
        .expect("the fixture directory is readable")
        .map(|entry| entry.expect("the fixture entry is readable").path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_files(root, &path, out);
        } else {
            out.push(
                path.strip_prefix(root)
                    .expect("the entry sits below the fixture root")
                    .to_path_buf(),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Report bundles
// ---------------------------------------------------------------------------

/// Writes a directory bundle whose `stages.json` names the real hashes, unless
/// `section_override` replaces the `section:4` hash.
pub(crate) fn write_bundle(
    name: &str,
    seed: u64,
    column: (i32, i32),
    section_override: Option<&str>,
) -> PathBuf {
    let directory = temp_dir(name);
    let digest = description::default_v1()
        .description_hash()
        .expect("the v1 description hashes");
    let terrain = Terrain::new(seed).expect("the bundle terrain builds");

    let blocks = terrain
        .column(column.0, column.1)
        .expect("the bundle column generates");
    let mut column_bytes = Vec::with_capacity(blocks.len() * 2);
    for id in &blocks {
        column_bytes.extend_from_slice(&id.to_le_bytes());
    }
    let column_hash = hex(&sha256(&column_bytes));

    let section = terrain
        .section(
            column.0.div_euclid(32),
            FIXTURE_SECTION_Y,
            column.1.div_euclid(32),
        )
        .expect("the bundle section generates");
    let section_hash = hex(&baseline_hash(&section));
    let recorded_section = section_override.unwrap_or(&section_hash);

    let manifest = format!(
        "{{\n  \"generator_version\": {GENERATOR_VERSION},\n  \"seed\": {seed},\n  \
         \"description_hash\": \"{}\",\n  \"platform_tag\": \"test-triple\",\n  \
         \"failure\": {{ \"column\": [{}, {}], \"stage\": \"column\" }}\n}}\n",
        hex(&digest),
        column.0,
        column.1
    );
    let stages = format!(
        "{{\n  \"column\": [{}, {}],\n  \"stages\": [\n    \
         {{ \"stage\": \"column\", \"hash\": \"{column_hash}\" }},\n    \
         {{ \"stage\": \"section:{FIXTURE_SECTION_Y}\", \"hash\": \"{recorded_section}\" }}\n  ]\n}}\n",
        column.0, column.1
    );
    fs::write(directory.join("manifest.json"), manifest).expect("the fixture manifest is written");
    fs::write(directory.join("stages.json"), stages).expect("the fixture stages are written");
    directory
}

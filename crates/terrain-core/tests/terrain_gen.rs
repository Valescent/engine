//! Integration tests for the `terrain-gen` acceptance harness.
//!
//! Each test drives the real binary, not the library, so it also covers argument
//! parsing, exit codes, and the stable stdout the CI gates read.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use terrain_core::content::{self, ContentPack};
use terrain_core::description;
use terrain_core::error::{ErrorKind, SectionFault};
use terrain_core::hash::sha256;
use terrain_core::header::{self, HASH_ALGORITHM, HEADER_FORMAT_VERSION, Header, MigrationState};
use terrain_core::pipeline::Terrain;
use terrain_core::save::SectionStore;
use terrain_core::section::{self, BlockId, Section, SectionKey};
use terrain_core::world_save::WorldSave;

const BIN: &str = env!("CARGO_BIN_EXE_terrain-gen");
/// The vertical section index a fixture uses: it contains the sea-level surface.
const FIXTURE_SECTION_Y: i32 = 4;

fn run(args: &[&str]) -> Output {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    Command::new(BIN)
        .args(args)
        .current_dir(root)
        .output()
        .expect("the terrain-gen binary must run")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("stdout is UTF-8")
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).expect("stderr is UTF-8")
}

fn assert_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{context} failed with {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        stdout(output),
        stderr(output)
    );
}

fn temp_dir(name: &str) -> PathBuf {
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

fn text(path: &Path) -> &str {
    path.to_str().expect("the temporary path is valid UTF-8")
}

fn hex(bytes: &[u8]) -> String {
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
fn baseline_hash(section: &Section) -> [u8; 32] {
    let mut bytes = Vec::with_capacity(description::SECTION_VOLUME * 2);
    for index in 0..description::SECTION_VOLUME {
        bytes.extend_from_slice(&section.get_index(index).to_le_bytes());
    }
    sha256(&bytes)
}

/// `[x, y, z]` display coordinates to the canonical on-disk key.
fn section_key(coord: [i32; 3]) -> SectionKey {
    SectionKey {
        column: [coord[0], coord[2]],
        section: u8::try_from(coord[1]).expect("a fixture section slot fits u8"),
    }
}

/// `sections/c.<column x>.<column z>.<section y>.sect`.
fn section_file(directory: &Path, coord: [i32; 3]) -> PathBuf {
    directory.join(format!(
        "sections/c.{}.{}.{}.sect",
        coord[0], coord[2], coord[1]
    ))
}

/// Recomputes a record's trailing checksum after a deliberate field edit.
fn reseal(mut bytes: Vec<u8>) -> Vec<u8> {
    let crc_at = bytes.len() - 4;
    let crc = header::crc32(&bytes[..crc_at]);
    bytes[crc_at..].copy_from_slice(&crc.to_le_bytes());
    bytes
}

fn write_header(directory: &Path, seed: u64) {
    let digest = description::default_v1()
        .description_hash()
        .expect("the v1 description hashes");
    let header = Header {
        format_version: HEADER_FORMAT_VERSION,
        generator_version: 2,
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
fn write_world(name: &str, seed: u64, coords: &[[i32; 3]]) -> PathBuf {
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
fn directory_hash(root: &Path) -> [u8; 32] {
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

fn collect_files(root: &Path, directory: &Path, out: &mut Vec<PathBuf>) {
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
fn write_bundle(
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
        "{{\n  \"generator_version\": 2,\n  \"seed\": {seed},\n  \
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

// ---------------------------------------------------------------------------
// Existing subcommands
// ---------------------------------------------------------------------------

#[test]
fn describe_round_trips_through_a_file() {
    let path = std::env::temp_dir().join(format!(
        "engine-terrain-gen-describe-{}.toml",
        std::process::id()
    ));
    let path_text = path
        .to_str()
        .expect("the temporary path must be valid UTF-8");

    let written = run(&["describe", "--out", path_text]);
    assert_success(&written, "describe --out");
    let text = stdout(&written);
    assert!(
        text.contains("description_hash "),
        "describe must print the hash:\n{text}"
    );

    let diff = run(&["describe-diff", "--file", path_text]);
    let _ = std::fs::remove_file(&path);
    assert_success(&diff, "describe-diff on an emitted description");
    assert!(
        stdout(&diff).contains("v1_default_shape yes"),
        "stdout:\n{}",
        stdout(&diff)
    );
}

#[test]
fn gen_is_thread_and_order_invariant() {
    let base = run(&["gen", "--seed", "7", "--from", "-3", "5", "--size", "6"]);
    assert_success(&base, "baseline gen");
    let expected = stdout(&base);

    let variants: [&[&str]; 5] = [
        &[
            "gen",
            "--seed",
            "7",
            "--from",
            "-3",
            "5",
            "--size",
            "6",
            "--threads",
            "4",
        ],
        &[
            "gen", "--seed", "7", "--from", "-3", "5", "--size", "6", "--order", "reverse",
        ],
        &[
            "gen", "--seed", "7", "--from", "-3", "5", "--size", "6", "--order", "shuffled",
        ],
        &[
            "gen", "--seed", "7", "--from", "-3", "5", "--size", "6", "--order", "3",
        ],
        &[
            "gen",
            "--seed",
            "7",
            "--from",
            "-3",
            "5",
            "--size",
            "6",
            "--threads",
            "4",
            "--order",
            "shuffled",
        ],
    ];
    for variant in variants {
        let output = run(variant);
        assert_success(&output, &format!("gen {variant:?}"));
        assert_eq!(expected, stdout(&output), "stdout changed for {variant:?}");
    }

    let json = run(&[
        "gen", "--seed", "7", "--from", "-3", "5", "--size", "6", "--json",
    ]);
    assert_success(&json, "gen --json");
    let text = stdout(&json);
    assert_eq!(
        text.trim().lines().count(),
        1,
        "stdout must be one JSON line"
    );
    assert!(text.contains("\"subcommand\":\"gen\""));
}

#[test]
fn golden_file_passes() {
    let output = run(&["golden", "check"]);
    assert_success(&output, "golden check");
    let text = stdout(&output);
    assert!(text.contains("entries 6"), "stdout:\n{text}");
    assert!(text.contains("ok"), "stdout:\n{text}");
}

#[test]
fn seams_are_bit_identical() {
    let output = run(&["seams", "--window", "2x2", "--seeds", "1"]);
    assert_success(&output, "seams");
    let text = stdout(&output);
    assert!(text.contains("pairs 4"), "stdout:\n{text}");
    assert!(text.contains("ok"), "stdout:\n{text}");
}

#[test]
fn sweep_pr_passes() {
    let output = run(&["sweep", "--tier", "pr", "--seeds", "4"]);
    assert_success(&output, "sweep pr");
    let text = stdout(&output);
    assert!(text.contains("failures=0"), "stdout:\n{text}");
    assert_eq!(
        text.lines()
            .filter(|line| line.starts_with("seed "))
            .count(),
        4
    );
}

#[test]
fn usage_error_exits_two() {
    let empty = run(&[]);
    assert_eq!(empty.status.code(), Some(2), "no arguments must exit 2");
    assert!(stderr(&empty).contains("usage:"));

    let unknown = run(&["definitely-not-a-command"]);
    assert_eq!(
        unknown.status.code(),
        Some(2),
        "unknown command must exit 2"
    );
    assert!(
        stderr(&unknown).contains("unknown subcommand"),
        "stderr:\n{}",
        stderr(&unknown)
    );
}

// ---------------------------------------------------------------------------
// verify
// ---------------------------------------------------------------------------

#[test]
fn verify_matches_a_generated_world_and_leaves_it_byte_identical() {
    let world = write_world("verify-ok", 0x5EED, &[[0, 4, 0], [0, 5, 0]]);
    let before = directory_hash(&world);

    let output = run(&["verify", "--world", text(&world)]);
    assert_success(&output, "verify on a generated world");
    assert!(
        stdout(&output).contains("2 sections checked, 0 differ"),
        "stdout:\n{}",
        stdout(&output)
    );

    let json = run(&["verify", "--world", text(&world), "--json"]);
    assert_success(&json, "verify --json");
    let line = stdout(&json);
    assert_eq!(
        line.trim().lines().count(),
        1,
        "stdout must be one JSON line"
    );
    assert!(
        line.contains("\"subcommand\":\"verify\"") && line.contains("\"ok\":true"),
        "stdout:\n{line}"
    );

    let after = directory_hash(&world);
    let _ = fs::remove_dir_all(&world);
    assert_eq!(before, after, "verify wrote to the world directory");
}

#[test]
fn verify_detects_a_flipped_bit_and_names_the_coordinate() {
    let world = write_world("verify-drift", 0x5EED, &[[0, 4, 0]]);
    let section = section_file(&world, [0, 4, 0]);
    let mut bytes = fs::read(&section).expect("the fixture section is readable");
    let middle = bytes.len() / 2;
    bytes[middle] ^= 0x01;
    fs::write(&section, &bytes).expect("the corrupt copy is written");

    let output = run(&["verify", "--world", text(&world)]);
    let code = output.status.code();
    let out = stdout(&output);
    let _ = fs::remove_dir_all(&world);
    assert_eq!(
        code,
        Some(1),
        "a corrupt section must exit 1\nstdout:\n{out}"
    );
    assert!(
        out.contains("1 sections checked, 1 differ"),
        "stdout:\n{out}"
    );
    assert!(
        out.contains("(0,4,0)"),
        "the drift must name the coordinate:\n{out}"
    );
}

#[test]
fn verify_reports_a_baseline_hash_change() {
    let world = write_world("verify-baseline", 0x5EED, &[[0, 4, 0]]);
    let section = section_file(&world, [0, 4, 0]);
    let mut bytes = fs::read(&section).expect("the fixture section is readable");
    // The v2 baseline hash starts at offset 16: magic + version + column + slot + flags.
    bytes[16] ^= 0x01;
    let bytes = reseal(bytes);
    fs::write(&section, &bytes).expect("the edited section is written");

    let output = run(&["verify", "--world", text(&world)]);
    let code = output.status.code();
    let out = stdout(&output);
    let _ = fs::remove_dir_all(&world);
    assert_eq!(
        code,
        Some(1),
        "a baseline change must exit 1\nstdout:\n{out}"
    );
    assert!(
        out.contains("1 sections checked, 1 differ"),
        "stdout:\n{out}"
    );
    assert!(out.contains("(0,4,0)"), "stdout:\n{out}");
    assert!(out.contains("baseline"), "stdout:\n{out}");
}

#[test]
fn verify_skips_edited_sections() {
    let world = write_world("verify-edited", 0x5EED, &[[0, 4, 0]]);
    let section = section_file(&world, [0, 4, 0]);
    let mut bytes = fs::read(&section).expect("the fixture section is readable");
    // The v2 `edited` flag sits at offset 15: magic + version + column + slot.
    bytes[15] = 1;
    let bytes = reseal(bytes);
    fs::write(&section, &bytes).expect("the edited section is written");

    let output = run(&["verify", "--world", text(&world)]);
    let _ = fs::remove_dir_all(&world);
    assert_success(&output, "verify skips an edited section");
    assert!(
        stdout(&output).contains("0 sections checked, 0 differ"),
        "stdout:\n{}",
        stdout(&output)
    );
}

#[test]
fn verify_honours_sections_and_ignores_order_and_threads() {
    let world = write_world("verify-order", 0x5EED, &[[0, 4, 0]]);

    let base = run(&["verify", "--world", text(&world)]);
    assert_success(&base, "baseline verify");
    let expected = stdout(&base);

    for order in ["reverse", "shuffled", "3"] {
        let variant = run(&[
            "verify",
            "--world",
            text(&world),
            "--threads",
            "4",
            "--order",
            order,
        ]);
        assert_success(&variant, &format!("verify --order {order}"));
        assert_eq!(
            expected,
            stdout(&variant),
            "stdout changed for --order {order}"
        );
    }

    let filtered = run(&["verify", "--world", text(&world), "--sections", "9,4,9"]);
    let _ = fs::remove_dir_all(&world);
    assert_success(&filtered, "verify --sections with no match");
    assert!(
        stdout(&filtered).contains("0 sections checked, 0 differ"),
        "stdout:\n{}",
        stdout(&filtered)
    );
}

#[test]
fn verify_usage_errors_exit_two() {
    let missing = run(&["verify"]);
    assert_eq!(missing.status.code(), Some(2), "verify needs --world");

    let world = temp_dir("verify-usage");
    let malformed = run(&[
        "verify",
        "--world",
        text(&world),
        "--sections",
        "not-a-coordinate",
    ]);
    let _ = fs::remove_dir_all(&world);
    assert_eq!(
        malformed.status.code(),
        Some(2),
        "a malformed --sections value is a usage error\nstderr:\n{}",
        stderr(&malformed)
    );
}

#[test]
fn saved_record_round_trips_through_the_canonical_encoder() {
    let directory = temp_dir("record-round-trip");
    let section_directory = directory.join("sections");
    fs::create_dir_all(&section_directory).expect("the sections directory is created");
    let path = section_directory.join("c.-2.7.4.sect");

    let pack = content::v1_default();
    let terrain = Terrain::new(0x5EED).expect("the fixture terrain builds");
    let section = terrain
        .section(0, FIXTURE_SECTION_Y, 0)
        .expect("the fixture section generates");
    let key = SectionKey {
        column: [-2, 7],
        section: u8::try_from(FIXTURE_SECTION_Y).expect("the slot fits u8"),
    };
    let baseline = baseline_hash(&section);
    let bytes = section::encode_record(&pack, key, baseline, true, &section);
    fs::write(&path, &bytes).expect("the record is written");

    let read = fs::read(&path).expect("the record is readable");
    let saved = section::decode_record_at(&pack, &path, &read).expect("the record decodes");
    let _ = fs::remove_dir_all(&directory);

    assert_eq!(saved.key, key, "the key round-trips");
    assert!(saved.edited, "the edited bit round-trips");
    assert_eq!(saved.baseline_hash, baseline, "the baseline round-trips");
    assert_eq!(saved.section, section, "the block ids round-trip");
    assert!(
        saved.unknown_names.is_empty(),
        "the fixture uses pack names"
    );
}

#[test]
fn truncated_v2_record_is_corrupt_with_the_path() {
    let pack = content::v1_default();
    let key = SectionKey {
        column: [3, -5],
        section: 7,
    };
    let bytes = section::encode_record(&pack, key, [0xAB; 32], false, &Section::uniform(0));
    let path = Path::new("world/sections/c.3.-5.7.sect");

    for length in [0, 10, 53, bytes.len() - 1] {
        let error = section::decode_record_at(&pack, path, &bytes[..length])
            .expect_err("a truncated record must not decode");
        match &error.kind {
            ErrorKind::CorruptSection { path: named, fault } => {
                assert_eq!(named, path, "the error names the record");
                assert!(
                    matches!(fault, SectionFault::Length | SectionFault::Checksum),
                    "a {length}-byte record reported {fault:?}"
                );
            }
            other => panic!("expected CorruptSection, found {other:?}"),
        }
    }
}

#[test]
fn a_missing_core_name_is_an_error() {
    let mut records = content::v1_default().records().to_vec();
    records.retain(|record| record.name != "core:water");
    let reduced = ContentPack::load(records).expect("the reduced fixture pack is valid");

    let pack = content::v1_default();
    let key = SectionKey {
        column: [0, 0],
        section: 0,
    };
    let bytes = section::encode_record(&pack, key, [0_u8; 32], false, &Section::uniform(0));
    let error = section::decode_record(&reduced, &bytes)
        .expect_err("a pack without core:water cannot read saved sections");
    assert!(
        matches!(&error.kind, ErrorKind::ManifestIncompatible { id, .. } if id == "core:water"),
        "found {error:?}"
    );
}

#[test]
fn unknown_block_name_is_reported_and_the_section_still_decodes() {
    let default_pack = content::v1_default();
    let air = default_pack
        .id("core:air")
        .expect("core:air is in the pack");
    let unknown_name = "mod:gone";

    let mut records = default_pack.records().to_vec();
    let mut extra = records[0].clone();
    extra.name = unknown_name.to_owned();
    records.push(extra);
    let source = ContentPack::load(records).expect("the extended fixture pack is valid");

    // A palette entry no lane references: the decoded ids stay all air, so the
    // baseline still matches generation while the unknown name is reported.
    let unknown = source.id(unknown_name).expect("the extra name resolves");
    let palette: Vec<BlockId> = vec![air, unknown];
    let words = vec![0_u64; description::SECTION_VOLUME / 64];
    let section = Section::from_palette(&palette, 1, &words).expect("a valid paletted section");
    let key = SectionKey {
        column: [0, 0],
        section: 11,
    };
    let baseline = baseline_hash(&Section::uniform(air));
    let bytes = section::encode_record(&source, key, baseline, false, &section);

    let saved =
        section::decode_record(&default_pack, &bytes).expect("an unknown name is not corruption");
    assert_eq!(saved.unknown_names, [unknown_name]);
    assert_eq!(
        saved.section.get_index(0),
        air,
        "the referenced lane decodes normally"
    );

    let world = temp_dir("verify-unknown");
    write_header(&world, 0x5EED);
    fs::create_dir_all(world.join("sections")).expect("the sections directory is created");
    let generated = Terrain::new(0x5EED)
        .expect("the fixture terrain builds")
        .section(0, 11, 0)
        .expect("the fixture section generates");
    assert_eq!(
        generated.is_uniform(),
        Some(air),
        "slot 11 must be uniform air for this fixture"
    );
    fs::write(section_file(&world, [0, 11, 0]), &bytes)
        .expect("the unknown-name record is written");

    let output = run(&["verify", "--world", text(&world)]);
    let code = output.status.code();
    let out = stdout(&output);
    let err = stderr(&output);
    let _ = fs::remove_dir_all(&world);
    assert_eq!(code, Some(0), "an unknown name must not fail verify\n{err}");
    assert!(
        out.contains("1 sections checked, 0 differ"),
        "stdout:\n{out}"
    );
    assert!(
        err.contains("warning:") && err.contains(unknown_name),
        "the unknown name must be reported:\n{err}"
    );
}

// ---------------------------------------------------------------------------
// replay
// ---------------------------------------------------------------------------

#[test]
fn replay_matches_its_own_bundle() {
    let bundle = write_bundle("replay-ok", 0x5EED, (5, 9), None);

    let output = run(&["replay", "--report", text(&bundle)]);
    assert_success(&output, "replay on its own bundle");
    let out = stdout(&output);
    assert!(out.contains("stage column ok"), "stdout:\n{out}");
    assert!(out.contains("stage section:4 ok"), "stdout:\n{out}");
    assert!(
        stderr(&output).contains("warning:"),
        "a differing platform tag must warn, stderr:\n{}",
        stderr(&output)
    );

    // The same bundle may be named by its manifest file, and its output never
    // depends on `--threads` or `--order`.
    let manifest = bundle.join("manifest.json");
    let variant = run(&[
        "replay",
        "--report",
        text(&manifest),
        "--threads",
        "3",
        "--order",
        "reverse",
    ]);
    assert_success(&variant, "replay is thread and order invariant");
    assert_eq!(
        out,
        stdout(&variant),
        "stdout changed for --threads/--order"
    );

    let from_file = run(&["replay", "--report", text(&manifest), "--json"]);
    let _ = fs::remove_dir_all(&bundle);
    assert_success(&from_file, "replay --report manifest.json --json");
    let line = stdout(&from_file);
    assert_eq!(
        line.trim().lines().count(),
        1,
        "stdout must be one JSON line"
    );
    assert!(
        line.contains("\"subcommand\":\"replay\"") && line.contains("\"ok\":true"),
        "stdout:\n{line}"
    );
}

#[test]
fn replay_names_the_first_differing_stage() {
    let wrong = "0".repeat(64);
    let bundle = write_bundle("replay-drift", 0x5EED, (5, 9), Some(&wrong));

    let output = run(&["replay", "--report", text(&bundle)]);
    let code = output.status.code();
    let out = stdout(&output);
    let _ = fs::remove_dir_all(&bundle);
    assert_eq!(
        code,
        Some(1),
        "a differing stage must exit 1\nstdout:\n{out}"
    );

    let column = out
        .find("stage column ok")
        .expect("the first stage is reported ok");
    let section = out
        .find("stage section:4 differ")
        .expect("the differing stage is named");
    assert!(column < section, "stages must be reported in order:\n{out}");
    assert!(out.contains(&wrong), "the expected hash is printed:\n{out}");
}

#[test]
fn replay_refuses_an_unknown_generator_version() {
    let bundle = write_bundle("replay-version", 0x5EED, (5, 9), None);
    let path = bundle.join("manifest.json");
    let manifest = fs::read_to_string(&path).expect("the manifest is readable");
    fs::write(
        &path,
        manifest.replace("\"generator_version\": 2", "\"generator_version\": 999"),
    )
    .expect("the edited manifest is written");

    let output = run(&["replay", "--report", text(&bundle)]);
    let code = output.status.code();
    let message = format!("{}{}", stdout(&output), stderr(&output));
    let _ = fs::remove_dir_all(&bundle);
    assert_eq!(code, Some(1), "an unshipped generator version is refused");
    assert!(
        message.contains("generator version"),
        "the refusal names the version:\n{message}"
    );
}

#[test]
fn repair_restores_a_corrupt_section_from_its_backup() {
    let world = temp_dir("repair");
    let store = SectionStore::create(&world, content::v1_default()).expect("the store opens");
    let key = SectionKey {
        column: [0, 0],
        section: 0,
    };
    let blocks: Box<[BlockId; description::SECTION_VOLUME]> =
        vec![content::v1_default().id("core:stone").unwrap(); description::SECTION_VOLUME]
            .into_boxed_slice()
            .try_into()
            .expect("the section volume matches");
    let first = Section::from_blocks(&blocks);
    let blocks: Box<[BlockId; description::SECTION_VOLUME]> =
        vec![content::v1_default().id("core:dirt").unwrap(); description::SECTION_VOLUME]
            .into_boxed_slice()
            .try_into()
            .expect("the section volume matches");
    let second = Section::from_blocks(&blocks);
    store
        .write(key, [1; 32], true, &first)
        .expect("the first generation writes");
    store
        .write(key, [1; 32], true, &second)
        .expect("the second generation writes");
    fs::write(store.path_for(key), b"broken current").expect("the current is damaged");

    let output = run(&[
        "repair",
        "--world",
        text(&world),
        "--section",
        "0,0,0",
        "--action",
        "restore",
    ]);
    assert_success(&output, "repair restore");
    assert!(
        stdout(&output).contains("restored"),
        "the report names the outcome:\n{}",
        stdout(&output)
    );
    assert_eq!(
        store.read(key).expect("the current reads").unwrap().section,
        first
    );
    let _ = fs::remove_dir_all(&world);
}

#[test]
fn migrate_rewrites_a_world_and_clears_the_state() {
    let world = temp_dir("migrate");
    drop(WorldSave::create(&world, 7, "Migration fixture").expect("the world creates"));

    let output = run(&["migrate", "--world", text(&world)]);
    assert_success(&output, "migrate");
    assert!(
        stdout(&output).contains("migrated"),
        "the report names the work:\n{}",
        stdout(&output)
    );
    let reopened = WorldSave::open(&world, 7).expect("the world reopens");
    assert_eq!(reopened.header.migration_state, MigrationState::None);
    let report = fs::read_dir(&world)
        .expect("the world reads")
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("migration-report-")
        })
        .expect("the migration writes a report");
    let report = fs::read_to_string(report.path()).expect("the report reads");
    assert!(report.contains("\"ok\":true"));
    let _ = fs::remove_dir_all(&world);
}

#[test]
fn verify_writes_a_report_bundle() {
    let world = write_world("verify-bundle", 0x5EED, &[[0, 4, 0]]);
    let reports = temp_dir("verify-bundle-report");
    let output = run(&[
        "verify",
        "--world",
        text(&world),
        "--report",
        text(&reports),
    ]);
    assert_success(&output, "verify --report");
    for name in ["manifest.json", "verify.json", "verify.txt"] {
        assert!(reports.join(name).is_file(), "the bundle contains {name}");
    }
    let json = fs::read_to_string(reports.join("verify.json")).expect("verify.json reads");
    assert!(json.contains("\"subcommand\":\"verify\""));
    assert!(json.contains("\"ok\":true"));
    let manifest = fs::read_to_string(reports.join("manifest.json")).expect("manifest.json reads");
    assert!(manifest.contains("\"schema\":\"valescent/verify-report@1\""));
    let _ = fs::remove_dir_all(&world);
    let _ = fs::remove_dir_all(&reports);
}

#[test]
fn map_prints_one_glyph_per_cell() {
    let output = run(&[
        "map", "--seed", "7", "--from", "0", "0", "--size", "8", "--cell", "8",
    ]);
    assert_success(&output, "map");
    let text = stdout(&output);
    let mut lines = text.lines();
    assert!(
        lines
            .next()
            .is_some_and(|header| header.starts_with("map seed=7")),
        "the header names the seed:\n{text}"
    );
    let rows: Vec<&str> = lines.collect();
    assert_eq!(rows.len(), 8, "one row per requested cell");
    for row in rows {
        assert_eq!(row.chars().count(), 8, "one glyph per column in {row:?}");
        assert!(
            row.chars().all(|glyph| ".,-+*#%@~".contains(glyph)),
            "unexpected glyph in {row:?}"
        );
    }
}

#[test]
fn replay_usage_errors_exit_two() {
    let missing = run(&["replay"]);
    assert_eq!(missing.status.code(), Some(2), "replay needs --report");

    let directory = temp_dir("replay-usage");
    fs::write(directory.join("manifest.json"), "{}\n").expect("the bad manifest is written");
    let malformed = run(&["replay", "--report", text(&directory)]);
    let _ = fs::remove_dir_all(&directory);
    assert_eq!(
        malformed.status.code(),
        Some(2),
        "an incomplete manifest is a usage error\nstderr:\n{}",
        stderr(&malformed)
    );
}

// ---------------------------------------------------------------------------
// calibrate-blend
// ---------------------------------------------------------------------------

#[test]
fn calibrate_blend_is_thread_and_order_invariant() {
    let base = run(&[
        "calibrate-blend",
        "--seeds",
        "2",
        "--from",
        "0",
        "0",
        "--size",
        "256",
        "--step",
        "16",
    ]);
    let expected = stdout(&base);
    let code = base.status.code();
    assert!(
        expected.contains("pair samples median p10 p90 ok"),
        "the table is pinned:\n{expected}"
    );

    for (threads, order) in [("4", "reverse"), ("2", "shuffled")] {
        let variant = run(&[
            "calibrate-blend",
            "--seeds",
            "2",
            "--from",
            "0",
            "0",
            "--size",
            "256",
            "--step",
            "16",
            "--threads",
            threads,
            "--order",
            order,
        ]);
        assert_eq!(
            code,
            variant.status.code(),
            "exit code changed with {order}"
        );
        assert_eq!(
            expected,
            stdout(&variant),
            "stdout changed for --threads {threads} --order {order}"
        );
    }

    let json = run(&[
        "calibrate-blend",
        "--seeds",
        "2",
        "--from",
        "0",
        "0",
        "--size",
        "256",
        "--step",
        "16",
        "--json",
    ]);
    let line = stdout(&json);
    assert_eq!(
        line.trim().lines().count(),
        1,
        "stdout must be one JSON line"
    );
    assert!(
        line.contains("\"subcommand\":\"calibrate-blend\"") && line.contains("\"pairs\":["),
        "stdout:\n{line}"
    );
}

#[test]
fn calibrate_blend_usage_errors_exit_two() {
    let missing = run(&["calibrate-blend"]);
    assert_eq!(
        missing.status.code(),
        Some(2),
        "calibrate-blend needs --seeds"
    );

    let bad_step = run(&["calibrate-blend", "--seeds", "1", "--step", "3"]);
    assert_eq!(
        bad_step.status.code(),
        Some(2),
        "--step must be a multiple of the macro cell\nstderr:\n{}",
        stderr(&bad_step)
    );

    let outside = run(&[
        "calibrate-blend",
        "--seeds",
        "1",
        "--from",
        "2000",
        "0",
        "--size",
        "256",
    ]);
    assert_eq!(
        outside.status.code(),
        Some(2),
        "a window outside the world is a usage error\nstderr:\n{}",
        stderr(&outside)
    );
}

// ---------------------------------------------------------------------------
// counters
// ---------------------------------------------------------------------------

#[test]
fn counters_match_the_checked_in_baseline() {
    let output = run(&["counters", "--tier", "pr"]);
    assert_success(&output, "counters --tier pr");
    let text = stdout(&output);
    assert!(text.contains("tier pr"), "stdout:\n{text}");
    assert!(text.contains("blocks_air "), "stdout:\n{text}");
    assert!(text.contains("ok"), "stdout:\n{text}");

    let json = run(&["counters", "--tier", "pr", "--json"]);
    assert_success(&json, "counters --json");
    let line = stdout(&json);
    assert_eq!(
        line.trim().lines().count(),
        1,
        "stdout must be one JSON line"
    );
    assert!(
        line.contains("\"subcommand\":\"counters\"") && line.contains("\"ok\":true"),
        "stdout:\n{line}"
    );
}

#[test]
fn counters_are_thread_and_order_invariant() {
    let directory = temp_dir("counters-order");
    let file = directory.join("baseline.json");
    let file_text = text(&file);
    let update = run(&[
        "counters", "--tier", "pr", "--seeds", "2", "--file", file_text, "--update",
    ]);
    assert_success(&update, "counters --update");

    let base = run(&[
        "counters", "--tier", "pr", "--seeds", "2", "--file", file_text,
    ]);
    assert_success(&base, "counters check");
    let expected = stdout(&base);

    for (threads, order) in [("4", "reverse"), ("2", "shuffled")] {
        let variant = run(&[
            "counters",
            "--tier",
            "pr",
            "--seeds",
            "2",
            "--file",
            file_text,
            "--threads",
            threads,
            "--order",
            order,
        ]);
        assert_success(&variant, "counters invariance");
        assert_eq!(
            expected,
            stdout(&variant),
            "stdout changed for --threads {threads} --order {order}"
        );
    }
    let _ = fs::remove_dir_all(&directory);
}

/// Scales one baseline counter by `percent`, keeping the rest of the file.
fn scale_counter(text: &str, name: &str, percent: u64) -> String {
    let needle = format!("\"{name}\": ");
    let start = text
        .find(&needle)
        .unwrap_or_else(|| panic!("baseline has no {name}"))
        + needle.len();
    let digits: String = text[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let value = digits.parse::<u64>().expect("the counter is an integer");
    let drifted = value * (100 + percent) / 100;
    text.replacen(
        &format!("\"{name}\": {value}"),
        &format!("\"{name}\": {drifted}"),
        1,
    )
}

#[test]
fn counters_fail_on_a_ten_percent_drift() {
    let directory = temp_dir("counters-drift");
    let file = directory.join("baseline.json");
    let file_text = text(&file);
    let update = run(&[
        "counters", "--tier", "pr", "--seeds", "2", "--file", file_text, "--update",
    ]);
    assert_success(&update, "counters --update");

    let baseline = fs::read_to_string(&file).expect("the baseline is readable");
    let drifted = scale_counter(&baseline, "blocks_air", 10);
    assert_ne!(baseline, drifted, "the fixture must actually change");
    fs::write(&file, drifted).expect("the drifted baseline is written");

    let output = run(&[
        "counters", "--tier", "pr", "--seeds", "2", "--file", file_text,
    ]);
    let code = output.status.code();
    let out = stdout(&output);
    let err = stderr(&output);
    assert_eq!(code, Some(1), "a 10% drift must exit 1\nstdout:\n{out}");
    assert!(
        out.contains("drift blocks_air"),
        "the drift names the counter:\n{out}"
    );
    assert!(
        err.contains("counter drift"),
        "the diagnostic names the drift:\n{err}"
    );

    let json = run(&[
        "counters", "--tier", "pr", "--seeds", "2", "--file", file_text, "--json",
    ]);
    let _ = fs::remove_dir_all(&directory);
    assert_eq!(json.status.code(), Some(1), "JSON drift must exit 1");
    let line = stdout(&json);
    assert_eq!(
        line.trim().lines().count(),
        1,
        "stdout must be one JSON line"
    );
    assert!(
        line.contains("\"ok\":false") && line.contains("\"name\":\"blocks_air\""),
        "stdout:\n{line}"
    );
}

#[test]
fn counters_usage_errors_exit_two() {
    let missing = run(&["counters"]);
    assert_eq!(missing.status.code(), Some(2), "counters needs --tier");

    let missing_file = run(&[
        "counters",
        "--tier",
        "pr",
        "--file",
        "no-such-baseline.json",
    ]);
    assert_eq!(
        missing_file.status.code(),
        Some(2),
        "a missing baseline is a usage error\nstderr:\n{}",
        stderr(&missing_file)
    );
}

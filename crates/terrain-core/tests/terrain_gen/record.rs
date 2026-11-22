//! Record encoding and content-pack compatibility tests.

use std::fs;
use std::path::Path;

use terrain_core::content::{self, ContentPack};
use terrain_core::description;
use terrain_core::error::{ErrorKind, SectionFault};
use terrain_core::pipeline::Terrain;
use terrain_core::section::{self, BlockId, Section, SectionKey};

use crate::support::{
    FIXTURE_SECTION_Y, baseline_hash, run, section_file, stderr, stdout, temp_dir, text,
    write_header,
};

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

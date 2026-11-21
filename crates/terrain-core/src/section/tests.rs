//! Tests for section storage, the saved record codec, and baseline hashing.
//!
//! These were the `tests` module of `section.rs`; every test and assertion is
//! preserved by the split.

use std::path::Path;

use super::codec::RECORD_HEADER_LEN;
use super::storage::{bits_for, lane, lanes_per_word, mask_for, read_word, word_count, write_word};
use super::*;
use crate::content::{ContentPack, UNKNOWN_BASE};
use crate::description::{SECTION_SIZE, SECTION_VOLUME};
use crate::error::{ErrorKind, SectionFault, TerrainError};
use crate::hash::{domain, hash64};
use crate::header::crc32;

/// The smallest palette that forces each width.
const WIDTHS: [(u8, usize); 5] = [(1, 2), (2, 3), (4, 5), (8, 17), (16, 257)];

/// A heap-backed cell array, because the API takes `&[BlockId; SECTION_VOLUME]`.
fn scratch_blocks() -> Box<[BlockId; SECTION_VOLUME]> {
    vec![0_u16; SECTION_VOLUME]
        .into_boxed_slice()
        .try_into()
        .expect("the scratch array matches the section volume")
}

fn palette_of(entries: usize) -> Vec<BlockId> {
    (0..entries)
        .map(|slot| u16::try_from(slot + 1).expect("a palette id fits u16"))
        .collect()
}

fn boundary_cells(per_word: usize) -> Vec<usize> {
    let middle = (SECTION_VOLUME / 2 / per_word) * per_word;
    let last = SECTION_VOLUME - per_word;
    let mut cells = vec![
        0,
        per_word - 1,
        per_word,
        2 * per_word - 1,
        middle - 1,
        middle,
        middle + per_word - 1,
        last,
        last + per_word - 1,
    ];
    cells.sort_unstable();
    cells.dedup();
    cells
}

#[test]
fn index_and_inverse_round_trip() {
    for index in 0..SECTION_VOLUME {
        let (x, y, z) = section_coord(index);
        assert!(x < SECTION_SIZE && y < SECTION_SIZE && z < SECTION_SIZE);
        assert_eq!(section_index(x, y, z), index);
        let recomputed = usize::try_from(x + 32 * (z + 32 * y)).expect("index fits usize");
        assert_eq!(section_index(x, y, z), recomputed);
    }

    assert_eq!(section_index(SECTION_SIZE, 0, 0), section_index(0, 0, 0));
    assert_eq!(
        section_index(0, 0, SECTION_SIZE + 7),
        section_index(0, 0, 7)
    );
    assert_eq!(
        section_index(0, SECTION_SIZE + 3, 0),
        section_index(0, 3, 0)
    );
}

#[test]
fn pack_unpack_at_every_bit_width() {
    let tag = domain("section:test");
    for (bits, entries) in WIDTHS {
        let palette = palette_of(entries);
        let empty = vec![0_u64; word_count(bits)];
        let mut section = Section::from_palette(&palette, bits, &empty).expect("valid palette");
        assert_eq!(section.bits(), bits);

        let mut expected = vec![palette[0]; SECTION_VOLUME];
        for (index, cell) in expected.iter_mut().enumerate() {
            let hash = hash64(1, tag, &[i64::try_from(index).expect("index fits")]);
            let slot = usize::try_from(hash % u64::try_from(entries).expect("entries fit"))
                .expect("slot fits usize");
            *cell = palette[slot];
            section.set_index(index, palette[slot]);
        }

        assert_eq!(section.bits(), bits, "no growth for a full palette");
        let mut out = scratch_blocks();
        section.write_blocks(&mut out);
        assert_eq!(out.as_slice(), expected.as_slice(), "bits {bits}");
        for (index, id) in expected.iter().enumerate() {
            assert_eq!(section.get_index(index), *id, "bits {bits} index {index}");
        }
    }
}

#[test]
fn palette_growth_repacks() {
    let ids: Vec<BlockId> = (0..300_u16).map(|i| i * 7 + 3).collect();
    let mut section = Section::uniform(ids[0]);
    let mut expected = vec![ids[0]; SECTION_VOLUME];
    let mut assigned = 0;

    for (count, width) in [(1_usize, 1_u8), (3, 2), (9, 4), (17, 8), (300, 16)] {
        while assigned < count {
            // 101 is coprime with the volume, so the first 300 cells differ.
            let cell = (assigned * 101 + 7) % SECTION_VOLUME;
            section.set_index(cell, ids[assigned]);
            expected[cell] = ids[assigned];
            assigned += 1;
        }
        assert_eq!(section.bits(), width, "count {count}");
        assert_eq!(section.palette().len(), count, "count {count}");
        let mut out = scratch_blocks();
        section.write_blocks(&mut out);
        assert_eq!(out.as_slice(), expected.as_slice(), "count {count}");
    }
}

#[test]
fn uniform_promotes_and_demotes() {
    let mut section = Section::uniform(9);
    assert_eq!(section.is_uniform(), Some(9));
    assert_eq!(section.solid_count(), SECTION_VOLUME);
    assert!(matches!(section, Section::Uniform(9)));

    section.set(4, 5, 6, 12);
    assert!(matches!(section, Section::Paletted { .. }));
    assert_eq!(section.is_uniform(), None);
    assert_eq!(section.get(4, 5, 6), 12);
    assert_eq!(section.get(4, 5, 7), 9);
    assert_eq!(section.bits(), 1);

    section.set(4, 5, 6, 9);
    assert!(
        matches!(section, Section::Paletted { .. }),
        "a write never demotes implicitly"
    );
    assert_eq!(section.solid_count(), SECTION_VOLUME);

    section.compact();
    assert!(matches!(section, Section::Uniform(9)));
    assert_eq!(section.is_uniform(), Some(9));
    assert_eq!(section.bits(), 1);
}

#[test]
fn unknown_name_round_trip() {
    let name: BlockId = 60_000;
    let mut blocks = scratch_blocks();
    blocks.fill(name);
    blocks[0] = 0;
    blocks[17] = 12_345;

    let mut section = Section::from_blocks(&blocks);
    assert!(matches!(section, Section::Paletted { .. }));
    assert_eq!(section.get_index(0), 0);
    assert_eq!(section.get_index(17), 12_345);
    assert_eq!(section.get(1, 2, 3), name);

    section.compact();
    assert!(
        matches!(section, Section::Paletted { .. }),
        "two ids stay paletted"
    );
    section.set_index(SECTION_VOLUME - 1, name);
    assert_eq!(section.get(31, 31, 31), name);

    let mut out = scratch_blocks();
    section.write_blocks(&mut out);
    assert_eq!(out.as_slice(), blocks.as_slice());

    let rebuilt = Section::from_blocks(&blocks);
    assert_eq!(rebuilt.content_hash(), section.content_hash());
    assert_eq!(rebuilt, section);
    assert!(section.palette_snapshot().contains(&name));
}

#[test]
fn content_hash_ignores_palette_order() {
    let mut blocks = scratch_blocks();
    blocks.fill(11);
    blocks[1] = 22;
    blocks[2] = 33;

    let sorted = Section::from_blocks(&blocks);
    let mut incremental = Section::uniform(11);
    for (index, id) in blocks.iter().enumerate().rev() {
        incremental.set_index(index, *id);
    }

    assert_ne!(
        sorted.palette_snapshot(),
        incremental.palette_snapshot(),
        "the test needs two different palette orders"
    );
    assert_eq!(sorted, incremental);
    assert_eq!(sorted.content_hash(), incremental.content_hash());

    let mut other = blocks;
    other[3] = 44;
    assert_ne!(
        sorted.content_hash(),
        Section::from_blocks(&other).content_hash()
    );
}

#[test]
fn solid_count_is_o1_and_correct() {
    let mut section = Section::uniform(0);
    assert_eq!(section.solid_count(), 0);
    assert!(section.is_empty(0));

    section = Section::uniform(5);
    assert_eq!(section.solid_count(), SECTION_VOLUME);
    assert!(!section.is_empty(0));

    let mut solid = SECTION_VOLUME;
    for step in 0..1000_usize {
        let index = (step * 37) % SECTION_VOLUME;
        let id = u16::try_from(step % 6).expect("id fits u16");
        let previous = section.get_index(index);
        section.set_index(index, id);
        if previous != id {
            solid -= usize::from(previous != 0);
            solid += usize::from(id != 0);
        }
        assert_eq!(section.solid_count(), solid, "step {step}");
    }

    let mut cleared = Section::uniform(3);
    for index in 0..64 {
        cleared.set_index(index, 4);
    }
    for index in 0..SECTION_VOLUME {
        cleared.set_index(index, 0);
    }
    assert_eq!(cleared.solid_count(), 0);
    assert!(
        cleared.is_empty(0),
        "stale palette entries do not keep a section non-empty"
    );

    let air = 7_u16;
    let mut uniform_air = Section::uniform(air);
    assert!(uniform_air.is_empty(air));
    assert!(!uniform_air.is_empty(0));
    uniform_air.set(1, 1, 1, 8);
    assert!(!uniform_air.is_empty(air));
}

#[test]
fn words_never_straddle() {
    for (bits, entries) in WIDTHS {
        let palette = palette_of(entries);
        let empty = vec![0_u64; word_count(bits)];
        let mut section = Section::from_palette(&palette, bits, &empty).expect("valid palette");

        for index in 0..SECTION_VOLUME {
            section.set_index(index, palette[index % entries]);
        }

        let per_word = lanes_per_word(bits);
        for index in 0..SECTION_VOLUME {
            let (word, shift) = lane(index, bits);
            assert!(shift + usize::from(bits) <= 64, "bits {bits} index {index}");
            let stored = usize::try_from((section.words()[word] >> shift) & mask_for(bits))
                .expect("lane fits usize");
            assert_eq!(stored, index % entries, "bits {bits} index {index}");
        }

        for index in boundary_cells(per_word) {
            let (word, shift) = lane(index, bits);
            let mask = mask_for(bits) << shift;
            let before = section.words().to_vec();
            let next = (read_word(&before, index, bits) + 1) % entries;
            section.set_index(index, palette[next]);

            for (position, (old, new)) in before.iter().zip(section.words()).enumerate() {
                if position == word {
                    assert_eq!(
                        (old ^ new) & !mask,
                        0,
                        "bits {bits} index {index} wrote outside its lane"
                    );
                    assert_ne!(old, new, "bits {bits} index {index} did not change");
                } else {
                    assert_eq!(
                        old, new,
                        "bits {bits} index {index} touched word {position}"
                    );
                }
            }
        }
    }
}

/// A pack with `extra` synthetic `test:bNNN` records for wide palettes.
fn test_pack(extra: usize) -> ContentPack {
    let mut records = crate::content::v1_default().records().to_vec();
    let template = records[0].clone();
    for slot in 0..extra {
        let mut record = template.clone();
        record.name = format!("test:b{slot:03}");
        records.push(record);
    }
    ContentPack::load(records).expect("the test pack is valid")
}

/// The first `entries` names of `pack`, as ids.
fn palette_ids(pack: &ContentPack, entries: usize) -> Vec<BlockId> {
    pack.records()[..entries]
        .iter()
        .map(|record| pack.id(&record.name).expect("every record has an id"))
        .collect()
}

/// A paletted section holding `entries` distinct blocks at every cell.
fn paletted(pack: &ContentPack, entries: usize) -> Section {
    let palette = palette_ids(pack, entries);
    let bits = bits_for(entries);
    let mut data = vec![0_u64; word_count(bits)].into_boxed_slice();
    for index in 0..SECTION_VOLUME {
        let slot = (index.wrapping_mul(37) + index / 11) % entries;
        write_word(&mut data, index, bits, slot);
    }
    Section::from_palette(&palette, bits, &data).expect("a valid paletted section")
}

fn record_key() -> SectionKey {
    SectionKey {
        column: [3, -5],
        section: 7,
    }
}

const BASELINE: [u8; 32] = [0xAB; 32];

fn fault(error: &TerrainError) -> SectionFault {
    match &error.kind {
        ErrorKind::CorruptSection { fault, .. } => *fault,
        other => panic!("expected CorruptSection, found {other:?}"),
    }
}

/// Recomputes the trailing checksum after a deliberate field edit.
fn reseal(mut bytes: Vec<u8>) -> Vec<u8> {
    let body_end = bytes.len() - 4;
    let checksum = crc32(&bytes[..body_end]);
    bytes[body_end..].copy_from_slice(&checksum.to_le_bytes());
    bytes
}

fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{byte:02x}"));
    }
    out
}

/// A paletted section with the palette reversed and its lanes remapped.
fn reversed_palette(section: &Section) -> Section {
    let Section::Paletted {
        palette,
        bits,
        data,
        ..
    } = section
    else {
        panic!("the test needs a paletted section");
    };
    let reversed: Vec<BlockId> = palette.iter().rev().copied().collect();
    let last = palette.len() - 1;
    let mut words = vec![0_u64; word_count(*bits)].into_boxed_slice();
    for index in 0..SECTION_VOLUME {
        let old = read_word(data, index, *bits);
        write_word(&mut words, index, *bits, last - old);
    }
    Section::from_palette(&reversed, *bits, &words).expect("a valid reversed section")
}

#[test]
fn saved_record_round_trips_at_every_bit_width() {
    let pack = test_pack(300);
    for (bits, entries) in WIDTHS {
        let section = paletted(&pack, entries);
        assert_eq!(bits_for(entries), bits, "the test covers every width");
        let bytes = encode_record(&pack, record_key(), BASELINE, false, &section);
        let saved =
            decode_record(&pack, &bytes).unwrap_or_else(|error| panic!("bits {bits}: {error}"));
        assert_eq!(saved.key, record_key(), "bits {bits}");
        assert!(!saved.edited, "bits {bits}");
        assert_eq!(saved.baseline_hash, BASELINE, "bits {bits}");
        assert_eq!(saved.section, section, "bits {bits}");
        assert!(saved.unknown_names.is_empty(), "bits {bits}");
    }
}

#[test]
fn one_entry_palette_round_trips() {
    let pack = crate::content::v1_default();
    let stone = pack.id("core:stone").expect("core:stone is in the pack");
    let data = vec![0_u64; word_count(1)];
    let section = Section::from_palette(&[stone], 1, &data).expect("a one-entry palette is valid");

    let bytes = encode_record(&pack, record_key(), BASELINE, false, &section);
    let saved = decode_record(&pack, &bytes).expect("the record decodes");
    assert_eq!(saved.section, section);
    assert!(
        matches!(saved.section, Section::Paletted { .. }),
        "the record keeps the one-entry palette paletted"
    );
}

#[test]
fn uniform_round_trips_and_stays_small() {
    let pack = crate::content::v1_default();
    let stone = pack.id("core:stone").expect("core:stone is in the pack");
    for (id, expected_solid) in [(0_u16, 0_u32), (stone, 32_768)] {
        let section = Section::uniform(id);
        let bytes = encode_record(&pack, record_key(), BASELINE, false, &section);
        let name = pack.props(id).expect("the id resolves").name.as_str();
        assert_eq!(
            bytes.len(),
            RECORD_HEADER_LEN + 2 + name.len() + 4,
            "a uniform record carries no index payload"
        );
        let saved = decode_record(&pack, &bytes).expect("the record decodes");
        assert_eq!(saved.section, section);
        assert_eq!(saved.section.is_uniform(), Some(id));
        assert_eq!(
            saved.section.solid_count(),
            usize::try_from(expected_solid).expect("a solid count fits usize")
        );
    }
}

#[test]
fn unknown_names_degrade_and_are_reported() {
    let mut records = crate::content::v1_default().records().to_vec();
    let template = records[0].clone();
    let mut gone = template.clone();
    gone.name = "mod:gone".to_owned();
    let mut also = template;
    also.name = "mod:also".to_owned();
    records.push(gone);
    records.push(also);
    let source = ContentPack::load(records).expect("the extended pack is valid");
    let pack = crate::content::v1_default();

    let gone_id = source.id("mod:gone").expect("the extra name resolves");
    let also_id = source.id("mod:also").expect("the extra name resolves");
    let mut blocks = scratch_blocks();
    blocks.fill(0);
    blocks[0] = gone_id;
    blocks[7] = also_id;
    let section = Section::from_blocks(&blocks);
    let bytes = encode_record(&source, record_key(), BASELINE, false, &section);
    let saved = decode_record(&pack, &bytes).expect("an unknown name is not corruption");

    assert_eq!(saved.unknown_names, ["mod:also", "mod:gone"]);
    assert_eq!(saved.section.get_index(0), UNKNOWN_BASE + 1);
    assert_eq!(saved.section.get_index(7), UNKNOWN_BASE);
    assert_eq!(saved.section.get_index(1), 0);

    // An id outside the pack has no stable name; encode marks it so a
    // reader reports it instead of inventing content.
    let placeholder = Section::uniform(UNKNOWN_BASE);
    let bytes = encode_record(&pack, record_key(), BASELINE, false, &placeholder);
    let saved = decode_record(&pack, &bytes).expect("a placeholder decodes as unknown");
    assert_eq!(saved.section.is_uniform(), Some(UNKNOWN_BASE));
    assert_eq!(saved.unknown_names, ["?unknown:61440"]);
}

#[test]
fn truncation_is_a_hard_error_at_every_length() {
    let pack = test_pack(0);
    let section = paletted(&pack, 3);
    let bytes = encode_record(&pack, record_key(), BASELINE, false, &section);
    for length in 0..bytes.len() {
        let error = decode_record(&pack, &bytes[..length]).unwrap_err();
        let fault = fault(&error);
        assert!(
            matches!(fault, SectionFault::Length | SectionFault::Checksum),
            "a {length}-byte record reported {fault:?}"
        );
        if length < RECORD_HEADER_LEN + 4 {
            assert_eq!(
                fault,
                SectionFault::Length,
                "a short record is a length fault"
            );
        }
    }
}

#[test]
fn checksum_covers_payload_and_crc() {
    let pack = test_pack(0);
    let section = paletted(&pack, 3);
    let bytes = encode_record(&pack, record_key(), BASELINE, false, &section);

    let mut payload = bytes.clone();
    payload[RECORD_HEADER_LEN + 1] ^= 0x01;
    assert_eq!(
        fault(&decode_record(&pack, &payload).unwrap_err()),
        SectionFault::Checksum
    );

    let mut crc = bytes;
    let last = crc.len() - 1;
    crc[last] ^= 0x01;
    assert_eq!(
        fault(&decode_record(&pack, &crc).unwrap_err()),
        SectionFault::Checksum
    );
}

#[test]
fn impossible_bit_widths_are_framing_faults() {
    let pack = test_pack(0);
    let section = paletted(&pack, 3);
    assert_eq!(bits_for(3), 2);
    let bytes = encode_record(&pack, record_key(), BASELINE, false, &section);

    for bad in [0_u8, 1, 3, 5, 9, 32] {
        let mut mutated = bytes.clone();
        mutated[RECORD_HEADER_LEN] = bad;
        let mutated = reseal(mutated);
        assert_eq!(
            fault(&decode_record(&pack, &mutated).unwrap_err()),
            SectionFault::Framing,
            "bits {bad}"
        );
    }
}

#[test]
fn an_out_of_range_index_is_a_framing_fault() {
    let pack = test_pack(0);
    let section = paletted(&pack, 3);
    let bytes = encode_record(&pack, record_key(), BASELINE, false, &section);
    let words_at = bytes.len() - 4 - word_count(2) * 8;

    let mut mutated = bytes;
    mutated[words_at] = 3;
    let mutated = reseal(mutated);
    assert_eq!(
        fault(&decode_record(&pack, &mutated).unwrap_err()),
        SectionFault::Framing
    );
}

#[test]
fn edited_and_baseline_are_preserved() {
    let pack = crate::content::v1_default();
    let section = Section::uniform(0);
    for edited in [false, true] {
        let baseline = [u8::from(edited); 32];
        let bytes = encode_record(&pack, record_key(), baseline, edited, &section);
        let saved = decode_record(&pack, &bytes).expect("the record decodes");
        assert_eq!(saved.edited, edited);
        assert_eq!(saved.baseline_hash, baseline);
    }
}

#[test]
fn encoding_is_deterministic_and_ignores_palette_order() {
    let pack = test_pack(0);
    let section = paletted(&pack, 5);
    let first = encode_record(&pack, record_key(), BASELINE, true, &section);
    let second = encode_record(&pack, record_key(), BASELINE, true, &section);
    assert_eq!(first, second, "two encodes of one section are identical");

    let reversed = reversed_palette(&section);
    assert_ne!(
        section.palette_snapshot(),
        reversed.palette_snapshot(),
        "the test needs two different palette orders"
    );
    assert_eq!(section, reversed);
    assert_eq!(
        first,
        encode_record(&pack, record_key(), BASELINE, true, &reversed),
        "a name-sorted palette makes the bytes canonical"
    );
}

#[test]
fn a_missing_core_name_is_a_manifest_failure() {
    let mut records = crate::content::v1_default().records().to_vec();
    records.retain(|record| record.name != "core:water");
    let pack = ContentPack::load(records).expect("the reduced pack is valid");
    let bytes = encode_record(
        &crate::content::v1_default(),
        record_key(),
        BASELINE,
        false,
        &Section::uniform(0),
    );
    let error = decode_record(&pack, &bytes).unwrap_err();
    assert!(
        matches!(&error.kind, ErrorKind::ManifestIncompatible { id, .. } if id == "core:water"),
        "found {error:?}"
    );
}

#[test]
fn a_path_is_named_in_a_file_record_error() {
    let pack = crate::content::v1_default();
    let bytes = encode_record(&pack, record_key(), BASELINE, false, &Section::uniform(0));
    let mut corrupted = bytes;
    corrupted[RECORD_HEADER_LEN + 2] ^= 0x01;
    let path = Path::new("world/chunks/c.0.0.0.sect");
    let error = decode_record_at(&pack, path, &corrupted).unwrap_err();
    match &error.kind {
        ErrorKind::CorruptSection { path: named, fault } => {
            assert_eq!(named, path);
            assert_eq!(*fault, SectionFault::Checksum);
        }
        other => panic!("expected CorruptSection, found {other:?}"),
    }
}

#[test]
fn uniform_record_bytes_are_golden() {
    let pack = crate::content::v1_default();
    let stone = pack.id("core:stone").expect("core:stone is in the pack");
    let bytes = encode_record(
        &pack,
        record_key(),
        BASELINE,
        true,
        &Section::uniform(stone),
    );
    assert_eq!(to_hex(&bytes), GOLDEN_UNIFORM_HEX);
}

#[test]
fn paletted_record_prefix_is_golden() {
    let pack = crate::content::v1_default();
    let stone = pack.id("core:stone").expect("core:stone is in the pack");
    let air = pack.id("core:air").expect("core:air is in the pack");
    let mut data = vec![0_u64; word_count(1)].into_boxed_slice();
    write_word(&mut data, 0, 1, 1);
    let section =
        Section::from_palette(&[air, stone], 1, &data).expect("a two-entry palette is valid");

    let bytes = encode_record(&pack, record_key(), BASELINE, false, &section);
    assert_eq!(bytes.len(), RECORD_HEADER_LEN + 1 + 4 + 22 + 4096 + 4);
    assert_eq!(
        u64::from_le_bytes(
            bytes[RECORD_HEADER_LEN + 27..RECORD_HEADER_LEN + 35]
                .try_into()
                .expect("eight bytes")
        ),
        1,
        "cell zero holds palette slot one"
    );
    assert_eq!(
        to_hex(&bytes[..RECORD_HEADER_LEN + 35]),
        GOLDEN_PALETTED_PREFIX_HEX
    );
}

/// A fixed three-id fixture pins the content hash, the baseline hash, and the
/// record length and CRC, so no part of the split can change hashing or bytes.
#[test]
fn fixture_hash_and_record_bytes_are_pinned() {
    let pack = crate::content::v1_default();
    let stone = pack.id("core:stone").expect("core:stone is in the pack");
    let water = pack.id("core:water").expect("core:water is in the pack");
    let mut blocks = scratch_blocks();
    blocks.fill(stone);
    for index in [0, 1, 17, SECTION_VOLUME / 2, SECTION_VOLUME - 1] {
        blocks[index] = 0;
    }
    blocks[section_index(3, 4, 5)] = water;
    let section = Section::from_blocks(&blocks);

    assert_eq!(section.content_hash(), 0x323b_4b35_f080_7b12);
    assert_eq!(
        baseline_hash(&section),
        [
            0x14, 0x4b, 0x61, 0xb1, 0x06, 0x00, 0x8a, 0x68, 0xc5, 0x83, 0x9b, 0xac, 0x0d, 0xc1,
            0x6f, 0x25, 0x65, 0x79, 0x82, 0x95, 0xaa, 0xe5, 0x4c, 0x12, 0x89, 0xdc, 0xd0, 0x62,
            0xf5, 0x78, 0x0e, 0x53,
        ]
    );

    let bytes = encode_record(&pack, record_key(), BASELINE, false, &section);
    assert_eq!(bytes.len(), 8288);
    assert_eq!(
        u32::from_le_bytes(bytes[bytes.len() - 4..].try_into().expect("four bytes")),
        0x6a48_5c7f
    );
}

const GOLDEN_UNIFORM_HEX: &str = "54534543020003000000fbffffff0701abababababababababababababababababababababababababababababababab00008000000a00636f72653a73746f6e655fd7787b";
const GOLDEN_PALETTED_PREFIX_HEX: &str = "54534543020003000000fbffffff0700abababababababababababababababababababababababababababababababab010100000001020000000800636f72653a6169720a00636f72653a73746f6e650100000000000000";

//! `repair` tests.

use std::fs;

use terrain_core::content;
use terrain_core::description;
use terrain_core::save::SectionStore;
use terrain_core::section::{BlockId, Section, SectionKey};

use crate::support::{assert_success, run, stdout, temp_dir, text};

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

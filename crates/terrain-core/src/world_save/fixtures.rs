//! Shared fixtures for the `world_save` module tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::description::SECTION_VOLUME;
use crate::editlog::{Edit, EditBatch};
use crate::section::{BlockId, Section, SectionKey};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

pub(super) struct TempWorld(PathBuf);

impl TempWorld {
    pub(super) fn new(tag: &str) -> Self {
        let serial = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "valescent-world-{}-{tag}-{serial}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        Self(directory)
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempWorld {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub(super) fn uniform(block: BlockId) -> Section {
    let blocks: Box<[BlockId; SECTION_VOLUME]> = vec![block; SECTION_VOLUME]
        .into_boxed_slice()
        .try_into()
        .expect("the section volume matches");
    Section::from_blocks(&blocks)
}

pub(super) fn key(x: i32, z: i32, section: u8) -> SectionKey {
    SectionKey {
        column: [x, z],
        section,
    }
}

pub(super) fn batch(revision: u64) -> EditBatch {
    let record = Edit::record([0, 1, 0], "core:air", "core:stone", revision, 0, 0, 0)
        .expect("the change is recorded");
    EditBatch::new(revision, 0, vec![record])
}

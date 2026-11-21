//! Shared fixtures for the `save` module tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::content::{self, ContentPack};
use crate::description::SECTION_VOLUME;
use crate::editlog::{Edit, EditBatch};
use crate::section::{BlockId, Section, SectionKey};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

/// A unique world directory, removed when the test ends.
pub(super) struct TempWorld(PathBuf);

impl TempWorld {
    pub(super) fn new(tag: &str) -> Self {
        let serial = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "valescent-save-{}-{tag}-{serial}",
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

pub(super) fn pack() -> ContentPack {
    content::v1_default()
}

pub(super) fn id(name: &str) -> BlockId {
    pack().id(name).expect("the v1 pack has the name")
}

pub(super) fn uniform(block: BlockId) -> Section {
    let blocks: Box<[BlockId; SECTION_VOLUME]> = vec![block; SECTION_VOLUME]
        .into_boxed_slice()
        .try_into()
        .expect("the section volume matches");
    Section::from_blocks(&blocks)
}

pub(super) fn key() -> SectionKey {
    SectionKey {
        column: [2, -3],
        section: 4,
    }
}

pub(super) fn baseline() -> [u8; 32] {
    [7; 32]
}

pub(super) fn batch(revision: u64) -> EditBatch {
    let record = Edit::record([0, 1, 0], "core:air", "core:stone", revision, 0, 0, 0)
        .expect("the change is recorded");
    EditBatch::new(revision, 0, vec![record])
}

/// Hex-encodes bytes for a pinned-bytes assertion.
pub(super) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

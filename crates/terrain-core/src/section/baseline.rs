//! Content and baseline hashing for a section (`I2.1`, `J4.1`).
//!
//! [`Section::content_hash`] is the 64-bit palette-order-independent content
//! digest. [`baseline_hash`] is the separate 32-byte digest of the ids as first
//! generated, which the saved record stores.

use super::storage::Section;
use crate::description::SECTION_VOLUME;
use crate::hash::mix;

/// Seed of [`Section::content_hash`].
///
/// Changing it changes every baseline hash, so it is part of the save contract.
const CONTENT_HASH_SEED: u64 = 0x5345_4354_494f_4e00;

impl Section {
    /// A stable hash of the content, independent of palette order.
    ///
    /// Folds the sorted palette first, then the decoded ids in index order. Two
    /// sections with the same blocks hash equally however their palettes were
    /// discovered, which is what `I2.1` baselines.
    #[must_use]
    pub fn content_hash(&self) -> u64 {
        let mut hash = mix(CONTENT_HASH_SEED);
        match self {
            Self::Uniform(id) => {
                hash = mix(hash ^ u64::from(*id));
                for _ in 0..SECTION_VOLUME {
                    hash = mix(hash ^ u64::from(*id));
                }
            }
            Self::Paletted { palette, .. } => {
                let mut sorted = palette.clone();
                sorted.sort_unstable();
                for id in &sorted {
                    hash = mix(hash ^ u64::from(*id));
                }
                for index in 0..SECTION_VOLUME {
                    hash = mix(hash ^ u64::from(self.get_index(index)));
                }
            }
        }
        hash
    }
}

/// The saved `baseline_hash` (`J4.1`): `SHA-256` over the decoded block ids of
/// one section, two little-endian bytes per cell, in
/// `crate::section::section_index` order.
///
/// This is the one pinned derivation. The save writer computes it over the
/// section as first generated and records it in the v2 record; `terrain-gen
/// verify` recomputes it over the decoded regenerated section and compares the
/// two. It is deliberately **not** [`Section::content_hash`]: that is a 64-bit
/// content digest with a different domain and does not fit the record's
/// 32-byte field.
#[must_use]
pub fn baseline_hash(section: &Section) -> [u8; 32] {
    let mut bytes = Vec::with_capacity(SECTION_VOLUME * 2);
    for index in 0..SECTION_VOLUME {
        bytes.extend_from_slice(&section.get_index(index).to_le_bytes());
    }
    crate::hash::sha256(&bytes)
}

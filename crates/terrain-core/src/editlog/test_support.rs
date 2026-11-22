//! Shared fixtures for the edit-log unit tests.

use crate::error::Coord;

use super::frame::{Edit, EditBatch};

pub(super) fn record(position: Coord, change: (&str, &str), order: (u64, u32, u32)) -> Edit {
    Edit::record(position, change.0, change.1, order.0, 7, order.1, order.2)
        .expect("test records always change the block")
}

/// Bypasses [`Edit::record`] so a test can hand a no-op to a filter.
pub(super) fn raw(position: Coord, old: &str, new: &str, order: (u64, u32, u32)) -> Edit {
    Edit {
        x: position[0],
        y: position[1],
        z: position[2],
        old: old.to_owned(),
        new: new.to_owned(),
        revision: order.0,
        author: 7,
        batch: order.1,
        seq: order.2,
    }
}

pub(super) fn sample_batch() -> EditBatch {
    let mut batch = EditBatch::new(0, 7, Vec::new());
    assert!(batch.push([1, 2, 3], "core:stone", "core:air", 0));
    assert!(batch.push([4, 5, 6], "core:air", "modpacks:mythril_ore", 0));
    batch.commit(3);
    batch
}

pub(super) fn batch_at(revision: u64) -> EditBatch {
    let mut batch = EditBatch::new(revision, 7, Vec::new());
    assert!(batch.push([1, 2, 3], "core:stone", "core:air", 0));
    batch.commit(revision);
    batch
}

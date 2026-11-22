//! The edit queue: one block change queues its journal batch and section save.

use glam::IVec3;
use tracing::warn;

use crate::description;
use crate::editlog;
use crate::section;
use crate::world_save;

use super::TerrainDemo;

/// The save key for a world cell (`J4.1`).
pub(crate) fn section_key(cell: IVec3) -> section::SectionKey {
    let size = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
    section::SectionKey {
        column: [cell.x.div_euclid(size), cell.z.div_euclid(size)],
        section: u8::try_from(cell.y.div_euclid(size)).unwrap_or(0),
    }
}

/// Queues one edit's journal batch and section write.
///
/// The caller has already written the block into `demo.columns`; `baseline`
/// and both names must come from before the write.
pub(crate) fn queue_demo_edit(
    save: &world_save::SaveWorker,
    demo: &mut TerrainDemo,
    cell: IVec3,
    key: section::SectionKey,
    baseline: [u8; 32],
    old_name: String,
    new_name: String,
) {
    demo.revision = demo.revision.saturating_add(1);
    let revision = demo.revision;
    let section = demo
        .columns
        .get(&(key.column[0], key.column[1]))
        .and_then(|stack| stack.get(usize::from(key.section)))
        .cloned();
    let Some(section) = section else {
        return;
    };
    let Some(record) = editlog::Edit::record(
        [cell.x, cell.y, cell.z],
        old_name,
        new_name,
        revision,
        0,
        0,
        0,
    ) else {
        return;
    };
    let batch = editlog::EditBatch::new(revision, 0, vec![record]);
    if let Err(error) = save.record_batch(batch) {
        warn!(%error, "terrain journal write failed");
    }
    if let Err(error) = save.save_section(key, baseline, true, section, revision) {
        warn!(%error, "terrain section save failed");
    }
}

#[cfg(test)]
mod tests {
    use glam::{IVec2, IVec3};

    use crate::description;
    use crate::world_save;

    use super::super::persist::build_terrain_demo;

    use super::*;

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn an_edit_queues_a_journal_batch_and_a_section_save() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "valescent-demo-save-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&directory);
        {
            let mut demo =
                build_terrain_demo(0x5EED, IVec2::new(2, -3), 0).expect("the v1 terrain builds");
            let world = world_save::WorldSave::open_or_create(&directory, demo.seed, "Demo test")
                .expect("the world opens");
            let save = world_save::SaveWorker::start(world, 8).expect("the worker starts");

            let cell = IVec3::new(
                demo.spawn.x.floor() as i32,
                demo.spawn.y.round() as i32 - 1,
                demo.spawn.z.floor() as i32,
            );
            let key = section_key(cell);
            let baseline = demo.baseline_for(key);
            let old = demo.block_at(cell).expect("the spawn block exists");
            let old_name = demo
                .pack
                .name(old)
                .expect("the block has a name")
                .to_owned();
            let new_name = demo.pack.name(demo.air).expect("air has a name").to_owned();
            assert!(demo.set_block(cell, demo.air));
            queue_demo_edit(&save, &mut demo, cell, key, baseline, old_name, new_name);
            let revision = demo.revision;
            save.shutdown().expect("the worker stops");

            let reopened =
                world_save::WorldSave::open(&directory, demo.seed).expect("the world reopens");
            assert_eq!(reopened.header.revision, revision);
            assert!(
                reopened.recovered().is_empty(),
                "the section save covers and trims the batch"
            );
            let sections = reopened.read_sections().expect("the sections read");
            assert_eq!(sections.len(), 1);
            assert_eq!(sections[0].key, key);
            let size = i32::try_from(description::SECTION_SIZE).unwrap_or(32);
            assert_eq!(
                sections[0].section.get(
                    u32::try_from(cell.x.rem_euclid(size)).unwrap_or(0),
                    u32::try_from(cell.y.rem_euclid(size)).unwrap_or(0),
                    u32::try_from(cell.z.rem_euclid(size)).unwrap_or(0),
                ),
                demo.air
            );
        }
        let _ = std::fs::remove_dir_all(&directory);
    }
}

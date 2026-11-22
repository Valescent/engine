//! `migrate` tests.

use std::fs;

use terrain_core::header::MigrationState;
use terrain_core::world_save::WorldSave;

use crate::support::{assert_success, run, stdout, temp_dir, text};

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

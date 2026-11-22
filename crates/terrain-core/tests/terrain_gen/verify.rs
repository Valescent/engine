//! `verify` tests.

use std::fs;

use crate::support::{
    assert_success, directory_hash, reseal, run, section_file, stderr, stdout, temp_dir, text,
    write_world,
};

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

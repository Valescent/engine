//! `replay` tests.

use std::fs;

use terrain_core::world_save::GENERATOR_VERSION;

use crate::support::{assert_success, run, stderr, stdout, temp_dir, text, write_bundle};

#[test]
fn replay_matches_its_own_bundle() {
    let bundle = write_bundle("replay-ok", 0x5EED, (5, 9), None);

    let output = run(&["replay", "--report", text(&bundle)]);
    assert_success(&output, "replay on its own bundle");
    let out = stdout(&output);
    assert!(out.contains("stage column ok"), "stdout:\n{out}");
    assert!(out.contains("stage section:4 ok"), "stdout:\n{out}");
    assert!(
        stderr(&output).contains("warning:"),
        "a differing platform tag must warn, stderr:\n{}",
        stderr(&output)
    );

    // The same bundle may be named by its manifest file, and its output never
    // depends on `--threads` or `--order`.
    let manifest = bundle.join("manifest.json");
    let variant = run(&[
        "replay",
        "--report",
        text(&manifest),
        "--threads",
        "3",
        "--order",
        "reverse",
    ]);
    assert_success(&variant, "replay is thread and order invariant");
    assert_eq!(
        out,
        stdout(&variant),
        "stdout changed for --threads/--order"
    );

    let from_file = run(&["replay", "--report", text(&manifest), "--json"]);
    let _ = fs::remove_dir_all(&bundle);
    assert_success(&from_file, "replay --report manifest.json --json");
    let line = stdout(&from_file);
    assert_eq!(
        line.trim().lines().count(),
        1,
        "stdout must be one JSON line"
    );
    assert!(
        line.contains("\"subcommand\":\"replay\"") && line.contains("\"ok\":true"),
        "stdout:\n{line}"
    );
}

#[test]
fn replay_names_the_first_differing_stage() {
    let wrong = "0".repeat(64);
    let bundle = write_bundle("replay-drift", 0x5EED, (5, 9), Some(&wrong));

    let output = run(&["replay", "--report", text(&bundle)]);
    let code = output.status.code();
    let out = stdout(&output);
    let _ = fs::remove_dir_all(&bundle);
    assert_eq!(
        code,
        Some(1),
        "a differing stage must exit 1\nstdout:\n{out}"
    );

    let column = out
        .find("stage column ok")
        .expect("the first stage is reported ok");
    let section = out
        .find("stage section:4 differ")
        .expect("the differing stage is named");
    assert!(column < section, "stages must be reported in order:\n{out}");
    assert!(out.contains(&wrong), "the expected hash is printed:\n{out}");
}

#[test]
fn replay_refuses_an_unknown_generator_version() {
    let bundle = write_bundle("replay-version", 0x5EED, (5, 9), None);
    let path = bundle.join("manifest.json");
    let manifest = fs::read_to_string(&path).expect("the manifest is readable");
    fs::write(
        &path,
        manifest.replace(
            &format!("\"generator_version\": {GENERATOR_VERSION}"),
            "\"generator_version\": 999",
        ),
    )
    .expect("the edited manifest is written");

    let output = run(&["replay", "--report", text(&bundle)]);
    let code = output.status.code();
    let message = format!("{}{}", stdout(&output), stderr(&output));
    let _ = fs::remove_dir_all(&bundle);
    assert_eq!(code, Some(1), "an unshipped generator version is refused");
    assert!(
        message.contains("generator version"),
        "the refusal names the version:\n{message}"
    );
}

#[test]
fn replay_usage_errors_exit_two() {
    let missing = run(&["replay"]);
    assert_eq!(missing.status.code(), Some(2), "replay needs --report");

    let directory = temp_dir("replay-usage");
    fs::write(directory.join("manifest.json"), "{}\n").expect("the bad manifest is written");
    let malformed = run(&["replay", "--report", text(&directory)]);
    let _ = fs::remove_dir_all(&directory);
    assert_eq!(
        malformed.status.code(),
        Some(2),
        "an incomplete manifest is a usage error\nstderr:\n{}",
        stderr(&malformed)
    );
}

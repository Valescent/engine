//! `describe`, `gen`, `seams`, `sweep`, and usage tests.

use crate::support::{assert_success, run, stderr, stdout};

#[test]
fn describe_round_trips_through_a_file() {
    let path = std::env::temp_dir().join(format!(
        "engine-terrain-gen-describe-{}.toml",
        std::process::id()
    ));
    let path_text = path
        .to_str()
        .expect("the temporary path must be valid UTF-8");

    let written = run(&["describe", "--out", path_text]);
    assert_success(&written, "describe --out");
    let text = stdout(&written);
    assert!(
        text.contains("description_hash "),
        "describe must print the hash:\n{text}"
    );

    let diff = run(&["describe-diff", "--file", path_text]);
    let _ = std::fs::remove_file(&path);
    assert_success(&diff, "describe-diff on an emitted description");
    assert!(
        stdout(&diff).contains("v1_default_shape yes"),
        "stdout:\n{}",
        stdout(&diff)
    );
}

#[test]
fn gen_is_thread_and_order_invariant() {
    let base = run(&["gen", "--seed", "7", "--from", "-3", "5", "--size", "6"]);
    assert_success(&base, "baseline gen");
    let expected = stdout(&base);

    let variants: [&[&str]; 5] = [
        &[
            "gen",
            "--seed",
            "7",
            "--from",
            "-3",
            "5",
            "--size",
            "6",
            "--threads",
            "4",
        ],
        &[
            "gen", "--seed", "7", "--from", "-3", "5", "--size", "6", "--order", "reverse",
        ],
        &[
            "gen", "--seed", "7", "--from", "-3", "5", "--size", "6", "--order", "shuffled",
        ],
        &[
            "gen", "--seed", "7", "--from", "-3", "5", "--size", "6", "--order", "3",
        ],
        &[
            "gen",
            "--seed",
            "7",
            "--from",
            "-3",
            "5",
            "--size",
            "6",
            "--threads",
            "4",
            "--order",
            "shuffled",
        ],
    ];
    for variant in variants {
        let output = run(variant);
        assert_success(&output, &format!("gen {variant:?}"));
        assert_eq!(expected, stdout(&output), "stdout changed for {variant:?}");
    }

    let json = run(&[
        "gen", "--seed", "7", "--from", "-3", "5", "--size", "6", "--json",
    ]);
    assert_success(&json, "gen --json");
    let text = stdout(&json);
    assert_eq!(
        text.trim().lines().count(),
        1,
        "stdout must be one JSON line"
    );
    assert!(text.contains("\"subcommand\":\"gen\""));
}

#[test]
fn seams_are_bit_identical() {
    let output = run(&["seams", "--window", "2x2", "--seeds", "1"]);
    assert_success(&output, "seams");
    let text = stdout(&output);
    assert!(text.contains("pairs 4"), "stdout:\n{text}");
    assert!(text.contains("ok"), "stdout:\n{text}");
}

#[test]
fn sweep_pr_passes() {
    let output = run(&["sweep", "--tier", "pr", "--seeds", "4"]);
    assert_success(&output, "sweep pr");
    let text = stdout(&output);
    assert!(text.contains("failures=0"), "stdout:\n{text}");
    assert_eq!(
        text.lines()
            .filter(|line| line.starts_with("seed "))
            .count(),
        4
    );
}

#[test]
fn usage_error_exits_two() {
    let empty = run(&[]);
    assert_eq!(empty.status.code(), Some(2), "no arguments must exit 2");
    assert!(stderr(&empty).contains("usage:"));

    let unknown = run(&["definitely-not-a-command"]);
    assert_eq!(
        unknown.status.code(),
        Some(2),
        "unknown command must exit 2"
    );
    assert!(
        stderr(&unknown).contains("unknown subcommand"),
        "stderr:\n{}",
        stderr(&unknown)
    );
}

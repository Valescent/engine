//! `counters` tests.

use std::fs;

use crate::support::{assert_success, run, stderr, stdout, temp_dir, text};

/// Scales one baseline counter by `percent`, keeping the rest of the file.
fn scale_counter(text: &str, name: &str, percent: u64) -> String {
    let needle = format!("\"{name}\": ");
    let start = text
        .find(&needle)
        .unwrap_or_else(|| panic!("baseline has no {name}"))
        + needle.len();
    let digits: String = text[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let value = digits.parse::<u64>().expect("the counter is an integer");
    let drifted = value * (100 + percent) / 100;
    text.replacen(
        &format!("\"{name}\": {value}"),
        &format!("\"{name}\": {drifted}"),
        1,
    )
}

#[test]
fn counters_match_the_checked_in_baseline() {
    let output = run(&["counters", "--tier", "pr"]);
    assert_success(&output, "counters --tier pr");
    let text = stdout(&output);
    assert!(text.contains("tier pr"), "stdout:\n{text}");
    assert!(text.contains("blocks_air "), "stdout:\n{text}");
    assert!(text.contains("ok"), "stdout:\n{text}");

    let json = run(&["counters", "--tier", "pr", "--json"]);
    assert_success(&json, "counters --json");
    let line = stdout(&json);
    assert_eq!(
        line.trim().lines().count(),
        1,
        "stdout must be one JSON line"
    );
    assert!(
        line.contains("\"subcommand\":\"counters\"") && line.contains("\"ok\":true"),
        "stdout:\n{line}"
    );
}

#[test]
fn counters_are_thread_and_order_invariant() {
    let directory = temp_dir("counters-order");
    let file = directory.join("baseline.json");
    let file_text = text(&file);
    let update = run(&[
        "counters", "--tier", "pr", "--seeds", "2", "--file", file_text, "--update",
    ]);
    assert_success(&update, "counters --update");

    let base = run(&[
        "counters", "--tier", "pr", "--seeds", "2", "--file", file_text,
    ]);
    assert_success(&base, "counters check");
    let expected = stdout(&base);

    for (threads, order) in [("4", "reverse"), ("2", "shuffled")] {
        let variant = run(&[
            "counters",
            "--tier",
            "pr",
            "--seeds",
            "2",
            "--file",
            file_text,
            "--threads",
            threads,
            "--order",
            order,
        ]);
        assert_success(&variant, "counters invariance");
        assert_eq!(
            expected,
            stdout(&variant),
            "stdout changed for --threads {threads} --order {order}"
        );
    }
    let _ = fs::remove_dir_all(&directory);
}

#[test]
fn counters_fail_on_a_ten_percent_drift() {
    let directory = temp_dir("counters-drift");
    let file = directory.join("baseline.json");
    let file_text = text(&file);
    let update = run(&[
        "counters", "--tier", "pr", "--seeds", "2", "--file", file_text, "--update",
    ]);
    assert_success(&update, "counters --update");

    let baseline = fs::read_to_string(&file).expect("the baseline is readable");
    let drifted = scale_counter(&baseline, "blocks_air", 10);
    assert_ne!(baseline, drifted, "the fixture must actually change");
    fs::write(&file, drifted).expect("the drifted baseline is written");

    let output = run(&[
        "counters", "--tier", "pr", "--seeds", "2", "--file", file_text,
    ]);
    let code = output.status.code();
    let out = stdout(&output);
    let err = stderr(&output);
    assert_eq!(code, Some(1), "a 10% drift must exit 1\nstdout:\n{out}");
    assert!(
        out.contains("drift blocks_air"),
        "the drift names the counter:\n{out}"
    );
    assert!(
        err.contains("counter drift"),
        "the diagnostic names the drift:\n{err}"
    );

    let json = run(&[
        "counters", "--tier", "pr", "--seeds", "2", "--file", file_text, "--json",
    ]);
    let _ = fs::remove_dir_all(&directory);
    assert_eq!(json.status.code(), Some(1), "JSON drift must exit 1");
    let line = stdout(&json);
    assert_eq!(
        line.trim().lines().count(),
        1,
        "stdout must be one JSON line"
    );
    assert!(
        line.contains("\"ok\":false") && line.contains("\"name\":\"blocks_air\""),
        "stdout:\n{line}"
    );
}

#[test]
fn counters_usage_errors_exit_two() {
    let missing = run(&["counters"]);
    assert_eq!(missing.status.code(), Some(2), "counters needs --tier");

    let missing_file = run(&[
        "counters",
        "--tier",
        "pr",
        "--file",
        "no-such-baseline.json",
    ]);
    assert_eq!(
        missing_file.status.code(),
        Some(2),
        "a missing baseline is a usage error\nstderr:\n{}",
        stderr(&missing_file)
    );
}

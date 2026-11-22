//! `calibrate-blend` tests.

use crate::support::{run, stderr, stdout};

#[test]
fn calibrate_blend_is_thread_and_order_invariant() {
    let base = run(&[
        "calibrate-blend",
        "--seeds",
        "2",
        "--from",
        "0",
        "0",
        "--size",
        "256",
        "--step",
        "16",
    ]);
    let expected = stdout(&base);
    let code = base.status.code();
    assert!(
        expected.contains("pair samples median p10 p90 ok"),
        "the table is pinned:\n{expected}"
    );

    for (threads, order) in [("4", "reverse"), ("2", "shuffled")] {
        let variant = run(&[
            "calibrate-blend",
            "--seeds",
            "2",
            "--from",
            "0",
            "0",
            "--size",
            "256",
            "--step",
            "16",
            "--threads",
            threads,
            "--order",
            order,
        ]);
        assert_eq!(
            code,
            variant.status.code(),
            "exit code changed with {order}"
        );
        assert_eq!(
            expected,
            stdout(&variant),
            "stdout changed for --threads {threads} --order {order}"
        );
    }

    let json = run(&[
        "calibrate-blend",
        "--seeds",
        "2",
        "--from",
        "0",
        "0",
        "--size",
        "256",
        "--step",
        "16",
        "--json",
    ]);
    let line = stdout(&json);
    assert_eq!(
        line.trim().lines().count(),
        1,
        "stdout must be one JSON line"
    );
    assert!(
        line.contains("\"subcommand\":\"calibrate-blend\"") && line.contains("\"pairs\":["),
        "stdout:\n{line}"
    );
}

#[test]
fn calibrate_blend_usage_errors_exit_two() {
    let missing = run(&["calibrate-blend"]);
    assert_eq!(
        missing.status.code(),
        Some(2),
        "calibrate-blend needs --seeds"
    );

    let bad_step = run(&["calibrate-blend", "--seeds", "1", "--step", "3"]);
    assert_eq!(
        bad_step.status.code(),
        Some(2),
        "--step must be a multiple of the macro cell\nstderr:\n{}",
        stderr(&bad_step)
    );

    let outside = run(&[
        "calibrate-blend",
        "--seeds",
        "1",
        "--from",
        "2000",
        "0",
        "--size",
        "256",
    ]);
    assert_eq!(
        outside.status.code(),
        Some(2),
        "a window outside the world is a usage error\nstderr:\n{}",
        stderr(&outside)
    );
}

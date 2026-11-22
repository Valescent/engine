//! The checked-in golden file must pass unchanged.

use crate::support::{assert_success, run, stdout};

#[test]
fn golden_file_passes() {
    let output = run(&["golden", "check"]);
    assert_success(&output, "golden check");
    let text = stdout(&output);
    assert!(text.contains("entries 6"), "stdout:\n{text}");
    assert!(text.contains("ok"), "stdout:\n{text}");
}

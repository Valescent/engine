//! `map` tests.

use crate::support::{assert_success, run, stdout};

#[test]
fn map_prints_one_glyph_per_cell() {
    let output = run(&[
        "map", "--seed", "7", "--from", "0", "0", "--size", "8", "--cell", "8",
    ]);
    assert_success(&output, "map");
    let text = stdout(&output);
    let mut lines = text.lines();
    assert!(
        lines
            .next()
            .is_some_and(|header| header.starts_with("map seed=7")),
        "the header names the seed:\n{text}"
    );
    let rows: Vec<&str> = lines.collect();
    assert_eq!(rows.len(), 8, "one row per requested cell");
    for row in rows {
        assert_eq!(row.chars().count(), 8, "one glyph per column in {row:?}");
        assert!(
            row.chars().all(|glyph| ".,-+*#%@~".contains(glyph)),
            "unexpected glyph in {row:?}"
        );
    }
}

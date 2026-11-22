//! Read-side ordering, coalescing, and undo checks (`J2.1`, `J2.3`, `J3.2`).

use std::collections::BTreeMap;

use crate::error::Coord;

use super::frame::Edit;

/// One record per coordinate: the highest `(revision, batch, seq)` wins.
///
/// Output is sorted by coordinate (`x`, then `y`, then `z`) and is a pure
/// function of the input set, so two arrival orders produce identical bytes.
/// This is the read-side answer to `J2.1`; the log itself is never rewritten.
#[must_use]
pub(crate) fn last_write_wins(edits: &[Edit]) -> Vec<Edit> {
    let mut winners: BTreeMap<Coord, &Edit> = BTreeMap::new();
    for edit in edits {
        let key = edit.position();
        let candidate = edit.order_key();
        let wins = winners
            .get(&key)
            .is_none_or(|current| candidate > current.order_key());
        if wins {
            winners.insert(key, edit);
        }
    }
    winners.into_values().cloned().collect()
}

/// Net diff per coordinate: the first `old`, the last `new` (`J3.2`).
///
/// Records are visited in total order, so a coordinate that was changed and
/// then changed back disappears: its net diff is the baseline and `J1.4` says
/// nothing is recorded when `old == new`. The surviving record carries the last
/// write's revision, author, batch, and seq, because that is the write the net
/// value came from. Output is sorted by coordinate.
#[must_use]
pub(crate) fn coalesce(edits: &[Edit]) -> Vec<Edit> {
    let mut ordered: Vec<&Edit> = edits.iter().collect();
    ordered.sort();

    let mut net: BTreeMap<Coord, Edit> = BTreeMap::new();
    for edit in ordered {
        match net.get_mut(&edit.position()) {
            Some(existing) => {
                existing.new.clone_from(&edit.new);
                existing.revision = edit.revision;
                existing.author = edit.author;
                existing.batch = edit.batch;
                existing.seq = edit.seq;
            }
            None => {
                net.insert(edit.position(), edit.clone());
            }
        }
    }
    net.into_values()
        .filter(|edit| edit.old != edit.new)
        .collect()
}

/// Whether an edit can be undone against the block that is there now (`J2.3`).
///
/// Undo is allowed only when the block still holds the record's `new` value;
/// anything else is a conflict and is reported, never forced. `current` is
/// `None` when the caller has no value for the coordinate at all, which never
/// counts as a match: the caller maps "no block" to the air name explicitly.
#[must_use]
pub(crate) fn can_undo(edit: &Edit, current: Option<&str>) -> bool {
    current.is_some_and(|name| name == edit.new)
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use super::*;
    use crate::editlog::test_support::record;
    use crate::error::Coord;

    #[test]
    fn last_write_wins_uses_revision_batch_seq() {
        let edits = vec![
            record([1, 0, 1], ("core:stone", "core:dirt"), (4, 0, 0)),
            record([1, 0, 1], ("core:dirt", "core:grass"), (5, 0, 0)),
            record([1, 0, 1], ("core:grass", "core:sand"), (5, 1, 0)),
            record([0, 0, 0], ("core:stone", "core:air"), (1, 0, 0)),
            record([2, 0, 2], ("core:stone", "core:gravel"), (9, 0, 0)),
            record([2, 0, 2], ("core:gravel", "core:clay"), (9, 0, 3)),
            record([2, 0, 2], ("core:clay", "core:moss"), (8, 7, 7)),
        ];

        let winners = last_write_wins(&edits);
        assert_eq!(
            winners.iter().map(Edit::position).collect::<Vec<_>>(),
            vec![[0, 0, 0], [1, 0, 1], [2, 0, 2]]
        );
        assert_eq!(winners[0].new, "core:air");
        assert_eq!(winners[1].new, "core:sand");
        assert_eq!(winners[1].order_key(), (5, 1, 0));
        assert_eq!(winners[2].new, "core:clay");

        let mut reversed = edits;
        reversed.reverse();
        let shuffled_winners = last_write_wins(&reversed);
        assert_eq!(shuffled_winners.len(), 3);
        for (left, right) in shuffled_winners.iter().zip(&winners) {
            assert!(left.same_record(right));
        }
    }

    #[test]
    fn order_is_total_and_stable_under_shuffle() {
        let records = [
            record([0, 0, 0], ("core:a", "core:b"), (2, 0, 0)),
            record([1, 0, 0], ("core:a", "core:b"), (1, 5, 0)),
            record([2, 0, 0], ("core:a", "core:b"), (1, 0, 7)),
            record([3, 0, 0], ("core:a", "core:b"), (1, 0, 2)),
            record([4, 0, 0], ("core:a", "core:b"), (3, 0, 0)),
        ];
        let expected = [3, 2, 1, 0, 4];

        for permutation in [
            [0, 1, 2, 3, 4],
            [4, 3, 2, 1, 0],
            [2, 4, 0, 3, 1],
            [1, 3, 0, 4, 2],
        ] {
            let mut shuffled: Vec<Edit> = permutation
                .iter()
                .map(|&index| records[index].clone())
                .collect();
            shuffled.sort();
            let order: Vec<Coord> = shuffled.iter().map(Edit::position).collect();
            let want: Vec<Coord> = expected
                .iter()
                .map(|&index| records[index].position())
                .collect();
            assert_eq!(order, want);
            assert!(shuffled.windows(2).all(|pair| pair[0] <= pair[1]));
        }

        // The order is on the key, not the payload: equal keys compare equal
        // and a stable sort keeps them in log order.
        let first = record([0, 0, 0], ("core:a", "core:b"), (1, 1, 1));
        let second = record([9, 9, 9], ("core:c", "core:d"), (1, 1, 1));
        assert_eq!(first.cmp(&second), Ordering::Equal);
        assert_eq!(first, second);
        assert!(!first.same_record(&second));
        let mut pair = [second.clone(), first];
        pair.sort();
        assert_eq!(pair[0].position(), second.position());
    }

    #[test]
    fn undo_requires_the_new_value_to_still_be_there() {
        let placed = record([0, 0, 0], ("core:air", "core:stone"), (2, 0, 0));
        assert!(can_undo(&placed, Some("core:stone")));
        assert!(!can_undo(&placed, Some("core:dirt")));
        assert!(!can_undo(&placed, None));

        let removed = record([0, 0, 0], ("core:stone", "core:air"), (3, 0, 0));
        assert!(can_undo(&removed, Some("core:air")));
        assert!(!can_undo(&removed, None));
    }

    #[test]
    fn coalesce_keeps_first_old_and_last_new() {
        let edits = vec![
            record([1, 0, 1], ("core:stone", "core:dirt"), (2, 0, 0)),
            record([1, 0, 1], ("core:dirt", "core:grass"), (3, 0, 0)),
            record([1, 0, 1], ("core:grass", "core:sand"), (4, 0, 0)),
            record([0, 0, 0], ("core:stone", "core:air"), (5, 0, 0)),
            record([0, 0, 0], ("core:air", "core:stone"), (6, 0, 0)),
        ];

        let net = coalesce(&edits);
        assert_eq!(net.len(), 1, "change-and-revert has no net diff");
        assert_eq!(net[0].position(), [1, 0, 1]);
        assert_eq!(net[0].old, "core:stone");
        assert_eq!(net[0].new, "core:sand");
        assert_eq!(net[0].order_key(), (4, 0, 0));

        let mut reversed = edits;
        reversed.reverse();
        assert_eq!(
            coalesce(&reversed)
                .iter()
                .map(|edit| (edit.position(), edit.old.clone(), edit.new.clone()))
                .collect::<Vec<_>>(),
            vec![([1, 0, 1], "core:stone".to_owned(), "core:sand".to_owned())]
        );
    }
}

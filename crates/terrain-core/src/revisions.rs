//! W12 - section, border, and column revisions (`I3.2`) and request keys (`K3.1`).
//!
//! Revisions answer one question: is a derived result (light, mesh, LOD,
//! `HeightTile` edits) still current? They are **runtime state only**: every
//! counter starts at the saved world revision and nothing here is ever written
//! to disk (`I3.2`).
//!
//! Two stored counters and one derived value:
//!
//! - `section_rev` per section, 12 per column, bumped once per edit batch that
//!   touches the section and on `Uniform` <-> `Allocated` promotion.
//! - `border_rev` per horizontal face, bumped when a face-adjacent cell of the
//!   neighbour changes - not when the neighbour's interior changes.
//! - `column_rev`, derived as the maximum of the 12 section revisions and never
//!   stored.
//!
//! Every accessor is total. A section index of `12` or more is ignored by the
//! bump functions and reported as the world revision by the readers, so a bad
//! index in a job cannot panic.

use crate::error::Stage;

/// Sections in a column (`I1.3`).
pub(crate) const SECTION_COUNT: usize = 12;

/// Horizontal faces of a column.
pub(crate) const FACE_COUNT: usize = 4;

/// One horizontal face of a column.
///
/// Revisions are keyed by face only, never by an axis: which world direction a
/// face means is the caller's mapping.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) enum Face {
    /// One horizontal side.
    North,
    /// One horizontal side.
    South,
    /// One horizontal side.
    East,
    /// One horizontal side.
    West,
}

impl Face {
    /// Every face, in the order [`ColumnRevisions`] stores them.
    pub(crate) const ALL: [Self; FACE_COUNT] = [Self::North, Self::South, Self::East, Self::West];

    /// Index into the per-face arrays.
    const fn index(self) -> usize {
        match self {
            Self::North => 0,
            Self::South => 1,
            Self::East => 2,
            Self::West => 3,
        }
    }
}

/// Revision counters for one column.
///
/// Constructed from the saved world revision, so a result built at load time is
/// current until the first edit or promotion (`I3.2`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ColumnRevisions {
    sections: [u64; SECTION_COUNT],
    borders: [u64; FACE_COUNT],
    world: u64,
}

impl ColumnRevisions {
    /// Starts every counter at `world_revision`, the revision loaded from the
    /// save header.
    #[must_use]
    pub(crate) const fn new(world_revision: u64) -> Self {
        Self {
            sections: [world_revision; SECTION_COUNT],
            borders: [world_revision; FACE_COUNT],
            world: world_revision,
        }
    }

    /// The world revision these counters started from.
    #[must_use]
    pub(crate) const fn world_revision(&self) -> u64 {
        self.world
    }

    /// Bumps one section.
    ///
    /// An index outside `0..SECTION_COUNT` is ignored and nothing panics: a job
    /// that computes a bad index keeps running and simply dirties nothing.
    /// Saturating, so a revision never wraps and monotonicity holds.
    pub(crate) const fn bump_section(&mut self, section: usize) {
        if section < SECTION_COUNT {
            self.sections[section] = self.sections[section].saturating_add(1);
        }
    }

    /// Bumps every section selected by `mask`, once each.
    ///
    /// This is the batch entry point: one edit batch is exactly one bump per
    /// touched section, no matter how many blocks it changed (`I3.2`). Bits at
    /// or above [`SECTION_COUNT`] are ignored.
    pub(crate) const fn bump_sections(&mut self, mask: u16) {
        let mut section = 0;
        while section < SECTION_COUNT {
            if mask & (1 << section) != 0 {
                self.bump_section(section);
            }
            section += 1;
        }
    }

    /// Bumps one face because a face-adjacent cell of the neighbour changed.
    ///
    /// A neighbour's interior change is not this column's business (`I3.2`), so
    /// it must not call this. Saturating, for the same reason as
    /// [`Self::bump_section`].
    pub(crate) const fn bump_border(&mut self, face: Face) {
        let index = face.index();
        self.borders[index] = self.borders[index].saturating_add(1);
    }

    /// Revision of one section.
    ///
    /// An index outside `0..SECTION_COUNT` has no counters of its own, so it
    /// reports the world revision - the state it would have had at load time.
    #[must_use]
    pub(crate) const fn section_rev(&self, section: usize) -> u64 {
        if section < SECTION_COUNT {
            self.sections[section]
        } else {
            self.world
        }
    }

    /// Revision of one face.
    #[must_use]
    pub(crate) const fn border_rev(&self, face: Face) -> u64 {
        self.borders[face.index()]
    }

    /// The column's revision: the maximum of its 12 section revisions.
    ///
    /// Derived on every call and never stored, so the two can never disagree.
    #[must_use]
    pub(crate) const fn column_rev(&self) -> u64 {
        let mut highest = 0;
        let mut section = 0;
        while section < SECTION_COUNT {
            if self.sections[section] > highest {
                highest = self.sections[section];
            }
            section += 1;
        }
        highest
    }

    /// The derived-data key for one section at its current revision.
    ///
    /// An index outside `0..SECTION_COUNT` still yields a stable key: the
    /// world revision and a clamped slot, never a panic.
    #[must_use]
    pub(crate) fn section_key(&self, column: [i32; 2], section: usize) -> SectionKey {
        SectionKey {
            column,
            section: u8::try_from(section).unwrap_or(u8::MAX),
            rev: self.section_rev(section),
        }
    }

    /// Whether a result built at `rev` is out of date for `section`.
    ///
    /// Equal revisions are current, not dirty: the result already saw that
    /// edit. A bad index is never dirty relative to the world revision.
    #[must_use]
    pub(crate) const fn is_dirty_since(&self, section: usize, rev: u64) -> bool {
        self.section_rev(section) > rev
    }
}

/// Identity of one section plus the revision a derived result was built from.
///
/// Ordering is field order, so a `BTreeMap<SectionKey, _>` iterates by column
/// `x`, then `z`, then section, then revision - the same order for every
/// insertion order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct SectionKey {
    /// The column, `[x, z]`.
    pub column: [i32; 2],
    /// Section slot in the column, `0..12`.
    pub section: u8,
    /// `section_rev` when the result was requested.
    pub rev: u64,
}

/// One queued request, keyed by coordinate, stage, and revision (`K3.1`).
///
/// A finished result is applied only when its key still
/// `accepts` the current one. Otherwise it is discarded as
/// stale and counted separately from a job dropped before it started (`K3.2`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub(crate) struct RequestKey {
    /// The request's coordinate, in its stage's units.
    pub coord: [i32; 3],
    /// The stage that produces the result.
    pub stage: Stage,
    /// Per-key `u64` bumped by an edit, an unload, a re-class, or a radius change.
    pub revision: u64,
}

impl RequestKey {
    /// Whether `other` names the same request: all three fields equal.
    #[must_use]
    pub(crate) fn accepts(&self, other: &Self) -> bool {
        self.coord == other.coord && self.stage == other.stage && self.revision == other.revision
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn section_bumps_are_monotone_and_per_section() {
        let mut revisions = ColumnRevisions::new(10);

        for expected in 11..=14 {
            revisions.bump_section(3);
            assert_eq!(revisions.section_rev(3), expected);
            assert_eq!(
                revisions.section_rev(2),
                10,
                "a neighbour section is untouched"
            );
            assert_eq!(revisions.section_rev(4), 10);
        }
        assert_eq!(revisions.column_rev(), revisions.section_rev(3));
    }

    #[test]
    fn a_batch_bumps_each_touched_section_once() {
        let mut revisions = ColumnRevisions::new(0);
        // Sections 0, 2, and 7.
        revisions.bump_sections(0b1000_0101);

        let moved: Vec<usize> = (0..SECTION_COUNT)
            .filter(|&section| revisions.section_rev(section) == 1)
            .collect();
        assert_eq!(moved, [0, 2, 7], "exactly three counters, one bump each");
        assert_eq!(revisions.column_rev(), 1);

        // Bits above the 12 real sections are ignored, not an index panic.
        revisions.bump_sections(u16::MAX);
        for section in 0..SECTION_COUNT {
            let expected = if [0, 2, 7].contains(&section) { 2 } else { 1 };
            assert_eq!(
                revisions.section_rev(section),
                expected,
                "section {section}"
            );
        }
        assert_eq!(revisions.column_rev(), 2);
    }

    #[test]
    fn column_rev_is_the_max_and_is_not_stored() {
        let mut revisions = ColumnRevisions::new(5);
        assert_eq!(revisions.column_rev(), 5);

        revisions.bump_section(9);
        assert_eq!(revisions.column_rev(), 6);
        revisions.bump_section(9);
        assert_eq!(revisions.column_rev(), 7);

        revisions.bump_section(0);
        assert_eq!(
            revisions.column_rev(),
            7,
            "a bump below the maximum does not lower it"
        );
        assert_eq!(revisions.section_rev(0), 6);

        revisions.bump_section(0);
        assert_eq!(revisions.column_rev(), 7, "a tie does not raise it");
        revisions.bump_section(0);
        assert_eq!(
            revisions.column_rev(),
            8,
            "column_rev follows the highest section"
        );
    }

    #[test]
    fn border_bumps_only_the_touched_face() {
        let mut revisions = ColumnRevisions::new(3);
        revisions.bump_border(Face::East);

        assert_eq!(revisions.border_rev(Face::East), 4);
        for face in Face::ALL {
            if face != Face::East {
                assert_eq!(revisions.border_rev(face), 3, "{face:?}");
            }
        }
        assert_eq!(revisions.column_rev(), 3, "borders are not sections");
        assert_eq!(
            revisions.section_rev(0),
            3,
            "a border bump leaves sections alone"
        );
    }

    #[test]
    fn revisions_start_at_the_world_revision() {
        let revisions = ColumnRevisions::new(41);

        assert_eq!(revisions.world_revision(), 41);
        for section in 0..SECTION_COUNT {
            assert_eq!(revisions.section_rev(section), 41, "section {section}");
        }
        for face in Face::ALL {
            assert_eq!(revisions.border_rev(face), 41, "{face:?}");
        }
        assert_eq!(revisions.column_rev(), 41);
        assert!(
            !revisions.is_dirty_since(0, 41),
            "equal revisions are current"
        );
        assert!(revisions.is_dirty_since(0, 40));
    }

    #[test]
    fn request_key_requires_all_three_fields() {
        let key = RequestKey {
            coord: [4, 5, 6],
            stage: Stage::Meshing,
            revision: 9,
        };
        let same = key;
        assert!(key.accepts(&same));

        let other_coord = RequestKey {
            coord: [4, 5, 7],
            ..key
        };
        let other_stage = RequestKey {
            stage: Stage::Lighting,
            ..key
        };
        let other_revision = RequestKey {
            revision: 10,
            ..key
        };

        assert!(!key.accepts(&other_coord), "a coordinate change breaks it");
        assert!(!key.accepts(&other_stage), "a stage change breaks it");
        assert!(!key.accepts(&other_revision), "a revision change breaks it");
        assert!(other_coord.accepts(&other_coord));
    }

    #[test]
    fn section_key_orders_deterministically() {
        let mut revisions = ColumnRevisions::new(0);
        revisions.bump_section(11);
        revisions.bump_section(0);

        let keys = [
            revisions.section_key([1, 0], 0),
            revisions.section_key([0, 1], 2),
            revisions.section_key([0, 0], 11),
            revisions.section_key([0, 0], 0),
            revisions.section_key([0, 0], 2),
        ];

        let mut forward = BTreeMap::new();
        for (value, key) in keys.iter().enumerate() {
            forward.insert(*key, value);
        }
        let mut reverse = BTreeMap::new();
        for (value, key) in keys.iter().enumerate().rev() {
            reverse.insert(*key, value);
        }

        let forward_pairs: Vec<_> = forward.iter().map(|(key, value)| (*key, *value)).collect();
        let reverse_pairs: Vec<_> = reverse.iter().map(|(key, value)| (*key, *value)).collect();
        assert_eq!(
            forward_pairs, reverse_pairs,
            "insertion order does not matter"
        );

        let actual: Vec<_> = forward
            .keys()
            .map(|key| (key.column, key.section, key.rev))
            .collect();
        assert_eq!(
            actual,
            [
                ([0, 0], 0, 1),
                ([0, 0], 2, 0),
                ([0, 0], 11, 1),
                ([0, 1], 2, 0),
                ([1, 0], 0, 1),
            ],
            "column x, then z, then section, then revision"
        );
    }

    #[test]
    fn out_of_range_section_is_ignored() {
        let mut revisions = ColumnRevisions::new(7);
        revisions.bump_section(12);
        revisions.bump_section(200);

        assert_eq!(revisions.column_rev(), 7);
        assert_eq!(
            revisions.section_rev(12),
            7,
            "a bad index reports the world revision"
        );
        assert!(!revisions.is_dirty_since(200, 7));
        assert_eq!(revisions.section_key([0, 0], 200).rev, 7);

        revisions.bump_sections(u16::MAX);
        assert_eq!(
            revisions.column_rev(),
            8,
            "only the twelve real sections moved"
        );
        assert_eq!(revisions.section_rev(200), 7);
    }
}

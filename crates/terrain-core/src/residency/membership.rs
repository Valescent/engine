//! Class order, ring membership, and the pinned radii (`K1.1`, `K1.4`, `K2.1`).

use crate::hash::mix;

/// `K1.1`: visible radius in columns, a square Chebyshev ring.
///
/// Runtime config in the end; pinned here as the `v1` default so the tests have
/// one number to hold.
pub const VIEW_RADIUS: i32 = 16;

/// `K1.4`: columns kept resident beyond the visible radius.
pub const UNLOAD_MARGIN: i32 = 2;

/// `K1.1`: meshing stops one ring inside the loaded set, so the outer generated
/// ring always has meshed neighbours.
pub const MESHING_MARGIN: i32 = 1;

/// `K2.2`: the coarse frustum test is widened by this many degrees so a small
/// head turn does not reshuffle the queue.
pub const FRUSTUM_SLACK_DEGREES: f32 = 20.0;

/// `K1.1`: the visible radius. Columns at ring `<= 16` are wanted (`K1.1`).
#[must_use]
pub const fn load_radius() -> i32 {
    VIEW_RADIUS
}

/// `K1.4`: columns beyond this ring are unloaded.
///
/// `R + UNLOAD_MARGIN = 18`, so at most `(2*18+1)^2 = 1,369` columns are
/// resident against `1,089` wanted.
#[must_use]
pub const fn unload_radius() -> i32 {
    VIEW_RADIUS + UNLOAD_MARGIN
}

/// `K1.1`: columns inside this ring are meshed; the outer ring is not.
#[must_use]
pub const fn meshing_radius() -> i32 {
    VIEW_RADIUS - MESHING_MARGIN
}

/// `K2.2`: [`FRUSTUM_SLACK_DEGREES`] in radians, the unit a frustum test wants.
#[must_use]
pub fn frustum_slack() -> f32 {
    FRUSTUM_SLACK_DEGREES * (std::f32::consts::PI / 180.0)
}

/// `K2.1`: the five work classes, in strict priority order.
///
/// Declaration order is the priority order and is relied on: `Ord` is derived,
/// so `Urgent < Visible < Ring < Prefetch < Background` and a sort by class puts
/// urgent work first. Class order always outranks every other key (`K2.1`).
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub enum Class {
    /// `0`: the player's column and 8 neighbours, pending edits, collision and
    /// lighting needs. Never dropped and never blocked by the column cap.
    Urgent,
    /// `1`: in the frustum (plus slack) inside `R - 1`.
    Visible,
    /// `2`: in the ring, outside the frustum.
    Ring,
    /// `3`: speculative fill, only above `PREFETCH_MIN_SPEED`.
    Prefetch,
    /// `4`: compaction, cache work, LOD rebuilds.
    Background,
}

impl Class {
    /// Every class, in priority order.
    pub const ALL: [Self; 5] = [
        Self::Urgent,
        Self::Visible,
        Self::Ring,
        Self::Prefetch,
        Self::Background,
    ];

    /// Stable lowercase name for logs, counters, and the overlay.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Urgent => "urgent",
            Self::Visible => "visible",
            Self::Ring => "ring",
            Self::Prefetch => "prefetch",
            Self::Background => "background",
        }
    }

    /// Index into the fixed per-class arrays, `0..5`, in priority order.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Urgent => 0,
            Self::Visible => 1,
            Self::Ring => 2,
            Self::Prefetch => 3,
            Self::Background => 4,
        }
    }
}

/// `K1.4`: where a column sits relative to the visible ring.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub enum Membership {
    /// Inside the visible radius: loaded, meshed, and prioritised.
    Visible,
    /// Inside the unload margin: resident and unmeshed, but not prioritised.
    Resident,
    /// Outside `visible + UNLOAD_MARGIN`: unloaded, farthest-first.
    Absent,
}

/// `K1.1`: whether `column` is inside a Chebyshev ring of `radius` around
/// `centre`.
///
/// Both arguments are column coordinates, which are already `floor_div` results
/// (`A4.1`); this function never divides, so a negative column cannot land in
/// the wrong cell. A negative radius encloses nothing.
#[must_use]
pub fn in_ring(column: [i32; 2], centre: [i32; 2], radius: i32) -> bool {
    let dx = i64::from(column[0]) - i64::from(centre[0]);
    let dz = i64::from(column[1]) - i64::from(centre[1]);
    dx.abs().max(dz.abs()) <= i64::from(radius)
}

/// `K1.4`: classifies a column with the load/unload hysteresis.
///
/// `visible_radius` is the current effective radius (`K4.3` may have cut it).
/// Rings `0..=visible_radius` are [`Membership::Visible`], the next
/// [`UNLOAD_MARGIN`] rings are [`Membership::Resident`], everything beyond is
/// [`Membership::Absent`]. A negative radius makes every column absent.
#[must_use]
pub fn membership(column: [i32; 2], centre: [i32; 2], visible_radius: i32) -> Membership {
    if visible_radius < 0 {
        return Membership::Absent;
    }
    let dx = i64::from(column[0]) - i64::from(centre[0]);
    let dz = i64::from(column[1]) - i64::from(centre[1]);
    let ring = dx.abs().max(dz.abs());
    let visible = i64::from(visible_radius);
    if ring <= visible {
        Membership::Visible
    } else if ring <= visible + i64::from(UNLOAD_MARGIN) {
        Membership::Resident
    } else {
        Membership::Absent
    }
}

/// Domain word for [`column_index`]. Pinned: changing it changes the queue order
/// of ties, never the terrain.
const COLUMN_DOMAIN: u64 = 0x636f_6c75_6d6e_2d76;

/// `K2.2`: a stable mixing of the two column coordinates.
///
/// Used only to break a ring-and-distance tie, so it must be identical on every
/// run and every platform but need not be a cryptographic hash. The in-tree
/// `SplitMix64` finaliser is integer-only (`A3.1`), so no hash crate is involved.
#[must_use]
pub fn column_index(column: [i32; 2]) -> u64 {
    let x = u64::from(column[0].cast_unsigned());
    let z = u64::from(column[1].cast_unsigned());
    mix(mix(COLUMN_DOMAIN ^ x) ^ z)
}

/// The demo's column-ring cap: the spec's [`VIEW_RADIUS`].
///
/// The demo chooses its radius from `config.toml` with [`demo_view_radius`], so
/// this is an upper bound rather than the radius actually streamed.
/// `[world] render_distance_chunks = 16` gives the spec ring: `(2 * 16 + 1)^2 =
/// 1,089` columns wanted, with the two-column unload margin bounding the
/// resident set at 1,369. The default `render_distance_chunks = 12` gives a
/// 625-column ring (`(2 * 12 + 1)^2`).
pub const DEMO_VIEW_RADIUS: i32 = 16;

/// Smallest demo radius the render distance may select.
///
/// It matches the spec's degradation floor (`MIN_VIEW_RADIUS`), keeping a
/// coherent ring even when `render_distance_chunks` is set below it.
pub const MIN_DEMO_VIEW_RADIUS: i32 = 4;

/// Derives the demo's column-ring radius from `render_distance_chunks`.
///
/// The ring follows the camera's visible extent: the camera's fog end is
/// `render_distance_chunks * 32` blocks, so a ring of that many columns ends
/// exactly at the fog and no resident column is spent beyond it. The spec's
/// [`DEMO_VIEW_RADIUS`] remains the cap, and [`MIN_DEMO_VIEW_RADIUS`] keeps a
/// small setting coherent. With the default `12` this is a 625-column ring;
/// `[world] render_distance_chunks = 16` selects the spec's 1,089-column ring.
#[must_use]
pub const fn demo_view_radius(render_distance_chunks: i32) -> i32 {
    if render_distance_chunks < MIN_DEMO_VIEW_RADIUS {
        MIN_DEMO_VIEW_RADIUS
    } else if render_distance_chunks > DEMO_VIEW_RADIUS {
        DEMO_VIEW_RADIUS
    } else {
        render_distance_chunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_hysteresis() {
        assert_eq!(load_radius(), 16);
        assert_eq!(unload_radius(), 18);
        assert_eq!(meshing_radius(), 15);
        assert_eq!(unload_radius() - load_radius(), UNLOAD_MARGIN);
        assert_eq!(load_radius() - meshing_radius(), MESHING_MARGIN);

        // The same shape at a small radius: 8 loads, 9-10 stay resident, 11 is
        // gone. This is what the pinned 16/18 pair does one scale up.
        let centre = [0, 0];
        let visible = 8;
        assert_eq!(
            membership([visible, 0], centre, visible),
            Membership::Visible
        );
        assert_eq!(
            membership([0, visible], centre, visible),
            Membership::Visible,
            "the ring is square, not circular"
        );
        assert_eq!(
            membership([9, 0], centre, visible),
            Membership::Resident,
            "9 is loaded but not visible"
        );
        assert_eq!(membership([-10, 10], centre, visible), Membership::Resident);
        assert_eq!(
            membership([11, 0], centre, visible),
            Membership::Absent,
            "11 is unloaded"
        );
        assert_eq!(membership([-11, 11], centre, visible), Membership::Absent);

        // And at the pinned radius.
        assert_eq!(
            membership([16, -16], centre, load_radius()),
            Membership::Visible
        );
        assert_eq!(
            membership([17, 0], centre, load_radius()),
            Membership::Resident
        );
        assert_eq!(
            membership([18, 0], centre, load_radius()),
            Membership::Resident
        );
        assert_eq!(
            membership([19, 0], centre, load_radius()),
            Membership::Absent
        );

        // `in_ring` agrees with the visible half of the same rule.
        assert!(in_ring([8, -8], centre, 8));
        assert!(!in_ring([9, 0], centre, 8));
        assert!(!in_ring([0, 0], centre, -1), "a negative ring is empty");

        // Off-centre columns measure from their own centre.
        assert!(in_ring([108, -8], [100, 0], 8));
        assert!(!in_ring([109, -8], [100, 0], 8));
    }

    #[test]
    fn column_index_is_stable_and_distinct() {
        let mut seen = std::collections::BTreeSet::new();
        for x in -32..32 {
            for z in -32..32 {
                let index = column_index([x, z]);
                assert_eq!(index, column_index([x, z]), "stable at ({x}, {z})");
                assert!(seen.insert(index), "collision at ({x}, {z})");
            }
        }
        assert_eq!(seen.len(), 64 * 64, "no collisions over a 64x64 sweep");

        // The obvious pairs must not collapse: the axes are folded in order.
        assert_ne!(column_index([1, 0]), column_index([0, 1]));
        assert_ne!(column_index([-1, 0]), column_index([0, -1]));
        assert_ne!(column_index([0, 0]), column_index([0, 1]));
    }

    #[test]
    fn demo_radius_caps_at_the_spec_view_radius() {
        assert_eq!(DEMO_VIEW_RADIUS, VIEW_RADIUS);
        let side = 2 * DEMO_VIEW_RADIUS + 1;
        assert_eq!(side, 33);
        assert_eq!(side * side, 1_089, "33 x 33 columns at the cap");

        // The default render distance selects a smaller ring; the same cap and
        // margin arithmetic holds one scale down.
        assert_eq!(demo_view_radius(12), 12);
        assert_eq!((2 * demo_view_radius(12) + 1).pow(2), 625);
        // The two-column unload margin is what bounds the resident set.
        assert_eq!(unload_radius(), 18);
        assert_eq!((2 * unload_radius() + 1) * (2 * unload_radius() + 1), 1_369);
    }

    #[test]
    fn demo_view_radius_clamps_low_and_high() {
        assert_eq!(demo_view_radius(i32::MIN), MIN_DEMO_VIEW_RADIUS);
        assert_eq!(demo_view_radius(0), MIN_DEMO_VIEW_RADIUS);
        assert_eq!(demo_view_radius(3), MIN_DEMO_VIEW_RADIUS);
        assert_eq!(demo_view_radius(4), 4);
        assert_eq!(demo_view_radius(8), 8);
        assert_eq!(demo_view_radius(12), 12);
        assert_eq!(demo_view_radius(16), DEMO_VIEW_RADIUS);
        assert_eq!(demo_view_radius(24), DEMO_VIEW_RADIUS);
        assert_eq!(demo_view_radius(i32::MAX), DEMO_VIEW_RADIUS);
    }
}

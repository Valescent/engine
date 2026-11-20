//! Residency: class caps, ring order, cancellation keys, budgets (`K1`-`K4`).
//!
//! Every decision in this module is a pure function of its arguments: no
//! threads, no world, no I/O, no clock. The caller owns the queues, the
//! counters, and the decision to report anything. That is what makes the
//! scheduling rules testable without a running game:
//!
//! - [`Class`] and [`compare`] - the `K2.1`/`K2.2` order: class, Chebyshev
//!   ring, Euclidean distance, column index. No floats anywhere in the path.
//! - [`ClassCaps`] and [`admit`](ClassCaps::admit) - the `K2.4` per-class caps
//!   and the drop-the-farthest rule. Urgent overflow is returned, never hidden.
//! - [`Applied`] - the `K3.1`/`K3.2` apply-time decision and its three counters.
//! - [`in_ring`], [`membership`], [`load_radius`], ... - the `K1.1`/`K1.4` ring
//!   and its hysteresis, plus a stable [`column_index`] for tie-breaking.
//! - [`Budget`] - the `K4.1` per-frame caps, steady and post-teleport (`K4.4`).
//!
//! The `K4.3` shed ladder is [`crate::metrics2::ShedState`]; this module does
//! not duplicate it. A caller asks the ladder whether a class may start and
//! asks [`Budget`] whether one more upload fits, which are different questions.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use crate::error::Stage;
use crate::hash::mix;
use crate::revisions::RequestKey;

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

/// `K2.4`: the check name an urgent-queue overflow is reported under.
///
/// The scheduler never panics on this. [`ClassCaps::admit`] returns
/// [`Admit::Overflow`] and the caller raises
/// [`ErrorKind::InvariantViolated`](crate::error::ErrorKind::InvariantViolated)
/// with this name.
pub const URGENT_OVERFLOW_CHECK: &str = "urgent_queue_overflow";

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

/// One column-level scheduling request (`K2.4`, `K3.1`).
///
/// The column is `[key.coord[0], key.coord[2]]`: residency is per column, so the
/// vertical coordinate of a section-level key is not part of the order (`K1.2`).
/// `centre` is the camera column the request was classified against; it is
/// stored rather than read from a global so [`compare`] stays a pure function of
/// its two arguments.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Request {
    /// Priority class.
    pub class: Class,
    /// Identity used at apply time: coordinate, stage, revision.
    pub key: RequestKey,
    /// Camera column this request's ring and distance were measured from.
    pub centre: [i32; 2],
}

impl Request {
    /// A request for `key`'s column, classified against `centre`.
    #[must_use]
    pub const fn new(class: Class, key: RequestKey, centre: [i32; 2]) -> Self {
        Self { class, key, centre }
    }

    /// The column this request is for, `[x, z]`.
    #[must_use]
    pub const fn column(&self) -> [i32; 2] {
        [self.key.coord[0], self.key.coord[2]]
    }

    /// Chebyshev ring around the centre, saturating at [`i32::MAX`].
    ///
    /// Computed in `i64` first, so `i32::MIN` and `i32::MAX` on opposite sides
    /// cannot overflow or wrap.
    #[must_use]
    pub fn ring(&self) -> i32 {
        let [dx, dz] = self.offset();
        i32::try_from(dx.abs().max(dz.abs())).unwrap_or(i32::MAX)
    }

    /// Squared Euclidean distance to the centre, in whole columns.
    ///
    /// Integer-only on purpose (`K2.2`): `sqrt` would introduce a float and a
    /// rounding rule into the order, and the sort never needs the root.
    #[must_use]
    pub fn distance_squared(&self) -> i64 {
        let [dx, dz] = self.offset();
        dx * dx + dz * dz
    }

    /// The offset from the centre, widened so it cannot overflow.
    fn offset(&self) -> [i64; 2] {
        let column = self.column();
        [
            i64::from(column[0]) - i64::from(self.centre[0]),
            i64::from(column[1]) - i64::from(self.centre[1]),
        ]
    }
}

/// `K2.1`/`K2.2`: orders work, nearest-and-most-important first.
///
/// The keys are applied in this order and no other:
///
/// 1. [`Class`], strict (`K2.1`);
/// 2. Chebyshev ring (`K2.2`);
/// 3. squared Euclidean distance (`K2.2`);
/// 4. [`column_index`] (`K2.2` "column index"), then the column itself so two
///    distinct columns always compare unequal.
///
/// [`Ordering::Less`] means `a` runs before `b`. The result is a total order:
/// same class, same column, and same centre compares [`Ordering::Equal`].
#[must_use]
pub fn compare(a: &Request, b: &Request) -> Ordering {
    a.class
        .cmp(&b.class)
        .then_with(|| a.ring().cmp(&b.ring()))
        .then_with(|| a.distance_squared().cmp(&b.distance_squared()))
        .then_with(|| column_index(a.column()).cmp(&column_index(b.column())))
        .then_with(|| a.column().cmp(&b.column()))
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

/// `K2.4`: the per-class queue depth caps, as answered.
///
/// Fields are public so the caller can make them runtime config; the values from
/// [`Self::answered`] are the `v1` defaults and the ones the tests pin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClassCaps {
    /// Cap for [`Class::Urgent`] requests.
    pub urgent: usize,
    /// Cap for [`Class::Visible`] requests.
    pub visible: usize,
    /// Cap for [`Class::Ring`] requests.
    pub ring: usize,
    /// Cap for [`Class::Prefetch`] requests.
    pub prefetch: usize,
    /// Cap for [`Class::Background`] requests.
    pub background: usize,
}

impl ClassCaps {
    /// The `K2.4` answer: `64 / 1,200 / 1,200 / 128 / 64`.
    #[must_use]
    pub const fn answered() -> Self {
        Self {
            urgent: 64,
            visible: 1_200,
            ring: 1_200,
            prefetch: 128,
            background: 64,
        }
    }

    /// The cap for one class.
    #[must_use]
    pub const fn cap(&self, class: Class) -> usize {
        match class {
            Class::Urgent => self.urgent,
            Class::Visible => self.visible,
            Class::Ring => self.ring,
            Class::Prefetch => self.prefetch,
            Class::Background => self.background,
        }
    }

    /// `K2.4`: admits one request into its class queue.
    ///
    /// - Below the cap: the request is pushed, [`Admit::Queued`].
    /// - At the cap, classes 1-4: the lower-priority of the arrival and the
    ///   queue's farthest is dropped. The queue stays at its cap and
    ///   [`Admit::DroppedLowestPriority`] says one request was lost, whether it
    ///   was the arrival or an incumbent. Classes 1/2 count that as
    ///   `dropped_before_start` (`K2.4`); the caller does the counting.
    /// - At the cap, class 0: the request is **still pushed** - class 0 is never
    ///   dropped or blocked - and [`Admit::Overflow`] is returned. That is the
    ///   `K2.4` "overflow is a bug": the caller reports
    ///   `InvariantViolated { check: URGENT_OVERFLOW_CHECK }` and carries on.
    ///
    /// A zero cap drops every arrival of that class without growing the queue.
    pub fn admit(&self, queue: &mut Vec<Request>, request: Request) -> Admit {
        let cap = self.cap(request.class);
        if request.class == Class::Urgent {
            queue.push(request);
            return if queue.len() > cap {
                Admit::Overflow
            } else {
                Admit::Queued
            };
        }
        if cap == 0 {
            return Admit::DroppedLowestPriority;
        }
        if queue.len() < cap {
            queue.push(request);
            return Admit::Queued;
        }

        // Full: compare the arrival against the current worst. The scan is
        // linear over a queue bounded at 1,200, and it never sorts or hashes.
        let Some((worst, _)) = queue
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| compare(a, b))
        else {
            // Unreachable while `cap > 0` implies a non-empty queue when full,
            // but never panic on a caller-set cap.
            queue.push(request);
            return Admit::DroppedLowestPriority;
        };
        if compare(&request, &queue[worst]) == Ordering::Less {
            queue[worst] = request;
        }
        Admit::DroppedLowestPriority
    }
}

impl Default for ClassCaps {
    fn default() -> Self {
        Self::answered()
    }
}

/// `K2.4`: what [`ClassCaps::admit`] did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Admit {
    /// Pushed below the cap.
    Queued,
    /// One lowest-priority request was dropped; the queue is at its cap.
    DroppedLowestPriority,
    /// Class 0 passed its cap. The request was queued anyway; the caller must
    /// report `InvariantViolated` (`K2.4`).
    Overflow,
}

/// `K3.1`/`K3.2`: apply-time outcomes, counted exactly once per decision.
///
/// `K3.2` keeps a job dropped while queued (`dropped_before_start`) separate from
/// a finished result that arrived too late (`discarded_stale`) and from a result
/// that was applied. All three are plain counts; saturation at `u64::MAX` is the
/// only clamp.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Applied {
    /// Results that matched their request and were used.
    pub applied: u64,
    /// Results whose request had moved on; the buffers are freed (`K3.4`).
    pub discarded_stale: u64,
    /// Requests dropped while still queued, before any work started.
    pub dropped_before_start: u64,
}

impl Applied {
    /// All three counters at zero.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            applied: 0,
            discarded_stale: 0,
            dropped_before_start: 0,
        }
    }

    /// `K3.1`: whether a finished result still matches its request.
    ///
    /// Returns `true` and counts `applied` when [`RequestKey::accepts`] holds for
    /// all three fields. Otherwise the result is stale: returns `false` and
    /// counts `discarded_stale`. Either way exactly one of those two counters
    /// moves.
    ///
    /// `dropped_before_start` is the third outcome and is counted by
    /// [`Self::drop_before_start`] on the admission path, because a job that was
    /// dropped never produces a result to pass here (`K3.2`).
    pub fn accept(&mut self, requested: &RequestKey, finished: &RequestKey) -> bool {
        if requested.accepts(finished) {
            self.applied = self.applied.saturating_add(1);
            true
        } else {
            self.discarded_stale = self.discarded_stale.saturating_add(1);
            false
        }
    }

    /// `K3.2`: counts one request dropped before it started.
    pub fn drop_before_start(&mut self) {
        self.dropped_before_start = self.dropped_before_start.saturating_add(1);
    }

    /// Every outcome counted so far, saturating at `u64::MAX`.
    ///
    /// Used to assert the accounting is total: one counter moves per decision.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.applied
            .saturating_add(self.discarded_stale)
            .saturating_add(self.dropped_before_start)
    }
}

/// `K4.1`: the per-frame work caps, in items and bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Budget {
    /// Columns whose work may start in one frame. Class 0 is exempt.
    pub max_columns_per_frame: u32,
    /// Mesh bytes that may be uploaded in one frame.
    pub max_upload_bytes_per_frame: u64,
    /// Safety stop for the apply phase, in milliseconds.
    pub apply_time_cap_ms: u32,
}

impl Budget {
    /// The steady-state `K4.1` answer: `2` columns, `4 MiB`, `3 ms`.
    #[must_use]
    pub const fn v1() -> Self {
        Self {
            max_columns_per_frame: 2,
            max_upload_bytes_per_frame: 4 * 1024 * 1024,
            apply_time_cap_ms: 3,
        }
    }

    /// Whether one more column of `class` may start this frame.
    ///
    /// Class 0 bypasses the column cap by definition (`K4.1`); the caller still
    /// has to pass the byte test below.
    #[must_use]
    pub const fn allows_columns(&self, columns: u32, class: Class) -> bool {
        matches!(class, Class::Urgent) || columns < self.max_columns_per_frame
    }

    /// Whether `bytes` more may be uploaded after `used` this frame.
    ///
    /// The byte cap is absolute: class 0 is exempt from the column cap and never
    /// from this one (`K4.1`). The class is taken so the call site states which
    /// rule it applies; the answer does not depend on it.
    #[must_use]
    pub fn allows_upload(&self, bytes: u64, used: u64, class: Class) -> bool {
        let _ = class;
        used.checked_add(bytes)
            .is_some_and(|total| total <= self.max_upload_bytes_per_frame)
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self::v1()
    }
}

/// `K4.4`: the post-teleport budget, `8` columns and `16 MiB`, until the 3x3 is
/// resident. The apply cap does not change.
#[must_use]
pub const fn teleport_budget() -> Budget {
    Budget {
        max_columns_per_frame: 8,
        max_upload_bytes_per_frame: 16 * 1024 * 1024,
        apply_time_cap_ms: 3,
    }
}

/// The demo's column-ring radius.
///
/// The spec default is [`VIEW_RADIUS`] (16, `1,369` resident columns), but the
/// demo keeps a smaller ring so raw section storage stays under the ~600 MB
/// working-set target: `(2 * 6 + 1)^2 = 169` columns, against 289 at radius 8
/// and 1,369 at radius 16. LOD1 carries the view beyond the fine ring.
pub const DEMO_VIEW_RADIUS: i32 = 6;

/// `K4.1`: the per-frame ring budget, in columns.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RingBudget {
    /// Missing columns whose generation may start this frame.
    pub generate: usize,
    /// Generated columns whose light/mesh work may start this frame.
    pub build: usize,
    /// Columns outside the unload margin that may be dropped this frame.
    pub evict: usize,
    /// Total jobs (generate + build) allowed outstanding at once.
    pub max_in_flight: usize,
}

impl RingBudget {
    /// `2` generate, `2` build, `2` evict, `16` in flight.
    #[must_use]
    pub const fn v1() -> Self {
        Self {
            generate: 2,
            build: 2,
            evict: 2,
            max_in_flight: 16,
        }
    }
}

impl Default for RingBudget {
    fn default() -> Self {
        Self::v1()
    }
}

/// One frame's bounded ring work.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RingPlan {
    /// Missing resident columns to generate, nearest first.
    pub generate: Vec<[i32; 2]>,
    /// Generated columns to light and mesh, nearest first.
    pub build: Vec<[i32; 2]>,
    /// Columns beyond the unload margin to drop, farthest first.
    pub evict: Vec<[i32; 2]>,
}

/// `K1.1`/`K4.1`: decides one frame of column-ring work.
///
/// Pure: no world, no I/O, no clock. `resident` is the set of columns whose
/// blocks are present, `needs_build` the subset waiting for light/mesh, and
/// `in_flight` every column with a generation or build job running. `protected`
/// columns are never evicted (an edit whose save is not yet queued, or the
/// player's own column).
///
/// Rules:
/// - generate: missing columns inside `radius`, nearest first, bounded by
///   `budget.generate` and the remaining in-flight capacity;
/// - build: resident columns in `needs_build` that are not beyond
///   `radius + UNLOAD_MARGIN`, nearest first, bounded by `budget.build` and the
///   remaining capacity;
/// - evict: resident columns outside `radius + UNLOAD_MARGIN`, farthest first,
///   bounded by `budget.evict`, never a protected or in-flight column.
///
/// A column appears in at most one list. Every returned column is distinct.
#[must_use]
pub fn plan_ring(
    centre: [i32; 2],
    radius: i32,
    resident: &BTreeSet<[i32; 2]>,
    in_flight: &BTreeSet<[i32; 2]>,
    needs_build: &BTreeSet<[i32; 2]>,
    protected: &BTreeSet<[i32; 2]>,
    budget: RingBudget,
) -> RingPlan {
    let mut capacity = budget.max_in_flight.saturating_sub(in_flight.len());

    let mut build: Vec<[i32; 2]> = needs_build
        .iter()
        .copied()
        .filter(|column| {
            resident.contains(column)
                && !in_flight.contains(column)
                && membership(*column, centre, radius) != Membership::Absent
        })
        .collect();
    build.sort_by(|a, b| compare(&ring_request(*a, centre), &ring_request(*b, centre)));
    let take = budget.build.min(capacity);
    build.truncate(take);
    capacity -= build.len();

    let mut generate = Vec::new();
    if radius >= 0 {
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let column = [centre[0] + dx, centre[1] + dz];
                if resident.contains(&column) || in_flight.contains(&column) {
                    continue;
                }
                generate.push(column);
            }
        }
    }
    generate.sort_by(|a, b| compare(&ring_request(*a, centre), &ring_request(*b, centre)));
    generate.truncate(budget.generate.min(capacity));

    let mut evict: Vec<[i32; 2]> = resident
        .iter()
        .copied()
        .filter(|column| {
            !protected.contains(column)
                && !in_flight.contains(column)
                && membership(*column, centre, radius) == Membership::Absent
        })
        .collect();
    evict.sort_by(|a, b| compare(&ring_request(*b, centre), &ring_request(*a, centre)));
    evict.truncate(budget.evict);
    // Defensive: a column scheduled for generation or build is never evicted.
    evict.retain(|column| !generate.contains(column) && !build.contains(column));

    RingPlan {
        generate,
        build,
        evict,
    }
}

/// One column-level request used only to reuse [`compare`]'s ring order.
fn ring_request(column: [i32; 2], centre: [i32; 2]) -> Request {
    Request::new(
        Class::Ring,
        RequestKey {
            coord: [column[0], 0, column[1]],
            stage: Stage::Streaming,
            revision: 0,
        },
        centre,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(column: [i32; 2], revision: u64) -> RequestKey {
        RequestKey {
            coord: [column[0], 0, column[1]],
            stage: Stage::Streaming,
            revision,
        }
    }

    fn request(class: Class, column: [i32; 2], centre: [i32; 2]) -> Request {
        Request::new(class, key(column, 0), centre)
    }

    #[test]
    fn class_order_is_strict() {
        for pair in Class::ALL.windows(2) {
            assert!(
                pair[0] < pair[1],
                "{:?} must precede {:?}",
                pair[0],
                pair[1]
            );
        }
        assert!(Class::Urgent < Class::Background);
        assert_eq!(Class::ALL.len(), 5);

        let names: Vec<&str> = Class::ALL.iter().map(|class| class.name()).collect();
        assert_eq!(
            names,
            ["urgent", "visible", "ring", "prefetch", "background"]
        );
        for (index, class) in Class::ALL.into_iter().enumerate() {
            assert_eq!(class.index(), index, "{class:?}");
        }

        // Class outranks every spatial key: a far urgent request beats a near
        // visible one.
        let centre = [0, 0];
        let far_urgent = request(Class::Urgent, [9, 9], centre);
        let near_visible = request(Class::Visible, [0, 1], centre);
        assert!(far_urgent.ring() > near_visible.ring());
        assert_eq!(compare(&far_urgent, &near_visible), Ordering::Less);
        assert_eq!(compare(&near_visible, &far_urgent), Ordering::Greater);
    }

    #[test]
    fn ring_then_distance_then_index() {
        let centre = [0, 0];

        let ring_one = request(Class::Visible, [1, 1], centre);
        let ring_two = request(Class::Visible, [2, 0], centre);
        assert_eq!(compare(&ring_one, &ring_two), Ordering::Less, "ring first");
        assert_eq!(
            compare(&ring_two, &ring_one),
            Ordering::Greater,
            "and it is antisymmetric"
        );

        // Same ring, different squared distance: [2, 0] is 4, [2, 1] is 5.
        let near = request(Class::Visible, [2, 0], centre);
        let far = request(Class::Visible, [2, 1], centre);
        assert_eq!(near.ring(), far.ring());
        assert_eq!(near.distance_squared(), 4);
        assert_eq!(far.distance_squared(), 5);
        assert_eq!(compare(&near, &far), Ordering::Less, "distance second");

        // Same ring and same distance: the mixed column index decides, and the
        // order is stable across calls.
        let a = request(Class::Visible, [1, 2], centre);
        let b = request(Class::Visible, [2, 1], centre);
        assert_eq!(a.ring(), b.ring());
        assert_eq!(a.distance_squared(), b.distance_squared());
        let expected = if column_index(a.column()) < column_index(b.column()) {
            Ordering::Less
        } else {
            Ordering::Greater
        };
        assert_ne!(expected, Ordering::Equal, "the mix keeps them distinct");
        assert_eq!(compare(&a, &b), expected, "index third");
        assert_eq!(compare(&a, &b), compare(&a, &b), "stable");

        // A column is equal to itself and only to itself.
        assert_eq!(compare(&a, &a), Ordering::Equal);
        assert_ne!(
            compare(&a, &request(Class::Visible, [1, 2], centre)),
            Ordering::Greater
        );
    }

    #[test]
    fn distance_uses_squared_integers() {
        let centre = [0, 0];
        let classic = request(Class::Visible, [3, 4], centre);
        assert_eq!(classic.ring(), 4);
        assert_eq!(classic.distance_squared(), 25);

        // 50,000^2 + 1 = 2,500,000,001. An `f32` has 24 bits of mantissa and
        // rounds this to 2,500,000,000, so an exact assertion proves the path is
        // integer-only.
        let near = request(Class::Visible, [50_000, 0], centre);
        let far = request(Class::Visible, [50_000, 1], centre);
        assert_eq!(near.distance_squared(), 2_500_000_000);
        assert_eq!(far.distance_squared(), 2_500_000_001);
        assert_eq!(
            compare(&near, &far),
            Ordering::Less,
            "a one-unit difference survives, which floats would lose"
        );

        // Negative coordinates are measured from the same centre.
        let negative = request(Class::Visible, [-3, -4], centre);
        assert_eq!(negative.distance_squared(), 25);
        assert_eq!(negative.ring(), 4);
    }

    #[test]
    fn caps_are_the_answered_values() {
        let caps = ClassCaps::answered();
        assert_eq!(caps.urgent, 64);
        assert_eq!(caps.visible, 1_200);
        assert_eq!(caps.ring, 1_200);
        assert_eq!(caps.prefetch, 128);
        assert_eq!(caps.background, 64);

        assert_eq!(caps.cap(Class::Urgent), 64);
        assert_eq!(caps.cap(Class::Visible), 1_200);
        assert_eq!(caps.cap(Class::Ring), 1_200);
        assert_eq!(caps.cap(Class::Prefetch), 128);
        assert_eq!(caps.cap(Class::Background), 64);
        assert_eq!(ClassCaps::default(), caps);
    }

    #[test]
    fn urgent_overflow_is_reported_not_dropped() {
        let caps = ClassCaps::answered();
        let centre = [0, 0];
        let mut queue = Vec::new();

        for i in 0..caps.urgent {
            let column = [i32::try_from(i).unwrap(), 0];
            assert_eq!(
                caps.admit(&mut queue, request(Class::Urgent, column, centre)),
                Admit::Queued,
                "urgent {i}"
            );
        }
        assert_eq!(queue.len(), caps.urgent);

        let overflow = request(Class::Urgent, [999, 999], centre);
        assert_eq!(
            caps.admit(&mut queue, overflow),
            Admit::Overflow,
            "the caller reports InvariantViolated"
        );
        assert_eq!(
            queue.len(),
            caps.urgent + 1,
            "class 0 is never dropped or blocked"
        );
        assert!(queue.contains(&overflow), "the arrival was still admitted");
        assert_eq!(URGENT_OVERFLOW_CHECK, "urgent_queue_overflow");

        // A non-urgent class at its cap does not grow: the farthest is dropped.
        let mut prefetch = Vec::new();
        for i in 0..caps.prefetch {
            let column = [i32::try_from(i).unwrap(), 0];
            assert_eq!(
                caps.admit(&mut prefetch, request(Class::Prefetch, column, centre)),
                Admit::Queued
            );
        }
        assert_eq!(prefetch.len(), caps.prefetch);

        let nearer = request(Class::Prefetch, [0, 1], centre);
        assert_eq!(
            caps.admit(&mut prefetch, nearer),
            Admit::DroppedLowestPriority
        );
        assert_eq!(prefetch.len(), caps.prefetch, "cap holds");
        assert!(
            prefetch.contains(&nearer),
            "the nearer arrival displaced one"
        );

        // The arrival that is itself the worst is dropped and the queue stands.
        let farthest = request(Class::Prefetch, [10_000, 10_000], centre);
        assert_eq!(
            caps.admit(&mut prefetch, farthest),
            Admit::DroppedLowestPriority
        );
        assert_eq!(prefetch.len(), caps.prefetch);
        assert!(!prefetch.contains(&farthest));

        // A zero cap drops everything without panicking.
        let zero = ClassCaps {
            prefetch: 0,
            ..ClassCaps::answered()
        };
        let mut none = Vec::new();
        assert_eq!(
            zero.admit(&mut none, request(Class::Prefetch, [0, 0], centre)),
            Admit::DroppedLowestPriority
        );
        assert_eq!(none.len(), 0);
    }

    #[test]
    fn accept_counts_exactly_one_outcome() {
        let mut applied = Applied::new();
        let requested = key([4, 7], 9);

        let before = applied;
        assert!(applied.accept(&requested, &requested));
        assert_eq!(
            applied,
            Applied {
                applied: 1,
                ..before
            }
        );
        assert_eq!(applied.total(), 1);

        for stale in [
            RequestKey {
                coord: [4, 0, 8],
                ..requested
            },
            RequestKey {
                stage: Stage::Meshing,
                ..requested
            },
            RequestKey {
                revision: 8,
                ..requested
            },
        ] {
            let before = applied;
            assert!(!applied.accept(&requested, &stale), "{stale:?}");
            let after = applied;
            assert_eq!(after.discarded_stale, before.discarded_stale + 1);
            assert_eq!(after.applied, before.applied, "nothing else moved");
            assert_eq!(
                after.dropped_before_start, before.dropped_before_start,
                "nothing else moved"
            );
            assert_eq!(after.total(), before.total() + 1);
        }

        applied.drop_before_start();
        assert_eq!(applied.dropped_before_start, 1);
        assert_eq!(applied.applied, 1);
        assert_eq!(applied.discarded_stale, 3);
        assert_eq!(
            applied.total(),
            5,
            "every decision landed in exactly one bucket"
        );

        // Saturating, never wrapping.
        let mut full = Applied {
            applied: u64::MAX,
            discarded_stale: u64::MAX,
            dropped_before_start: u64::MAX,
        };
        let _ = full.accept(&requested, &requested);
        full.drop_before_start();
        assert_eq!(full.applied, u64::MAX);
        assert_eq!(full.discarded_stale, u64::MAX);
        assert_eq!(full.dropped_before_start, u64::MAX);
        assert_eq!(full.total(), u64::MAX);
    }

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
    fn upload_cap_applies_to_urgent() {
        let budget = Budget::v1();
        let cap = budget.max_upload_bytes_per_frame;
        assert_eq!(cap, 4 * 1024 * 1024);

        assert!(budget.allows_upload(cap, 0, Class::Urgent));
        assert!(
            !budget.allows_upload(1, cap, Class::Urgent),
            "urgent bypasses the column cap, never the byte cap"
        );
        assert!(!budget.allows_upload(cap, 1, Class::Urgent));
        assert!(budget.allows_upload(1, cap - 1, Class::Urgent));
        assert!(!budget.allows_upload(u64::MAX, 1, Class::Urgent), "no wrap");

        for class in Class::ALL {
            assert!(!budget.allows_upload(1, cap, class), "{class:?}");
        }

        // The column cap is the one urgent bypasses.
        assert!(budget.allows_columns(2, Class::Urgent));
        assert!(budget.allows_columns(100, Class::Urgent));
        assert!(budget.allows_columns(1, Class::Visible));
        assert!(!budget.allows_columns(2, Class::Visible));
        assert_eq!(budget.apply_time_cap_ms, 3);
    }

    #[test]
    fn teleport_budget_is_larger_than_steady() {
        let steady = Budget::v1();
        let teleport = teleport_budget();

        assert!(teleport.max_columns_per_frame > steady.max_columns_per_frame);
        assert!(
            teleport.max_upload_bytes_per_frame > steady.max_upload_bytes_per_frame,
            "16 MiB is four times 4 MiB"
        );
        assert_eq!(teleport.max_columns_per_frame, 8);
        assert_eq!(teleport.max_upload_bytes_per_frame, 16 * 1024 * 1024);
        assert_eq!(
            teleport.apply_time_cap_ms, steady.apply_time_cap_ms,
            "the safety stop does not rise after a teleport"
        );
        assert_eq!(Budget::default(), steady);
        assert_eq!(teleport_budget(), teleport, "deterministic");

        // The frustum slack is the pinned 20 degrees, in radians.
        assert!((frustum_slack() - 20.0_f32.to_radians()).abs() < 1e-6);
        assert_eq!(FRUSTUM_SLACK_DEGREES, 20.0);
    }

    fn set(columns: &[[i32; 2]]) -> BTreeSet<[i32; 2]> {
        columns.iter().copied().collect()
    }

    fn ring_columns(centre: [i32; 2], radius: i32) -> Vec<[i32; 2]> {
        let mut out = Vec::new();
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                out.push([centre[0] + dx, centre[1] + dz]);
            }
        }
        out
    }

    #[test]
    fn ring_generates_nearest_first() {
        let centre = [0, 0];
        let plan = plan_ring(
            centre,
            2,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            RingBudget {
                generate: 5,
                ..RingBudget::v1()
            },
        );
        assert_eq!(plan.generate.len(), 5);
        // Ring order reuses `compare`: ring, then squared distance.
        assert_eq!(plan.generate[0], [0, 0]);
        let rings: Vec<i32> = plan
            .generate
            .iter()
            .map(|column| ring_request(*column, centre).ring())
            .collect();
        assert!(rings.windows(2).all(|pair| pair[0] <= pair[1]), "{rings:?}");
        let unique: BTreeSet<_> = plan.generate.iter().copied().collect();
        assert_eq!(unique.len(), plan.generate.len(), "no duplicates");
    }

    #[test]
    fn ring_work_is_bounded_and_respects_in_flight() {
        let centre = [0, 0];
        let resident = set(&ring_columns(centre, 3));
        let needs: BTreeSet<[i32; 2]> = resident.iter().copied().collect();
        let in_flight = set(&[[0, 0], [1, 0], [0, 1]]);
        let budget = RingBudget {
            generate: 4,
            build: 4,
            evict: 4,
            max_in_flight: 6,
        };
        let plan = plan_ring(
            centre,
            6,
            &resident,
            &in_flight,
            &needs,
            &BTreeSet::new(),
            budget,
        );

        assert!(plan.generate.len() <= budget.generate);
        assert!(plan.build.len() <= budget.build);
        assert!(
            plan.generate.len() + plan.build.len() <= 3,
            "capacity is 6 - 3"
        );

        for column in &plan.build {
            assert!(resident.contains(column));
            assert!(!in_flight.contains(column));
        }
        for column in &plan.generate {
            assert!(!resident.contains(column));
            assert!(!in_flight.contains(column));
            assert!(in_ring(*column, centre, 6));
        }

        // Zero capacity means no new jobs at all.
        let saturated = plan_ring(
            centre,
            6,
            &resident,
            &set(&ring_columns(centre, 3)),
            &needs,
            &BTreeSet::new(),
            budget,
        );
        assert_eq!(saturated.generate, Vec::<[i32; 2]>::new());
        assert_eq!(saturated.build, Vec::<[i32; 2]>::new());
        assert!(
            saturated.evict.is_empty(),
            "in-flight columns are never evicted"
        );
    }

    #[test]
    fn ring_evicts_farthest_first_and_protects_edits() {
        let centre = [0, 0];
        let near = set(&ring_columns(centre, 2));
        let mut resident = near.clone();
        resident.extend(set(&[[9, 0], [0, 9], [10, 0], [-9, 9]]));
        let protected = set(&[[10, 0]]);
        let plan = plan_ring(
            centre,
            2,
            &resident,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &protected,
            RingBudget {
                generate: 0,
                build: 0,
                evict: 2,
                max_in_flight: 8,
            },
        );

        // `radius + UNLOAD_MARGIN = 4`: 9, 9, 10 and (-9, 9) are all absent.
        assert_eq!(plan.evict.len(), 2, "budget bounded");
        assert!(
            !plan.evict.contains(&[10, 0]),
            "a protected edit is never evicted"
        );
        let rings: Vec<i32> = plan
            .evict
            .iter()
            .map(|column| ring_request(*column, centre).ring())
            .collect();
        assert!(
            rings.windows(2).all(|pair| pair[0] >= pair[1]),
            "farthest first: {rings:?}"
        );
        assert_eq!(plan.evict[0], [-9, 9], "ring 9 is the farthest");
        assert!(plan.generate.is_empty(), "the near ring is complete");
    }

    #[test]
    fn ring_lists_never_overlap_and_boundary_is_exact() {
        let centre = [3, -4];
        let resident = set(&ring_columns(centre, 4));
        let needs = set(&[[3, -4], [4, -4], [5, -4]]);
        let in_flight = set(&[[6, -4], [7, -4]]);
        let plan = plan_ring(
            centre,
            3,
            &resident,
            &in_flight,
            &needs,
            &BTreeSet::new(),
            RingBudget::v1(),
        );

        let mut all = BTreeSet::new();
        for column in plan.generate.iter().chain(&plan.build).chain(&plan.evict) {
            assert!(all.insert(*column), "{column:?} appears twice");
        }
        assert_eq!(plan.build, vec![[3, -4], [4, -4]], "nearest ready first");
        assert!(plan.evict.is_empty(), "radius 4 is still inside 3 + 2");

        let outside = plan_ring(
            centre,
            3,
            &set(&[[centre[0] + 6, centre[1]]]),
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeSet::new(),
            RingBudget::v1(),
        );
        assert_eq!(outside.evict, vec![[centre[0] + 6, centre[1]]]);
    }

    #[test]
    fn demo_radius_fits_the_memory_target() {
        assert_eq!(DEMO_VIEW_RADIUS, 6);
        let side = 2 * DEMO_VIEW_RADIUS + 1;
        assert_eq!(side, 13);
        assert_eq!(side * side, 169, "13 x 13 columns");
        // Radius 16 with the two-column unload margin is 1,369 columns; radius
        // 8 would be 289.
        assert_eq!((2 * unload_radius() + 1) * (2 * unload_radius() + 1), 1_369);
        assert_eq!((2 * 8 + 1) * (2 * 8 + 1), 289);
    }
}

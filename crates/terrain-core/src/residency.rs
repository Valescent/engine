//! Residency: class caps, ring order, cancellation keys, budgets (`K1`-`K4`).
//!
//! Every decision in this module is a pure function of its arguments: no
//! threads, no world, no I/O, no clock. The caller owns the queues, the
//! counters, and the decision to report anything. That is what makes the
//! scheduling rules testable without a running game:
//!
//! - `Class` and `compare` - the `K2.1`/`K2.2` order: class, Chebyshev
//!   ring, Euclidean distance, column index. No floats anywhere in the path.
//! - `ClassCaps` and `admit` - the `K2.4` per-class caps
//!   and the drop-the-farthest rule. Urgent overflow is returned, never hidden.
//! - `Applied` - the `K3.1`/`K3.2` apply-time decision and its three counters.
//! - [`in_ring`], `membership`, `load_radius`, ... - the `K1.1`/`K1.4` ring
//!   and its hysteresis, plus a stable `column_index` for tie-breaking.
//! - `Budget` - the `K4.1` per-frame caps, steady and post-teleport (`K4.4`).
//!
//! The `K4.3` shed ladder is `crate::metrics2::ShedState`; this module does
//! not duplicate it. A caller asks the ladder whether a class may start and
//! asks `Budget` whether one more upload fits, which are different questions.
//!
//! # Structure
//!
//! `membership` holds the class order, ring membership, and pinned radii;
//! `ring` the request ordering, queue caps, and `plan_ring`; `budget` the
//! per-frame work and ring budgets. Every item is re-exported here, so
//! `residency::<item>` paths are unchanged.

mod budget;
mod membership;
mod ring;

pub use budget::RingBudget;
#[allow(unused_imports)] // crate-private re-exports keep the flat module paths
pub(crate) use budget::{Budget, teleport_budget};
#[allow(unused_imports)] // crate-private re-exports keep the flat module paths
pub(crate) use membership::{
    Class, FRUSTUM_SLACK_DEGREES, MESHING_MARGIN, MIN_DEMO_VIEW_RADIUS, Membership, VIEW_RADIUS,
    column_index, frustum_slack, load_radius, membership, meshing_radius, unload_radius,
};
pub use membership::{DEMO_VIEW_RADIUS, UNLOAD_MARGIN, demo_view_radius, in_ring};
#[allow(unused_imports)] // crate-private re-exports keep the flat module paths
pub(crate) use ring::{Admit, Applied, ClassCaps, Request, URGENT_OVERFLOW_CHECK, compare};
pub use ring::{RingPlan, plan_ring};

//! Per-frame work and ring budgets (`K4.1`, `K4.4`).

use super::membership::Class;

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

#[cfg(test)]
mod tests {
    use super::super::membership::{FRUSTUM_SLACK_DEGREES, frustum_slack};
    use super::*;

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
}

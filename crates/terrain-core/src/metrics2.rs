//! W10 - pinned terrain counters, memory classes, and the overlay read model.
//!
//! Caller-owned state and a read model, no globals, no rendering:
//!
//! - [`METRICS`] and [`Counters`] - the pinned `Q1.1` metric list and its values.
//! - [`MemoryLedger`] - per-class byte accounting against the `S1.7` cap.
//! - [`ShedState`] - the `K4.3` load-shedding ladder.
//! - [`overlay_lines`] - the `Q1.5` text read model, one line per group.
//!
//! Every counter is a `u64`; the unit lives in [`Unit`], never in the value.
//! Latency names end in `_p50` or `_p95`, byte names end in `_bytes`, rate names
//! end in `_per_s`, and counts carry no suffix (`_count` is deliberately unused).
//!
//! `Counters` is an indexed `Vec` keyed by `METRICS`, so a lookup is a scan over
//! a table of a few dozen `&'static str`s. It grows no map and takes no lock:
//! the owner decides where it lives and how it is synchronised.
//!
//! Each slot is a [`Cell<u64>`], so a shared `&Counters` can record. That is what
//! lets [`crate::pipeline::Terrain`] expose `counters(&self) -> &Counters` while
//! its generation entry points also take `&self`. `Counters` stays `!Sync` and
//! single-owner; the cell is not a lock.

use std::cell::Cell;

use crate::error::{ErrorKind, Stage, TerrainError};

/// The unit a metric is measured in.
///
/// The name of a metric repeats the unit only where the unit is not the default
/// count (`_ms`, `_bytes`, `_per_s`); latency percentiles keep their `_p50` and
/// `_p95` suffixes instead of a `_ms` suffix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unit {
    /// Milliseconds.
    Milliseconds,
    /// A plain count.
    Count,
    /// Bytes.
    Bytes,
    /// A per-second rate.
    PerSecond,
}

/// One pinned metric: a stable name, its unit, and the stage that owns it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MetricSpec {
    /// Stable name, `lower_snake_case`.
    pub name: &'static str,
    /// Unit of the value.
    pub unit: Unit,
    /// Stage that owns the measurement.
    pub stage: Stage,
}

const fn metric(name: &'static str, unit: Unit, stage: Stage) -> MetricSpec {
    MetricSpec { name, unit, stage }
}

/// The pinned metric list from `Q1.1`.
///
/// Append only: a rename breaks dashboards, captures, and grep-based tooling, so
/// the names are treated as a wire format. The order is the `Counters` index
/// order and is not part of the overlay layout.
pub const METRICS: &[MetricSpec] = &[
    // Per-stage worker latency, one p50/p95 pair per stage that runs on a worker.
    metric("fields_p50", Unit::Milliseconds, Stage::Fields),
    metric("fields_p95", Unit::Milliseconds, Stage::Fields),
    metric("hydrology_p50", Unit::Milliseconds, Stage::Hydrology),
    metric("hydrology_p95", Unit::Milliseconds, Stage::Hydrology),
    metric("height_p50", Unit::Milliseconds, Stage::Height),
    metric("height_p95", Unit::Milliseconds, Stage::Height),
    metric("volume_p50", Unit::Milliseconds, Stage::Volume),
    metric("volume_p95", Unit::Milliseconds, Stage::Volume),
    metric("features_p50", Unit::Milliseconds, Stage::Features),
    metric("features_p95", Unit::Milliseconds, Stage::Features),
    metric("lighting_p50", Unit::Milliseconds, Stage::Lighting),
    metric("lighting_p95", Unit::Milliseconds, Stage::Lighting),
    metric("meshing_p50", Unit::Milliseconds, Stage::Meshing),
    metric("meshing_p95", Unit::Milliseconds, Stage::Meshing),
    // Edit latency is end to end (input to visible frame, `P1.7`) and is
    // measured at the apply point in the streaming stage.
    metric("edit_p50", Unit::Milliseconds, Stage::Streaming),
    metric("edit_p95", Unit::Milliseconds, Stage::Streaming),
    // Streaming and residency.
    metric("queue_depth", Unit::Count, Stage::Streaming),
    metric("in_flight_jobs", Unit::Count, Stage::Streaming),
    metric("resident_columns", Unit::Count, Stage::Streaming),
    metric("ready_columns", Unit::Count, Stage::Streaming),
    metric("queued_columns", Unit::Count, Stage::Streaming),
    metric("lod0_columns", Unit::Count, Stage::Streaming),
    metric("lod1_columns", Unit::Count, Stage::Streaming),
    metric("lod2_columns", Unit::Count, Stage::Streaming),
    metric("upload_bytes", Unit::Bytes, Stage::Streaming),
    metric("upload_bytes_per_s", Unit::PerSecond, Stage::Streaming),
    // Meshing.
    metric("meshes", Unit::Count, Stage::Meshing),
    metric("quads", Unit::Count, Stage::Meshing),
    // Lighting.
    metric("light_repairs", Unit::Count, Stage::Lighting),
    // Revisions and cancellation (`K3.1`, `K3.2`).
    metric("dropped_before_start", Unit::Count, Stage::Streaming),
    metric("discarded_stale", Unit::Count, Stage::Streaming),
    metric("applied", Unit::Count, Stage::Streaming),
    // Worker health (`S1.2`, `S1.3`); contained at the pool boundary, which is
    // owned by the streaming stage.
    metric("panics_caught", Unit::Count, Stage::Streaming),
    metric("stuck_threads", Unit::Count, Stage::Streaming),
];

/// Index of a pinned name, or `None` when the name is not registered.
fn index_of(name: &str) -> Option<usize> {
    METRICS.iter().position(|metric| metric.name == name)
}

/// A `Vec<Cell<u64>>` sampler indexed by [`METRICS`].
///
/// Unknown names are no-ops: `add` and `set` return `false` and `get` returns
/// `None`. Nothing panics on a typo, so a diagnostics bug can never take the
/// frame down. Additions saturate instead of wrapping.
///
/// Every write takes `&self`: a cell per slot is what allows a generator to
/// record through a shared reference. The owner is still single-threaded, so
/// `Counters` is `!Sync` and no write is atomic.
#[derive(Clone, Debug)]
pub struct Counters {
    values: Vec<Cell<u64>>,
}

impl Counters {
    /// Allocates one zeroed slot per pinned metric.
    #[must_use]
    pub fn new() -> Self {
        Self {
            values: vec![Cell::new(0); METRICS.len()],
        }
    }

    /// Adds `delta` to `name`, saturating at `u64::MAX`.
    ///
    /// Returns `false` when `name` is not pinned.
    pub fn add(&self, name: &str, delta: u64) -> bool {
        let Some(index) = index_of(name) else {
            return false;
        };
        match self.values.get(index) {
            Some(value) => {
                value.set(value.get().saturating_add(delta));
                true
            }
            None => false,
        }
    }

    /// Overwrites `name` with `value`.
    ///
    /// Returns `false` when `name` is not pinned.
    pub fn set(&self, name: &str, value: u64) -> bool {
        let Some(index) = index_of(name) else {
            return false;
        };
        match self.values.get(index) {
            Some(slot) => {
                slot.set(value);
                true
            }
            None => false,
        }
    }

    /// Reads `name`, or `None` when `name` is not pinned.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<u64> {
        index_of(name).and_then(|index| self.values.get(index).map(Cell::get))
    }

    /// Records one latency sample for `stage` in its pinned `_p50` slot.
    ///
    /// `micros` is converted to whole milliseconds, the pinned unit: a
    /// sub-millisecond stage reads `0` rather than a rounded-up number the table
    /// cannot express. The slot is a gauge, so the sample replaces the previous
    /// one and the HUD shows the most recently completed stage. The paired `_p95`
    /// slot is left alone; the pinned table has one bucket per percentile and a
    /// single sample is not a distribution.
    ///
    /// Returns `false` and records nothing when `stage` owns no latency pair (the
    /// remaining [`Stage`] variants). No stage is guessed at and nothing panics.
    pub fn record_latency(&self, stage: Stage, micros: u64) -> bool {
        let Some(name) = latency_name(stage) else {
            return false;
        };
        self.set(name, micros / 1_000)
    }

    /// Sets the three residency gauges a tile cache can report.
    ///
    /// These are gauges, not deltas: the owner recomputes them from its cache.
    pub fn set_gauges(&self, resident: u64, ready: u64, queued: u64) {
        self.set("resident_columns", resident);
        self.set("ready_columns", ready);
        self.set("queued_columns", queued);
    }
}

/// The pinned `_p50` name for a stage that runs on a worker.
///
/// Every [`Stage`] is listed, so a new variant forces a decision instead of
/// silently defaulting. `None` is the lexicographic tail that owns no latency
/// pair: description, graph, surface, strata, streaming, persistence, migration,
/// and diagnostics.
const fn latency_name(stage: Stage) -> Option<&'static str> {
    match stage {
        Stage::Fields => Some("fields_p50"),
        Stage::Hydrology => Some("hydrology_p50"),
        Stage::Height => Some("height_p50"),
        Stage::Volume => Some("volume_p50"),
        Stage::Features => Some("features_p50"),
        Stage::Lighting => Some("lighting_p50"),
        Stage::Meshing => Some("meshing_p50"),
        Stage::Description
        | Stage::Graph
        | Stage::Surface
        | Stage::Strata
        | Stage::Streaming
        | Stage::Persistence
        | Stage::Migration
        | Stage::Diagnostics => None,
    }
}

/// Records one latency sample in `stage`'s pinned `_p50` slot.
///
/// Returns `false` when `stage` has no pinned latency pair; nothing panics.
pub fn record_latency(counters: &mut Counters, stage: Stage, micros: u64) -> bool {
    counters.record_latency(stage, micros)
}

/// Sets `resident_columns`, `ready_columns`, and `queued_columns` together.
///
/// Pure over the pinned table; it adds no metric.
pub fn set_gauges(counters: &mut Counters, resident: u64, ready: u64, queued: u64) {
    counters.set_gauges(resident, ready, queued);
}

impl Default for Counters {
    fn default() -> Self {
        Self::new()
    }
}

/// A memory class from `S1.7`. The order is the class order used to break ties.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryClass {
    /// Saved and loaded section payloads.
    Sections,
    /// Derived tiles kept between stages.
    DerivedTiles,
    /// Mesh buffers, CPU and GPU staging.
    Meshes,
    /// Light nibble arrays and scratch for relight.
    Light,
    /// Feature candidate indexes.
    FeatureIndex,
    /// Per-job scratch and padding buffers.
    Scratch,
}

impl MemoryClass {
    /// Every class, in class order.
    pub const ALL: [Self; 6] = [
        Self::Sections,
        Self::DerivedTiles,
        Self::Meshes,
        Self::Light,
        Self::FeatureIndex,
        Self::Scratch,
    ];

    /// Stable lowercase name used by the overlay and by `ErrorKind::ResourceCap`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sections => "sections",
            Self::DerivedTiles => "derived_tiles",
            Self::Meshes => "meshes",
            Self::Light => "light",
            Self::FeatureIndex => "feature_index",
            Self::Scratch => "scratch",
        }
    }

    /// Index into `MemoryLedger::used`.
    const fn index(self) -> usize {
        match self {
            Self::Sections => 0,
            Self::DerivedTiles => 1,
            Self::Meshes => 2,
            Self::Light => 3,
            Self::FeatureIndex => 4,
            Self::Scratch => 5,
        }
    }
}

/// Per-class byte accounting against the `S1.7` hard cap.
///
/// Reservations are made against the projected *whole-ledger* total, which is
/// what lets admission stop workers from overshooting the cap together. A
/// reservation that would exceed the cap fails with [`ErrorKind::ResourceCap`]
/// and changes nothing; the caller decides whether to evict, shed, or report.
#[derive(Debug)]
pub struct MemoryLedger {
    cap_bytes: u64,
    used: [u64; 6],
}

impl MemoryLedger {
    /// A ledger with no bytes held and a hard cap of `cap_bytes`.
    #[must_use]
    pub fn new(cap_bytes: u64) -> Self {
        Self {
            cap_bytes,
            used: [0; 6],
        }
    }

    /// The hard cap in bytes.
    #[must_use]
    pub const fn cap(&self) -> u64 {
        self.cap_bytes
    }

    /// Bytes held by one class.
    #[must_use]
    pub fn class_bytes(&self, class: MemoryClass) -> u64 {
        self.used[class.index()]
    }

    /// Total bytes held across every class, saturating at `u64::MAX`.
    #[must_use]
    pub fn used(&self) -> u64 {
        self.used
            .iter()
            .fold(0, |total, bytes| total.saturating_add(*bytes))
    }

    /// `used / cap`, clamped to `[0, 1]`. A zero cap reports zero pressure.
    #[allow(clippy::cast_precision_loss)]
    #[must_use]
    pub fn pressure(&self) -> f32 {
        if self.cap_bytes == 0 {
            return 0.0;
        }
        (self.used() as f32 / self.cap_bytes as f32).clamp(0.0, 1.0)
    }

    /// Reserves `bytes` in `class`.
    ///
    /// The cap is a whole-ledger cap: the check is against `used() + bytes`, so
    /// two classes cannot each spend the whole budget.
    ///
    /// # Errors
    ///
    /// Returns a `ResourceCap` error (stage `Streaming`, the admission point)
    /// when the projected total would exceed the cap or overflow a `u64`. The
    /// ledger is left unchanged on failure.
    pub fn reserve(&mut self, class: MemoryClass, bytes: u64) -> Result<(), TerrainError> {
        let Some(projected) = self.used().checked_add(bytes) else {
            return Err(TerrainError::new(
                Stage::Streaming,
                ErrorKind::ResourceCap {
                    class: class.name(),
                    bytes,
                },
            ));
        };
        if projected > self.cap_bytes {
            return Err(TerrainError::new(
                Stage::Streaming,
                ErrorKind::ResourceCap {
                    class: class.name(),
                    bytes: projected,
                },
            ));
        }
        let slot = &mut self.used[class.index()];
        *slot = slot.saturating_add(bytes);
        Ok(())
    }

    /// Gives `bytes` back to `class`.
    ///
    /// Saturates at zero: a discard may race a release, and accounting must
    /// never go negative or panic (`K3.4`).
    pub fn release(&mut self, class: MemoryClass, bytes: u64) {
        let slot = &mut self.used[class.index()];
        *slot = slot.saturating_sub(bytes);
    }

    /// The classes with the most bytes held, largest first, ties by class order.
    ///
    /// Every class is returned, including empty ones, so callers can render a
    /// fixed table. `limit` truncates the list.
    #[must_use]
    pub fn largest_classes(&self, limit: usize) -> Vec<(MemoryClass, u64)> {
        let mut classes: Vec<(MemoryClass, u64)> = MemoryClass::ALL
            .into_iter()
            .map(|class| (class, self.class_bytes(class)))
            .collect();
        classes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.index().cmp(&b.0.index())));
        classes.truncate(limit);
        classes
    }
}

/// One rung of the `K4.3` load-shedding ladder, from least to most shed.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ShedStep {
    /// Nothing shed.
    Full,
    /// Background work (compaction, cache, LOD rebuild) stopped.
    NoBackground,
    /// No new `LOD2` columns.
    NoLod2,
    /// No new `LOD1` columns.
    NoLod1,
    /// No prefetch beyond the effective radius.
    NoPrefetch,
    /// Fog pulled in and the radius cut. This is the floor.
    RadiusCut,
}

impl ShedStep {
    /// The next rung down, or `None` at [`ShedStep::RadiusCut`].
    const fn next_down(self) -> Option<Self> {
        match self {
            Self::Full => Some(Self::NoBackground),
            Self::NoBackground => Some(Self::NoLod2),
            Self::NoLod2 => Some(Self::NoLod1),
            Self::NoLod1 => Some(Self::NoPrefetch),
            Self::NoPrefetch => Some(Self::RadiusCut),
            Self::RadiusCut => None,
        }
    }

    /// The next rung up, or `None` at [`ShedStep::Full`].
    const fn next_up(self) -> Option<Self> {
        match self {
            Self::Full => None,
            Self::NoBackground => Some(Self::Full),
            Self::NoLod2 => Some(Self::NoBackground),
            Self::NoLod1 => Some(Self::NoLod2),
            Self::NoPrefetch => Some(Self::NoLod1),
            Self::RadiusCut => Some(Self::NoPrefetch),
        }
    }
}

/// The largest cut the ladder may make to the view radius, in columns.
pub const RADIUS_CUT: i32 = 2;

/// The smallest view radius the ladder may cut to.
pub const MIN_VIEW_RADIUS: i32 = 8;

/// Lag-free windows required before one restore step, at `LAG_WINDOW` = 2 s and
/// `RESTORE_WINDOW` = 10 s.
pub const RESTORE_WINDOWS: u32 = 5;

/// The `K4.3` ladder state. Never shed past [`ShedStep::RadiusCut`], never
/// restored past [`ShedStep::Full`].
///
/// The effective radius is owned by the caller, not by `shed` and `restore`:
/// `K4.3` says the radius is the *minimum* of this ladder and the `S1.7` memory
/// ladder, and only the caller knows both. It applies [`RADIUS_CUT`] when this
/// ladder enters `RadiusCut` and gives it back when it leaves.
#[derive(Debug)]
pub struct ShedState {
    step: ShedStep,
    effective_radius: i32,
    restore_after: u32,
}

impl ShedState {
    /// A ladder at [`ShedStep::Full`] with an uncut radius.
    #[must_use]
    pub fn new(effective_radius: i32) -> Self {
        Self {
            step: ShedStep::Full,
            effective_radius: effective_radius.max(MIN_VIEW_RADIUS),
            restore_after: 0,
        }
    }

    /// The current rung.
    #[must_use]
    pub const fn step(&self) -> ShedStep {
        self.step
    }

    /// The current radius, as last set by the caller.
    #[must_use]
    pub const fn effective_radius(&self) -> i32 {
        self.effective_radius
    }

    /// Lag-free windows still required before the next restore may fire.
    #[must_use]
    pub const fn restore_after(&self) -> u32 {
        self.restore_after
    }

    /// Sets the caller-computed effective radius, floored at [`MIN_VIEW_RADIUS`].
    pub fn set_effective_radius(&mut self, radius: i32) {
        self.effective_radius = radius.max(MIN_VIEW_RADIUS);
    }

    /// Counts one lag-free window, decrementing the restore hold.
    pub fn count_restore_window(&mut self) {
        self.restore_after = self.restore_after.saturating_sub(1);
    }

    /// Whether a job of `class` may start at the current rung.
    ///
    /// Class 0 (player edits) and class 1 (urgent streaming) are never shed;
    /// every other class is background work and waits for [`ShedStep::Full`].
    #[must_use]
    pub const fn allows(&self, class: u8) -> bool {
        class < 2 || matches!(self.step, ShedStep::Full)
    }
}

/// Sheds one rung. Returns `false` when already at [`ShedStep::RadiusCut`].
pub fn shed(state: &mut ShedState) -> bool {
    let Some(next) = state.step.next_down() else {
        return false;
    };
    state.step = next;
    state.restore_after = RESTORE_WINDOWS;
    true
}

/// Restores one rung. Returns `false` when already at [`ShedStep::Full`].
pub fn restore(state: &mut ShedState) -> bool {
    let Some(next) = state.step.next_up() else {
        return false;
    };
    state.step = next;
    state.restore_after = if next == ShedStep::Full {
        0
    } else {
        RESTORE_WINDOWS
    };
    true
}

/// Reads a pinned counter for the overlay. Unknown names are a bug in this
/// module, so they are caught in debug builds and read as zero in release.
fn counter(counters: &Counters, name: &str) -> u64 {
    debug_assert!(index_of(name).is_some(), "unknown overlay metric {name}");
    counters.get(name).unwrap_or(0)
}

/// The `Q1.5` overlay read model: one line per group, in a fixed order.
///
/// Groups, in order: `revisions`, `queues`, `jobs`, `memory`, `lod`, `light`,
/// `edits`, `shed`. Per-stage latency percentiles are not part of the pinned
/// groups; the HUD reads them from [`Counters`] beside this block.
#[must_use]
pub fn overlay_lines(counters: &Counters, ledger: &MemoryLedger, shed: &ShedState) -> Vec<String> {
    let classes: Vec<String> = MemoryClass::ALL
        .into_iter()
        .map(|class| format!("{}={}", class.name(), ledger.class_bytes(class)))
        .collect();

    vec![
        format!(
            "revisions: dropped_before_start={} discarded_stale={} applied={}",
            counter(counters, "dropped_before_start"),
            counter(counters, "discarded_stale"),
            counter(counters, "applied"),
        ),
        format!(
            "queues: resident_columns={} ready_columns={} queued_columns={} queue_depth={}",
            counter(counters, "resident_columns"),
            counter(counters, "ready_columns"),
            counter(counters, "queued_columns"),
            counter(counters, "queue_depth"),
        ),
        format!(
            "jobs: in_flight_jobs={} panics_caught={} stuck_threads={}",
            counter(counters, "in_flight_jobs"),
            counter(counters, "panics_caught"),
            counter(counters, "stuck_threads"),
        ),
        format!(
            "memory: used={} cap={} pressure={:.3} {}",
            ledger.used(),
            ledger.cap(),
            ledger.pressure(),
            classes.join(" "),
        ),
        format!(
            "lod: lod0_columns={} lod1_columns={} lod2_columns={} meshes={} quads={}",
            counter(counters, "lod0_columns"),
            counter(counters, "lod1_columns"),
            counter(counters, "lod2_columns"),
            counter(counters, "meshes"),
            counter(counters, "quads"),
        ),
        format!(
            "light: light_repairs={}",
            counter(counters, "light_repairs")
        ),
        format!(
            "edits: edit_p50={} edit_p95={} upload_bytes={} upload_bytes_per_s={}",
            counter(counters, "edit_p50"),
            counter(counters, "edit_p95"),
            counter(counters, "upload_bytes"),
            counter(counters, "upload_bytes_per_s"),
        ),
        format!(
            "shed: step={:?} radius={} restore_after={}",
            shed.step(),
            shed.effective_radius(),
            shed.restore_after(),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_lower_snake_case(name: &str) -> bool {
        !name.is_empty()
            && !name.starts_with('_')
            && !name.ends_with('_')
            && !name.contains("__")
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    }

    #[test]
    fn metric_names_are_unique_and_suffixed() {
        let mut seen = std::collections::BTreeSet::new();
        for metric in METRICS {
            let name = metric.name;
            assert!(seen.insert(name), "duplicate metric name: {name}");
            assert!(is_lower_snake_case(name), "not lower_snake_case: {name}");
            match metric.unit {
                Unit::Milliseconds => assert!(
                    name.ends_with("_p50") || name.ends_with("_p95"),
                    "latency name must end in _p50 or _p95: {name}"
                ),
                Unit::Bytes => assert!(name.ends_with("_bytes"), "byte name: {name}"),
                Unit::PerSecond => assert!(name.ends_with("_per_s"), "rate name: {name}"),
                Unit::Count => assert!(!name.ends_with("_count"), "count suffix: {name}"),
            }
        }
        assert_eq!(Counters::new().values.len(), METRICS.len());
    }

    #[test]
    fn every_metric_maps_to_a_real_stage() {
        let worker_stages = [
            Stage::Fields,
            Stage::Hydrology,
            Stage::Height,
            Stage::Volume,
            Stage::Features,
            Stage::Lighting,
            Stage::Meshing,
        ];

        for stage in worker_stages {
            assert!(
                METRICS.iter().any(|metric| metric.stage == stage),
                "no metric for {stage:?}"
            );
            let stage_name = format!("{stage:?}").to_ascii_lowercase();
            for percentile in ["p50", "p95"] {
                let name = format!("{stage_name}_{percentile}");
                let found = METRICS.iter().any(|metric| {
                    metric.stage == stage
                        && metric.unit == Unit::Milliseconds
                        && metric.name == name
                });
                assert!(found, "missing {name} for {stage:?}");
            }
        }

        for (suffix, pair) in [("_p50", "_p95"), ("_p95", "_p50")] {
            for metric in METRICS {
                let Some(stem) = metric.name.strip_suffix(suffix) else {
                    continue;
                };
                let paired = format!("{stem}{pair}");
                let found = METRICS.iter().any(|other| {
                    other.name == paired && other.stage == metric.stage && other.unit == metric.unit
                });
                let name = metric.name;
                assert!(found, "{name} has no {paired}");
            }
        }
    }

    #[test]
    fn unknown_names_are_ignored() {
        let counters = Counters::new();

        assert!(!counters.add("not_a_metric", 1));
        assert!(!counters.set("not_a_metric", 1));
        assert_eq!(counters.get("not_a_metric"), None);
        assert!(!counters.add("applied_", 1));
        assert_eq!(counters.get("applied"), Some(0));

        assert!(counters.add("applied", 2));
        assert_eq!(counters.get("applied"), Some(2));
        assert!(counters.add("applied", 3));
        assert_eq!(counters.get("applied"), Some(5));
        assert!(counters.set("applied", 9));
        assert_eq!(counters.get("applied"), Some(9));
        assert!(counters.set("applied", 9));

        assert!(counters.add("quads", u64::MAX));
        assert!(counters.add("quads", 1));
        assert_eq!(counters.get("quads"), Some(u64::MAX));
    }

    #[test]
    fn ledger_respects_the_cap() {
        let mut ledger = MemoryLedger::new(1000);
        assert_eq!(ledger.used(), 0);
        assert!((ledger.pressure() - 0.0).abs() < f32::EPSILON);

        ledger.reserve(MemoryClass::Sections, 400).unwrap();
        ledger.reserve(MemoryClass::Meshes, 600).unwrap();
        assert_eq!(ledger.used(), 1000);
        assert!((ledger.pressure() - 1.0).abs() < f32::EPSILON);

        let error = ledger.reserve(MemoryClass::Scratch, 1).unwrap_err();
        assert_eq!(error.stage, Stage::Streaming);
        assert!(matches!(
            error.kind,
            ErrorKind::ResourceCap {
                class: "scratch",
                bytes: 1001
            }
        ));
        assert_eq!(ledger.used(), 1000, "a failed reserve changes nothing");
        assert_eq!(ledger.class_bytes(MemoryClass::Scratch), 0);

        let overflow = ledger.reserve(MemoryClass::Sections, u64::MAX).unwrap_err();
        assert!(matches!(overflow.kind, ErrorKind::ResourceCap { .. }));

        ledger.release(MemoryClass::Sections, 400);
        assert_eq!(ledger.used(), 600);
        assert_eq!(ledger.class_bytes(MemoryClass::Sections), 0);
        assert!((ledger.pressure() - 0.6).abs() < 1e-6);

        ledger.release(MemoryClass::Sections, 50);
        assert_eq!(ledger.class_bytes(MemoryClass::Sections), 0);

        let mut empty = MemoryLedger::new(0);
        assert!((empty.pressure() - 0.0).abs() < f32::EPSILON);
        assert!(empty.reserve(MemoryClass::Light, 0).is_ok());
        assert!(empty.reserve(MemoryClass::Light, 1).is_err());
    }

    #[test]
    fn largest_classes_is_deterministic() {
        let mut ledger = MemoryLedger::new(1 << 20);
        ledger.reserve(MemoryClass::Meshes, 10).unwrap();
        ledger.reserve(MemoryClass::Sections, 10).unwrap();
        ledger.reserve(MemoryClass::Light, 30).unwrap();

        assert_eq!(
            ledger.largest_classes(3),
            vec![
                (MemoryClass::Light, 30),
                (MemoryClass::Sections, 10),
                (MemoryClass::Meshes, 10),
            ]
        );
        assert_eq!(
            ledger.largest_classes(3),
            ledger.largest_classes(3),
            "stable across calls"
        );
        assert_eq!(ledger.largest_classes(0), Vec::new());
        assert_eq!(ledger.largest_classes(usize::MAX).len(), 6);
    }

    #[test]
    fn shed_ladder_is_monotone_and_bounded() {
        let mut state = ShedState::new(16);
        assert_eq!(state.step(), ShedStep::Full);
        assert!(state.allows(0));
        assert!(state.allows(1));
        assert!(state.allows(2));

        for expected in [
            ShedStep::NoBackground,
            ShedStep::NoLod2,
            ShedStep::NoLod1,
            ShedStep::NoPrefetch,
            ShedStep::RadiusCut,
        ] {
            assert!(shed(&mut state));
            assert_eq!(state.step(), expected);
            assert_eq!(state.restore_after(), RESTORE_WINDOWS);
            assert!(state.allows(0), "class 0 is never shed");
            assert!(state.allows(1), "class 1 is never shed");
            assert!(!state.allows(2));
        }
        assert!(!shed(&mut state), "the ladder stops at RadiusCut");
        assert_eq!(state.step(), ShedStep::RadiusCut);

        for expected in [
            ShedStep::NoPrefetch,
            ShedStep::NoLod1,
            ShedStep::NoLod2,
            ShedStep::NoBackground,
            ShedStep::Full,
        ] {
            assert!(restore(&mut state));
            assert_eq!(state.step(), expected);
        }
        assert!(!restore(&mut state), "the ladder stops at Full");
        assert_eq!(state.restore_after(), 0);
        assert!(state.allows(2));

        assert!(shed(&mut state));
        for _ in 0..RESTORE_WINDOWS {
            assert!(state.restore_after() > 0);
            state.count_restore_window();
        }
        assert_eq!(state.restore_after(), 0);

        state.set_effective_radius(2);
        assert_eq!(state.effective_radius(), MIN_VIEW_RADIUS);
        state.set_effective_radius(24);
        assert_eq!(state.effective_radius(), 24);
    }

    #[test]
    fn overlay_has_one_line_per_group() {
        let counters = Counters::new();
        counters.add("applied", 3);
        counters.set("ready_columns", 12);
        counters.set("light_repairs", 7);

        let mut ledger = MemoryLedger::new(1024);
        ledger.reserve(MemoryClass::Sections, 512).unwrap();
        let state = ShedState::new(12);

        let lines = overlay_lines(&counters, &ledger, &state);
        assert_eq!(lines.len(), 8);
        let prefixes = [
            "revisions: ",
            "queues: ",
            "jobs: ",
            "memory: ",
            "lod: ",
            "light: ",
            "edits: ",
            "shed: ",
        ];
        for (line, prefix) in lines.iter().zip(prefixes) {
            assert!(line.starts_with(prefix), "expected {prefix}, got {line}");
        }
        assert_eq!(lines, overlay_lines(&counters, &ledger, &state));

        assert!(lines[0].contains("applied=3"));
        assert!(lines[1].contains("ready_columns=12"));
        assert!(lines[3].contains("used=512"));
        assert!(lines[3].contains("cap=1024"));
        assert!(lines[3].contains("sections=512"));
        assert!(lines[3].contains("scratch=0"));
        assert!(lines[5].contains("light_repairs=7"));
        assert!(lines[6].contains("edit_p50=0"));
        assert!(lines[7].contains("step=Full"));
        assert!(lines[7].contains("radius=12"));
        assert!(lines[7].contains("restore_after=0"));
    }

    #[test]
    fn every_worker_stage_has_a_latency_name() {
        let mut counters = Counters::new();

        // Whole milliseconds, truncated: a fraction of a millisecond reads 0
        // rather than a rounded-up sample the pinned unit cannot express.
        assert!(record_latency(&mut counters, Stage::Meshing, 999));
        assert_eq!(counters.get("meshing_p50"), Some(0));

        for stage in [
            Stage::Fields,
            Stage::Hydrology,
            Stage::Height,
            Stage::Volume,
            Stage::Features,
            Stage::Lighting,
            Stage::Meshing,
        ] {
            assert!(record_latency(&mut counters, stage, 3_500), "{stage:?}");
            let stem = format!("{stage:?}").to_ascii_lowercase();
            let p50 = format!("{stem}_p50");
            let p95 = format!("{stem}_p95");
            assert_eq!(counters.get(&p50), Some(3), "{p50}");
            assert_eq!(
                counters.get(&p95),
                Some(0),
                "one sample is not a p95: {p95}"
            );
        }

        for stage in [
            Stage::Description,
            Stage::Graph,
            Stage::Streaming,
            Stage::Persistence,
            Stage::Migration,
            Stage::Diagnostics,
            Stage::Surface,
            Stage::Strata,
        ] {
            assert!(!record_latency(&mut counters, stage, 5_000), "{stage:?}");
        }
    }

    #[test]
    fn gauges_round_trip_through_get() {
        let mut counters = Counters::new();
        set_gauges(&mut counters, 40, 32, 8);

        assert_eq!(counters.get("resident_columns"), Some(40));
        assert_eq!(counters.get("ready_columns"), Some(32));
        assert_eq!(counters.get("queued_columns"), Some(8));

        // Gauges overwrite, they do not accumulate.
        set_gauges(&mut counters, 1, 2, 3);
        assert_eq!(counters.get("resident_columns"), Some(1));
        assert_eq!(counters.get("ready_columns"), Some(2));
        assert_eq!(counters.get("queued_columns"), Some(3));

        // The pinned table gained nothing from the write.
        assert_eq!(counters.values.len(), METRICS.len());
    }

    #[test]
    fn unknown_stage_does_not_panic() {
        let counters = Counters::new();
        let stages = [
            Stage::Description,
            Stage::Graph,
            Stage::Fields,
            Stage::Hydrology,
            Stage::Height,
            Stage::Surface,
            Stage::Strata,
            Stage::Volume,
            Stage::Features,
            Stage::Lighting,
            Stage::Meshing,
            Stage::Streaming,
            Stage::Persistence,
            Stage::Migration,
            Stage::Diagnostics,
        ];

        for stage in stages {
            // The shared-reference path is the one a generator uses; the
            // `&mut` path must agree with it for every variant, including the
            // ones with no pinned name.
            let before = counters.get("fields_p50");
            let shared = counters.record_latency(stage, 1_500);
            let exclusive = record_latency(&mut Counters::new(), stage, 1_500);
            assert_eq!(shared, exclusive, "{stage:?}");
            if !shared {
                assert_eq!(counters.get("fields_p50"), before, "{stage:?}");
            }
        }

        assert!(!counters.set("no_such_stage_p50", 1));
        assert_eq!(counters.get("no_such_stage_p50"), None);
    }
}

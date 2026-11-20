//! Typed errors that cross every terrain stage boundary.
//!
//! One shape everywhere: [`TerrainError`]. Callers branch on [`Severity`] and
//! [`Disposition`], never on message text. Severity and disposition are a fixed
//! table with no wildcard arms, so adding a kind forces a decision.

use std::path::PathBuf;

use thiserror::Error;

/// A block coordinate in world space.
pub type Coord = [i32; 3];

/// The stage that produced an error.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Stage {
    /// Reading or validating the world description.
    Description,
    /// Compiling the field graph.
    Graph,
    /// Evaluating field tiles.
    Fields,
    /// Priority-flood, routing, accumulation, lakes.
    Hydrology,
    /// Height tiles and surface stacking.
    Height,
    /// Surface rule resolution.
    Surface,
    /// Strata and deep rock.
    Strata,
    /// 3D detail, caves, spans.
    Volume,
    /// Feature discovery and application.
    Features,
    /// Sky and block light.
    Lighting,
    /// Section meshing.
    Meshing,
    /// Streaming, residency, scheduling.
    Streaming,
    /// Section and journal persistence.
    Persistence,
    /// World migration.
    Migration,
    /// Counters and overlay.
    Diagnostics,
}

/// How bad an error is, for reporting and for the banner.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Severity {
    /// The process or the world cannot continue.
    Fatal,
    /// The session cannot continue; the world is intact.
    SessionFailed,
    /// The operation failed; the world continues.
    Recoverable,
    /// The operation continued with a substitution.
    Degraded,
}

/// What a caller is supposed to do about an error.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Disposition {
    /// Refuse to open the world.
    RefuseLoad,
    /// Fail this job only.
    FailJob,
    /// Continue with a placeholder and report it.
    DegradeAndContinue,
    /// Retry the same job once.
    RetryOnce,
    /// Report it and carry on.
    Report,
}

/// Fault classes inside one section payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SectionFault {
    /// The checksum does not match the payload.
    Checksum,
    /// The framing or bit width is impossible.
    Framing,
    /// A palette name is unknown and cannot be a placeholder.
    UnknownName,
    /// The payload is shorter or longer than its header claims.
    Length,
}

/// What went wrong, with typed payloads.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum ErrorKind {
    /// A saved section failed validation.
    #[error("corrupt section at {path:?}: {fault:?}")]
    CorruptSection {
        /// File that failed validation.
        path: PathBuf,
        /// Which check failed.
        fault: SectionFault,
    },
    /// The recomputed description digest differs from the stored one.
    #[error("description hash mismatch: stored {found:02x?}, recomputed {expected:02x?}")]
    DescriptionHashMismatch {
        /// Digest recomputed from the description.
        expected: [u8; 32],
        /// Digest recorded in the world header.
        found: [u8; 32],
    },
    /// The world names a generator version this build does not know.
    #[error("unknown generator version {0}")]
    UnknownGeneratorVersion(u16),
    /// The world was written by a newer engine.
    #[error("world needs generator version {found}, this build has {engine}")]
    NewerThanEngine {
        /// Version recorded in the world.
        found: u16,
        /// Newest version this build evaluates.
        engine: u16,
    },
    /// The journal ends mid-record. Recoverable by truncation.
    #[error("journal torn tail at {path:?}, {valid_bytes} bytes usable")]
    JournalTornTail {
        /// Journal file.
        path: PathBuf,
        /// Bytes before the torn record.
        valid_bytes: u64,
    },
    /// An operating-system file error.
    #[error("{op} failed for {path:?}: os error {os_error}")]
    IoError {
        /// Operation name, for example `read` or `rename`.
        op: &'static str,
        /// File involved.
        path: PathBuf,
        /// Raw OS error code.
        os_error: i32,
    },
    /// A saved block name is not in this build's content pack.
    #[error("unknown block {name:?} in section {section:?}")]
    UnknownBlock {
        /// Stable name from the save file.
        name: String,
        /// Section that referenced it.
        section: Coord,
    },
    /// The content pack cannot satisfy what the description pins (`A5.4`).
    #[error("content manifest is incompatible: {id:?} ({reason})")]
    ManifestIncompatible {
        /// Stable id of the record that failed.
        id: String,
        /// Why the record is incompatible.
        reason: &'static str,
    },
    /// The description failed validation.
    #[error("description key {key:?} violates {rule:?}")]
    DescriptionInvalid {
        /// Offending key.
        key: &'static str,
        /// Violated rule.
        rule: &'static str,
    },
    /// The graph failed compilation.
    #[error("graph node {node} violates {rule:?}")]
    GraphInvalid {
        /// Node id.
        node: u32,
        /// Violated rule.
        rule: &'static str,
    },
    /// A job panicked and was contained at the pool boundary.
    #[error("job panicked in {stage:?}")]
    JobPanicked {
        /// Stage that panicked.
        stage: Stage,
    },
    /// A job outlived its soft deadline.
    #[error("job in {stage:?} overdue after {budget_ms} ms")]
    JobOverdue {
        /// Stage that overran.
        stage: Stage,
        /// Deadline that was exceeded.
        budget_ms: u64,
    },
    /// A job failed after its retry.
    #[error("job failed in {stage:?}")]
    JobFailed {
        /// Stage that failed.
        stage: Stage,
    },
    /// A memory class hit its cap.
    #[error("resource cap for {class:?}: {bytes} bytes")]
    ResourceCap {
        /// Memory class name.
        class: &'static str,
        /// Bytes requested or held.
        bytes: u64,
    },
    /// A checked invariant did not hold.
    #[error("invariant {check:?} violated at {coord:?}")]
    InvariantViolated {
        /// Invariant name.
        check: &'static str,
        /// Where it failed.
        coord: Option<Coord>,
    },
}

impl ErrorKind {
    /// Severity of this kind. Exhaustive: no wildcard arm.
    #[must_use]
    pub const fn severity(&self) -> Severity {
        match self {
            Self::CorruptSection { .. }
            | Self::DescriptionHashMismatch { .. }
            | Self::UnknownGeneratorVersion(_)
            | Self::NewerThanEngine { .. }
            | Self::ManifestIncompatible { .. } => Severity::Fatal,
            Self::IoError { .. } | Self::JobPanicked { .. } => Severity::SessionFailed,
            Self::JournalTornTail { .. }
            | Self::DescriptionInvalid { .. }
            | Self::GraphInvalid { .. }
            | Self::JobOverdue { .. }
            | Self::JobFailed { .. }
            | Self::ResourceCap { .. }
            | Self::InvariantViolated { .. } => Severity::Recoverable,
            Self::UnknownBlock { .. } => Severity::Degraded,
        }
    }

    /// Disposition of this kind. Exhaustive: no wildcard arm.
    #[must_use]
    pub const fn disposition(&self) -> Disposition {
        match self {
            Self::CorruptSection { .. }
            | Self::DescriptionHashMismatch { .. }
            | Self::UnknownGeneratorVersion(_)
            | Self::NewerThanEngine { .. }
            | Self::DescriptionInvalid { .. }
            | Self::ManifestIncompatible { .. } => Disposition::RefuseLoad,
            Self::JournalTornTail { .. } => Disposition::RetryOnce,
            Self::IoError { .. }
            | Self::JobPanicked { .. }
            | Self::JobOverdue { .. }
            | Self::JobFailed { .. } => Disposition::FailJob,
            Self::GraphInvalid { .. }
            | Self::InvariantViolated { .. }
            | Self::ResourceCap { .. } => Disposition::Report,
            Self::UnknownBlock { .. } => Disposition::DegradeAndContinue,
        }
    }
}

/// One boundary crossing, appended as the error travels up.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Frame {
    /// Stage entered.
    pub stage: Stage,
    /// Operation performed there.
    pub op: &'static str,
}

/// The single error type crossing every terrain stage boundary.
#[derive(Clone, Debug, Eq, PartialEq, Error)]
#[error("{stage:?}: {kind}")]
pub struct TerrainError {
    /// Stage that first produced the error.
    pub stage: Stage,
    /// What went wrong.
    pub kind: ErrorKind,
    /// Coordinate involved, when there is one.
    pub coord: Option<Coord>,
    /// Boundary crossings from producer to caller.
    pub context: Vec<Frame>,
}

impl TerrainError {
    /// Builds an error with an empty context.
    #[must_use]
    pub fn new(stage: Stage, kind: ErrorKind) -> Self {
        Self {
            stage,
            kind,
            coord: None,
            context: Vec::new(),
        }
    }

    /// Attaches a coordinate.
    #[must_use]
    pub fn at(mut self, coord: Coord) -> Self {
        self.coord = Some(coord);
        self
    }

    /// Appends one boundary crossing.
    #[must_use]
    pub fn frame(mut self, stage: Stage, op: &'static str) -> Self {
        self.context.push(Frame { stage, op });
        self
    }

    /// Severity of the wrapped kind.
    #[must_use]
    pub const fn severity(&self) -> Severity {
        self.kind.severity()
    }

    /// Disposition of the wrapped kind.
    #[must_use]
    pub const fn disposition(&self) -> Disposition {
        self.kind.disposition()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_kind() -> Vec<ErrorKind> {
        vec![
            ErrorKind::CorruptSection {
                path: PathBuf::from("a"),
                fault: SectionFault::Checksum,
            },
            ErrorKind::DescriptionHashMismatch {
                expected: [1; 32],
                found: [2; 32],
            },
            ErrorKind::UnknownGeneratorVersion(9),
            ErrorKind::NewerThanEngine {
                found: 9,
                engine: 2,
            },
            ErrorKind::JournalTornTail {
                path: PathBuf::from("j"),
                valid_bytes: 7,
            },
            ErrorKind::IoError {
                op: "read",
                path: PathBuf::from("p"),
                os_error: 2,
            },
            ErrorKind::UnknownBlock {
                name: "mythril".to_owned(),
                section: [0, 0, 0],
            },
            ErrorKind::ManifestIncompatible {
                id: "core:stone".to_owned(),
                reason: "missing",
            },
            ErrorKind::DescriptionInvalid {
                key: "SEA",
                rule: "positive",
            },
            ErrorKind::GraphInvalid {
                node: 3,
                rule: "cycle",
            },
            ErrorKind::JobPanicked {
                stage: Stage::Volume,
            },
            ErrorKind::JobOverdue {
                stage: Stage::Volume,
                budget_ms: 250,
            },
            ErrorKind::JobFailed {
                stage: Stage::Volume,
            },
            ErrorKind::ResourceCap {
                class: "sections",
                bytes: 4096,
            },
            ErrorKind::InvariantViolated {
                check: "range",
                coord: None,
            },
        ]
    }

    #[test]
    fn severity_and_disposition_are_pinned_per_kind() {
        let expected = [
            (Severity::Fatal, Disposition::RefuseLoad),
            (Severity::Fatal, Disposition::RefuseLoad),
            (Severity::Fatal, Disposition::RefuseLoad),
            (Severity::Fatal, Disposition::RefuseLoad),
            (Severity::Recoverable, Disposition::RetryOnce),
            (Severity::SessionFailed, Disposition::FailJob),
            (Severity::Degraded, Disposition::DegradeAndContinue),
            (Severity::Fatal, Disposition::RefuseLoad),
            (Severity::Recoverable, Disposition::RefuseLoad),
            (Severity::Recoverable, Disposition::Report),
            (Severity::SessionFailed, Disposition::FailJob),
            (Severity::Recoverable, Disposition::FailJob),
            (Severity::Recoverable, Disposition::FailJob),
            (Severity::Recoverable, Disposition::Report),
            (Severity::Recoverable, Disposition::Report),
        ];
        for (kind, (severity, disposition)) in every_kind().into_iter().zip(expected) {
            assert_eq!(kind.severity(), severity, "{kind:?}");
            assert_eq!(kind.disposition(), disposition, "{kind:?}");
        }
    }

    #[test]
    fn context_appends_in_order() {
        let error = TerrainError::new(
            Stage::Hydrology,
            ErrorKind::JobFailed {
                stage: Stage::Hydrology,
            },
        )
        .at([4, 0, -9])
        .frame(Stage::Height, "column")
        .frame(Stage::Streaming, "apply");

        assert_eq!(error.coord, Some([4, 0, -9]));
        assert_eq!(error.context.len(), 2);
        assert_eq!(error.context[0].op, "column");
        assert_eq!(error.context[1].op, "apply");
        assert_eq!(error.severity(), Severity::Recoverable);
        assert!(error.to_string().starts_with("Hydrology: "));
    }
}

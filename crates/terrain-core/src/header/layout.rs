/// File name of the world header inside a world directory.
pub const HEADER_FILE: &str = "header.bin";

/// First four bytes of every header.
pub(crate) const MAGIC: [u8; 4] = *b"TWLD";

/// Header/save format version this build writes (`A1.2`).
///
/// Version 1 has no migration state; version 2 appends it. Both are read.
pub const HEADER_FORMAT_VERSION: u16 = 2;

/// The oldest header layout this build still reads.
pub(crate) const HEADER_FORMAT_V1: u16 = 1;

/// Number of steps in the load order (`A1.5`).
pub(crate) const LOAD_STEP_COUNT: usize = 7;

/// Migration state carried by the header (`A16`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MigrationState {
    /// No migration is in progress.
    None,
    /// A migration opened the world in resume mode.
    InProgress {
        /// Format the world is migrating from.
        from_format: u16,
        /// Format the world is migrating to.
        to_format: u16,
        /// UTC Unix seconds when the migration began.
        started: i64,
    },
}

impl MigrationState {
    /// The on-disk tag.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::None => 0,
            Self::InProgress { .. } => 1,
        }
    }

    /// Whether a migration must be resumed before the world opens.
    #[must_use]
    pub const fn is_in_progress(self) -> bool {
        matches!(self, Self::InProgress { .. })
    }
}

/// The mandatory world header (`A1.2`).
///
/// Seed and platform tag are here rather than in the description because one
/// description serves many worlds (`A1.1`). `description_hash` is the 32-byte
/// SHA-256 digest that `Description::description_hash` returns, so the header and
/// `ErrorKind::DescriptionHashMismatch` carry the same width.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Header {
    /// On-disk format version. A reader refuses any value it does not know.
    pub format_version: u16,
    /// Generator version this world is pinned to (`T1.2`).
    pub generator_version: u16,
    /// Digest of the canonical description (`A1.1`, `A3.5`).
    pub description_hash: [u8; 32],
    /// Algorithm `description_hash` was produced with (`A3.5`).
    pub hash_algorithm: String,
    /// World seed. Not part of the description hash (`T1.2`).
    pub seed: u64,
    /// Target triple of first generation (`A2.6`), display and diagnostics only.
    pub platform_tag: String,
    /// Target triple of the most recent save (`A2.6`), display only.
    pub last_saved_platform_tag: String,
    /// 128-bit world identity, never reused; a Fork assigns a new one (`A1.2`).
    pub world_id: [u8; 16],
    /// Display name. Never identity, never a path.
    pub name: String,
    /// Creation time, UTC Unix seconds (`A2.4` allows clocks in timestamps only).
    pub created: i64,
    /// Most recent save time, UTC Unix seconds.
    pub last_saved: i64,
    /// Monotonic committed-edit revision (`J1.2`).
    pub revision: u64,
    /// Description schema version this world was written against.
    pub schema_version: u16,
    /// Migration state; version 1 headers decode as [`MigrationState::None`].
    pub migration_state: MigrationState,
}

/// The current UTC time in Unix seconds, for `created` and `last_saved`.
///
/// `A2.4` allows a clock in header timestamps and nowhere that reaches a block,
/// a field, a hash, or a cache key.
pub fn now_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

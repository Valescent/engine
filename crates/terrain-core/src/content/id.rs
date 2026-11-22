//! The block id type, owned by `content` so `section` can depend on it.
//!
//! `BlockId` is a content concept: ids are derived from the pack's stable
//! names (`A5.1`) and are only an in-memory index into the pack's records.
//! Keeping the alias here makes `section -> content` a one-way edge.

/// A block type id. Id `0` is air by registry convention.
pub type BlockId = u16;

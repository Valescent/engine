//! Quad pool allocation for ADR-0017.
//!
//! The pool is a flat array of 12-byte quad records. It is divided into pages of
//! [`PAGE_QUADS`] quads; a chunk reserves a contiguous run of pages and stores the
//! first quad index in its chunk-info record. Runs are freed after two frames in
//! flight, then returned to a coalescing free list.
//!
//! This module is pure bookkeeping: it owns no GPU buffer and performs no I/O.

#![allow(dead_code)] // The renderer consumes the allocator when the pool path lands.

use std::collections::BTreeMap;

/// Quads per pool page. One page is 768 bytes of records.
pub(crate) const PAGE_QUADS: u32 = 64;

/// A chunk's reserved range in the flat quad array.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct QuadRange {
    /// Index of the first quad in the pool.
    pub(crate) first_quad: u32,
    /// Quads the chunk actually uses; the draw count comes from this.
    pub(crate) quads: u32,
    /// Pages reserved, which may exceed `ceil(quads / PAGE_QUADS)` after a split.
    pub(crate) pages: u32,
}

impl QuadRange {
    /// Quads the reservation can hold.
    pub(crate) fn capacity(self) -> u32 {
        self.pages * PAGE_QUADS
    }
}

/// A free page run, keyed by its first page.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Run {
    pages: u32,
}

/// Fixed-capacity page allocator with a coalescing free list and deferred frees.
#[derive(Debug)]
pub(crate) struct QuadPool {
    capacity_pages: u32,
    free: BTreeMap<u32, Run>,
    deferred: Vec<(u64, u32, Run)>,
    allocated_pages: u32,
}

impl QuadPool {
    /// Creates a pool of `capacity_quads`, rounded up to whole pages.
    pub(crate) fn new(capacity_quads: u32) -> Self {
        let capacity_pages = capacity_quads.div_ceil(PAGE_QUADS).max(1);
        let mut free = BTreeMap::new();
        free.insert(
            0,
            Run {
                pages: capacity_pages,
            },
        );
        Self {
            capacity_pages,
            free,
            deferred: Vec::new(),
            allocated_pages: 0,
        }
    }

    /// Reserves a contiguous run for `quads`, or returns `None` when no run fits.
    pub(crate) fn allocate(&mut self, quads: u32) -> Option<QuadRange> {
        if quads == 0 || quads > self.capacity_pages * PAGE_QUADS {
            return None;
        }
        let pages = quads.div_ceil(PAGE_QUADS);
        let (&start, &run) = self.free.iter().find(|(_, run)| run.pages >= pages)?;

        self.free.remove(&start);
        if run.pages > pages {
            let tail = start + pages;
            self.free.insert(
                tail,
                Run {
                    pages: run.pages - pages,
                },
            );
        }
        self.allocated_pages += pages;

        Some(QuadRange {
            first_quad: start * PAGE_QUADS,
            quads,
            pages,
        })
    }

    /// Schedules a range for release after `frame + 2`.
    pub(crate) fn free(&mut self, range: QuadRange, frame: u64) {
        self.deferred.push((
            frame,
            range.first_quad / PAGE_QUADS,
            Run { pages: range.pages },
        ));
    }

    /// Returns runs whose two-frame age has passed to the free list.
    ///
    /// Call once per frame with the current frame number.
    pub(crate) fn retire(&mut self, frame: u64) {
        let mut index = 0;
        while index < self.deferred.len() {
            if self.deferred[index].0 + 2 <= frame {
                let (_, start, run) = self.deferred.swap_remove(index);
                self.insert_run(start, run);
            } else {
                index += 1;
            }
        }
    }

    /// Inserts a run and coalesces it with its neighbours.
    fn insert_run(&mut self, start: u32, run: Run) {
        self.allocated_pages = self.allocated_pages.saturating_sub(run.pages);
        let mut start = start;
        let mut pages = run.pages;

        if let Some((&left_start, &left)) = self.free.range(..=start).next_back()
            && left_start + left.pages == start
        {
            self.free.remove(&left_start);
            start = left_start;
            pages += left.pages;
        }
        if let Some((&right_start, &right)) = self.free.range(start + pages..).next()
            && right_start == start + pages
        {
            self.free.remove(&right_start);
            pages += right.pages;
        }

        self.free.insert(start, Run { pages });
    }

    /// Pages currently reserved by chunks.
    pub(crate) fn allocated_pages(&self) -> u32 {
        self.allocated_pages
    }

    /// Total pages in the pool.
    pub(crate) fn capacity_pages(&self) -> u32 {
        self.capacity_pages
    }

    /// Quads currently reserved, including page rounding.
    pub(crate) fn allocated_quads(&self) -> u32 {
        self.allocated_pages * PAGE_QUADS
    }

    /// Runs waiting for their two-frame age, for diagnostics.
    pub(crate) fn deferred_len(&self) -> usize {
        self.deferred.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocates_page_aligned_runs() {
        let mut pool = QuadPool::new(PAGE_QUADS * 4);
        let first = pool.allocate(1).expect("first");
        assert_eq!(
            first,
            QuadRange {
                first_quad: 0,
                quads: 1,
                pages: 1,
            }
        );
        let second = pool.allocate(PAGE_QUADS + 1).expect("second");
        assert_eq!(second.first_quad, PAGE_QUADS);
        assert_eq!(second.pages, 2);
        assert_eq!(pool.allocated_pages(), 3);
    }

    #[test]
    fn reuses_freed_runs() {
        let mut pool = QuadPool::new(PAGE_QUADS * 2);
        let range = pool.allocate(PAGE_QUADS).expect("first");
        pool.free(range, 0);
        pool.retire(2);
        assert_eq!(pool.allocated_pages(), 0);
        let again = pool.allocate(PAGE_QUADS).expect("again");
        assert_eq!(again.first_quad, 0);
    }

    #[test]
    fn waits_two_frames_before_reuse() {
        let mut pool = QuadPool::new(PAGE_QUADS * 2);
        let range = pool.allocate(PAGE_QUADS).expect("first");
        pool.free(range, 10);
        pool.retire(11);
        assert_eq!(pool.deferred_len(), 1);
        assert_eq!(
            pool.allocate(PAGE_QUADS).map(|r| r.first_quad),
            Some(PAGE_QUADS)
        );
        pool.retire(12);
        assert_eq!(pool.deferred_len(), 0);
        assert_eq!(pool.allocate(PAGE_QUADS).map(|r| r.first_quad), Some(0));
    }

    #[test]
    fn coalesces_adjacent_runs() {
        let mut pool = QuadPool::new(PAGE_QUADS * 4);
        let a = pool.allocate(PAGE_QUADS).expect("a");
        let b = pool.allocate(PAGE_QUADS).expect("b");
        let c = pool.allocate(PAGE_QUADS).expect("c");
        pool.free(c, 0);
        pool.free(b, 0);
        pool.free(a, 0);
        pool.retire(2);
        let whole = pool.allocate(PAGE_QUADS * 3).expect("coalesced");
        assert_eq!(whole.first_quad, 0);
        assert_eq!(whole.pages, 3);
    }

    #[test]
    fn refuses_when_exhausted_or_empty() {
        let mut pool = QuadPool::new(PAGE_QUADS);
        assert!(pool.allocate(0).is_none());
        assert!(pool.allocate(PAGE_QUADS).is_some());
        assert!(pool.allocate(1).is_none());
    }

    #[test]
    fn reports_capacity_and_allocation() {
        let mut pool = QuadPool::new(PAGE_QUADS * 4 + 1);
        assert_eq!(pool.capacity_pages(), 5);
        let range = pool.allocate(PAGE_QUADS + 1).expect("range");
        assert_eq!(range.pages, 2);
        assert_eq!(range.capacity(), PAGE_QUADS * 2);
        assert_eq!(pool.allocated_quads(), PAGE_QUADS * 2);
    }
}

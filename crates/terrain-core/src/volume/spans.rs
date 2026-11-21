//! Solid spans, their kinds, and the rule set each span top takes.

use crate::section::BlockId;

/// What a solid span is, relative to the cave void that touches it (`F4.1`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SpanKind {
    /// No cave void touches the span: an ordinary column, or a fully buried layer.
    Surface,
    /// A cave void sits directly above the span's top, which is a cave floor.
    CaveFloor,
    /// A cave void sits directly below the span's bottom, and not above it: the
    /// span's underside is a cave ceiling.
    Ceiling,
}

/// One maximal run of solid blocks (`F4.1`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Span {
    /// Lowest block of the run, inclusive.
    pub y0: i32,
    /// Highest block of the run, inclusive.
    pub y1: i32,
    /// The run's role against the void that touches it.
    pub kind: SpanKind,
}

/// Which `E4.5` rule set a span's top takes (`F4.1`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SpanRule {
    /// The full `E4.5` column stack: bed, sea floor, beach, snow, slope, biome.
    Stack,
    /// Cave floor open to the sky: stone or gravel, plus biome topsoil, never snow
    /// or beach.
    OpenFloor,
    /// Roofed cave floor: stone or gravel only.
    CaveFloor,
    /// Cave ceiling underside: always stone.
    Stone,
}

/// The span's top rule set (`F4.1`).
///
/// `highest` marks the span at the top of the column: `F4.1` keeps the full `E4.5`
/// stack there whatever void it roofs, so it always answers [`SpanRule::Stack`].
/// Every lower span takes the reduced set: a roofed floor is stone or gravel only,
/// an open floor may also take biome topsoil, and a ceiling is stone. Snow and
/// beach exist only in [`SpanRule::Stack`], which is why a cave floor can never
/// take either.
#[must_use]
pub fn span_rule(kind: SpanKind, highest: bool, open_to_sky: bool) -> SpanRule {
    if highest {
        return SpanRule::Stack;
    }
    match kind {
        SpanKind::Surface => SpanRule::Stack,
        SpanKind::CaveFloor => {
            if open_to_sky {
                SpanRule::OpenFloor
            } else {
                SpanRule::CaveFloor
            }
        }
        SpanKind::Ceiling => SpanRule::Stone,
    }
}

/// The maximal solid runs of a finished column, top first (`F4.1`).
///
/// `air` is the caller's air id. A column with no solid block yields an empty
/// vector; a column with no cave yields exactly one [`SpanKind::Surface`] run whose
/// top is the column's top. Runs are maximal, so any run but the first has a void
/// above it and any run but the last has a void below it.
#[must_use]
pub fn resolve_spans(column: &[BlockId], air: BlockId) -> Vec<Span> {
    let mut spans = Vec::new();
    let len = i32::try_from(column.len()).unwrap_or(i32::MAX);
    let mut y = len - 1;
    while y >= 0 {
        let index = usize::try_from(y).unwrap_or(0);
        if column[index] == air {
            y -= 1;
            continue;
        }
        let y1 = y;
        while y > 0 && column[usize::try_from(y - 1).unwrap_or(0)] != air {
            y -= 1;
        }
        spans.push(Span {
            y0: y,
            y1,
            kind: SpanKind::Surface,
        });
        y -= 1;
    }

    let count = spans.len();
    for (position, span) in spans.iter_mut().enumerate() {
        let above = position > 0;
        let below = position + 1 < count;
        span.kind = match (above, below) {
            (false, false) => SpanKind::Surface,
            (true, _) => SpanKind::CaveFloor,
            (false, true) => SpanKind::Ceiling,
        };
    }
    spans
}

/// Whether everything above `top` is air, i.e. skylight reaches `top` (`F4.1`).
///
/// This is a walk to the top of the column, not a span count: a shaft that reaches
/// the sky is open however many spans surround it, and a pocket roofed by a single
/// block is not.
#[must_use]
pub fn open_to_sky(column: &[BlockId], air: BlockId, top: i32) -> bool {
    let start = usize::try_from(top.saturating_add(1)).unwrap_or(usize::MAX);
    let start = start.min(column.len());
    column[start..].iter().all(|cell| *cell == air)
}

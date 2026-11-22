//! `SpanMap` construction: the template's literal SQL segments anchored
//! to the compiled text in order, ignoring whitespace.
//!
//! Segments in untaken branches and repeated text make a greedy scan
//! skip too far, so the anchors are the maximum-weight chain of segment
//! occurrences, computed with a Fenwick tree over compiled positions.
//! An anchor is byte-identical on both sides (`SpanMap::map` translates by
//! offset), so a segment splits where its whitespace differs.

use std::collections::BTreeMap;

use cogitator_ir::origin::SpanMap;
use cogitator_ir::Span;

use super::chain::{best_chain, Occurrence};
use super::template::{segments, Segment};
use super::{trim_span, Prepared};

/// Basis points of the template's unconditional literal SQL that must be
/// found in the compiled text for it to count as this template's output.
/// That SQL always reaches the compiled text, so a shortfall means an edit
/// the compile has not seen; unconditional SQL is the only part whose
/// absence is evidence, since macros and untaken branches explain the rest.
pub const MIN_LITERAL_COVERAGE: u16 = 9500;

/// `segments × normalized compiled bytes` scanned at most.
const MAX_SCAN_BYTES: u64 = 256 * 1024 * 1024;
const BASIS_POINTS: u64 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alignment {
    pub map: SpanMap,
    /// Basis points of the template's unconditional literal SQL that was
    /// found. 100% when the template has none.
    pub literal_coverage: u16,
}

/// Non-whitespace bytes of a text, with the offset each came from.
pub(super) struct Normalized {
    pub(super) text: String,
    pub(super) at: Vec<u32>,
}

impl Normalized {
    fn of(text: &str, range: std::ops::Range<usize>, skip: &[Span]) -> Self {
        let mut out = Normalized {
            text: String::new(),
            at: Vec::new(),
        };
        let mut skip = skip.iter().peekable();
        let start = range.start;
        for (i, c) in text.get(range).unwrap_or("").char_indices() {
            let offset = (start + i) as u32;
            while skip.next_if(|s| s.end <= offset).is_some() {}
            if c.is_ascii_whitespace() || skip.peek().is_some_and(|s| s.start <= offset) {
                continue;
            }
            out.text.push(c);
            out.at.extend((0..c.len_utf8() as u32).map(|b| offset + b));
        }
        out
    }
}

/// Anchors `template` to `compiled`, which `prepared` was made from.
pub fn align(template: &str, prepared: &Prepared, compiled: &str) -> Alignment {
    let segments = segments(template);
    let haystack = Normalized::of(compiled, 0..compiled.len(), prepared.blanks());
    let needles: Vec<Normalized> = segments
        .iter()
        .map(|s| Normalized::of(template, s.span.clone(), &[]))
        .collect();
    let scan = (needles.len() as u64).saturating_mul(haystack.text.len() as u64);
    let chain = if scan > MAX_SCAN_BYTES {
        Vec::new()
    } else {
        one_branch(&segments, best_chain(&needles, &haystack))
    };

    let mut map = SpanMap::default();
    let mut cursor = (0u32, 0u32);
    let mut matched = 0u64;
    let mut required_matched = 0u64;
    for occurrence in &chain {
        let Some(needle) = needles.get(occurrence.segment as usize) else {
            continue;
        };
        let (Some(&first_t), Some(&last_t)) = (needle.at.first(), needle.at.last()) else {
            continue;
        };
        let Some(&first_c) = haystack.at.get(occurrence.pos as usize) else {
            continue;
        };
        let Some(&last_c) = haystack
            .at
            .get((occurrence.pos + occurrence.len - 1) as usize)
        else {
            continue;
        };
        push_gap(
            &mut map.jinja_gaps,
            Span::new(cursor.0, first_c),
            Span::new(cursor.1, first_t),
            compiled,
            template,
        );
        push_anchors(
            &mut map.anchors,
            needle,
            occurrence.pos,
            &haystack,
            template,
            compiled,
        );
        cursor = (last_c + 1, last_t + 1);
        matched += u64::from(occurrence.len);
        if segments
            .get(occurrence.segment as usize)
            .is_some_and(Segment::required)
        {
            required_matched += u64::from(occurrence.len);
        }
    }
    push_gap(
        &mut map.jinja_gaps,
        Span::new(cursor.0, compiled.len() as u32),
        Span::new(cursor.1, template.len() as u32),
        compiled,
        template,
    );

    let total = compiled
        .bytes()
        .filter(|b| !b.is_ascii_whitespace())
        .count() as u64;
    map.coverage = basis_points(matched, total);
    let required_total: u64 = segments
        .iter()
        .zip(&needles)
        .filter(|(s, _)| s.required())
        .map(|(_, n)| n.text.len() as u64)
        .sum();
    Alignment {
        map,
        literal_coverage: if required_total == 0 {
            BASIS_POINTS as u16
        } else {
            basis_points(required_matched, required_total)
        },
    }
}

fn basis_points(part: u64, whole: u64) -> u16 {
    if whole == 0 {
        return 0;
    }
    (part.saturating_mul(BASIS_POINTS) / whole).min(BASIS_POINTS) as u16
}

/// Keeps one branch of each `if`, the one that found the most SQL: a
/// compile takes one branch, so anchors in two are accidental.
fn one_branch(segments: &[Segment], chain: Vec<Occurrence>) -> Vec<Occurrence> {
    let branches = |o: &Occurrence| {
        segments
            .get(o.segment as usize)
            .map(|s| s.branches.as_slice())
    };
    let mut weights: BTreeMap<(u32, u32), u64> = BTreeMap::new();
    for occurrence in &chain {
        for &key in branches(occurrence).unwrap_or(&[]) {
            *weights.entry(key).or_default() += u64::from(occurrence.len);
        }
    }
    let mut best: BTreeMap<u32, (u64, u32)> = BTreeMap::new();
    for (&(id, branch), &weight) in &weights {
        let slot = best.entry(id).or_insert((weight, branch));
        if weight > slot.0 {
            *slot = (weight, branch);
        }
    }
    chain
        .into_iter()
        .filter(|o| {
            branches(o)
                .unwrap_or(&[])
                .iter()
                .all(|(id, branch)| best.get(id).is_none_or(|b| b.1 == *branch))
        })
        .collect()
}

fn push_anchors(
    out: &mut Vec<(Span, Span)>,
    needle: &Normalized,
    pos: u32,
    haystack: &Normalized,
    template: &str,
    compiled: &str,
) {
    let at = |k: usize| {
        let t = *needle.at.get(k)?;
        let c = *haystack.at.get(pos as usize + k)?;
        Some((t, c))
    };
    let same_gap = |a: (u32, u32), b: (u32, u32)| {
        let t = template.as_bytes().get((a.0 + 1) as usize..b.0 as usize);
        let c = compiled.as_bytes().get((a.1 + 1) as usize..b.1 as usize);
        t.is_some() && t == c
    };
    let Some(mut run) = at(0) else { return };
    let mut prev = run;
    for k in 1..needle.at.len() {
        let Some(next) = at(k) else { return };
        if !same_gap(prev, next) {
            out.push(anchor(run, prev));
            run = next;
        }
        prev = next;
    }
    out.push(anchor(run, prev));
}

fn anchor(first: (u32, u32), last: (u32, u32)) -> (Span, Span) {
    (
        Span::new(first.1, last.1 + 1),
        Span::new(first.0, last.0 + 1),
    )
}

/// Compiled text between two anchors came from the Jinja between them.
fn push_gap(
    out: &mut Vec<(Span, Span)>,
    compiled_span: Span,
    template_span: Span,
    compiled: &str,
    template: &str,
) {
    let c = trim_span(compiled, compiled_span);
    let t = trim_span(template, template_span);
    if !c.is_empty() && !t.is_empty() {
        out.push((c, t));
    }
}

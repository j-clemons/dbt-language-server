//! Findings on a compiled plan, moved into template coordinates. Spans
//! come out of the passes indexing compiled text; only the plan's own
//! `SpanMap` says where they are in the editor's document.

use cogitator_ir::origin::{Mapped, SourceOrigin, SpanMap, Staleness};
use cogitator_ir::Span;

use super::report::{FindingView, FixView, Placement};

/// Maps every finding of a compiled-origin plan through its `SpanMap` and
/// decides what each may offer. Template-origin findings are unchanged.
pub(super) fn place_findings(findings: &mut [FindingView], origin: &SourceOrigin) {
    let SourceOrigin::Compiled { staleness, map } = origin else {
        return;
    };
    let fresh = *staleness == Staleness::Fresh;
    for finding in findings.iter_mut() {
        let (span, placement) = locate(finding.span, map);
        finding.span = span;
        finding.placement = placement;
        // A stale or unmapped span has no trustworthy place in the editor.
        finding.diagnostics_eligible = fresh && placement != Placement::Unmapped;
        if fresh && placement == Placement::Exact {
            finding.fixes.retain_mut(|fix| remap_fix(fix, map));
        } else {
            finding.fixes.clear();
        }
    }
    // Mapping can reorder; unmapped findings keep compiled offsets and sort last.
    findings.sort_by_key(|f| (f.placement == Placement::Unmapped, f.span, f.id));
}

fn locate(span: Span, map: &SpanMap) -> (Span, Placement) {
    match map.map(span) {
        Mapped::Exact(template) => (template, Placement::Exact),
        Mapped::InJinja(template) => (template, Placement::InJinja),
        Mapped::Unmapped => match covering(span, map) {
            Some(template) => (template, Placement::InJinja),
            None => (span, Placement::Unmapped),
        },
    }
}

/// The template range of a span crossing literal SQL and Jinja output,
/// such as a join over `{{ ref(...) }}`: no single anchor holds it, but
/// both of its ends can be placed. It highlights; it never carries a fix.
fn covering(span: Span, map: &SpanMap) -> Option<Span> {
    if span.end <= span.start {
        return None;
    }
    let first = placed(map.map(Span::new(span.start, span.start + 1)))?;
    let last = placed(map.map(Span::new(span.end - 1, span.end)))?;
    Some(Span::new(
        first.start.min(last.start),
        first.end.max(last.end),
    ))
}

fn placed(mapped: Mapped) -> Option<Span> {
    match mapped {
        Mapped::Exact(span) | Mapped::InJinja(span) => Some(span),
        Mapped::Unmapped => None,
    }
}

/// True when every edit lands exactly; a fix is never applied in part.
fn remap_fix(fix: &mut FixView, map: &SpanMap) -> bool {
    let mapped: Option<Vec<Span>> = fix
        .edits
        .iter()
        .map(|edit| match map.map(edit.span) {
            Mapped::Exact(span) => Some(span),
            _ => None,
        })
        .collect();
    let Some(mapped) = mapped else {
        return false;
    };
    for (edit, span) in fix.edits.iter_mut().zip(mapped) {
        edit.span = span;
    }
    true
}

//! Which text a plan was built from, and the map from compiled SQL back
//! to the template the user is editing.

use serde::Serialize;

use crate::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum TemplateVariant {
    Plain,
    Incremental,
    FullRefresh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Staleness {
    Fresh,
    Stale,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum SourceOrigin {
    Template { variant: TemplateVariant },
    Compiled { staleness: Staleness, map: SpanMap },
}

/// A span in compiled text, resolved against the template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Mapped {
    Exact(Span),
    InJinja(Span),
    Unmapped,
}

/// Anchors tie compiled SQL (first) to template SQL (second) in order,
/// ignoring whitespace; `jinja_gaps` tie compiled text produced by a
/// Jinja region to that region. Both are sorted by the compiled span.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SpanMap {
    pub anchors: Vec<(Span, Span)>,
    pub jinja_gaps: Vec<(Span, Span)>,
    /// Share of compiled bytes covered by anchors, in basis points.
    pub coverage: u16,
}

impl SpanMap {
    pub fn map(&self, compiled: Span) -> Mapped {
        if let Some((c, t)) = find(&self.anchors, compiled) {
            let start = t.start.saturating_add(compiled.start - c.start);
            return Mapped::Exact(Span::new(start, start.saturating_add(compiled.len())));
        }
        match find(&self.jinja_gaps, compiled) {
            Some((_, t)) => Mapped::InJinja(*t),
            None => Mapped::Unmapped,
        }
    }
}

fn find(pairs: &[(Span, Span)], compiled: Span) -> Option<&(Span, Span)> {
    let after = pairs.partition_point(|(c, _)| c.start <= compiled.start);
    let hit = pairs.get(after.checked_sub(1)?)?;
    hit.0.contains(compiled).then_some(hit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> SpanMap {
        SpanMap {
            anchors: vec![
                (Span::new(0, 7), Span::new(0, 7)),
                (Span::new(20, 25), Span::new(30, 35)),
            ],
            jinja_gaps: vec![(Span::new(7, 20), Span::new(7, 30))],
            coverage: 5000,
        }
    }

    #[test]
    fn exact_translates_by_anchor_offset() {
        assert_eq!(
            map().map(Span::new(21, 23)),
            Mapped::Exact(Span::new(31, 33))
        );
    }

    #[test]
    fn jinja_output_maps_to_the_region() {
        assert_eq!(
            map().map(Span::new(9, 12)),
            Mapped::InJinja(Span::new(7, 30))
        );
    }

    #[test]
    fn straddling_or_outside_is_unmapped() {
        assert_eq!(map().map(Span::new(5, 9)), Mapped::Unmapped);
        assert_eq!(map().map(Span::new(40, 41)), Mapped::Unmapped);
    }
}

//! Advice output. Sorted by (span, id) with a message tiebreak, so the
//! order is fixed and any HashMap iteration upstream cannot leak out.

use std::sync::Arc;

use cogitator_ir::common::RuleId;
use cogitator_ir::{Level, Span};

use crate::meta::{FixSafety, Severity};

/// How the claim was established (specs/ir-pipeline/06).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Evidence {
    Exact,
    Catalog {
        generated_at: Arc<str>,
    },
    /// A dbt test (`unique`, `not_null`) recorded in the project YAML.
    Tested {
        schema_revision: u64,
    },
    Heuristic,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixEdit {
    pub span: Span,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fix {
    pub title: String,
    pub safety: FixSafety,
    pub edits: Vec<FixEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub id: RuleId,
    pub span: Span,
    pub level: Level,
    pub severity: Severity,
    pub message: String,
    pub evidence: Evidence,
    pub fixes: Vec<Fix>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Findings {
    items: Vec<Finding>,
}

impl Findings {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, finding: Finding) {
        self.items.push(finding);
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The findings in report order.
    pub fn into_sorted(mut self) -> Vec<Finding> {
        self.items
            .sort_by(|a, b| (a.span, a.id, &a.message).cmp(&(b.span, b.id, &b.message)));
        self.items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finding(id: &'static str, start: u32) -> Finding {
        Finding {
            id: RuleId(id),
            span: Span::new(start, start + 1),
            level: Level::Mir,
            severity: Severity::Hint,
            message: "m".to_string(),
            evidence: Evidence::Unknown,
            fixes: vec![],
        }
    }

    #[test]
    fn sorts_by_span_then_id() {
        let mut f = Findings::new();
        f.push(finding("b.rule", 5));
        f.push(finding("z.rule", 1));
        f.push(finding("a.rule", 5));
        let sorted = f.into_sorted();
        let ids: Vec<&str> = sorted.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, vec!["z.rule", "a.rule", "b.rule"]);
    }
}

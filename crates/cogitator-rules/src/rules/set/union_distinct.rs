//! `set.union_distinct`: UNION where nothing suggests deduplication is
//! needed (DataFusion `replace_distinct_aggregate.rs`, `optimize_unions.rs`).
//!
//! Silent when two arms read the same relation: duplicates across them
//! are then likely, and the UNION may be doing real work. The fix to
//! UNION ALL is P only when discriminator literals keep arms apart *and*
//! each arm has no duplicates of its own, since UNION also dedupes within
//! one arm; everything else is N.

use std::collections::BTreeSet;

use cogitator_ir::common::{Lit, SetOpKind};
use cogitator_ir::hir::{Body, Hir, QueryId};
use cogitator_ir::mir::{HirNode, Mir, RelId, RelOp, ScalarKind, TableRef};
use cogitator_ir::origin::SourceOrigin;
use cogitator_ir::{Level, RuleId, Span};

use crate::cx::Cx;
use crate::findings::{Evidence, Finding, Findings, Fix, FixEdit};
use crate::meta::{FixSafety, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};
use crate::rules::join::support::{hir_ref, is_closed, is_one_row, no_opaque};
use crate::rules::join::text::{is_word_ci, match_case, slice, word_before};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("set.union_distinct"),
    level: RuleLevel::Mir,
    class: RuleClass::Advice,
    needs: Needs::NONE,
    severity: Severity::Info,
    fix: FixSafety::Mixed,
};

pub struct UnionDistinct;

impl Rule for UnionDistinct {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Mir> for UnionDistinct {
    fn check(&self, ir: &Mir, cx: &Cx<'_>, out: &mut Findings) {
        for id in ir.reachable() {
            if let Some(finding) = examine(ir, id, cx) {
                out.push(finding);
            }
        }
    }
}

fn examine(mir: &Mir, id: RelId, cx: &Cx<'_>) -> Option<Finding> {
    let rel = mir.rels.get(id)?;
    let RelOp::SetOp {
        op: SetOpKind::Union,
        all: false,
        inputs,
        ..
    } = &rel.op
    else {
        return None;
    };
    // "This UNION dedupes" needs no column list; only an `Opaque` arm hides
    // what it reads from `shares_a_relation`.
    if inputs.iter().any(|a| !no_opaque(mir, *a)) || shares_a_relation(mir, inputs) {
        return None;
    }
    // Discriminators are found by position, which an open star shifts: the
    // P proof keeps the closed-schema requirement.
    let disjoint = inputs.iter().all(|a| is_closed(mir, *a))
        && literals_disjoint(mir, inputs)
        && inputs.iter().all(|a| duplicate_free(mir, *a));
    Some(Finding {
        id: META.id,
        span: mir.prov.span(rel.prov).unwrap_or_default(),
        level: Level::Mir,
        severity: META.severity,
        message: if disjoint {
            "This UNION removes duplicates, but its arms cannot overlap; UNION ALL gives the \
             same rows without the sort or hash step"
                .to_owned()
        } else {
            "This UNION removes duplicates; if the arms cannot overlap, UNION ALL is cheaper"
                .to_owned()
        },
        evidence: if disjoint {
            Evidence::Exact
        } else {
            Evidence::Heuristic
        },
        fixes: fix(mir, rel.prov, cx, disjoint).into_iter().collect(),
    })
}

/// Two arms reading the same base table: duplicates across them are
/// plausible, so the UNION is not shown to be unnecessary.
fn shares_a_relation(mir: &Mir, arms: &[RelId]) -> bool {
    let mut seen: Vec<&TableRef> = Vec::new();
    for arm in arms {
        let mut own: Vec<&TableRef> = Vec::new();
        for id in mir.reachable_from(&[*arm]) {
            if let Some(RelOp::Scan { table, .. }) = mir.rels.get(id).map(|r| &r.op) {
                if let Some(def) = mir.tables.get(*table) {
                    own.push(&def.target);
                }
            }
        }
        if own.iter().any(|t| seen.contains(t)) {
            return true;
        }
        seen.extend(own);
    }
    false
}

/// The `Project` at the top of an arm, under ordering and DISTINCT.
fn top_project(
    mir: &Mir,
    arm: RelId,
) -> Option<&Vec<(cogitator_ir::mir::ColId, cogitator_ir::mir::ScalarId)>> {
    let mut at = arm;
    for _ in 0..=mir.rels.len() {
        match &mir.rels.get(at)?.op {
            RelOp::Project { items, .. } => return Some(items),
            RelOp::Sort { input, .. } | RelOp::Distinct(input) => at = *input,
            _ => return None,
        }
    }
    None
}

/// True when some output position holds a pairwise distinct non-null
/// literal in every arm: rows from different arms cannot be equal.
fn literals_disjoint(mir: &Mir, arms: &[RelId]) -> bool {
    let projects: Vec<_> = arms.iter().filter_map(|a| top_project(mir, *a)).collect();
    if projects.len() != arms.len() {
        return false;
    }
    let width = projects.first().map_or(0, |p| p.len());
    (0..width).any(|i| {
        let mut seen: Vec<Lit> = Vec::new();
        projects.iter().all(|items| {
            let lit = items
                .get(i)
                .and_then(|(_, s)| match mir.scalars.get(*s).map(|n| &n.kind) {
                    Some(ScalarKind::Lit(lit)) => Some(*lit),
                    _ => None,
                });
            match lit {
                Some(lit) if distinct_from_all(mir, &seen, lit) => {
                    seen.push(lit);
                    true
                }
                _ => false,
            }
        })
    })
}

fn distinct_from_all(mir: &Mir, seen: &[Lit], lit: Lit) -> bool {
    match lit {
        Lit::Str(_) | Lit::Number(_) | Lit::Bool(_) => {}
        _ => return false,
    }
    seen.iter().all(|other| literals_differ(mir, *other, lit))
}

/// Provably different values. `1` and `'1'` may compare equal after a
/// coercion, so mixed kinds do not count, and numbers compare by value.
fn literals_differ(mir: &Mir, a: Lit, b: Lit) -> bool {
    match (a, b) {
        (Lit::Str(x), Lit::Str(y)) => mir.syms.text(x) != mir.syms.text(y),
        (Lit::Bool(x), Lit::Bool(y)) => x != y,
        (Lit::Number(x), Lit::Number(y)) => {
            match (
                mir.syms.text(x).parse::<f64>(),
                mir.syms.text(y).parse::<f64>(),
            ) {
                (Ok(x), Ok(y)) => x != y,
                _ => false,
            }
        }
        _ => false,
    }
}

/// True when an arm cannot produce the same row twice.
fn duplicate_free(mir: &Mir, arm: RelId) -> bool {
    let mut at = arm;
    for _ in 0..=mir.rels.len() {
        let Some(rel) = mir.rels.get(at) else {
            return false;
        };
        match &rel.op {
            RelOp::Sort { input, .. } => at = *input,
            RelOp::Distinct(_) => return true,
            RelOp::Aggregate { sets, .. } => return sets.is_none(),
            RelOp::Project { input, items } => {
                return is_one_row(mir, *input) || keeps_every_group_key(mir, *input, items);
            }
            _ => return false,
        }
    }
    false
}

/// A Project over a GROUP BY that keeps every grouping key is one row
/// per group, so no two of its rows are equal.
fn keeps_every_group_key(
    mir: &Mir,
    input: RelId,
    items: &[(cogitator_ir::mir::ColId, cogitator_ir::mir::ScalarId)],
) -> bool {
    let Some(RelOp::Aggregate {
        keys, sets: None, ..
    }) = mir.rels.get(input).map(|r| &r.op)
    else {
        return false;
    };
    let kept: BTreeSet<_> = items
        .iter()
        .filter_map(|(_, s)| match mir.scalars.get(*s).map(|n| &n.kind) {
            Some(ScalarKind::Col(c)) => Some(*c),
            _ => None,
        })
        .collect();
    !keys.is_empty() && keys.iter().all(|(col, _)| kept.contains(col))
}

fn fix(mir: &Mir, prov: cogitator_ir::mir::ProvId, cx: &Cx<'_>, disjoint: bool) -> Option<Fix> {
    let (hir, text) = (cx.hir?, cx.text?);
    if !matches!(hir.origin, SourceOrigin::Template { .. }) {
        return None;
    }
    let HirNode::Query(q) = hir_ref(mir, prov)?.node else {
        return None;
    };
    let edits = union_keyword_edits(hir, text, q)?;
    Some(if disjoint {
        Fix {
            title: "Change to UNION ALL (the arms cannot overlap)".to_owned(),
            safety: FixSafety::Preserving,
            edits,
        }
    } else {
        Fix {
            title: "Change to UNION ALL (keeps any duplicates)".to_owned(),
            safety: FixSafety::ChangesIntent,
            edits,
        }
    })
}

/// One edit per `UNION` in this query's chain of plain UNIONs. Withheld
/// entirely unless every keyword is where the text says it is: a partial
/// conversion would change the bag differently from what is offered.
fn union_keyword_edits(hir: &Hir, text: &str, q: QueryId) -> Option<Vec<FixEdit>> {
    let mut right_arms = Vec::new();
    let mut at = q;
    let mut budget = hir.queries.len() + 1;
    while let Body::SetOp {
        op: SetOpKind::Union,
        all: false,
        by_name: false,
        l,
        r,
    } = &hir.queries.get(at)?.body
    {
        if budget == 0 {
            return None;
        }
        budget -= 1;
        right_arms.push(*r);
        at = *l;
    }
    if right_arms.is_empty() {
        return None;
    }
    right_arms.reverse();
    right_arms
        .iter()
        .map(|arm| {
            let start = hir.queries.get(*arm)?.span.start;
            union_edit(text, start)
        })
        .collect()
}

/// Inserts `ALL` after the `UNION` before `arm_start`. A preceding
/// `DISTINCT` is the BigQuery spelling and is replaced outright.
fn union_edit(text: &str, arm_start: u32) -> Option<FixEdit> {
    let word = word_before(text, arm_start)?;
    if is_word_ci(text, word, "distinct") {
        let keyword = word_before(text, word.start)?;
        if !is_word_ci(text, keyword, "union") {
            return None;
        }
        return Some(FixEdit {
            span: word,
            new_text: match_case(slice(text, keyword)?, "ALL"),
        });
    }
    if !is_word_ci(text, word, "union") {
        return None;
    }
    Some(FixEdit {
        span: Span::new(word.end, word.end),
        new_text: format!(" {}", match_case(slice(text, word)?, "ALL")),
    })
}

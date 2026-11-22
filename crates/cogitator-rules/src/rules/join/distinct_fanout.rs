//! `join.distinct_fanout`: DISTINCT over a join on a key not known to be
//! unique (DuckDB `join_elimination.cpp`, DataFusion
//! `replace_distinct_aggregate.rs`).
//!
//! `unique` tests are optional evidence, so `Need::Tests` is not declared:
//! a project without tests still gets the Hint. Tests come from YAML through
//! `ProjectSchemas`; the finding names them with `Evidence::Tested`.

use std::collections::BTreeSet;

use cogitator_ir::common::{BinaryOp, JoinKind};
use cogitator_ir::mir::{ColId, Mir, RelId, RelOp, ScalarKind, TableRef};
use cogitator_ir::{Level, RuleId};

use super::support::{is_closed, reads};
use crate::cx::Cx;
use crate::findings::{Evidence, Finding, Findings};
use crate::meta::{FixSafety, Need, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("join.distinct_fanout"),
    level: RuleLevel::Mir,
    class: RuleClass::Advice,
    needs: Needs::NONE,
    // Info is the most this reports, and only with a `unique` test behind it.
    severity: Severity::Info,
    fix: FixSafety::None,
};

pub struct DistinctFanout;

impl Rule for DistinctFanout {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Mir> for DistinctFanout {
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
    let RelOp::Distinct(input) = &rel.op else {
        return None;
    };
    let (join, kept) = join_below(mir, *input, &rel.out)?;
    let RelOp::Join { kind, l, r, on } = &mir.rels.get(join)?.op else {
        return None;
    };
    if matches!(kind, JoinKind::Semi | JoinKind::Anti | JoinKind::Cross)
        || !is_closed(mir, *l)
        || !is_closed(mir, *r)
    {
        return None;
    }
    let (left, right) = (columns(mir, *l)?, columns(mir, *r)?);
    // A DISTINCT over columns of both sides keeps distinct pairs on purpose.
    if !(kept.is_subset(&left) || kept.is_subset(&right)) || kept.is_empty() {
        return None;
    }
    let keys = equi_keys(mir, on, &left, &right);
    if keys.is_empty() {
        return None;
    }
    let (severity, evidence, message) = verdict(mir, cx, *l, *r, &keys)?;
    Some(Finding {
        id: META.id,
        span: mir.prov.span(rel.prov).unwrap_or_default(),
        level: Level::Mir,
        severity,
        message,
        evidence,
        fixes: Vec::new(),
    })
}

type Verdict = (Severity, Evidence, String);

fn verdict(mir: &Mir, cx: &Cx<'_>, l: RelId, r: RelId, keys: &[(ColId, ColId)]) -> Option<Verdict> {
    let unique = |rel: RelId, side: fn(&(ColId, ColId)) -> ColId| {
        keys.iter().any(|k| column_is_unique(mir, cx, rel, side(k)))
    };
    let left_unique = unique(l, |k| k.0);
    let right_unique = unique(r, |k| k.1);
    match (left_unique, right_unique) {
        // Both keys unique: the join cannot fan out at all.
        (true, true) => None,
        (false, false) => Some((
            Severity::Hint,
            Evidence::Heuristic,
            "DISTINCT over this join may hide duplicate rows from join fan-out".to_owned(),
        )),
        _ => {
            let schema_revision = cx.schemas.map_or(0, |s| s.revision);
            Some((
                Severity::Info,
                Evidence::Tested { schema_revision },
                "DISTINCT over this join hides duplicate rows: a `unique` test covers only one \
                 side of the key"
                    .to_owned(),
            ))
        }
    }
}

/// The join under a DISTINCT, through Project, Filter and Sort, with the
/// join-output columns the DISTINCT's own columns are made from.
fn join_below(mir: &Mir, input: RelId, out: &[ColId]) -> Option<(RelId, BTreeSet<ColId>)> {
    let mut kept: BTreeSet<ColId> = out.iter().copied().collect();
    let mut at = input;
    for _ in 0..=mir.rels.len() {
        match &mir.rels.get(at)?.op {
            RelOp::Project { input, items } => {
                let mut next = BTreeSet::new();
                for (col, scalar) in items {
                    if kept.contains(col) {
                        next.extend(reads(mir, *scalar).cols);
                    }
                }
                kept = next;
                at = *input;
            }
            RelOp::Filter { input, .. } | RelOp::Sort { input, .. } => at = *input,
            RelOp::Join { .. } => return Some((at, kept)),
            _ => return None,
        }
    }
    None
}

fn columns(mir: &Mir, rel: RelId) -> Option<BTreeSet<ColId>> {
    Some(mir.rels.get(rel)?.out.iter().copied().collect())
}

/// `(left column, right column)` for each `a = b` tying the two sides.
fn equi_keys(
    mir: &Mir,
    on: &[cogitator_ir::mir::ScalarId],
    left: &BTreeSet<ColId>,
    right: &BTreeSet<ColId>,
) -> Vec<(ColId, ColId)> {
    let col = |id| match mir.scalars.get(id).map(|s| &s.kind) {
        Some(ScalarKind::Col(c)) => Some(*c),
        _ => None,
    };
    on.iter()
        .filter_map(|s| match mir.scalars.get(*s).map(|n| &n.kind) {
            Some(ScalarKind::Binary {
                op: BinaryOp::Eq,
                l,
                r,
            }) => {
                let (a, b) = (col(*l)?, col(*r)?);
                if left.contains(&a) && right.contains(&b) {
                    Some((a, b))
                } else if left.contains(&b) && right.contains(&a) {
                    Some((b, a))
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect()
}

/// True when `col` is a scan column with a `unique` test behind it. Only
/// a test counts as knowledge here, never the shape of the data.
fn column_is_unique(mir: &Mir, cx: &Cx<'_>, rel: RelId, col: ColId) -> bool {
    if !cx.has(Need::Tests) {
        return false;
    }
    let Some(schemas) = cx.schemas else {
        return false;
    };
    let Some((table, name)) = scan_of(mir, rel, col) else {
        return false;
    };
    let schema = match table {
        TableRef::Model { name, .. } => schemas.model(mir.syms.text(*name)),
        TableRef::Source { source, table } => {
            schemas.source(mir.syms.text(*source), mir.syms.text(*table))
        }
        _ => None,
    };
    schema.is_some_and(|s| {
        s.columns
            .iter()
            .any(|c| c.unique && c.name.eq_ignore_ascii_case(mir.syms.text(name)))
    })
}

/// The scan defining `col`, walking single-input operators and into the
/// join input that carries it. A renaming Project ends the walk: a
/// `unique` test on the source column says nothing about a new one.
fn scan_of(mir: &Mir, rel: RelId, col: ColId) -> Option<(&TableRef, cogitator_ir::Symbol)> {
    let mut at = rel;
    for _ in 0..=mir.rels.len() {
        match &mir.rels.get(at)?.op {
            RelOp::Scan { table, cols, .. } => {
                let name = mir.cols.get(col)?.name?;
                return cols
                    .contains(&col)
                    .then(|| Some((&mir.tables.get(*table)?.target, name)))?;
            }
            RelOp::Project { input, items } => {
                let (_, scalar) = items.iter().find(|(c, _)| *c == col)?;
                match mir.scalars.get(*scalar).map(|s| &s.kind) {
                    Some(ScalarKind::Col(c)) if *c == col => at = *input,
                    _ => return None,
                }
            }
            RelOp::Filter { input, .. }
            | RelOp::Sort { input, .. }
            | RelOp::Limit { input, .. }
            | RelOp::Distinct(input) => at = *input,
            RelOp::Join { l, r, .. } => {
                let in_left = mir.rels.get(*l)?.out.contains(&col);
                at = if in_left { *l } else { *r };
            }
            _ => return None,
        }
    }
    None
}

//! `join.redundant_left`: a LEFT JOIN that can be dropped, because its
//! right key is proven unique by a strict dbt test and none of its right
//! columns are read anywhere above the join (DuckDB `join_elimination.cpp`).
//!
//! At most one right row can match, so it cannot add rows; the right
//! columns are unused, so it cannot add data. An opaque node or scalar
//! anywhere in the plan could read those columns, so it silences the rule.

use std::collections::BTreeSet;

use cogitator_ir::common::{BinaryOp, JoinKind};
use cogitator_ir::mir::{ColId, Mir, RelId, RelOp, ScalarId, ScalarKind, TableRef};
use cogitator_ir::{Level, RuleId, Span};

use super::support::{is_singleton, join_from};
use crate::cx::Cx;
use crate::findings::{Evidence, Finding, Findings};
use crate::meta::{FixSafety, Need, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};
use crate::rules::subq::scan::touches_opaque;

pub static META: RuleMeta = RuleMeta {
    id: RuleId("join.redundant_left"),
    level: RuleLevel::Mir,
    class: RuleClass::Advice,
    // a Jinja branch not taken can hide a read of a right column
    needs: Needs::NONE.with(Need::Tests).with(Need::AllBranches),
    severity: Severity::Info,
    fix: FixSafety::None,
};

pub struct RedundantLeft;

impl Rule for RedundantLeft {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Mir> for RedundantLeft {
    fn check(&self, ir: &Mir, cx: &Cx<'_>, out: &mut Findings) {
        if cx.schemas.is_none() || !plan_is_transparent(ir) {
            return;
        }
        for id in ir.reachable() {
            if let Some(finding) = examine(ir, id, cx) {
                out.push(finding);
            }
        }
    }
}

/// False when any reachable node could read a column the plan does not
/// show: an `Opaque` relation or scalar, or an open scan.
fn plan_is_transparent(mir: &Mir) -> bool {
    mir.reachable().iter().all(|id| {
        let Some(rel) = mir.rels.get(*id) else {
            return false;
        };
        let scalars_ok = !rel.op.scalars().iter().any(|s| touches_opaque(mir, *s));
        match &rel.op {
            RelOp::Scan { open, .. } => !*open && scalars_ok,
            op @ RelOp::Opaque { .. } => is_singleton(op),
            _ => scalars_ok,
        }
    })
}

fn examine(mir: &Mir, id: RelId, cx: &Cx<'_>) -> Option<Finding> {
    let rel = mir.rels.get(id)?;
    let RelOp::Join {
        kind: JoinKind::Left,
        l,
        r,
        on,
    } = &rel.op
    else {
        return None;
    };
    let left: BTreeSet<ColId> = mir.rels.get(*l)?.out.iter().copied().collect();
    let right: BTreeSet<ColId> = mir.rels.get(*r)?.out.iter().copied().collect();
    let key = unique_right_key(mir, cx, on, *r, &left, &right)?;
    if right_is_read(mir, id, *r, &right) {
        return None;
    }
    let span = right_span(mir, rel.prov, cx)
        .unwrap_or_else(|| mir.prov.span(rel.prov).unwrap_or_default());
    let schema_revision = cx.schemas.map_or(0, |s| s.revision);
    Some(Finding {
        id: META.id,
        span,
        level: Level::Mir,
        severity: META.severity,
        message: format!(
            "This LEFT JOIN cannot add rows (`{key}` has a strict `unique` test) and none of \
             its columns are used, so it can be removed"
        ),
        evidence: Evidence::Tested { schema_revision },
        fixes: Vec::new(),
    })
}

/// The name of a right join key with a strict `unique` test, if one of
/// the ON equalities ties such a column to the left input.
fn unique_right_key(
    mir: &Mir,
    cx: &Cx<'_>,
    on: &[ScalarId],
    right_rel: RelId,
    left: &BTreeSet<ColId>,
    right: &BTreeSet<ColId>,
) -> Option<String> {
    let col = |id: ScalarId| match mir.scalars.get(id).map(|s| &s.kind) {
        Some(ScalarKind::Col(c)) => Some(*c),
        _ => None,
    };
    on.iter().find_map(|s| {
        let ScalarKind::Binary {
            op: BinaryOp::Eq,
            l,
            r,
        } = &mir.scalars.get(*s)?.kind
        else {
            return None;
        };
        let (a, b) = (col(*l)?, col(*r)?);
        let key = match (left.contains(&a), right.contains(&b)) {
            (true, true) => b,
            _ if left.contains(&b) && right.contains(&a) => a,
            _ => return None,
        };
        tested_unique(mir, cx, right_rel, key)
    })
}

/// The column's name when it reaches a scan with a strict `unique` test
/// along a chain that cannot duplicate rows.
fn tested_unique(mir: &Mir, cx: &Cx<'_>, rel: RelId, col: ColId) -> Option<String> {
    let schemas = cx.schemas?;
    let (mut at, mut col) = (rel, col);
    for _ in 0..=mir.rels.len() {
        match &mir.rels.get(at)?.op {
            RelOp::Scan { table, cols, .. } => {
                let name = mir.syms.text(mir.cols.get(col)?.name?);
                if !cols.contains(&col) {
                    return None;
                }
                let schema = match &mir.tables.get(*table)?.target {
                    TableRef::Model { name, .. } => schemas.model(mir.syms.text(*name)),
                    TableRef::Source { source, table } => {
                        schemas.source(mir.syms.text(*source), mir.syms.text(*table))
                    }
                    _ => None,
                }?;
                let found = schema
                    .columns
                    .iter()
                    .any(|c| c.unique && c.name.eq_ignore_ascii_case(name));
                return found.then(|| name.to_owned());
            }
            RelOp::Project { input, items } => {
                let (_, scalar) = items.iter().find(|(c, _)| *c == col)?;
                match mir.scalars.get(*scalar).map(|s| &s.kind) {
                    Some(ScalarKind::Col(c)) => col = *c,
                    _ => return None,
                }
                at = *input;
            }
            RelOp::CteRef { cte, out } => {
                let index = out.iter().position(|c| *c == col)?;
                let def = mir.ctes.get(*cte)?;
                col = *def.out.get(index)?;
                at = def.body;
            }
            RelOp::Filter { input, .. }
            | RelOp::Sort { input, .. }
            | RelOp::Limit { input, .. }
            | RelOp::Distinct(input) => at = *input,
            _ => return None,
        }
    }
    None
}

/// True when anything above `join` reads a column of `right`. The right
/// input's own predicates and the ON clause are exempt: they decide which
/// row matches, and at most one can.
fn right_is_read(mir: &Mir, join: RelId, right_rel: RelId, right: &BTreeSet<ColId>) -> bool {
    let inside: BTreeSet<RelId> = mir.reachable_from(&[right_rel]).into_iter().collect();
    mir.reachable().iter().any(|id| {
        if *id == join || inside.contains(id) {
            return false;
        }
        mir.rels
            .get(*id)
            .is_none_or(|rel| rel.op.scalars().iter().any(|s| reads_any(mir, *s, right)))
    })
}

fn reads_any(mir: &Mir, root: ScalarId, cols: &BTreeSet<ColId>) -> bool {
    let mut stack = vec![root];
    let mut seen = BTreeSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(node) = mir.scalars.get(id) else {
            return true;
        };
        match &node.kind {
            ScalarKind::Col(c) | ScalarKind::Outer(c) if cols.contains(c) => return true,
            ScalarKind::Subquery { correlated, .. }
                if correlated.iter().any(|c| cols.contains(c)) =>
            {
                return true;
            }
            _ => stack.extend(node.kind.children()),
        }
    }
    false
}

/// The joined item's authored span, when lowering kept the FROM id.
fn right_span(mir: &Mir, prov: cogitator_ir::mir::ProvId, cx: &Cx<'_>) -> Option<Span> {
    let hir = cx.hir?;
    let from = join_from(mir, prov)?;
    match &hir.from.get(from)?.kind {
        cogitator_ir::hir::FromKind::Join { r, .. } => Some(hir.from.get(*r)?.span),
        _ => None,
    }
}

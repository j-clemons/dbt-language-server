//! `join.key_type_mismatch`: join keys in incompatible type families
//! (DataFusion `analyzer/`).
//!
//! Implicit coercion means the query usually runs, so this is worded as a
//! likely pruning or correctness risk. INT against DECIMAL is the same
//! family, and anything `Unknown` is silent.

use cogitator_ir::common::BinaryOp;
use cogitator_ir::mir::{Mir, RelId, RelOp, ScalarId, ScalarKind};
use cogitator_ir::{Level, RuleId, SqlType};

use crate::cx::Cx;
use crate::findings::{Evidence, Finding, Findings};
use crate::meta::{FixSafety, Need, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("join.key_type_mismatch"),
    level: RuleLevel::Mir,
    class: RuleClass::Advice,
    needs: Needs::NONE.with(Need::Types),
    severity: Severity::Info,
    fix: FixSafety::None,
};

pub struct KeyTypeMismatch;

impl Rule for KeyTypeMismatch {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Mir> for KeyTypeMismatch {
    fn check(&self, ir: &Mir, cx: &Cx<'_>, out: &mut Findings) {
        for id in ir.reachable() {
            examine(ir, id, cx, out);
        }
    }
}

fn examine(mir: &Mir, id: RelId, cx: &Cx<'_>, out: &mut Findings) {
    let Some(RelOp::Join { on, .. }) = mir.rels.get(id).map(|r| &r.op) else {
        return;
    };
    for conjunct in on {
        if let Some(finding) = check_key(mir, *conjunct, cx) {
            out.push(finding);
        }
    }
}

fn check_key(mir: &Mir, id: ScalarId, cx: &Cx<'_>) -> Option<Finding> {
    let scalar = mir.scalars.get(id)?;
    let ScalarKind::Binary {
        op: BinaryOp::Eq,
        l,
        r,
    } = &scalar.kind
    else {
        return None;
    };
    let (left, left_ty) = column_type(mir, *l)?;
    let (right, right_ty) = column_type(mir, *r)?;
    if left == right {
        return None;
    }
    let (a, b) = (family(left_ty)?, family(right_ty)?);
    if a == b {
        return None;
    }
    Some(Finding {
        id: META.id,
        span: mir.prov.span(scalar.prov).unwrap_or_default(),
        level: Level::Mir,
        severity: META.severity,
        message: format!(
            "These join keys compare {} with {}: the implicit cast can defeat pruning or \
             misjoin values, so cast one side explicitly if this is intended",
            a.label(),
            b.label()
        ),
        evidence: type_evidence(cx),
        fixes: Vec::new(),
    })
}

/// Column types come from the catalog when one was read, else from YAML.
fn type_evidence(cx: &Cx<'_>) -> Evidence {
    match cx.dag.and_then(|d| d.catalog_generated_at.clone()) {
        Some(generated_at) => Evidence::Catalog { generated_at },
        None => Evidence::Exact,
    }
}

/// A column reference and its declared base type.
fn column_type(mir: &Mir, id: ScalarId) -> Option<(cogitator_ir::mir::ColId, &SqlType)> {
    let scalar = mir.scalars.get(id)?;
    match scalar.kind {
        ScalarKind::Col(col) => Some((col, &mir.cols.get(col)?.ty.ty)),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Number,
    Text,
    Bool,
    Temporal,
}

impl Family {
    fn label(self) -> &'static str {
        match self {
            Family::Number => "a number",
            Family::Text => "text",
            Family::Bool => "a boolean",
            Family::Temporal => "a date or time",
        }
    }
}

/// The family of a type, or `None` when comparing it would prove nothing.
fn family(ty: &SqlType) -> Option<Family> {
    Some(match ty {
        SqlType::Int { .. } | SqlType::Decimal { .. } | SqlType::Float { .. } => Family::Number,
        SqlType::Text => Family::Text,
        SqlType::Bool => Family::Bool,
        SqlType::Date | SqlType::Time | SqlType::Timestamp { .. } => Family::Temporal,
        _ => return None,
    })
}

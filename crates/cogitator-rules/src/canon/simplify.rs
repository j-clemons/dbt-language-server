//! `canon.simplify`: constant folding and boolean simplification under
//! SQL three-valued logic.
//!
//! Upstream: DuckDB `rule/constant_folding.cpp`, DataFusion
//! `simplify_expressions/` (pinned in specs/ir-pipeline/05).
//!
//! `x AND NULL` is not false: it is NULL when `x` is true, so it folds
//! only when `x` is a constant. Number literals are compared as decimal
//! text and only ever compared, never computed. Terms that would be
//! discarded must not hold a subquery or opaque hole, whose evaluation
//! the plan cannot speak for.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use cogitator_ir::common::{BinaryOp, Lit, UnaryOp};
use cogitator_ir::mir::{Mir, RelId, RelOp, ScalarId, ScalarKind};
use cogitator_ir::{RuleId, SqlType};

use super::rebuild::{self, Fold, Rebuilder};
use crate::cx::Cx;
use crate::meta::{FixSafety, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Canonicalize, Rewrite, Rule};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("canon.simplify"),
    level: RuleLevel::Mir,
    class: RuleClass::Canon,
    needs: Needs::NONE,
    severity: Severity::Off,
    fix: FixSafety::None,
};

/// Bounds the discard check on deep or shared terms; past it the term is
/// simply not discarded.
const MAX_DISCARD_WALK: usize = 1024;

pub struct Simplify;

impl Rule for Simplify {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Canonicalize for Simplify {
    fn apply(&self, ir: Mir, _cx: &Cx<'_>) -> Rewrite<Mir> {
        rebuild::run(ir, META.id, fold, step)
    }
}

/// Drops conjuncts that simplified to `true`, and a Filter left with
/// none. `false` and NULL conjuncts stay: they say the filter rejects
/// every row, which advice rules want to see.
fn step(rb: &mut Rebuilder, mir: &mut Mir, old: RelId, op: RelOp) -> RelId {
    let is_true = |mir: &Mir, s: ScalarId| truth(mir, s) == Some(Tri::True);
    match op {
        RelOp::Filter { input, conjuncts } => {
            let conjuncts: Vec<ScalarId> = conjuncts
                .into_iter()
                .filter(|c| !is_true(mir, *c))
                .collect();
            if conjuncts.is_empty() {
                return input;
            }
            rb.emit(mir, old, RelOp::Filter { input, conjuncts })
        }
        RelOp::Join { kind, l, r, on } => {
            let on = on.into_iter().filter(|c| !is_true(mir, *c)).collect();
            rb.emit(mir, old, RelOp::Join { kind, l, r, on })
        }
        op => rb.emit(mir, old, op),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tri {
    True,
    False,
    Null,
}

impl Tri {
    fn from_bool(b: bool) -> Self {
        if b {
            Tri::True
        } else {
            Tri::False
        }
    }

    fn lit(self) -> Lit {
        match self {
            Tri::True => Lit::Bool(true),
            Tri::False => Lit::Bool(false),
            Tri::Null => Lit::Null,
        }
    }

    fn and(self, other: Tri) -> Tri {
        match (self, other) {
            (Tri::False, _) | (_, Tri::False) => Tri::False,
            (Tri::Null, _) | (_, Tri::Null) => Tri::Null,
            (Tri::True, Tri::True) => Tri::True,
        }
    }

    fn or(self, other: Tri) -> Tri {
        match (self, other) {
            (Tri::True, _) | (_, Tri::True) => Tri::True,
            (Tri::Null, _) | (_, Tri::Null) => Tri::Null,
            (Tri::False, Tri::False) => Tri::False,
        }
    }

    fn not(self) -> Tri {
        match self {
            Tri::True => Tri::False,
            Tri::False => Tri::True,
            Tri::Null => Tri::Null,
        }
    }
}

fn kind(mir: &Mir, id: ScalarId) -> Option<&ScalarKind> {
    mir.scalars.get(id).map(|s| &s.kind)
}

fn literal(mir: &Mir, id: ScalarId) -> Option<Lit> {
    match kind(mir, id) {
        Some(ScalarKind::Lit(lit)) => Some(*lit),
        _ => None,
    }
}

fn truth(mir: &Mir, id: ScalarId) -> Option<Tri> {
    match literal(mir, id)? {
        Lit::Bool(b) => Some(Tri::from_bool(b)),
        Lit::Null => Some(Tri::Null),
        _ => None,
    }
}

/// Whether `id` can stand in for a boolean expression containing it.
/// `Unknown` counts: lowering could not type it, and a non-boolean in a
/// boolean position is rejected or coerced by the engine either way.
fn boolish(mir: &Mir, id: ScalarId) -> bool {
    mir.scalars
        .get(id)
        .is_some_and(|s| matches!(s.ty.ty, SqlType::Bool | SqlType::Unknown | SqlType::Null))
}

/// Whether the term may be dropped when its value cannot matter.
fn discardable(mir: &Mir, root: ScalarId) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        if seen.len() > MAX_DISCARD_WALK {
            return false;
        }
        match kind(mir, id) {
            None | Some(ScalarKind::Subquery { .. } | ScalarKind::Opaque { .. }) => return false,
            Some(node) => stack.extend(node.children()),
        }
    }
    true
}

fn fold(mir: &Mir, node: ScalarKind) -> Fold {
    match node {
        ScalarKind::Unary { op, e } => fold_unary(mir, op, e),
        ScalarKind::Binary { op, l, r } => fold_binary(mir, op, l, r),
        other => Fold::Keep(other),
    }
}

fn fold_unary(mir: &Mir, op: UnaryOp, e: ScalarId) -> Fold {
    let keep = Fold::Keep(ScalarKind::Unary { op, e });
    match op {
        UnaryOp::Not => {
            if let Some(t) = truth(mir, e) {
                return Fold::Lit(t.not().lit());
            }
            match kind(mir, e) {
                Some(ScalarKind::Unary {
                    op: UnaryOp::Not,
                    e: inner,
                }) if boolish(mir, *inner) => Fold::Replace(*inner),
                _ => keep,
            }
        }
        UnaryOp::IsNull | UnaryOp::IsNotNull => match literal(mir, e) {
            Some(lit) => {
                let null = lit == Lit::Null;
                Fold::Lit(Lit::Bool(null == (op == UnaryOp::IsNull)))
            }
            None => keep,
        },
        UnaryOp::IsTrue | UnaryOp::IsFalse => match truth(mir, e) {
            Some(t) => {
                let wanted = if op == UnaryOp::IsTrue {
                    Tri::True
                } else {
                    Tri::False
                };
                Fold::Lit(Lit::Bool(t == wanted))
            }
            None => keep,
        },
        UnaryOp::Neg | UnaryOp::Other(_) => keep,
    }
}

fn fold_binary(mir: &Mir, op: BinaryOp, l: ScalarId, r: ScalarId) -> Fold {
    let keep = Fold::Keep(ScalarKind::Binary { op, l, r });
    match op {
        BinaryOp::And => fold_logical(mir, l, r, Tri::False, Tri::and).unwrap_or(keep),
        BinaryOp::Or => fold_logical(mir, l, r, Tri::True, Tri::or).unwrap_or(keep),
        BinaryOp::Eq
        | BinaryOp::NotEq
        | BinaryOp::Lt
        | BinaryOp::LtEq
        | BinaryOp::Gt
        | BinaryOp::GtEq => fold_comparison(mir, op, l, r).unwrap_or(keep),
        _ => keep,
    }
}

/// `absorbing` is false for AND and true for OR. Only the absorbing
/// value decides the result alone, and the identity is its opposite.
/// NULL is neither, which is what keeps `x AND NULL` intact.
fn fold_logical(
    mir: &Mir,
    l: ScalarId,
    r: ScalarId,
    absorbing: Tri,
    combine: fn(Tri, Tri) -> Tri,
) -> Option<Fold> {
    let identity = absorbing.not();
    match (truth(mir, l), truth(mir, r)) {
        (Some(a), Some(b)) => Some(Fold::Lit(combine(a, b).lit())),
        (Some(t), None) if t == identity && boolish(mir, r) => Some(Fold::Replace(r)),
        (None, Some(t)) if t == identity && boolish(mir, l) => Some(Fold::Replace(l)),
        (Some(t), None) if t == absorbing && discardable(mir, r) => Some(Fold::Lit(t.lit())),
        (None, Some(t)) if t == absorbing && discardable(mir, l) => Some(Fold::Lit(t.lit())),
        _ => None,
    }
}

fn fold_comparison(mir: &Mir, op: BinaryOp, l: ScalarId, r: ScalarId) -> Option<Fold> {
    let (a, b) = (literal(mir, l), literal(mir, r));
    let null = a == Some(Lit::Null) || b == Some(Lit::Null);
    if null && discardable(mir, l) && discardable(mir, r) {
        return Some(Fold::Lit(Lit::Null));
    }
    let ordering = match (a?, b?) {
        (Lit::Number(x), Lit::Number(y)) => {
            compare_decimal(mir.syms.resolve(x)?, mir.syms.resolve(y)?)?
        }
        (Lit::Bool(x), Lit::Bool(y)) if matches!(op, BinaryOp::Eq | BinaryOp::NotEq) => x.cmp(&y),
        // identical text is equal under any collation; different text
        // may still be equal under one, so it stays unfolded
        (Lit::Str(x), Lit::Str(y)) if x == y && matches!(op, BinaryOp::Eq | BinaryOp::NotEq) => {
            Ordering::Equal
        }
        _ => return None,
    };
    Some(Fold::Lit(Lit::Bool(match op {
        BinaryOp::Eq => ordering == Ordering::Equal,
        BinaryOp::NotEq => ordering != Ordering::Equal,
        BinaryOp::Lt => ordering == Ordering::Less,
        BinaryOp::LtEq => ordering != Ordering::Greater,
        BinaryOp::Gt => ordering == Ordering::Greater,
        BinaryOp::GtEq => ordering != Ordering::Less,
        _ => return None,
    })))
}

/// Compares unsigned decimal text exactly. `None` for anything else
/// (exponents, hex), which stays unfolded.
fn compare_decimal(a: &str, b: &str) -> Option<Ordering> {
    let (int_a, frac_a) = split_decimal(a)?;
    let (int_b, frac_b) = split_decimal(b)?;
    Some(
        int_a
            .len()
            .cmp(&int_b.len())
            .then_with(|| int_a.cmp(int_b))
            .then_with(|| frac_a.cmp(frac_b)),
    )
}

fn split_decimal(text: &str) -> Option<(&str, &str)> {
    let (int, frac) = text.split_once('.').unwrap_or((text, ""));
    let digits = |s: &str| s.bytes().all(|b| b.is_ascii_digit());
    if !digits(int) || !digits(frac) || (int.is_empty() && frac.is_empty()) {
        return None;
    }
    Some((int.trim_start_matches('0'), frac.trim_end_matches('0')))
}

//! Three-valued evaluation of a predicate with one join input all NULL.
//!
//! A predicate null-rejects that input when it cannot be TRUE once the
//! input's columns are NULL. Anything the analysis does not understand
//! is `Any`, which never rejects: an unknown function may turn NULL into
//! a value, and so may `coalesce`.

use std::collections::{BTreeMap, BTreeSet};

use cogitator_ir::common::{BinaryOp, Lit, UnaryOp};
use cogitator_ir::mir::{ColId, Mir, ScalarId, ScalarKind, SubqueryKind};

/// Depth limit on the walk; deeper trees count as unknown.
const MAX_DEPTH: u32 = 512;

/// Functions that return NULL for any NULL argument on every dialect
/// we target. `concat` is excluded: DuckDB and Postgres skip NULLs.
const STRICT: &[&str] = &[
    "abs",
    "ceil",
    "ceiling",
    "date",
    "date_trunc",
    "day",
    "floor",
    "length",
    "lower",
    "ltrim",
    "month",
    "replace",
    "round",
    "rtrim",
    "substr",
    "substring",
    "trim",
    "upper",
    "year",
];

/// What a scalar can evaluate to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tv {
    Null,
    False,
    True,
    /// NULL or FALSE, but never TRUE.
    NotTrue,
    Any,
}

/// True when `root` cannot be TRUE with every column in `null_side` NULL.
pub(crate) fn rejects(mir: &Mir, root: ScalarId, null_side: &BTreeSet<ColId>) -> bool {
    let mut eval = Eval {
        mir,
        null_side,
        memo: BTreeMap::new(),
    };
    matches!(eval.tv(root, 0), Tv::Null | Tv::False | Tv::NotTrue)
}

struct Eval<'a> {
    mir: &'a Mir,
    null_side: &'a BTreeSet<ColId>,
    /// Scalars are shared after substitution; the memo keeps this linear.
    memo: BTreeMap<ScalarId, Tv>,
}

impl Eval<'_> {
    fn tv(&mut self, id: ScalarId, depth: u32) -> Tv {
        if depth > MAX_DEPTH {
            return Tv::Any;
        }
        if let Some(known) = self.memo.get(&id) {
            return *known;
        }
        let Some(kind) = self.mir.scalars.get(id).map(|s| &s.kind) else {
            return Tv::Any;
        };
        let value = self.kind(kind, depth + 1);
        self.memo.insert(id, value);
        value
    }

    fn kind(&mut self, kind: &ScalarKind, depth: u32) -> Tv {
        match kind {
            ScalarKind::Col(c) if self.null_side.contains(c) => Tv::Null,
            ScalarKind::Lit(Lit::Null) => Tv::Null,
            ScalarKind::Lit(Lit::Bool(true)) => Tv::True,
            ScalarKind::Lit(Lit::Bool(false)) => Tv::False,
            ScalarKind::Unary { op, e } => {
                let v = self.tv(*e, depth);
                unary(*op, v)
            }
            ScalarKind::Binary { op, l, r } => {
                let (a, b) = (self.tv(*l, depth), self.tv(*r, depth));
                binary(*op, a, b)
            }
            ScalarKind::Call { name, args, .. } => {
                let text = self.mir.syms.text(*name);
                let strict = STRICT.iter().any(|f| f.eq_ignore_ascii_case(text));
                let any_null = args.iter().any(|a| self.tv(*a, depth) == Tv::Null);
                if strict && any_null {
                    Tv::Null
                } else {
                    Tv::Any
                }
            }
            ScalarKind::Cast { e, .. } => match self.tv(*e, depth) {
                Tv::Null => Tv::Null,
                _ => Tv::Any,
            },
            ScalarKind::InList { e, .. } => match self.tv(*e, depth) {
                Tv::Null => Tv::Null,
                _ => Tv::Any,
            },
            // `NULL IN (subquery)` is NULL or FALSE. Its negation is TRUE
            // over an empty subquery, so only the positive form rejects.
            ScalarKind::Subquery {
                kind: SubqueryKind::In { e, negated: false },
                ..
            } => match self.tv(*e, depth) {
                Tv::Null => Tv::NotTrue,
                _ => Tv::Any,
            },
            _ => Tv::Any,
        }
    }
}

fn unary(op: UnaryOp, v: Tv) -> Tv {
    match (op, v) {
        (UnaryOp::Not, Tv::Null) | (UnaryOp::Neg, Tv::Null) => Tv::Null,
        (UnaryOp::Not, Tv::True) => Tv::False,
        (UnaryOp::Not, Tv::False) => Tv::True,
        (UnaryOp::IsNull, Tv::Null)
        | (UnaryOp::IsTrue, Tv::True)
        | (UnaryOp::IsFalse, Tv::False) => Tv::True,
        (UnaryOp::IsNotNull, Tv::Null)
        | (UnaryOp::IsTrue, Tv::Null | Tv::False | Tv::NotTrue)
        | (UnaryOp::IsFalse, Tv::Null | Tv::True) => Tv::False,
        _ => Tv::Any,
    }
}

fn binary(op: BinaryOp, a: Tv, b: Tv) -> Tv {
    match op {
        BinaryOp::And => and(a, b),
        BinaryOp::Or => or(a, b),
        BinaryOp::Eq
        | BinaryOp::NotEq
        | BinaryOp::Lt
        | BinaryOp::LtEq
        | BinaryOp::Gt
        | BinaryOp::GtEq
        | BinaryOp::Plus
        | BinaryOp::Minus
        | BinaryOp::Mul
        | BinaryOp::Div
        | BinaryOp::Mod
        | BinaryOp::Concat
        | BinaryOp::Like
        | BinaryOp::NotLike
        | BinaryOp::ILike
        | BinaryOp::NotILike => {
            if a == Tv::Null || b == Tv::Null {
                Tv::Null
            } else {
                Tv::Any
            }
        }
        // `IS DISTINCT FROM` and the like are not strict.
        BinaryOp::Other(_) => Tv::Any,
    }
}

fn and(a: Tv, b: Tv) -> Tv {
    match (a, b) {
        (Tv::False, _) | (_, Tv::False) => Tv::False,
        (Tv::True, x) | (x, Tv::True) => x,
        (Tv::Null, Tv::Null) => Tv::Null,
        (Tv::Null | Tv::NotTrue, _) | (_, Tv::Null | Tv::NotTrue) => Tv::NotTrue,
        (Tv::Any, Tv::Any) => Tv::Any,
    }
}

fn or(a: Tv, b: Tv) -> Tv {
    match (a, b) {
        (Tv::True, _) | (_, Tv::True) => Tv::True,
        (Tv::False, x) | (x, Tv::False) => x,
        (Tv::Null, Tv::Null) => Tv::Null,
        (Tv::Null | Tv::NotTrue, Tv::Null | Tv::NotTrue) => Tv::NotTrue,
        _ => Tv::Any,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn and_with_a_rejecting_side_rejects() {
        assert_eq!(and(Tv::Null, Tv::Any), Tv::NotTrue);
        assert_eq!(and(Tv::NotTrue, Tv::Any), Tv::NotTrue);
        assert_eq!(and(Tv::False, Tv::Any), Tv::False);
        assert_eq!(and(Tv::True, Tv::Null), Tv::Null);
    }

    #[test]
    fn or_needs_every_side_to_reject() {
        assert_eq!(or(Tv::Null, Tv::Any), Tv::Any);
        assert_eq!(or(Tv::Null, Tv::Null), Tv::Null);
        assert_eq!(or(Tv::Null, Tv::True), Tv::True);
    }

    #[test]
    fn is_null_does_not_reject() {
        assert_eq!(unary(UnaryOp::IsNull, Tv::Null), Tv::True);
        assert_eq!(unary(UnaryOp::IsNotNull, Tv::Null), Tv::False);
        assert_eq!(unary(UnaryOp::Not, Tv::NotTrue), Tv::Any);
    }

    #[test]
    fn is_true_and_is_false_on_unknown_stay_open() {
        assert_eq!(unary(UnaryOp::IsTrue, Tv::Any), Tv::Any);
        assert_eq!(unary(UnaryOp::IsFalse, Tv::NotTrue), Tv::Any);
        assert_eq!(unary(UnaryOp::IsTrue, Tv::Null), Tv::False);
    }
}

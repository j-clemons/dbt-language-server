//! `canon.simplify`: three-valued logic and constant folding.

use cogitator_ir::common::{BinaryOp, Lit, UnaryOp};
use cogitator_ir::mir::{print_mir, Mir, RelId, RelOp, ScalarId, ScalarKind, SubqueryKind};
use cogitator_ir::testkit::{int_col, MirBuilder};
use cogitator_ir::ColType;

use super::simplify::Simplify;
use super::test_util::{apply, assert_stable, bool_col};

/// `select x from t where <pred>` where `x` is a boolean column.
fn with_pred(build: impl FnOnce(&mut MirBuilder, ScalarId) -> ScalarId) -> Mir {
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("x", bool_col()), ("n", int_col())]);
    let x = b.out(scan)[0];
    let x_ref = b.col_ref(x);
    let pred = build(&mut b, x_ref);
    let filter = b.filter(scan, vec![pred]);
    b.finish(filter)
}

fn boolean(b: &mut MirBuilder, value: bool) -> ScalarId {
    b.scalar(ScalarKind::Lit(Lit::Bool(value)), bool_col())
}

fn filter_of(mir: &Mir) -> Vec<ScalarId> {
    match &mir.rels[mir.root].op {
        RelOp::Filter { conjuncts, .. } => conjuncts.clone(),
        other => panic!("expected a filter at the root, got {other:?}"),
    }
}

fn single_conjunct(mir: &Mir) -> ScalarKind {
    let conjuncts = filter_of(mir);
    assert_eq!(conjuncts.len(), 1);
    mir.scalars[conjuncts[0]].kind.clone()
}

/// The conjunct after folding; a conjunct that folded to `true` removes
/// the filter, which is reported as that literal.
fn folded(build: impl FnOnce(&mut MirBuilder, ScalarId) -> ScalarId) -> ScalarKind {
    let (mir, _, _) = apply(&Simplify, with_pred(build));
    match mir.rels[mir.root].op {
        RelOp::Filter { .. } => single_conjunct(&mir),
        _ => ScalarKind::Lit(Lit::Bool(true)),
    }
}

#[test]
fn where_one_equals_one_normalises_away() {
    let mir = with_pred(|b, _| {
        let (l, r) = (b.number("1"), b.number("1"));
        b.binary(BinaryOp::Eq, l, r)
    });
    let (mir, text, changed) = apply(&Simplify, mir);
    assert!(changed);
    assert!(matches!(mir.rels[mir.root].op, RelOp::Scan { .. }));
    insta::assert_snapshot!(text);
    assert_stable(&Simplify, mir);
}

#[test]
fn x_and_null_is_not_false() {
    let pred = folded(|b, x| {
        let null = b.null_lit();
        b.binary(BinaryOp::And, x, null)
    });
    assert!(
        matches!(
            pred,
            ScalarKind::Binary {
                op: BinaryOp::And,
                ..
            }
        ),
        "{pred:?}"
    );
}

#[test]
fn constant_and_or_follow_kleene_logic() {
    let cases: [(bool, BinaryOp, ScalarKind); 4] = [
        (false, BinaryOp::And, ScalarKind::Lit(Lit::Bool(false))),
        (true, BinaryOp::And, ScalarKind::Lit(Lit::Null)),
        (true, BinaryOp::Or, ScalarKind::Lit(Lit::Bool(true))),
        (false, BinaryOp::Or, ScalarKind::Lit(Lit::Null)),
    ];
    for (value, op, want) in cases {
        let got = folded(|b, _| {
            let l = boolean(b, value);
            let null = b.null_lit();
            b.binary(op, l, null)
        });
        assert_eq!(got, want, "{value} {op:?} NULL");
    }
}

#[test]
fn identity_and_absorbing_terms() {
    let x_and_true = folded(|b, x| {
        let t = boolean(b, true);
        b.binary(BinaryOp::And, x, t)
    });
    assert!(matches!(x_and_true, ScalarKind::Col(_)), "{x_and_true:?}");

    let x_and_false = folded(|b, x| {
        let f = boolean(b, false);
        b.binary(BinaryOp::And, x, f)
    });
    assert_eq!(x_and_false, ScalarKind::Lit(Lit::Bool(false)));

    let x_or_true = folded(|b, x| {
        let t = boolean(b, true);
        b.binary(BinaryOp::Or, t, x)
    });
    assert_eq!(x_or_true, ScalarKind::Lit(Lit::Bool(true)));
}

#[test]
fn a_subquery_is_not_folded_away() {
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("x", bool_col())]);
    let inner = b.scan("u", &[("y", int_col())]);
    let exists = b.scalar(
        ScalarKind::Subquery {
            kind: SubqueryKind::Exists,
            rel: inner,
            correlated: vec![],
        },
        bool_col(),
    );
    let f = boolean(&mut b, false);
    let pred = b.binary(BinaryOp::And, exists, f);
    let filter = b.filter(scan, vec![pred]);
    let (mir, _, changed) = apply(&Simplify, b.finish(filter));
    assert!(!changed);
    assert!(matches!(
        single_conjunct(&mir),
        ScalarKind::Binary {
            op: BinaryOp::And,
            ..
        }
    ));
}

#[test]
fn not_and_null_tests_fold_on_literals() {
    let not_null = folded(|b, _| {
        let null = b.null_lit();
        b.scalar(
            ScalarKind::Unary {
                op: UnaryOp::Not,
                e: null,
            },
            ColType::UNKNOWN,
        )
    });
    assert_eq!(not_null, ScalarKind::Lit(Lit::Null));

    let double = folded(|b, x| {
        let once = b.scalar(
            ScalarKind::Unary {
                op: UnaryOp::Not,
                e: x,
            },
            bool_col(),
        );
        b.scalar(
            ScalarKind::Unary {
                op: UnaryOp::Not,
                e: once,
            },
            bool_col(),
        )
    });
    assert!(matches!(double, ScalarKind::Col(_)), "{double:?}");

    let null_is_null = folded(|b, _| {
        let null = b.null_lit();
        b.scalar(
            ScalarKind::Unary {
                op: UnaryOp::IsNull,
                e: null,
            },
            bool_col(),
        )
    });
    // the conjunct became `true`, so the filter is gone
    assert_eq!(null_is_null, ScalarKind::Lit(Lit::Bool(true)));
}

#[test]
fn comparisons_fold_only_when_exact() {
    let eq = |b: &mut MirBuilder, l: &str, r: &str, op| {
        let (l, r) = (b.number(l), b.number(r));
        b.binary(op, l, r)
    };
    let false_lit = ScalarKind::Lit(Lit::Bool(false));
    assert_eq!(folded(|b, _| eq(b, "2", "10", BinaryOp::Gt)), false_lit);
    assert_eq!(
        folded(|b, _| eq(b, "1.50", "1.5", BinaryOp::NotEq)),
        false_lit
    );
    // an exponent is not plain decimal text: left alone
    let exp = folded(|b, _| eq(b, "1e2", "100", BinaryOp::Eq));
    assert!(matches!(exp, ScalarKind::Binary { .. }), "{exp:?}");

    let null_cmp = folded(|b, x| {
        let null = b.null_lit();
        b.binary(BinaryOp::Eq, x, null)
    });
    assert_eq!(null_cmp, ScalarKind::Lit(Lit::Null));
}

#[test]
fn string_equality_folds_only_on_identical_text() {
    let lit = |b: &mut MirBuilder, text: &str| {
        let sym = b.sym(text);
        b.scalar(ScalarKind::Lit(Lit::Str(sym)), ColType::UNKNOWN)
    };
    let same = folded(|b, _| {
        let (l, r) = (lit(b, "a"), lit(b, "a"));
        let eq = b.binary(BinaryOp::Eq, l, r);
        let f = boolean(b, false);
        b.binary(BinaryOp::Or, eq, f)
    });
    assert_eq!(same, ScalarKind::Lit(Lit::Bool(true)));

    let different = folded(|b, _| {
        let (l, r) = (lit(b, "a"), lit(b, "A"));
        b.binary(BinaryOp::Eq, l, r)
    });
    assert!(
        matches!(different, ScalarKind::Binary { .. }),
        "{different:?}"
    );
}

#[test]
fn join_on_true_is_dropped_but_the_join_stays() {
    use cogitator_ir::common::JoinKind;
    let mut b = MirBuilder::new();
    let t = b.scan("t", &[("a", int_col())]);
    let u = b.scan("u", &[("b", int_col())]);
    let (one, two) = (b.number("1"), b.number("1"));
    let on = b.binary(BinaryOp::Eq, one, two);
    let join = b.join(JoinKind::Left, t, u, vec![on]);
    let (mir, text, _) = apply(&Simplify, b.finish(join));
    assert!(matches!(
        mir.rels[mir.root].op,
        RelOp::Join { kind: JoinKind::Left, ref on, .. } if on.is_empty()
    ));
    insta::assert_snapshot!(text);
}

#[test]
fn origin_spans_survive_folding() {
    use cogitator_ir::mir::{HirNode, HirRef, Origin};
    use cogitator_ir::Span;
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("x", bool_col())]);
    let x = b.out(scan)[0];
    let x_ref = b.col_ref(x);
    let t = boolean(&mut b, true);
    let pred = b.binary(BinaryOp::And, x_ref, t);
    let authored = b.mir.prov.add(Origin::Hir(HirRef {
        node: HirNode::None,
        span: Span::new(10, 20),
    }));
    b.mir.scalars[pred].prov = authored;
    let n = b.number("1");
    let wrapped = b.binary(BinaryOp::Eq, pred, n);
    b.mir.scalars[wrapped].prov = authored;
    let filter = b.filter(scan, vec![wrapped]);
    let (mir, _, _) = apply(&Simplify, b.finish(filter));
    let root_scalar = filter_of(&mir)[0];
    assert_eq!(
        mir.prov.span(mir.scalars[root_scalar].prov),
        Some(Span::new(10, 20))
    );
}

#[test]
fn unchanged_plan_appends_nothing() {
    let mir = with_pred(|_, x| x);
    let (rels, scalars) = (mir.rels.len(), mir.scalars.len());
    let (mir, _, changed) = apply(&Simplify, mir);
    assert!(!changed);
    assert_eq!((mir.rels.len(), mir.scalars.len()), (rels, scalars));
    let _: RelId = mir.root;
    assert!(print_mir(&mir).contains("Filter"));
}

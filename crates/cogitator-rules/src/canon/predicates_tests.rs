//! `canon.predicates`: what moves past projections and joins.

use cogitator_ir::common::{BinaryOp, JoinKind};
use cogitator_ir::mir::{ColId, HirNode, HirRef, Mir, Origin, RelId, RelOp, ScalarId};
use cogitator_ir::testkit::{int_col, MirBuilder};
use cogitator_ir::Span;

use super::predicates::Predicates;
use super::test_util::{apply, assert_stable};

pub(super) fn gt(b: &mut MirBuilder, col: ColId, value: &str) -> ScalarId {
    let c = b.col_ref(col);
    let n = b.number(value);
    b.binary(BinaryOp::Gt, c, n)
}

/// Rendered plan after `canon.predicates`, verifying it moved or not.
pub(super) fn run(mir: Mir, expect_change: bool) -> (Mir, String) {
    let (mir, text, changed) = apply(&Predicates, mir);
    assert_eq!(changed, expect_change, "{text}");
    (mir.clone(), text)
}

/// The filter stayed directly above the join (or other blocker).
pub(super) fn assert_blocked(mir: Mir) {
    let (mir, text) = run(mir, false);
    assert!(
        matches!(mir.rels[mir.root].op, RelOp::Filter { .. }),
        "{text}"
    );
}

pub(super) fn filter_sits_above_scan(mir: &Mir, rel: RelId) -> bool {
    match &mir.rels[rel].op {
        RelOp::Filter { input, .. } => matches!(mir.rels[*input].op, RelOp::Scan { .. }),
        _ => false,
    }
}

#[test]
fn splits_and_pushes_through_project_to_the_scan() {
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("a", int_col()), ("b", int_col())]);
    let cols = b.out(scan);
    let pa = b.col_ref(cols[0]);
    let renamed = b.col("x", int_col());
    let pb = b.col_ref(cols[1]);
    let project = b.project(scan, vec![(renamed, pa), (cols[1], pb)]);
    let p1 = gt(&mut b, renamed, "1");
    let p2 = gt(&mut b, cols[1], "2");
    let both = b.binary(BinaryOp::And, p1, p2);
    let filter = b.filter(project, vec![both]);
    let (mir, text) = run(b.finish(filter), true);
    let RelOp::Project { input, .. } = mir.rels[mir.root].op else {
        panic!("{text}");
    };
    assert!(filter_sits_above_scan(&mir, input), "{text}");
    insta::assert_snapshot!(text);
    assert_stable(&Predicates, mir);
}

#[test]
fn a_pushed_filter_keeps_its_authored_span() {
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("a", int_col())]);
    let a = b.out(scan)[0];
    let ref_a = b.col_ref(a);
    let project = b.project(scan, vec![(a, ref_a)]);
    let pred = gt(&mut b, a, "1");
    let authored = b.mir.prov.add(Origin::Hir(HirRef {
        node: HirNode::None,
        span: Span::new(40, 52),
    }));
    b.mir.scalars[pred].prov = authored;
    let filter = b.filter(project, vec![pred]);
    let (mir, text) = run(b.finish(filter), true);
    let RelOp::Project { input, .. } = mir.rels[mir.root].op else {
        panic!("{text}");
    };
    assert_eq!(
        mir.prov.span(mir.rels[input].prov),
        Some(Span::new(40, 52)),
        "{text}"
    );
}

fn join_plan(kind: JoinKind, on_left: bool) -> Mir {
    let mut b = MirBuilder::new();
    let l = b.scan("l", &[("a", int_col())]);
    let r = b.scan("r", &[("b", int_col())]);
    let a = b.out(l)[0];
    let bb = b.out(r)[0];
    let join = b.join(kind, l, r, vec![]);
    let pred = gt(&mut b, if on_left { a } else { bb }, "1");
    let filter = b.filter(join, vec![pred]);
    b.finish(filter)
}

fn pushed_side(mir: &Mir) -> (bool, bool) {
    let RelOp::Join { l, r, .. } = mir.rels[mir.root].op else {
        panic!("expected a join at the root");
    };
    (
        filter_sits_above_scan(mir, l),
        filter_sits_above_scan(mir, r),
    )
}

#[test]
fn where_moves_into_preserved_inputs() {
    for (kind, on_left, want) in [
        (JoinKind::Inner, true, (true, false)),
        (JoinKind::Inner, false, (false, true)),
        (JoinKind::Left, true, (true, false)),
        (JoinKind::Right, false, (false, true)),
    ] {
        let (mir, _) = run(join_plan(kind, on_left), true);
        assert_eq!(pushed_side(&mir), want, "{kind:?} {on_left}");
    }
}

#[test]
fn where_stays_above_the_null_supplying_side() {
    // the negative that matters most: filtering the right input of a
    // LEFT JOIN below the join would keep rows the WHERE rejects
    assert_blocked(join_plan(JoinKind::Left, false));
    assert_blocked(join_plan(JoinKind::Right, true));
    assert_blocked(join_plan(JoinKind::Full, true));
    assert_blocked(join_plan(JoinKind::Full, false));
}

#[test]
fn a_predicate_over_both_sides_stays_above_the_join() {
    let mut b = MirBuilder::new();
    let l = b.scan("l", &[("a", int_col())]);
    let r = b.scan("r", &[("b", int_col())]);
    let (a, bb) = (b.out(l)[0], b.out(r)[0]);
    let join = b.join(JoinKind::Inner, l, r, vec![]);
    let (ra, rb) = (b.col_ref(a), b.col_ref(bb));
    let pred = b.binary(BinaryOp::Eq, ra, rb);
    let filter = b.filter(join, vec![pred]);
    assert_blocked(b.finish(filter));
}

fn on_plan(kind: JoinKind, on_left: bool) -> Mir {
    let mut b = MirBuilder::new();
    let l = b.scan("l", &[("a", int_col())]);
    let r = b.scan("r", &[("b", int_col())]);
    let (a, bb) = (b.out(l)[0], b.out(r)[0]);
    let pred = gt(&mut b, if on_left { a } else { bb }, "1");
    let join = b.join(kind, l, r, vec![pred]);
    b.finish(join)
}

#[test]
fn on_conditions_only_filter_the_side_that_is_not_preserved() {
    // LEFT JOIN r ON r.b > 1 filters r
    let (mir, text) = run(on_plan(JoinKind::Left, false), true);
    assert_eq!(pushed_side(&mir), (false, true), "{text}");
    // but LEFT JOIN r ON l.a > 1 must keep every left row
    run(on_plan(JoinKind::Left, true), false);
    run(on_plan(JoinKind::Right, false), false);
    run(on_plan(JoinKind::Full, true), false);
    run(on_plan(JoinKind::Full, false), false);
    // an ANTI join returns the left rows whose match failed
    run(on_plan(JoinKind::Anti, true), false);
    let (mir, text) = run(on_plan(JoinKind::Anti, false), true);
    assert_eq!(pushed_side(&mir), (false, true), "{text}");
}

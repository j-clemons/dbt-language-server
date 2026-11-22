//! `canon.predicates`: one negative test per pushdown blocker above a
//! Window, Aggregate or Limit, and for functions that may not move.

use cogitator_ir::common::BinaryOp;
use cogitator_ir::mir::{AggCall, ColId, GroupingSets, Mir, RelOp, WinCall};
use cogitator_ir::testkit::{int_col, MirBuilder};

use super::predicates::Predicates;
use super::predicates_tests::{assert_blocked, filter_sits_above_scan, gt, run};
use super::test_util::assert_stable;

fn window_plan(partition: bool, filter_on_func: bool) -> Mir {
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("a", int_col()), ("b", int_col())]);
    let cols = b.out(scan);
    let part = b.col_ref(cols[0]);
    let func = b.sym("row_number");
    let rn = b.col("rn", int_col());
    let call = WinCall {
        func,
        args: vec![],
        partition_by: if partition { vec![part] } else { vec![] },
        order_by: vec![],
        frame: None,
    };
    let window = b.window(scan, vec![(rn, call)]);
    let pred = gt(&mut b, if filter_on_func { rn } else { cols[0] }, "1");
    let filter = b.filter(window, vec![pred]);
    b.finish(filter)
}

#[test]
fn window_filters_move_only_on_partition_keys() {
    let (mir, text) = run(window_plan(true, false), true);
    let RelOp::Window { input, .. } = mir.rels[mir.root].op else {
        panic!("{text}");
    };
    assert!(filter_sits_above_scan(&mir, input), "{text}");
    insta::assert_snapshot!(text);

    assert_blocked(window_plan(false, false));
    assert_blocked(window_plan(true, true));
}

fn aggregate_plan(filter_on_agg: bool, sets: Option<fn(&[ColId]) -> GroupingSets>) -> Mir {
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("a", int_col()), ("b", int_col())]);
    let cols = b.out(scan);
    let key = b.col_ref(cols[0]);
    let func = b.sym("sum");
    let arg = b.col_ref(cols[1]);
    let total = b.col("total", int_col());
    let agg = AggCall {
        func,
        args: vec![arg],
        distinct: false,
        filter: None,
    };
    let grouping = sets.map(|f| f(&cols));
    let aggregate = b.aggregate(scan, vec![(cols[0], key)], vec![(total, agg)], grouping);
    let pred = gt(&mut b, if filter_on_agg { total } else { cols[0] }, "1");
    let filter = b.filter(aggregate, vec![pred]);
    b.finish(filter)
}

#[test]
fn having_moves_only_on_grouping_keys() {
    let (mir, text) = run(aggregate_plan(false, None), true);
    let RelOp::Aggregate { input, .. } = mir.rels[mir.root].op else {
        panic!("{text}");
    };
    assert!(filter_sits_above_scan(&mir, input), "{text}");
    insta::assert_snapshot!(text);

    assert_blocked(aggregate_plan(true, None));
}

#[test]
fn grouping_sets_block_keys_they_do_not_always_hold() {
    assert_blocked(aggregate_plan(false, Some(|_| GroupingSets(vec![vec![]]))));
    let (mir, text) = run(
        aggregate_plan(false, Some(|cols| GroupingSets(vec![vec![cols[0]]]))),
        true,
    );
    assert!(
        matches!(mir.rels[mir.root].op, RelOp::Aggregate { .. }),
        "{text}"
    );
}

#[test]
fn a_constant_filter_stays_above_a_global_aggregate() {
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("a", int_col())]);
    let func = b.sym("count");
    let total = b.col("n", int_col());
    let agg = AggCall {
        func,
        args: vec![],
        distinct: false,
        filter: None,
    };
    let aggregate = b.aggregate(scan, vec![], vec![(total, agg)], None);
    let (l, r) = (b.number("1"), b.number("2"));
    let pred = b.binary(BinaryOp::Eq, l, r);
    let filter = b.filter(aggregate, vec![pred]);
    assert_blocked(b.finish(filter));
}

#[test]
fn limit_blocks_pushdown() {
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("a", int_col())]);
    let a = b.out(scan)[0];
    let n = b.number("10");
    let limit = b.rel(RelOp::Limit {
        input: scan,
        limit: Some(n),
        offset: None,
    });
    let pred = gt(&mut b, a, "1");
    let filter = b.filter(limit, vec![pred]);
    assert_blocked(b.finish(filter));
}

fn call_filter(func: &str) -> Mir {
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("a", int_col())]);
    let a = b.out(scan)[0];
    let r = b.col_ref(a);
    let project = b.project(scan, vec![(a, r)]);
    let arg = b.col_ref(a);
    let call = b.call(func, vec![arg]);
    let filter = b.filter(project, vec![call]);
    b.finish(filter)
}

#[test]
fn volatile_and_unknown_functions_do_not_move() {
    assert_blocked(call_filter("random"));
    assert_blocked(call_filter("current_timestamp"));
    assert_blocked(call_filter("my_udf"));
    // known deterministic functions do, in any case
    let (mir, text) = run(call_filter("LOWER"), true);
    assert!(
        matches!(mir.rels[mir.root].op, RelOp::Project { .. }),
        "{text}"
    );
}

#[test]
fn a_volatile_project_item_pins_filters_that_read_it() {
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("a", int_col())]);
    let a = b.out(scan)[0];
    let r = b.col_ref(a);
    let rnd = b.call("random", vec![]);
    let noise = b.col("noise", int_col());
    let project = b.project(scan, vec![(a, r), (noise, rnd)]);
    let on_noise = gt(&mut b, noise, "1");
    let filter = b.filter(project, vec![on_noise]);
    assert_blocked(b.finish(filter));
}

#[test]
fn a_filter_does_not_move_past_a_volatile_filter() {
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("a", int_col())]);
    let a = b.out(scan)[0];
    let rnd = b.call("random", vec![]);
    let inner = b.filter(scan, vec![rnd]);
    let pred = gt(&mut b, a, "1");
    let filter = b.filter(inner, vec![pred]);
    assert_blocked(b.finish(filter));
}

#[test]
fn adjacent_filters_merge_and_the_result_is_stable() {
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("a", int_col()), ("b", int_col())]);
    let cols = b.out(scan);
    let p1 = gt(&mut b, cols[0], "1");
    let inner = b.filter(scan, vec![p1]);
    let p2 = gt(&mut b, cols[1], "2");
    let outer = b.filter(inner, vec![p2]);
    let (mir, text) = run(b.finish(outer), true);
    assert!(filter_sits_above_scan(&mir, mir.root), "{text}");
    assert_stable(&Predicates, mir);
}

#[test]
fn substitution_shares_expressions_rather_than_copying_them() {
    // a filter over a computed column pushes a rewritten predicate; the
    // arena gains only the rewritten path
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("a", int_col())]);
    let a = b.out(scan)[0];
    let (ra, one) = (b.col_ref(a), b.number("1"));
    let expr = b.binary(BinaryOp::Plus, ra, one);
    let x = b.col("x", int_col());
    let project = b.project(scan, vec![(x, expr)]);
    let pred = gt(&mut b, x, "5");
    let filter = b.filter(project, vec![pred]);
    let (mir, text) = run(b.finish(filter), true);
    let RelOp::Project { input, .. } = mir.rels[mir.root].op else {
        panic!("{text}");
    };
    assert!(filter_sits_above_scan(&mir, input), "{text}");
    insta::assert_snapshot!(text);
}

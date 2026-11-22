//! HIR→MIR: the select pipeline, stars, grouping and ordering.

mod common;

use cogitator_ir::common::{BinaryOp, JoinKind};
use cogitator_ir::hir::{GroupBy, JoinCond, OrderItem, SelectItem};
use cogitator_ir::mir::{print_mir, RelOp};
use common::*;

#[test]
fn known_star_expands_to_one_item_per_column() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["a", "b"]);
    let root = select(&mut b, vec![], vec![star()], vec![t], |_| {});
    let mir = clean(&b.finish(root));
    insta::assert_snapshot!(print_mir(&mir));
}

#[test]
fn star_exclude_and_replace_apply_by_name() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["a", "b", "c"]);
    let one = b.number(SPAN, "1");
    let (a_ident, b_ident) = (b.ident(SPAN, "a"), b.ident(SPAN, "b"));
    let item = SelectItem::Star {
        span: SPAN,
        from: None,
        qualifier: None,
        exclude: vec![a_ident],
        replace: vec![(one, b_ident)],
    };
    let root = select(&mut b, vec![], vec![item], vec![t], |_| {});
    let mir = clean(&b.finish(root));
    insta::assert_snapshot!(print_mir(&mir));
}

#[test]
fn using_merges_the_column_in_a_star() {
    let mut b = builder();
    let l = b.base_model(SPAN, "l", None, &["id", "x"]);
    let r = b.base_model(SPAN, "r", None, &["id", "y"]);
    let id = b.ident(SPAN, "id");
    let joined = b.join(SPAN, JoinKind::Inner, l, r, JoinCond::Using(vec![id]));
    let root = select(&mut b, vec![], vec![star()], vec![joined], |_| {});
    let mir = clean(&b.finish(root));
    assert_eq!(mir.root_columns().len(), 3, "{}", print_mir(&mir));
    insta::assert_snapshot!(print_mir(&mir));
}

#[test]
fn full_join_using_coalesces_both_sides() {
    let mut b = builder();
    let l = b.base_model(SPAN, "l", None, &["id"]);
    let r = b.base_model(SPAN, "r", None, &["id"]);
    let id = b.ident(SPAN, "id");
    let joined = b.join(SPAN, JoinKind::Full, l, r, JoinCond::Using(vec![id]));
    let root = select(&mut b, vec![], vec![star()], vec![joined], |_| {});
    let printed = print_mir(&clean(&b.finish(root)));
    assert!(printed.contains("coalesce("), "{printed}");
}

#[test]
fn natural_join_equates_the_shared_names() {
    let mut b = builder();
    let l = b.base_model(SPAN, "l", None, &["id", "x"]);
    let r = b.base_model(SPAN, "r", None, &["id", "y"]);
    let joined = b.join(SPAN, JoinKind::Inner, l, r, JoinCond::Natural);
    let root = select(&mut b, vec![], vec![star()], vec![joined], |_| {});
    let mir = clean(&b.finish(root));
    assert_eq!(mir.root_columns().len(), 3);
    assert!(print_mir(&mir).contains("Join Inner on (id#"));
}

#[test]
fn a_bare_column_passes_through_and_an_alias_is_fresh() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["a"]);
    let (x, y, z) = (
        b.column(SPAN, "a", t, 0),
        b.column(SPAN, "a", t, 0),
        b.column(SPAN, "a", t, 0),
    );
    let items = vec![
        item(x, "", &mut b),
        item(y, "", &mut b),
        item(z, "renamed", &mut b),
    ];
    let root = select(&mut b, vec![], items, vec![t], |_| {});
    let mir = clean(&b.finish(root));
    let RelOp::Scan { cols, .. } = &mir.rels[mir.rels.ids().next().expect("scan")].op else {
        panic!("scan first");
    };
    let out = mir.root_columns();
    assert_eq!(out[0], cols[0], "first read passes through");
    assert_ne!(out[1], out[0], "a repeat gets a fresh id");
    assert_ne!(out[2], out[0], "an alias gets a fresh id");
}

#[test]
fn group_by_ordinal_and_having_lower_to_aggregate_and_filter() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["k", "v"]);
    let k = b.column(SPAN, "k", t, 0);
    let v = b.column(SPAN, "v", t, 1);
    let total = call(&mut b, "sum", vec![v]);
    let v2 = b.column(SPAN, "v", t, 1);
    let having_sum = call(&mut b, "sum", vec![v2]);
    let zero = b.number(SPAN, "0");
    let having = b.binary(SPAN, BinaryOp::Gt, having_sum, zero);
    let one = b.number(SPAN, "1");
    let items = vec![item(k, "", &mut b), item(total, "total", &mut b)];
    let root = select(&mut b, vec![], items, vec![t], |s| {
        s.group_by = GroupBy::Exprs(vec![one]);
        s.having = Some(having);
    });
    let mir = clean(&b.finish(root));
    insta::assert_snapshot!(print_mir(&mir));
    assert_eq!(count_ops(&mir, |op| matches!(op, RelOp::Filter { .. })), 1);
}

#[test]
fn group_by_all_uses_the_non_aggregate_items() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["k", "v"]);
    let k = b.column(SPAN, "k", t, 0);
    let v = b.column(SPAN, "v", t, 1);
    let total = call(&mut b, "sum", vec![v]);
    let items = vec![item(k, "", &mut b), item(total, "total", &mut b)];
    let root = select(&mut b, vec![], items, vec![t], |s| {
        s.group_by = GroupBy::All
    });
    let mir = clean(&b.finish(root));
    let key_count = mir
        .reachable()
        .into_iter()
        .find_map(|id| match &mir.rels[id].op {
            RelOp::Aggregate { keys, .. } => Some(keys.len()),
            _ => None,
        });
    assert_eq!(key_count, Some(1));
}

#[test]
fn a_column_neither_grouped_nor_aggregated_is_opaque_not_a_violation() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["k", "v"]);
    let k = b.column(SPAN, "k", t, 0);
    let v = b.column(SPAN, "v", t, 1);
    let key = b.column(SPAN, "k", t, 0);
    let items = vec![item(k, "", &mut b), item(v, "", &mut b)];
    let root = select(&mut b, vec![], items, vec![t], |s| {
        s.group_by = GroupBy::Exprs(vec![key]);
    });
    let lowered = cogitator_lower::lower_to_mir(&b.finish(root));
    lowered.assert_clean();
    assert_eq!(lowered.fidelity, cogitator_ir::Fidelity::Partial);
}

#[test]
fn rollup_enumerates_its_grouping_sets() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["a", "b"]);
    let (x, y) = (b.column(SPAN, "a", t, 0), b.column(SPAN, "b", t, 1));
    let (kx, ky) = (b.column(SPAN, "a", t, 0), b.column(SPAN, "b", t, 1));
    let items = vec![item(x, "", &mut b), item(y, "", &mut b)];
    let root = select(&mut b, vec![], items, vec![t], |s| {
        s.group_by = GroupBy::Rollup(vec![kx, ky]);
    });
    let mir = clean(&b.finish(root));
    let sets = mir
        .reachable()
        .into_iter()
        .find_map(|id| match &mir.rels[id].op {
            RelOp::Aggregate { sets, .. } => sets.clone(),
            _ => None,
        });
    assert_eq!(sets.expect("grouping sets").0.len(), 3);
}

#[test]
fn window_and_qualify_lower_to_window_and_filter() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["k"]);
    let k = b.column(SPAN, "k", t, 0);
    let key = b.column(SPAN, "k", t, 0);
    let rank = over(&mut b, "row_number", vec![key]);
    let one = b.number(SPAN, "1");
    let qualify = b.binary(SPAN, BinaryOp::Eq, rank, one);
    let items = vec![item(k, "", &mut b)];
    let root = select(&mut b, vec![], items, vec![t], |s| {
        s.qualify = Some(qualify)
    });
    let mir = clean(&b.finish(root));
    let partitions = mir
        .reachable()
        .into_iter()
        .find_map(|id| match &mir.rels[id].op {
            RelOp::Window { funcs, .. } => Some(funcs[0].1.partition_by.len()),
            _ => None,
        });
    assert_eq!(partitions, Some(1), "the printer omits partitions");
    insta::assert_snapshot!(print_mir(&mir));
}

#[test]
fn between_is_two_conjuncts() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["a"]);
    let a = b.column(SPAN, "a", t, 0);
    let (lo, hi) = (b.number(SPAN, "1"), b.number(SPAN, "9"));
    let between = expr(
        &mut b,
        cogitator_ir::hir::ExprKind::Between {
            e: a,
            lo,
            hi,
            negated: false,
        },
    );
    let root = select(&mut b, vec![], vec![star()], vec![t], |s| {
        s.filter = Some(between)
    });
    let mir = clean(&b.finish(root));
    let conjuncts = mir
        .reachable()
        .into_iter()
        .find_map(|id| match &mir.rels[id].op {
            RelOp::Filter { conjuncts, .. } => Some(conjuncts.len()),
            _ => None,
        });
    assert_eq!(conjuncts, Some(2));
}

#[test]
fn order_by_a_dropped_column_is_carried_and_trimmed() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["a", "b"]);
    let a = b.column(SPAN, "a", t, 0);
    let key = b.column(SPAN, "b", t, 1);
    let items = vec![item(a, "", &mut b)];
    let root = select(&mut b, vec![], items, vec![t], |_| {});
    b.hir.queries[root].order_by = vec![OrderItem {
        expr: key,
        desc: Some(true),
        nulls_first: None,
    }];
    let mir = clean(&b.finish(root));
    assert_eq!(mir.root_columns().len(), 1);
    insta::assert_snapshot!(print_mir(&mir));
}

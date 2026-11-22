//! Printer snapshots for one hand-built plan per level.

use crate::common::{BinaryOp, JoinKind};
use crate::hir::{print_hir, JoinCond, SelectItem};
use crate::lir::print_lir;
use crate::mir::print_mir;
use crate::testkit::{int_col, text_col, HirBuilder, LirBuilder, MirBuilder};

#[test]
fn hir_text_is_stable() {
    let text = "with a as (select 1 as x) select a.x from a join b on a.x = b.x";
    let mut b = HirBuilder::new(text);
    let one = b.number(b.span_of("1"), "1");
    let alias = b.ident(b.span_of("x"), "x");
    let inner = b.select(
        b.span_of("select 1 as x"),
        vec![],
        vec![SelectItem::Expr {
            expr: one,
            alias: Some(alias),
        }],
        vec![],
        None,
    );
    let cte = b.cte(b.span_of("a as"), "a", inner);
    let from_a = b.base_cte(b.span_of("from a"), cte, Some("a"), &["x"]);
    let from_b = b.base_model(b.span_of("join b"), "b", None, &["x"]);
    let lhs = b.column(b.span_of("a.x = b"), "x", from_a, 0);
    let rhs = b.column(b.span_of("b.x"), "x", from_b, 0);
    let on = b.binary(b.span_of("a.x = b.x"), BinaryOp::Eq, lhs, rhs);
    let joined = b.join(
        b.span_of("a join b"),
        JoinKind::Left,
        from_a,
        from_b,
        JoinCond::On(on),
    );
    let item = b.column(b.span_of("a.x from"), "x", from_a, 0);
    let root = b.select(
        Default::default(),
        vec![cte],
        vec![SelectItem::Expr {
            expr: item,
            alias: None,
        }],
        vec![joined],
        None,
    );
    insta::assert_snapshot!(print_hir(&b.finish(root)));
}

#[test]
fn mir_and_lir_text_are_stable() {
    let mut b = MirBuilder::new();
    let orders = b.scan("orders", &[("id", int_col()), ("status", text_col())]);
    let items = b.scan("items", &[("order_id", int_col())]);
    let cols = b.out(orders);
    let item_cols = b.out(items);
    let (id_ref, order_ref) = (b.col_ref(cols[0]), b.col_ref(item_cols[0]));
    let on = b.binary(BinaryOp::Eq, id_ref, order_ref);
    let joined = b.join(JoinKind::Left, orders, items, vec![on]);
    let status = b.col_ref(cols[1]);
    let open = b.number("1");
    let pred = b.binary(BinaryOp::Eq, status, open);
    let filter = b.filter(joined, vec![pred]);
    let mir = b.finish(filter);

    insta::assert_snapshot!("mir_text", print_mir(&mir));

    let mut lb = LirBuilder::new();
    let scan = lb.scan(&mir, orders);
    let filter_node = lb.filter(scan, vec![pred], filter);
    let lir = lb.finish(filter_node);
    insta::assert_snapshot!("lir_text", print_lir(&lir, &mir));
}

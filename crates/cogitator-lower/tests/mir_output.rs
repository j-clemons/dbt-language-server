//! HIR→MIR: ordering, DISTINCT, nullability and lateral aliases.

mod common;

use cogitator_ir::common::{BinaryOp, JoinKind};
use cogitator_ir::hir::{Distinct, JoinCond, OrderItem, Resolution};
use cogitator_ir::mir::{print_mir, RelOp, ScalarKind};
use cogitator_ir::{ColType, Nullability};
use common::*;

#[test]
fn order_by_ordinal_reads_the_output() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["a"]);
    let a = b.column(SPAN, "a", t, 0);
    let key = b.number(SPAN, "1");
    let items = vec![item(a, "x", &mut b)];
    let root = select(&mut b, vec![], items, vec![t], |_| {});
    b.hir.queries[root].order_by = vec![OrderItem {
        expr: key,
        desc: None,
        nulls_first: None,
    }];
    let mir = clean(&b.finish(root));
    assert!(
        print_mir(&mir).starts_with("Sort x#"),
        "{}",
        print_mir(&mir)
    );
}

#[test]
fn distinct_stays_a_distinct_node_and_distinct_on_is_opaque() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["a"]);
    let a = b.column(SPAN, "a", t, 0);
    let items = vec![item(a, "", &mut b)];
    let root = select(&mut b, vec![], items, vec![t], |s| {
        s.distinct = Distinct::All
    });
    let mir = clean(&b.finish(root));
    assert_eq!(count_ops(&mir, |op| matches!(op, RelOp::Distinct(_))), 1);

    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["a"]);
    let (a, on) = (b.column(SPAN, "a", t, 0), b.column(SPAN, "a", t, 0));
    let key = b.number(SPAN, "1");
    let items = vec![item(a, "", &mut b)];
    let root = select(&mut b, vec![], items, vec![t], |s| {
        s.distinct = Distinct::On(vec![on])
    });
    b.hir.queries[root].order_by = vec![OrderItem {
        expr: key,
        desc: None,
        nulls_first: None,
    }];
    let lowered = cogitator_lower::lower_to_mir(&b.finish(root));
    lowered.assert_clean();
    let mir = lowered.value.expect("a plan");
    assert_eq!(count_ops(&mir, |op| matches!(op, RelOp::Opaque { .. })), 1);
    assert_eq!(count_ops(&mir, |op| matches!(op, RelOp::Sort { .. })), 1);
}

#[test]
fn outer_join_forces_nullable_on_the_null_supplying_side_only() {
    let mut b = builder();
    let l = typed_from(&mut b, "l", &[("a", int_type())]);
    let r = typed_from(&mut b, "r", &[("b", int_type())]);
    let (la, rb) = (b.column(SPAN, "a", l, 0), b.column(SPAN, "b", r, 0));
    let joined = b.join(SPAN, JoinKind::Left, l, r, JoinCond::None);
    let items = vec![item(la, "", &mut b), item(rb, "", &mut b)];
    let root = select(&mut b, vec![], items, vec![joined], |_| {});
    let mir = clean(&b.finish(root));
    let out = mir.root_columns();
    assert_eq!(
        mir.cols[out[0]].ty,
        int_type(),
        "preserved side keeps not null"
    );
    assert_eq!(mir.cols[out[1]].ty.null, Nullability::Nullable);
    assert_eq!(mir.cols[out[1]].ty.ty, int_type().ty);
}

#[test]
fn a_plain_expression_item_infers_through_the_lattice() {
    let mut b = builder();
    let t = typed_from(&mut b, "t", &[("a", int_type())]);
    let a = b.column(SPAN, "a", t, 0);
    let dec = {
        let sym = b.sym("1.5");
        expr(
            &mut b,
            cogitator_ir::hir::ExprKind::Lit(cogitator_ir::common::Lit::Number(sym)),
        )
    };
    let sum = b.binary(SPAN, BinaryOp::Plus, a, dec);
    let items = vec![item(sum, "x", &mut b)];
    let root = select(&mut b, vec![], items, vec![t], |_| {});
    let mir = clean(&b.finish(root));
    assert!(matches!(
        mir.cols[mir.root_columns()[0]].ty.ty,
        cogitator_ir::SqlType::Decimal { .. }
    ));
}

#[test]
fn a_select_without_from_needs_no_degradation() {
    let mut b = builder();
    let one = b.number(SPAN, "1");
    let items = vec![item(one, "x", &mut b)];
    let root = select(&mut b, vec![], items, vec![], |_| {});
    let lowered = cogitator_lower::lower_to_mir(&b.finish(root));
    lowered.assert_clean();
    assert_eq!(lowered.fidelity, cogitator_ir::Fidelity::Complete);
}

#[test]
fn a_lateral_alias_expands_in_place() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["a"]);
    let a = b.column(SPAN, "a", t, 0);
    let later = b.col_expr(SPAN, "x", Some(Resolution::LateralAlias(0)));
    let items = vec![item(a, "x", &mut b), item(later, "y", &mut b)];
    let root = select(&mut b, vec![], items, vec![t], |_| {});
    let mir = clean(&b.finish(root));
    assert_eq!(mir.root_columns().len(), 2);
    let self_ref = mir
        .scalars
        .values()
        .any(|s| matches!(s.kind, ScalarKind::Opaque { .. }));
    assert!(!self_ref, "{}", print_mir(&mir));
}

#[test]
fn a_self_referencing_lateral_alias_is_opaque() {
    let mut b = builder();
    let x = b.col_expr(SPAN, "x", Some(Resolution::LateralAlias(0)));
    let items = vec![item(x, "x", &mut b)];
    let root = select(&mut b, vec![], items, vec![], |_| {});
    let lowered = cogitator_lower::lower_to_mir(&b.finish(root));
    lowered.assert_clean();
    assert_eq!(lowered.fidelity, cogitator_ir::Fidelity::Partial);
}

#[test]
fn unknown_columns_keep_unknown_types() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["a"]);
    let root = select(&mut b, vec![], vec![star()], vec![t], |_| {});
    let mir = clean(&b.finish(root));
    assert_eq!(mir.cols[mir.root_columns()[0]].ty, ColType::UNKNOWN);
}

//! HIR→MIR: set operations, VALUES, caps and totality on bad input.

mod common;

use cogitator_ir::common::SetOpKind;
use cogitator_ir::hir::{Body, FromKind, GroupBy, RelTarget, Resolution};
use cogitator_ir::mir::{print_mir, RelOp};
use cogitator_ir::{ColType, DegradationKind, Fidelity, Idx, Invariant, Nullability, SqlType};
use cogitator_lower::lower_to_mir;
use common::*;

#[test]
fn set_operations_join_arm_types_and_stay_set_ops() {
    let mut b = builder();
    let l = typed_from(&mut b, "l", &[("a", int_type())]);
    let dec = ColType::new(
        SqlType::Decimal {
            p: Some(10),
            s: Some(2),
        },
        Nullability::Nullable,
    );
    let r = typed_from(&mut b, "r", &[("a", dec)]);
    let (ql, qr) = (
        select(&mut b, vec![], vec![star()], vec![l], |_| {}),
        select(&mut b, vec![], vec![star()], vec![r], |_| {}),
    );
    let root = b.query(
        SPAN,
        vec![],
        Body::SetOp {
            op: SetOpKind::Intersect,
            all: false,
            by_name: false,
            l: ql,
            r: qr,
        },
    );
    let mir = clean(&b.finish(root));
    let out = mir.root_columns()[0];
    assert!(matches!(mir.cols[out].ty.ty, SqlType::Decimal { .. }));
    assert_eq!(mir.cols[out].ty.null, Nullability::Nullable);
    assert!(matches!(
        mir.rels[mir.root].op,
        RelOp::SetOp {
            op: SetOpKind::Intersect,
            ..
        }
    ));
}

#[test]
fn a_long_union_chain_flattens_without_consuming_depth() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["a"]);
    let mut acc = select(&mut b, vec![], vec![star()], vec![t], |_| {});
    for i in 0..200 {
        let next = b.base_model(SPAN, &format!("t{i}"), None, &["a"]);
        let arm = select(&mut b, vec![], vec![star()], vec![next], |_| {});
        acc = b.query(
            SPAN,
            vec![],
            Body::SetOp {
                op: SetOpKind::Union,
                all: true,
                by_name: false,
                l: acc,
                r: arm,
            },
        );
    }
    let mir = clean(&b.finish(acc));
    assert!(matches!(&mir.rels[mir.root].op, RelOp::SetOp { inputs, .. } if inputs.len() == 201));
}

#[test]
fn values_lowers_to_a_union_of_projects() {
    let mut b = builder();
    let (one, two) = (b.number(SPAN, "1"), b.number(SPAN, "2.5"));
    let root = b.query(SPAN, vec![], Body::Values(vec![vec![one], vec![two]]));
    let lowered = lower_to_mir(&b.finish(root));
    lowered.assert_clean();
    assert_eq!(lowered.fidelity, Fidelity::Complete);
    let mir = lowered.value.expect("a plan");
    assert!(matches!(
        mir.cols[mir.root_columns()[0]].ty.ty,
        SqlType::Decimal { .. }
    ));
}

#[test]
fn a_cte_used_before_its_definition_is_opaque() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["a"]);
    let inner = select(&mut b, vec![], vec![star()], vec![t], |_| {});
    let cte = b.cte(SPAN, "c", inner);
    let from = b.base_cte(SPAN, cte, None, &["a"]);
    let root = select(&mut b, vec![], vec![star()], vec![from], |_| {});
    let lowered = lower_to_mir(&b.finish(root));
    lowered.assert_clean();
    assert_eq!(lowered.fidelity, Fidelity::Partial);
}

#[test]
fn query_nesting_past_the_cap_degrades_instead_of_overflowing() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["a"]);
    let mut q = select(&mut b, vec![], vec![star()], vec![t], |_| {});
    for _ in 0..300 {
        let from = add_from(
            &mut b,
            FromKind::Derived {
                q,
                lateral: false,
                alias: None,
            },
            &["a"],
        );
        q = select(&mut b, vec![], vec![star()], vec![from], |_| {});
    }
    let lowered = lower_to_mir(&b.finish(q));
    lowered.assert_clean();
    assert!(lowered
        .degraded
        .iter()
        .any(|d| d.kind == DegradationKind::CapExceeded));
}

#[test]
fn a_missing_root_declines() {
    let hir = builder().hir;
    let lowered = lower_to_mir(&hir);
    assert_eq!(lowered.fidelity, Fidelity::Declined);
    assert!(lowered.value.is_none());
}

#[test]
fn out_of_range_ids_drop_the_level_without_panicking() {
    let mut b = builder();
    let bad = b.col_expr(
        SPAN,
        "a",
        Some(Resolution::Bound {
            from: Idx::from_raw(99),
            col: 0,
        }),
    );
    let missing = cogitator_ir::hir::ExprId::from_raw(4096);
    let items = vec![item(bad, "", &mut b), item(missing, "m", &mut b)];
    let root = select(&mut b, vec![], items, vec![Idx::from_raw(77)], |_| {});
    let lowered = lower_to_mir(&b.finish(root));
    assert!(lowered.value.is_none());
    assert_eq!(lowered.fidelity, Fidelity::Declined);
    assert!(lowered
        .violations
        .iter()
        .any(|v| v.invariant == Invariant::HirIdRange));
}

#[test]
fn a_query_id_cycle_stops_at_the_depth_cap() {
    let mut b = builder();
    let from = add_from(
        &mut b,
        FromKind::Derived {
            q: Idx::from_raw(0),
            lateral: false,
            alias: None,
        },
        &[],
    );
    let root = select(&mut b, vec![], vec![star()], vec![from], |_| {});
    assert_eq!(root.raw(), 0, "the cycle above relies on this id");
    let lowered = lower_to_mir(&b.finish(root));
    assert!(lowered
        .degraded
        .iter()
        .any(|d| d.kind == DegradationKind::CapExceeded));
}

#[test]
fn lowering_is_deterministic() {
    let build = || {
        let mut b = builder();
        let t = b.base_model(SPAN, "t", None, &["a", "b"]);
        let (k, v) = (b.column(SPAN, "a", t, 0), b.column(SPAN, "b", t, 1));
        let key = b.column(SPAN, "a", t, 0);
        let total = call(&mut b, "sum", vec![v]);
        let items = vec![item(k, "", &mut b), item(total, "s", &mut b)];
        let root = select(&mut b, vec![], items, vec![t], |s| {
            s.group_by = GroupBy::Exprs(vec![key]);
        });
        print_mir(&clean(&b.finish(root)))
    };
    assert_eq!(build(), build());
}

#[test]
fn a_cte_target_outside_the_arena_is_a_violation_not_a_panic() {
    let mut b = builder();
    let from = add_from(
        &mut b,
        FromKind::Base {
            target: RelTarget::Cte(Idx::from_raw(5)),
            alias: None,
        },
        &["a"],
    );
    let root = select(
        &mut b,
        vec![Idx::from_raw(5)],
        vec![star()],
        vec![from],
        |_| {},
    );
    let lowered = lower_to_mir(&b.finish(root));
    assert!(lowered.value.is_none());
}

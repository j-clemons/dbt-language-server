//! HIR→MIR: CTE sharing, correlation, holes and open schemas.

mod common;

use cogitator_ir::common::{BinaryOp, HoleKind, JoinKind, SetOpKind};
use cogitator_ir::hir::{Body, ExprKind, FromKind, Resolution};
use cogitator_ir::mir::{print_mir, HirNode, Origin, RelOp, ScalarKind};
use cogitator_ir::{DegradationKind, Fidelity};
use cogitator_lower::lower_to_mir;
use common::*;

#[test]
fn cte_references_share_only_through_cte_ref() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["a"]);
    let inner = select(&mut b, vec![], vec![star()], vec![t], |_| {});
    let cte = b.cte(SPAN, "c", inner);
    let c1 = b.base_cte(SPAN, cte, Some("x"), &["a"]);
    let c2 = b.base_cte(SPAN, cte, Some("y"), &["a"]);
    let joined = b.join(
        SPAN,
        JoinKind::Cross,
        c1,
        c2,
        cogitator_ir::hir::JoinCond::None,
    );
    let root = select(&mut b, vec![cte], vec![star()], vec![joined], |_| {});
    let mir = clean(&b.finish(root));
    assert_tree(&mir);
    assert_eq!(mir.ctes.len(), 1);
    assert_eq!(count_ops(&mir, |op| matches!(op, RelOp::CteRef { .. })), 2);
    insta::assert_snapshot!(print_mir(&mir));
}

#[test]
fn cte_column_list_renames_its_output() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["a"]);
    let inner = select(&mut b, vec![], vec![star()], vec![t], |_| {});
    let cte = b.cte(SPAN, "c", inner);
    let renamed = b.ident(SPAN, "renamed");
    b.hir.ctes[cte].columns = vec![renamed];
    let from = b.base_cte(SPAN, cte, None, &["renamed"]);
    let root = select(&mut b, vec![cte], vec![star()], vec![from], |_| {});
    let printed = print_mir(&clean(&b.finish(root)));
    assert!(printed.contains("renamed#"), "{printed}");
}

#[test]
fn provenance_reaches_the_hir_for_every_node() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["a"]);
    let a = b.column(SPAN, "a", t, 0);
    let items = vec![item(a, "", &mut b)];
    let root = select(&mut b, vec![], items, vec![t], |_| {});
    let mir = clean(&b.finish(root));
    for id in mir.reachable() {
        let origin = &mir.prov.entries[mir.rels[id].prov];
        assert!(matches!(origin, Origin::Hir(r) if r.node != HirNode::None));
    }
}

#[test]
fn correlated_subquery_declares_its_outer_columns() {
    let mut b = builder();
    let outer = b.base_model(SPAN, "o", None, &["a"]);
    let inner = b.base_model(SPAN, "i", None, &["b"]);
    let ib = b.column(SPAN, "b", inner, 0);
    let oa = outer_col(&mut b, "a", outer, 0);
    let cond = b.binary(SPAN, BinaryOp::Eq, ib, oa);
    let sub = select(&mut b, vec![], vec![star()], vec![inner], |s| {
        s.filter = Some(cond)
    });
    let exists = expr(&mut b, ExprKind::Exists(sub));
    let root = select(&mut b, vec![], vec![star()], vec![outer], |s| {
        s.filter = Some(exists)
    });
    let mir = clean(&b.finish(root));
    let correlated = mir.scalars.values().find_map(|s| match &s.kind {
        ScalarKind::Subquery { correlated, .. } => Some(correlated.clone()),
        _ => None,
    });
    assert_eq!(correlated, Some(mir.root_columns().to_vec()));
    insta::assert_snapshot!(print_mir(&mir));
}

#[test]
fn doubly_nested_correlation_is_declared_at_every_level() {
    let mut b = builder();
    let outer = b.base_model(SPAN, "o", None, &["a"]);
    let middle = b.base_model(SPAN, "m", None, &["b"]);
    let inner = b.base_model(SPAN, "i", None, &["c"]);
    let ic = b.column(SPAN, "c", inner, 0);
    let oa = outer_col(&mut b, "a", outer, 0);
    let cond = b.binary(SPAN, BinaryOp::Eq, ic, oa);
    let deepest = select(&mut b, vec![], vec![star()], vec![inner], |s| {
        s.filter = Some(cond)
    });
    let exists = expr(&mut b, ExprKind::Exists(deepest));
    let mid = select(&mut b, vec![], vec![star()], vec![middle], |s| {
        s.filter = Some(exists)
    });
    let outer_exists = expr(&mut b, ExprKind::Exists(mid));
    let root = select(&mut b, vec![], vec![star()], vec![outer], |s| {
        s.filter = Some(outer_exists)
    });
    clean(&b.finish(root));
}

#[test]
fn a_correlated_lateral_derived_table_is_opaque() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["a"]);
    let a = outer_col(&mut b, "a", t, 0);
    let items = vec![item(a, "x", &mut b)];
    let inner = select(&mut b, vec![], items, vec![], |_| {});
    let lateral = add_from(
        &mut b,
        FromKind::Derived {
            q: inner,
            lateral: true,
            alias: None,
        },
        &["x"],
    );
    let root = select(&mut b, vec![], vec![star()], vec![t, lateral], |_| {});
    let lowered = lower_to_mir(&b.finish(root));
    lowered.assert_clean();
    assert_eq!(lowered.fidelity, Fidelity::Partial);
}

#[test]
fn a_body_hole_is_opaque_and_keeps_the_rest_of_the_plan() {
    let mut b = builder();
    let hole = b.query(SPAN, vec![], Body::Hole(HoleKind::ParseError));
    let cte = b.cte(SPAN, "c", hole);
    let from = b.base_cte(SPAN, cte, None, &[]);
    open(&mut b, from);
    let col = open_col(&mut b, "x", from);
    let items = vec![item(col, "", &mut b)];
    let root = select(&mut b, vec![cte], items, vec![from], |_| {});
    let lowered = lower_to_mir(&b.finish(root));
    lowered.assert_clean();
    assert_eq!(lowered.fidelity, Fidelity::Partial);
    assert!(lowered
        .degraded
        .iter()
        .any(|d| d.kind == DegradationKind::ParseHole));
    insta::assert_snapshot!(print_mir(&lowered.value.expect("a plan")));
}

#[test]
fn an_expression_hole_is_opaque_with_children_retained() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &["a"]);
    let a = b.column(SPAN, "a", t, 0);
    let opaque = expr(&mut b, ExprKind::Opaque { children: vec![a] });
    let hole = expr(&mut b, ExprKind::Hole);
    let items = vec![item(opaque, "x", &mut b), item(hole, "y", &mut b)];
    let root = select(&mut b, vec![], items, vec![t], |_| {});
    let lowered = lower_to_mir(&b.finish(root));
    lowered.assert_clean();
    let mir = lowered.value.expect("a plan");
    let kept = mir
        .scalars
        .values()
        .any(|s| matches!(&s.kind, ScalarKind::Opaque { children, .. } if children.len() == 1));
    assert!(kept);
}

#[test]
fn a_star_over_an_open_schema_keeps_inputs_in_an_opaque_output() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &[]);
    open(&mut b, t);
    let a = open_col(&mut b, "a", t);
    let one = b.number(SPAN, "1");
    let cond = b.binary(SPAN, BinaryOp::Eq, a, one);
    let root = select(&mut b, vec![], vec![star()], vec![t], |s| {
        s.filter = Some(cond)
    });
    let lowered = lower_to_mir(&b.finish(root));
    lowered.assert_clean();
    let mir = lowered.value.expect("a plan");
    let RelOp::Opaque { inputs, open, .. } = &mir.rels[mir.root].op else {
        panic!("{}", print_mir(&mir));
    };
    assert!(*open);
    assert!(matches!(mir.rels[inputs[0]].op, RelOp::Filter { .. }));
    assert!(lowered
        .degraded
        .iter()
        .any(|d| d.kind == DegradationKind::OpenSchema));
}

#[test]
fn open_scans_carry_only_the_columns_the_query_reads() {
    let mut b = builder();
    let t = b.base_model(SPAN, "t", None, &[]);
    open(&mut b, t);
    let a = open_col(&mut b, "a", t);
    let items = vec![item(a, "", &mut b)];
    let root = select(&mut b, vec![], items, vec![t], |_| {});
    let mir = clean(&b.finish(root));
    let printed = print_mir(&mir);
    assert!(
        printed.contains("[a#0:") && printed.contains(" open"),
        "{printed}"
    );
}

#[test]
fn the_cte_over_open_star_pattern_lowers_with_demanded_columns() {
    // with src as (select * from ref), final as (select a from src) select * from final
    let mut b = builder();
    let base = b.base_model(SPAN, "raw", None, &[]);
    open(&mut b, base);
    let src_q = select(&mut b, vec![], vec![star()], vec![base], |_| {});
    let src = b.cte(SPAN, "src", src_q);
    let from_src = b.base_cte(SPAN, src, None, &[]);
    open(&mut b, from_src);
    let a = open_col(&mut b, "a", from_src);
    let items = vec![item(a, "", &mut b)];
    let final_q = select(&mut b, vec![], items, vec![from_src], |_| {});
    let fin = b.cte(SPAN, "final", final_q);
    let from_final = b.base_cte(SPAN, fin, None, &["a"]);
    let root = select(
        &mut b,
        vec![src, fin],
        vec![star()],
        vec![from_final],
        |_| {},
    );
    let lowered = lower_to_mir(&b.finish(root));
    lowered.assert_clean();
    let mir = lowered.value.expect("a plan");
    assert_eq!(mir.root_columns().len(), 1);
    insta::assert_snapshot!(print_mir(&mir));
}

#[test]
fn an_ambiguous_open_reference_is_opaque_rather_than_guessed() {
    let mut b = builder();
    let (x, y) = (
        b.base_model(SPAN, "x", None, &[]),
        b.base_model(SPAN, "y", None, &[]),
    );
    open(&mut b, x);
    open(&mut b, y);
    let col = b.col_expr(SPAN, "a", Some(Resolution::Candidates(vec![x, y])));
    let items = vec![item(col, "", &mut b)];
    let root = select(&mut b, vec![], items, vec![x, y], |_| {});
    let lowered = lower_to_mir(&b.finish(root));
    lowered.assert_clean();
    assert_eq!(lowered.fidelity, Fidelity::Partial);
}

#[test]
fn a_recursive_cte_is_opaque_and_keeps_its_anchor() {
    let mut b = builder();
    let base = b.base_model(SPAN, "seed", None, &["a"]);
    let anchor = select(&mut b, vec![], vec![star()], vec![base], |_| {});
    let t2 = b.base_model(SPAN, "t2", None, &["a"]);
    let term = select(&mut b, vec![], vec![star()], vec![t2], |_| {});
    let body = b.query(
        SPAN,
        vec![],
        Body::SetOp {
            op: SetOpKind::Union,
            all: true,
            by_name: false,
            l: anchor,
            r: term,
        },
    );
    let cte = b.cte(SPAN, "r", body);
    b.hir.ctes[cte].recursive = true;
    let from = b.base_cte(SPAN, cte, None, &["a"]);
    let root = select(&mut b, vec![cte], vec![star()], vec![from], |_| {});
    let lowered = lower_to_mir(&b.finish(root));
    lowered.assert_clean();
    assert!(lowered
        .degraded
        .iter()
        .any(|d| d.kind == DegradationKind::RecursiveCte));
    let mir = lowered.value.expect("a plan");
    let body = mir.ctes.values().next().expect("cte").body;
    assert!(matches!(&mir.rels[body].op, RelOp::Opaque { inputs, .. } if inputs.len() == 1));
    insta::assert_snapshot!(print_mir(&mir));
}

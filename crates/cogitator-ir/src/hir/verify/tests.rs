//! Each HIR invariant, shown to be able to fail. Each test starts from a
//! clean plan and breaks exactly one thing.

use crate::common::BinaryOp;
use crate::hir::{
    verify_hir, Body, ExprKind, Hir, JinjaKind, JinjaRole, QueryId, RelTarget, ScopeId, SelectItem,
};
use crate::testkit::HirBuilder;
use crate::{Invariant, Span};

const TEXT: &str = "with a as (select 1 as x) select x from a where x = 1 -- é";

fn clean() -> Hir {
    let mut b = HirBuilder::new(TEXT);
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
    let from_a = b.base_cte(b.span_of("from a"), cte, None, &["x"]);
    let col = b.column(b.span_of("x from"), "x", from_a, 0);
    let filter_l = b.column(b.span_of("x = 1"), "x", from_a, 0);
    let filter_r = b.number(b.span_of("1"), "1");
    let filter = b.binary(b.span_of("x = 1"), BinaryOp::Eq, filter_l, filter_r);
    let root = b.select(
        b.span_of("select x from a"),
        vec![cte],
        vec![SelectItem::Expr {
            expr: col,
            alias: None,
        }],
        vec![from_a],
        Some(filter),
    );
    b.finish(root)
}

fn assert_only(hir: &Hir, invariant: Invariant) {
    let violations = verify_hir(hir, TEXT);
    assert!(
        !violations.is_empty() && violations.iter().all(|v| v.invariant == invariant),
        "expected only {invariant:?}, got {violations:#?}"
    );
}

#[test]
fn clean_plan_verifies() {
    crate::assert_clean(&verify_hir(&clean(), TEXT));
}

#[test]
fn id_range() {
    let mut hir = clean();
    hir.root = Some(QueryId::from_raw(99));
    assert_only(&hir, Invariant::HirIdRange);
}

#[test]
fn bound_col_out_of_output_is_id_range() {
    let mut hir = clean();
    let colref = hir
        .cols
        .ids()
        .next()
        .expect("clean plan has a column reference");
    hir.cols[colref].res = Some(crate::hir::Resolution::Bound {
        from: crate::hir::FromId::from_raw(0),
        col: 7,
    });
    assert_only(&hir, Invariant::HirIdRange);
}

#[test]
fn scope_tree_rejects_two_roots_and_cycles() {
    let mut hir = clean();
    let extra = hir.scopes.ids().nth(1).expect("query scope");
    hir.scopes[extra].parent = None;
    assert_only(&hir, Invariant::HirScopeTree);

    let mut hir = clean();
    let a: ScopeId = hir.scopes.ids().nth(1).expect("scope");
    let b: ScopeId = hir.scopes.ids().nth(2).expect("scope");
    hir.scopes[a].parent = Some(b);
    hir.scopes[b].parent = Some(a);
    assert_only(&hir, Invariant::HirScopeTree);
}

#[test]
fn col_refs_must_be_bound() {
    let mut hir = clean();
    let colref = hir.cols.ids().next().expect("col");
    hir.cols[colref].res = None;
    assert_only(&hir, Invariant::HirColRefResolved);
}

#[test]
fn cte_must_be_declared_before_use() {
    let mut hir = clean();
    let root = hir.root.expect("root");
    hir.queries[root].ctes.clear();
    assert_only(&hir, Invariant::HirCteBackward);
}

#[test]
fn recursive_cte_may_reference_itself() {
    let mut hir = clean();
    let cte_id = hir.ctes.ids().next().expect("cte");
    let inner = hir.ctes[cte_id].query;
    hir.ctes[cte_id].recursive = true;
    let self_ref = crate::hir::From {
        span: Span::new(0, 1),
        kind: crate::hir::FromKind::Base {
            target: RelTarget::Cte(cte_id),
            alias: None,
        },
        output: vec![],
        open: true,
    };
    let from = hir.from.alloc(self_ref);
    if let Body::Select(select) = &mut hir.queries[inner].body {
        select.from.push(from);
    }
    crate::assert_clean(&verify_hir(&hir, TEXT));
}

#[test]
fn jinja_must_be_consumed_or_standalone() {
    let mut hir = clean();
    hir.jinja.alloc(crate::hir::JinjaNode {
        span: Span::new(0, 4),
        kind: JinjaKind::Expression,
        role: JinjaRole::Scalar,
    });
    assert_only(&hir, Invariant::HirJinjaConsumed);

    let mut standalone = clean();
    standalone.jinja.alloc(crate::hir::JinjaNode {
        span: Span::new(0, 4),
        kind: JinjaKind::Comment,
        role: JinjaRole::Blank,
    });
    crate::assert_clean(&verify_hir(&standalone, TEXT));
}

#[test]
fn jinja_consumed_twice_is_rejected() {
    let mut b = HirBuilder::new(TEXT);
    let j = b.jinja(Span::new(0, 4), JinjaKind::Var, JinjaRole::Scalar);
    let e1 = b.expr(Span::new(0, 4), ExprKind::Jinja(j));
    let e2 = b.expr(Span::new(0, 4), ExprKind::Jinja(j));
    let root = b.select(
        Span::new(0, 4),
        vec![],
        vec![
            SelectItem::Expr {
                expr: e1,
                alias: None,
            },
            SelectItem::Expr {
                expr: e2,
                alias: None,
            },
        ],
        vec![],
        None,
    );
    assert_only(&b.finish(root), Invariant::HirJinjaConsumed);
}

#[test]
fn spans_on_char_boundaries() {
    let mut hir = clean();
    let expr = hir.exprs.ids().next().expect("expr");
    let bad_start = TEXT.len() as u32 - 1;
    hir.exprs[expr].span = Span::new(bad_start, bad_start + 1);
    assert_only(&hir, Invariant::HirSpanBoundary);
}

#[test]
fn printing_is_deterministic_and_total() {
    let hir = clean();
    let a = crate::hir::print_hir(&hir);
    assert_eq!(a, crate::hir::print_hir(&hir));
    assert!(a.contains("select"));
    let mut broken = hir;
    broken.root = Some(QueryId::from_raw(50));
    assert_eq!(crate::hir::print_hir(&broken), "<bad query>\n");
}

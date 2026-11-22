//! Each MIR invariant, shown to be able to fail.

use crate::common::{BinaryOp, JoinKind};
use crate::mir::{
    verify_mir, verify_rewrite, Mir, Origin, ProvId, RelId, RelOp, ScalarKind, SubqueryKind,
};
use crate::testkit::{int_col, text_col, MirBuilder};
use crate::{ColType, Invariant, Nullability, SqlType};

/// scan -> filter(a = 1) -> project(a, b)
fn clean_with_builder() -> (MirBuilder, RelId) {
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("a", int_col()), ("b", text_col())]);
    let cols = b.out(scan);
    let a = b.col_ref(cols[0]);
    let one = b.number("1");
    let pred = b.binary(BinaryOp::Eq, a, one);
    let filter = b.filter(scan, vec![pred]);
    let pa = b.col_ref(cols[0]);
    let pb = b.col_ref(cols[1]);
    let project = b.project(filter, vec![(cols[0], pa), (cols[1], pb)]);
    (b, project)
}

fn clean() -> Mir {
    let (b, root) = clean_with_builder();
    b.finish(root)
}

fn assert_only(mir: &Mir, invariant: Invariant) {
    let violations = verify_mir(mir, None);
    assert!(
        !violations.is_empty() && violations.iter().all(|v| v.invariant == invariant),
        "expected only {invariant:?}, got {violations:#?}"
    );
}

#[test]
fn clean_plan_verifies() {
    crate::assert_clean(&verify_mir(&clean(), None));
}

#[test]
fn id_range() {
    let mut mir = clean();
    mir.root = RelId::from_raw(40);
    assert_only(&mir, Invariant::MirIdRange);
}

#[test]
fn operands_must_precede_users() {
    let mut mir = clean();
    let root = mir.root;
    let filter = match mir.rels[root].op {
        RelOp::Project { input, .. } => input,
        _ => panic!("project"),
    };
    if let RelOp::Filter { input, .. } = &mut mir.rels[filter].op {
        *input = root;
    }
    assert_only(&mir, Invariant::MirIdRange);
}

#[test]
fn a_column_is_defined_once() {
    let (mut b, root) = clean_with_builder();
    // a second scan redefining column a
    let cols = b.out(root);
    let table = b.table("u");
    let scan2 = b.rel(RelOp::Scan {
        table,
        cols: vec![cols[0]],
        open: false,
    });
    let joined = b.join(JoinKind::Cross, root, scan2, vec![]);
    assert_only(&b.finish(joined), Invariant::MirColDefinedOnce);
}

#[test]
fn passthrough_is_not_a_redefinition() {
    let (mut b, root) = clean_with_builder();
    let cols = b.out(root);
    let a = b.col_ref(cols[0]);
    let outer = b.project(root, vec![(cols[0], a)]);
    crate::assert_clean(&verify_mir(&b.finish(outer), None));
}

#[test]
fn columns_must_be_visible() {
    let mut b = MirBuilder::new();
    let t = b.scan("t", &[("a", int_col())]);
    let u = b.scan("u", &[("b", int_col())]);
    let hidden = b.out(u)[0];
    let r = b.col_ref(hidden);
    let filter = b.filter(t, vec![r]);
    let joined = b.join(JoinKind::Cross, filter, u, vec![]);
    assert_only(&b.finish(joined), Invariant::MirColVisible);
}

#[test]
fn correlated_column_must_be_declared_and_bound() {
    // subquery reading t.a through Outer, but the Exists declares no
    // correlation: the inner read is not declared
    let mut b = MirBuilder::new();
    let t = b.scan("t", &[("a", int_col())]);
    let a = b.out(t)[0];
    let u = b.scan("u", &[("b", int_col())]);
    let outer = b.scalar(ScalarKind::Outer(a), int_col());
    let inner = b.filter(u, vec![outer]);
    let sub = b.scalar(
        ScalarKind::Subquery {
            kind: SubqueryKind::Exists,
            rel: inner,
            correlated: vec![],
        },
        int_col(),
    );
    let filter = b.filter(t, vec![sub]);
    assert_only(&b.finish(filter), Invariant::MirColVisible);
}

#[test]
fn correlation_declared_and_visible_is_clean() {
    let mut b = MirBuilder::new();
    let t = b.scan("t", &[("a", int_col())]);
    let a = b.out(t)[0];
    let u = b.scan("u", &[("b", int_col())]);
    let outer = b.scalar(ScalarKind::Outer(a), int_col());
    let inner = b.filter(u, vec![outer]);
    let sub = b.scalar(
        ScalarKind::Subquery {
            kind: SubqueryKind::Exists,
            rel: inner,
            correlated: vec![a],
        },
        int_col(),
    );
    let filter = b.filter(t, vec![sub]);
    crate::assert_clean(&verify_mir(&b.finish(filter), None));
}

#[test]
fn declared_output_must_match() {
    let mut mir = clean();
    let root = mir.root;
    mir.rels[root].out.pop();
    assert_only(&mir, Invariant::MirDeclaredOutput);
}

#[test]
fn cte_cycles_are_rejected() {
    let mut b = MirBuilder::new();
    let t = b.scan("t", &[("a", int_col())]);
    let c1 = b.cte("c1", t);
    let c1_ref = b.cte_ref(c1);
    let c2 = b.cte("c2", c1_ref);
    let c2_ref = b.cte_ref(c2);
    // point c1's body at c2_ref: c1 -> c2 -> c1
    b.mir.ctes[c1].body = c2_ref;
    b.mir.ctes[c1].out = b.out(c2_ref);
    let c1_ref_again = b.cte_ref(c1);
    let root = b.distinct(c1_ref_again);
    assert_only(&b.finish(root), Invariant::MirCteAcyclic);
}

#[test]
fn provenance_is_total() {
    let mut mir = clean();
    let root = mir.root;
    mir.rels[root].prov = ProvId::from_raw(1000);
    assert_only(&mir, Invariant::MirProvenanceTotal);
}

#[test]
fn broken_derivation_is_rejected() {
    let mut mir = clean();
    let prov = mir.prov.add(Origin::Derived {
        rule: crate::RuleId("canon.simplify"),
        from: vec![],
    });
    let root = mir.root;
    mir.rels[root].prov = prov;
    assert_only(&mir, Invariant::MirProvenanceTotal);
}

#[test]
fn hir_provenance_checks_against_the_hir() {
    let mut mir = clean();
    let bad = mir.prov.add(Origin::Hir(crate::mir::HirRef {
        node: crate::mir::HirNode::Query(crate::hir::QueryId::from_raw(3)),
        span: crate::Span::default(),
    }));
    let root = mir.root;
    mir.rels[root].prov = bad;
    let hir = crate::hir::Hir::new(crate::origin::SourceOrigin::Template {
        variant: crate::origin::TemplateVariant::Plain,
    });
    let violations = verify_mir(&mir, Some(&hir));
    assert!(violations
        .iter()
        .any(|v| v.invariant == Invariant::MirProvenanceTotal));
    crate::assert_clean(&verify_mir(&mir, None));
}

#[test]
fn column_reference_may_not_lose_type_specificity() {
    let (mut b, root) = clean_with_builder();
    let cols = b.out(root);
    let weak = b.scalar(ScalarKind::Col(cols[0]), ColType::UNKNOWN);
    let filter = b.filter(root, vec![weak]);
    assert_only(&b.finish(filter), Invariant::MirTypeSpecificity);
}

#[test]
fn nullability_may_widen_on_a_reference() {
    let (mut b, root) = clean_with_builder();
    let cols = b.out(root);
    let nullable = ColType::new(SqlType::Int { bits: Some(64) }, Nullability::Nullable);
    let widened = b.scalar(ScalarKind::Col(cols[0]), nullable);
    let filter = b.filter(root, vec![widened]);
    crate::assert_clean(&verify_mir(&b.finish(filter), None));
}

#[test]
fn rewrite_must_keep_width_names_and_specificity() {
    let (mut b, root) = clean_with_builder();
    let cols = b.out(root);
    let a_ref = b.col_ref(cols[0]);
    let renamed_col = b.col("other", int_col());
    let dropped = b.project(root, vec![(renamed_col, a_ref)]);
    let widened_col = b.col("a", text_col());
    let b_ref = b.col_ref(cols[1]);
    let kept_b = b.col("b", text_col());
    let widened = b.project(root, vec![(widened_col, a_ref), (kept_b, b_ref)]);
    let renamed_b = b.col("c", text_col());
    let renamed = b.project(root, vec![(cols[0], a_ref), (renamed_b, b_ref)]);
    let good = b.project(root, vec![(cols[0], a_ref), (cols[1], b_ref)]);

    let mut mir = b.finish(dropped);
    let width = verify_rewrite(&mir, root);
    assert_eq!(width.len(), 1, "{width:#?}");
    assert_eq!(width[0].invariant, Invariant::MirRootColumns);

    mir.root = widened;
    let spec = verify_rewrite(&mir, root);
    assert_eq!(spec.len(), 1, "{spec:#?}");
    assert_eq!(spec[0].invariant, Invariant::MirTypeSpecificity);

    mir.root = renamed;
    assert_eq!(
        verify_rewrite(&mir, root)[0].invariant,
        Invariant::MirRootColumns
    );

    mir.root = good;
    crate::assert_clean(&verify_rewrite(&mir, root));
}

#[test]
fn plan_hash_ignores_ids_and_detects_changes() {
    let (mut b, root) = clean_with_builder();
    let cols = b.out(root);
    // the same project rebuilt over the same input: new ids, same plan
    let input = match b.mir.rels[root].op {
        RelOp::Project { input, .. } => input,
        _ => panic!("project"),
    };
    let a_ref = b.col_ref(cols[0]);
    let b_ref = b.col_ref(cols[1]);
    let rebuilt = b.project(input, vec![(cols[0], a_ref), (cols[1], b_ref)]);
    let wider = b.filter(root, vec![a_ref]);
    let mut mir = b.finish(root);
    let original = crate::mir::plan_hash(&mir);
    mir.root = rebuilt;
    assert_eq!(crate::mir::plan_hash(&mir), original);
    mir.root = wider;
    assert_ne!(crate::mir::plan_hash(&mir), original);
}

#[test]
fn printing_is_total_on_bad_ids() {
    let mut mir = clean();
    mir.root = RelId::from_raw(500);
    assert_eq!(crate::mir::print_mir(&mir), "<bad rel>\n");
}

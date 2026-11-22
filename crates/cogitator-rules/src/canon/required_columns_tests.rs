//! `canon.required_columns`.

use cogitator_ir::common::{BinaryOp, HoleKind, HoleRef, SetOpKind};
use cogitator_ir::mir::{ColId, Mir, RelOp, ScalarKind};
use cogitator_ir::testkit::{int_col, MirBuilder};
use cogitator_ir::{ColType, Span};

use super::required_columns::RequiredColumns;
use super::test_util::{apply, assert_stable};

fn scan_cols(mir: &Mir) -> Vec<Vec<ColId>> {
    mir.reachable()
        .into_iter()
        .filter_map(|id| match &mir.rels[id].op {
            RelOp::Scan { cols, .. } => Some(cols.clone()),
            _ => None,
        })
        .collect()
}

/// `select a from (select a, b, c from t)`, the inner query's `b` and
/// `c` being unused.
fn nested_select() -> (MirBuilder, Vec<ColId>, cogitator_ir::mir::RelId) {
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("a", int_col()), ("b", int_col()), ("c", int_col())]);
    let cols = b.out(scan);
    let items = cols.iter().map(|c| (*c, b.col_ref(*c))).collect();
    let inner = b.project(scan, items);
    (b, cols, inner)
}

#[test]
fn a_nested_select_narrows_to_what_the_root_reads() {
    let (mut b, cols, inner) = nested_select();
    let a = b.col_ref(cols[0]);
    let root = b.project(inner, vec![(cols[0], a)]);
    let (mir, text, changed) = apply(&RequiredColumns, b.finish(root));
    assert!(changed);
    assert_eq!(scan_cols(&mir), vec![vec![cols[0]]]);
    insta::assert_snapshot!(text);
    assert_stable(&RequiredColumns, mir);
}

#[test]
fn a_filter_keeps_the_columns_it_reads() {
    let (mut b, cols, inner) = nested_select();
    let bb = b.col_ref(cols[1]);
    let one = b.number("1");
    let pred = b.binary(BinaryOp::Gt, bb, one);
    let filter = b.filter(inner, vec![pred]);
    let a = b.col_ref(cols[0]);
    let root = b.project(filter, vec![(cols[0], a)]);
    let (mir, _, changed) = apply(&RequiredColumns, b.finish(root));
    assert!(changed);
    assert_eq!(scan_cols(&mir), vec![vec![cols[0], cols[1]]]);
}

#[test]
fn root_columns_are_never_dropped() {
    let (b, cols, inner) = nested_select();
    let (mir, _, changed) = apply(&RequiredColumns, b.finish(inner));
    assert!(!changed);
    assert_eq!(scan_cols(&mir), vec![cols]);
}

#[test]
fn open_scans_stay_open_and_unnarrowed() {
    let mut b = MirBuilder::new();
    let table = b.table("t");
    let a = b.col("a", int_col());
    let unused = b.col("b", int_col());
    let scan = b.rel(RelOp::Scan {
        table,
        cols: vec![a, unused],
        open: true,
    });
    let r = b.col_ref(a);
    let root = b.project(scan, vec![(a, r)]);
    let (mir, _, changed) = apply(&RequiredColumns, b.finish(root));
    assert!(!changed);
    let RelOp::Scan { open, ref cols, .. } = mir.rels[scan].op else {
        panic!("scan");
    };
    assert!(open);
    assert_eq!(cols, &vec![a, unused]);
}

#[test]
fn distinct_and_set_ops_need_every_column() {
    let (mut b, cols, inner) = nested_select();
    let distinct = b.distinct(inner);
    let a = b.col_ref(cols[0]);
    let root = b.project(distinct, vec![(cols[0], a)]);
    let (_, _, changed) = apply(&RequiredColumns, b.finish(root));
    assert!(!changed, "DISTINCT compares every column");

    let (mut b, cols, inner) = nested_select();
    let other = b.scan("u", &[("x", int_col()), ("y", int_col()), ("z", int_col())]);
    let out: Vec<ColId> = cols
        .iter()
        .map(|c| b.col("u", b.mir.cols[*c].ty.clone()))
        .collect();
    let union = b.set_op(SetOpKind::Union, true, vec![inner, other], out.clone());
    let a = b.col_ref(out[0]);
    let root = b.project(union, vec![(out[0], a)]);
    let (_, _, changed) = apply(&RequiredColumns, b.finish(root));
    assert!(!changed, "set operations align by position");
}

#[test]
fn an_opaque_expression_may_read_any_input_column() {
    let (mut b, cols, inner) = nested_select();
    let hole = b.scalar(
        ScalarKind::Opaque {
            children: vec![],
            hole: HoleRef {
                kind: HoleKind::Jinja,
                span: Span::default(),
            },
        },
        ColType::UNKNOWN,
    );
    let filter = b.filter(inner, vec![hole]);
    let a = b.col_ref(cols[0]);
    let root = b.project(filter, vec![(cols[0], a)]);
    let (_, _, changed) = apply(&RequiredColumns, b.finish(root));
    assert!(!changed);
}

#[test]
fn cte_columns_narrow_by_position_across_every_reference() {
    let (mut b, _cols, inner) = nested_select();
    let cte = b.cte("c", inner);
    let first = b.cte_ref(cte);
    let second = b.cte_ref(cte);
    let (first_out, second_out) = (b.out(first), b.out(second));
    let (x, y) = (b.col_ref(first_out[0]), b.col_ref(second_out[1]));
    let join = b.join(cogitator_ir::common::JoinKind::Cross, first, second, vec![]);
    let (cx, cy) = (b.col("x", int_col()), b.col("y", int_col()));
    let root = b.project(join, vec![(cx, x), (cy, y)]);
    let (mir, text, changed) = apply(&RequiredColumns, b.finish(root));
    assert!(changed);
    assert_eq!(scan_cols(&mir).len(), 1);
    assert_eq!(scan_cols(&mir)[0].len(), 2, "{text}");
    insta::assert_snapshot!(text);
    assert_stable(&RequiredColumns, mir);
}

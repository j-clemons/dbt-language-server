//! MIR→LIR over hand-built plans: pruning-key usage.

mod plan;

use cogitator_ir::common::BinaryOp;
use cogitator_ir::lir::{
    print_lir, Backend, KeyKind, Lir, PhysColumn, PhysOp, PhysicalFacts, PruneUse, TableFacts,
};
use cogitator_ir::mir::{ColId, Mir, ScalarId};
use cogitator_ir::testkit::{int_col, MirBuilder};
use cogitator_ir::{ColType, Nullability, SqlType};

use crate::backends::for_backend;
use crate::lower_to_lir;

fn ts_col() -> ColType {
    ColType::new(
        SqlType::Timestamp {
            tz: cogitator_ir::types::Tz::Unknown,
        },
        Nullability::Unknown,
    )
}

fn backend(b: Backend) -> &'static dyn crate::DialectBackend {
    for_backend(b).expect("registered")
}

fn lower(mir: &Mir, b: Backend, phys: &PhysicalFacts) -> Lir {
    let lowered = lower_to_lir(mir, backend(b), phys);
    lowered.assert_clean();
    lowered.value.expect("a plan")
}

/// `t(ts partition, a cluster, b cluster, c)` scanned as table 0.
struct Fixture {
    b: MirBuilder,
    scan: cogitator_ir::mir::RelId,
    cols: Vec<ColId>,
    phys: PhysicalFacts,
}

fn fixture() -> Fixture {
    fixture_reading(&["ts", "a", "b", "c"])
}

/// The same table when the query reads only `names`.
fn fixture_reading(names: &[&str]) -> Fixture {
    let mut b = MirBuilder::new();
    let read: Vec<_> = names
        .iter()
        .map(|n| (*n, if *n == "ts" { ts_col() } else { int_col() }))
        .collect();
    let scan = b.scan("t", &read);
    let cols = b.out(scan);
    let columns = [
        ("ts", KeyKind::Partition),
        ("a", KeyKind::Cluster),
        ("b", KeyKind::Cluster),
    ]
    .iter()
    .map(|(name, key)| PhysColumn {
        name: b.sym(name),
        ty: SqlType::Unknown,
        key: Some(*key),
    })
    .collect();
    let mut phys = PhysicalFacts::default();
    phys.tables.insert(
        0,
        TableFacts {
            columns,
            row_count: Some(1000),
        },
    );
    Fixture {
        b,
        scan,
        cols,
        phys,
    }
}

impl Fixture {
    fn cmp(&mut self, col: ColId, op: BinaryOp) -> ScalarId {
        let (l, r) = (self.b.col_ref(col), self.b.number("1"));
        self.b.binary(op, l, r)
    }

    fn wrapped(&mut self, col: ColId, func: &str) -> ScalarId {
        let inner = self.b.col_ref(col);
        let call = self.b.call(func, vec![inner]);
        let r = self.b.number("1");
        self.b.binary(BinaryOp::Eq, call, r)
    }

    fn filtered(mut self, conjuncts: Vec<ScalarId>) -> (Mir, PhysicalFacts) {
        let root = self.b.filter(self.scan, conjuncts);
        (self.b.finish(root), self.phys)
    }

    fn scanned(self) -> (Mir, PhysicalFacts) {
        let scan = self.scan;
        (self.b.finish(scan), self.phys)
    }
}

fn usages(lir: &Lir) -> Vec<(KeyKind, PruneUse)> {
    lir.nodes
        .values()
        .find_map(|n| match &n.op {
            PhysOp::TableScan { pruning, .. } => {
                Some(pruning.iter().map(|k| (k.kind, k.usage)).collect())
            }
            _ => None,
        })
        .unwrap_or_default()
}

fn usage(lir: &Lir, index: usize) -> PruneUse {
    usages(lir)[index].1
}

#[test]
fn a_bare_filter_uses_the_partition_key() {
    let mut f = fixture();
    let ts = f.cols[0];
    let pred = f.cmp(ts, BinaryOp::Eq);
    let (mir, phys) = f.filtered(vec![pred]);
    let lir = lower(&mir, Backend::BigQuery, &phys);
    assert_eq!(usage(&lir, 0), PruneUse::Used(pred));
    insta::assert_snapshot!(print_lir(&lir, &mir));
}

#[test]
fn an_unfiltered_key_is_not_filtered() {
    let (mir, phys) = fixture().scanned();
    let lir = lower(&mir, Backend::BigQuery, &phys);
    assert_eq!(usage(&lir, 0), PruneUse::NotFiltered);
}

#[test]
fn a_filter_above_a_project_is_unknown_not_not_filtered() {
    let mut f = fixture();
    let ts = f.cols[0];
    let pred = f.cmp(ts, BinaryOp::Eq);
    let r = f.b.col_ref(ts);
    let project = f.b.project(f.scan, vec![(ts, r)]);
    let root = f.b.filter(project, vec![pred]);
    let mir = f.b.finish(root);
    let lir = lower(&mir, Backend::BigQuery, &f.phys);
    assert_eq!(usage(&lir, 0), PruneUse::Unknown);
}

#[test]
fn bigquery_documented_function_on_partition_is_unknown() {
    let mut f = fixture();
    let ts = f.cols[0];
    let pred = f.wrapped(ts, "TIMESTAMP_TRUNC");
    let (mir, phys) = f.filtered(vec![pred]);
    assert_eq!(
        usage(&lower(&mir, Backend::BigQuery, &phys), 0),
        PruneUse::Unknown
    );
}

#[test]
fn bigquery_undocumented_function_on_partition_blocks() {
    let mut f = fixture();
    let ts = f.cols[0];
    let pred = f.wrapped(ts, "format_date");
    let (mir, phys) = f.filtered(vec![pred]);
    assert!(matches!(
        usage(&lower(&mir, Backend::BigQuery, &phys), 0),
        PruneUse::Blocked { .. }
    ));
}

#[test]
fn bigquery_arithmetic_on_the_partition_column_blocks() {
    let mut f = fixture();
    let ts = f.cols[0];
    let col = f.b.col_ref(ts);
    let one = f.b.number("1");
    let plus = f.b.binary(BinaryOp::Plus, col, one);
    let two = f.b.number("2");
    let pred = f.b.binary(BinaryOp::Gt, plus, two);
    let (mir, phys) = f.filtered(vec![pred]);
    assert!(matches!(
        usage(&lower(&mir, Backend::BigQuery, &phys), 0),
        PruneUse::Blocked { .. }
    ));
}

#[test]
fn bigquery_comparison_to_another_column_blocks() {
    let mut f = fixture();
    let (a, c) = (f.b.col_ref(f.cols[1]), f.b.col_ref(f.cols[3]));
    let pred = f.b.binary(BinaryOp::Eq, a, c);
    let (mir, phys) = f.filtered(vec![pred]);
    assert!(matches!(
        usage(&lower(&mir, Backend::BigQuery, &phys), 1),
        PruneUse::Blocked { .. }
    ));
}

#[test]
fn bigquery_filter_skipping_the_first_cluster_column_is_unknown() {
    let mut f = fixture();
    let b_col = f.cols[2];
    let pred = f.cmp(b_col, BinaryOp::Eq);
    let (mir, phys) = f.filtered(vec![pred]);
    let lir = lower(&mir, Backend::BigQuery, &phys);
    assert_eq!(usage(&lir, 1), PruneUse::NotFiltered);
    assert_eq!(usage(&lir, 2), PruneUse::Unknown);
}

#[test]
fn bigquery_unread_leading_cluster_column_still_breaks_the_order() {
    let mut f = fixture_reading(&["b"]);
    let b_col = f.cols[0];
    let pred = f.cmp(b_col, BinaryOp::Eq);
    let (mir, phys) = f.filtered(vec![pred]);
    let lir = lower(&mir, Backend::BigQuery, &phys);
    assert_eq!(usages(&lir).len(), 1);
    assert_eq!(usage(&lir, 0), PruneUse::Unknown);
}

#[test]
fn bigquery_filters_in_cluster_order_are_used() {
    let mut f = fixture();
    let (a, b) = (f.cols[1], f.cols[2]);
    let (p1, p2) = (f.cmp(a, BinaryOp::Eq), f.cmp(b, BinaryOp::Gt));
    let (mir, phys) = f.filtered(vec![p2, p1]);
    let lir = lower(&mir, Backend::BigQuery, &phys);
    assert_eq!(usage(&lir, 1), PruneUse::Used(p1));
    assert_eq!(usage(&lir, 2), PruneUse::Used(p2));
}

#[test]
fn bigquery_or_across_columns_blocks_the_partition() {
    let mut f = fixture();
    let (ts, c) = (f.cols[0], f.cols[3]);
    let (l, r) = (f.cmp(ts, BinaryOp::Eq), f.cmp(c, BinaryOp::Eq));
    let pred = f.b.binary(BinaryOp::Or, l, r);
    let (mir, phys) = f.filtered(vec![pred]);
    assert!(matches!(
        usage(&lower(&mir, Backend::BigQuery, &phys), 0),
        PruneUse::Blocked { .. }
    ));
}

#[test]
fn snowflake_function_wrap_stays_unknown() {
    let mut f = fixture();
    let a = f.cols[1];
    let pred = f.wrapped(a, "to_date");
    let (mir, phys) = f.filtered(vec![pred]);
    let lir = lower(&mir, Backend::Snowflake, &phys);
    // the partition key is not a Snowflake key kind, so only clusters remain
    assert_eq!(usages(&lir)[0], (KeyKind::Cluster, PruneUse::Unknown));
}

#[test]
fn keys_a_backend_cannot_use_are_not_stated() {
    let (mir, phys) = fixture().scanned();
    assert!(usages(&lower(&mir, Backend::Generic, &phys)).is_empty());
    assert!(usages(&lower(&mir, Backend::DuckDb, &phys)).is_empty());
}

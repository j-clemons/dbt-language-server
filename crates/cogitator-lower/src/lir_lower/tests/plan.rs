//! MIR→LIR over hand-built plans: sink, estimates, CTEs and operators.

use cogitator_ir::common::{BinaryOp, HoleKind, JoinKind, Materialization, SetOpKind};
use cogitator_ir::lir::{Backend, CtePolicy, Estimate, JoinHint, Lir, PhysOp, PhysicalFacts};
use cogitator_ir::mir::{Mir, RelOp, TableId};
use cogitator_ir::testkit::{int_col, MirBuilder};
use cogitator_ir::Fidelity;

use super::{backend, fixture, lower};
use crate::lower_to_lir;

#[test]
fn estimates_come_only_from_given_stats() {
    let (mir, phys) = fixture().scanned();
    let lir = lower(&mir, Backend::Generic, &phys);
    assert_eq!(lir.nodes[lir.root].est, Estimate::Stats { rows: 1000 });
    let lir = lower(&mir, Backend::Generic, &PhysicalFacts::default());
    assert_eq!(lir.nodes[lir.root].est, Estimate::Unknown);
}

#[test]
fn sink_resolves_config_names_against_root_columns() {
    let (mut mir, phys) = fixture().scanned();
    let (ts, ghost) = (mir.syms.intern("ts"), mir.syms.intern("ghost"));
    mir.config.materialized = Some(Materialization::Incremental);
    mir.config.partition_by = vec![ts, ghost];
    mir.config.contract_enforced = true;
    let lir = lower(&mir, Backend::BigQuery, &phys);
    assert_eq!(lir.sink.mat, Materialization::Incremental);
    assert_eq!(
        lir.sink.partition_by.len(),
        1,
        "an unknown name is not stated"
    );
    assert!(lir.sink.contract);
}

#[test]
fn an_absent_materialization_is_dbts_default_view() {
    let (mir, phys) = fixture().scanned();
    assert_eq!(
        lower(&mir, Backend::Generic, &phys).sink.mat,
        Materialization::View
    );
}

fn cte_plan() -> Mir {
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("a", int_col())]);
    let cte = b.cte("c", scan);
    let (x, y) = (b.cte_ref(cte), b.cte_ref(cte));
    let root = b.join(JoinKind::Cross, x, y, vec![]);
    b.finish(root)
}

fn first_cte(lir: &Lir) -> (CtePolicy, u32) {
    lir.nodes
        .values()
        .find_map(|n| match n.op {
            PhysOp::Cte { policy, uses, .. } => Some((policy, uses)),
            _ => None,
        })
        .expect("a cte")
}

#[test]
fn cte_use_counts_and_policy_follow_the_backend() {
    let mir = cte_plan();
    let phys = PhysicalFacts::default();
    assert_eq!(
        first_cte(&lower(&mir, Backend::Postgres, &phys)),
        (CtePolicy::Materialize, 2)
    );
    assert_eq!(
        first_cte(&lower(&mir, Backend::BigQuery, &phys)),
        (CtePolicy::Inline, 2)
    );
    assert_eq!(
        first_cte(&lower(&mir, Backend::DuckDb, &phys)),
        (CtePolicy::Auto, 2)
    );
}

#[test]
fn joins_carry_no_hint() {
    let mir = cte_plan();
    let lir = lower(&mir, Backend::Snowflake, &PhysicalFacts::default());
    let hint = lir.nodes.values().find_map(|n| match n.op {
        PhysOp::Join { hint, build, .. } => Some((hint, build)),
        _ => None,
    });
    assert_eq!(hint, Some((JoinHint::None, None)));
}

#[test]
fn union_lowers_and_intersect_is_opaque() {
    let build = |op: SetOpKind| {
        let mut b = MirBuilder::new();
        let (l, r) = (
            b.scan("l", &[("a", int_col())]),
            b.scan("r", &[("a", int_col())]),
        );
        let out = b.col("a", int_col());
        let root = b.set_op(op, true, vec![l, r], vec![out]);
        b.finish(root)
    };
    let phys = PhysicalFacts::default();
    let union = build(SetOpKind::Union);
    let lir = lower(&union, Backend::Generic, &phys);
    assert!(matches!(
        lir.nodes[lir.root].op,
        PhysOp::Union { all: true, .. }
    ));
    let intersect = build(SetOpKind::Intersect);
    let lir = lower(&intersect, Backend::Generic, &phys);
    assert!(matches!(lir.nodes[lir.root].op, PhysOp::Opaque { .. }));
}

#[test]
fn distinct_is_a_key_only_aggregate_when_key_scalars_exist() {
    let mut b = MirBuilder::new();
    let scan = b.scan("t", &[("a", int_col())]);
    let col = b.out(scan)[0];
    b.col_ref(col);
    let root = b.distinct(scan);
    let mir = b.finish(root);
    let lir = lower(&mir, Backend::Generic, &PhysicalFacts::default());
    assert!(
        matches!(&lir.nodes[lir.root].op, PhysOp::Aggregate { keys, aggs, .. } if keys.len() == 1 && aggs.is_empty())
    );
}

#[test]
fn an_opaque_mir_node_makes_the_level_partial() {
    let mut b = MirBuilder::new();
    let col = b.col("a", int_col());
    let root = b.opaque(vec![], vec![col], HoleKind::ParseError);
    let mir = b.finish(root);
    let lowered = lower_to_lir(&mir, backend(Backend::Generic), &PhysicalFacts::default());
    lowered.assert_clean();
    assert_eq!(lowered.fidelity, Fidelity::Partial);
}

#[test]
fn invalid_mir_declines_instead_of_panicking() {
    let (mut mir, phys) = fixture().scanned();
    let scan = mir.root;
    let dup = mir.rels[scan].op.clone();
    mir.rels[scan].op = match dup {
        RelOp::Scan {
            table,
            mut cols,
            open,
        } => {
            cols.push(cols[0]);
            RelOp::Scan { table, cols, open }
        }
        other => other,
    };
    let lowered = lower_to_lir(&mir, backend(Backend::Generic), &phys);
    assert!(lowered.value.is_none());
    assert_eq!(lowered.fidelity, Fidelity::Declined);
    assert!(lowered
        .violations
        .iter()
        .any(|v| v.invariant.level() == cogitator_ir::Level::Mir));
}

#[test]
fn every_backend_passes_the_lir_verifier_on_one_plan() {
    let mut f = fixture();
    let ts = f.cols[0];
    let pred = f.cmp(ts, BinaryOp::GtEq);
    let (mir, phys) = f.filtered(vec![pred]);
    for b in [
        Backend::Snowflake,
        Backend::BigQuery,
        Backend::DuckDb,
        Backend::Postgres,
        Backend::Generic,
    ] {
        let lir = lower(&mir, b, &phys);
        assert_eq!(lir.backend, b);
    }
}

#[test]
fn stats_are_keyed_by_table_id() {
    assert!(fixture().phys.table(TableId::from_raw(0)).is_some());
}

//! Per-backend pruning tables, read straight from `pruning`.

use cogitator_ir::lir::{Backend, BlockReason, KeyKind, PhysColumn, Pred, PredShape, Pruning};
use cogitator_ir::mir::ScalarId;
use cogitator_ir::{Interner, SqlType};

use super::{bigquery, duckdb, for_backend, postgres, snowflake};
use crate::DialectBackend;

const ALL_SHAPES: [fn(cogitator_ir::Symbol) -> PredShape; 7] = [
    |_| PredShape::BareCompare,
    |_| PredShape::Range,
    |name| PredShape::Function { name },
    |_| PredShape::Cast,
    |_| PredShape::Subquery,
    |_| PredShape::OrAcrossColumns,
    |_| PredShape::Other,
];

fn verdict(backend: &dyn DialectBackend, key: KeyKind, shape: PredShape) -> Pruning {
    let mut syms = Interner::new();
    let col = PhysColumn {
        name: syms.intern("c"),
        ty: SqlType::Unknown,
        key: Some(key),
    };
    let pred = Pred {
        scalar: ScalarId::from_raw(0),
        shape,
    };
    backend.pruning(&pred, &col)
}

fn func() -> PredShape {
    let mut syms = Interner::new();
    PredShape::Function {
        name: syms.intern("f"),
    }
}

fn blocks_with(verdict: Pruning, expected: BlockReason, doc: &str) {
    match verdict {
        Pruning::Blocks { why, doc: link } => {
            assert_eq!(why, expected);
            assert_eq!(link, doc);
        }
        other => panic!("expected Blocks, got {other:?}"),
    }
}

#[test]
fn every_backend_is_registered_and_links_a_doc() {
    for backend in [
        Backend::Snowflake,
        Backend::BigQuery,
        Backend::DuckDb,
        Backend::Postgres,
        Backend::Generic,
    ] {
        let found = for_backend(backend).expect("registered");
        assert_eq!(found.capabilities().backend, backend);
        let doc = found.capabilities().doc;
        if backend == Backend::Generic {
            assert!(doc.is_empty());
        } else {
            assert!(doc.starts_with("https://"), "{backend:?} lacks a doc link");
        }
    }
}

#[test]
fn no_backend_claims_a_join_hint() {
    for backend in [
        Backend::Snowflake,
        Backend::BigQuery,
        Backend::DuckDb,
        Backend::Postgres,
        Backend::Generic,
    ] {
        let found = for_backend(backend).expect("registered");
        assert!(found.capabilities().join_hints.is_empty());
    }
}

#[test]
fn each_backend_chooses_a_policy_its_table_lists() {
    for backend in [
        Backend::Snowflake,
        Backend::BigQuery,
        Backend::DuckDb,
        Backend::Postgres,
        Backend::Generic,
    ] {
        let found = for_backend(backend).expect("registered");
        for uses in [1, 2, 5] {
            assert!(found
                .capabilities()
                .cte_policies
                .contains(&found.cte_policy(uses)));
        }
    }
}

#[test]
fn bigquery_partition_table() {
    let b = bigquery::BigQuery;
    let key = KeyKind::Partition;
    assert_eq!(verdict(&b, key, PredShape::BareCompare), Pruning::Prunes);
    assert_eq!(verdict(&b, key, PredShape::Range), Pruning::Prunes);
    let doc = bigquery::PARTITION_DOC;
    blocks_with(
        verdict(&b, key, PredShape::Subquery),
        BlockReason::SubqueryValue,
        doc,
    );
    blocks_with(
        verdict(&b, key, PredShape::OrAcrossColumns),
        BlockReason::OrAcrossColumns,
        doc,
    );
    blocks_with(verdict(&b, key, func()), BlockReason::FunctionWrapped, doc);
    assert_eq!(verdict(&b, key, PredShape::Cast), Pruning::Unknown);
    assert_eq!(verdict(&b, key, PredShape::Other), Pruning::Unknown);
}

#[test]
fn bigquery_cluster_table() {
    let b = bigquery::BigQuery;
    let key = KeyKind::Cluster;
    assert_eq!(verdict(&b, key, PredShape::BareCompare), Pruning::Prunes);
    let doc = bigquery::CLUSTER_DOC;
    blocks_with(
        verdict(&b, key, PredShape::Cast),
        BlockReason::CastWrapped,
        doc,
    );
    blocks_with(verdict(&b, key, func()), BlockReason::FunctionWrapped, doc);
    // the clustering page mentions neither
    assert_eq!(verdict(&b, key, PredShape::Subquery), Pruning::Unknown);
    assert_eq!(
        verdict(&b, key, PredShape::OrAcrossColumns),
        Pruning::Unknown
    );
}

#[test]
fn snowflake_blocks_only_subqueries() {
    let s = snowflake::Snowflake;
    let key = KeyKind::Cluster;
    assert_eq!(verdict(&s, key, PredShape::BareCompare), Pruning::Prunes);
    blocks_with(
        verdict(&s, key, PredShape::Subquery),
        BlockReason::SubqueryValue,
        snowflake::PRUNING_DOC,
    );
    for shape in [
        func(),
        PredShape::Cast,
        PredShape::OrAcrossColumns,
        PredShape::Other,
    ] {
        assert_eq!(verdict(&s, key, shape), Pruning::Unknown, "{shape:?}");
    }
}

#[test]
fn duckdb_never_blocks() {
    let d = duckdb::DuckDb;
    for key in [KeyKind::Sort, KeyKind::Unique] {
        assert_eq!(verdict(&d, key, PredShape::BareCompare), Pruning::Prunes);
        assert_eq!(verdict(&d, key, PredShape::Range), Pruning::Prunes);
        for shape in [
            func(),
            PredShape::Cast,
            PredShape::Subquery,
            PredShape::OrAcrossColumns,
        ] {
            assert_eq!(verdict(&d, key, shape), Pruning::Unknown, "{shape:?}");
        }
    }
}

#[test]
fn postgres_blocks_function_wraps_with_the_expression_index_doc() {
    let p = postgres::Postgres;
    let key = KeyKind::Sort;
    assert_eq!(verdict(&p, key, PredShape::Range), Pruning::Prunes);
    blocks_with(
        verdict(&p, key, func()),
        BlockReason::FunctionWrapped,
        postgres::EXPRESSION_DOC,
    );
    assert_eq!(verdict(&p, key, PredShape::Cast), Pruning::Unknown);
    assert_eq!(verdict(&p, key, PredShape::Subquery), Pruning::Unknown);
}

#[test]
fn postgres_inlines_a_single_reference_and_materialises_repeats() {
    use cogitator_ir::lir::CtePolicy;
    let p = postgres::Postgres;
    assert_eq!(p.cte_policy(1), CtePolicy::Inline);
    assert_eq!(p.cte_policy(2), CtePolicy::Materialize);
}

#[test]
fn generic_claims_nothing() {
    let g = super::generic::Generic;
    let mut syms = Interner::new();
    let name = syms.intern("f");
    for make in ALL_SHAPES {
        for key in cogitator_ir::testkit::lir::ALL_KEY_KINDS {
            assert_eq!(verdict(&g, key, make(name)), Pruning::Unknown);
        }
    }
    assert!(g.capabilities().pruning_keys.is_empty());
}

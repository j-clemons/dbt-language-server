//! The differential corpus against the bundled DuckDB engine. Needs
//! `--features duckdb-oracle`; see the crate docs for how cases are added.
#![cfg(feature = "duckdb-oracle")]

use cogitator_duckdb::oracle::{bag_equal, explain_json};
use cogitator_oracle::corpus::{corpus_dir, discover, load_case, Case};
use cogitator_oracle::duckdb_engine::DuckDb;
use cogitator_oracle::duckdb_plan::plan_shape;
use cogitator_oracle::run::run_case;

fn case(name: &str) -> Case {
    load_case(&corpus_dir().join(name)).expect("case loads")
}

#[test]
fn the_corpus_passes() {
    let cases = discover(&corpus_dir()).expect("corpus loads");
    let failures: Vec<String> = cases
        .iter()
        .flat_map(|case| run_case(&mut DuckDb, case))
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The harness must be able to fail: dropping the WHERE is not bag-preserving.
#[test]
fn a_wrong_p_fix_is_caught() {
    let mut case = case("join_left_nullified_inner");
    case.after = "select o.id, o.customer_id, c.state from orders as o \
                  left join customers as c on o.customer_id = c.id"
        .to_owned();
    assert!(!run_case(&mut DuckDb, &case).is_empty());
}

/// An N label is unproven when the "fix" changes nothing on the witness.
#[test]
fn an_n_case_without_a_real_difference_is_caught() {
    let mut case = case("join_left_nullified_on");
    case.after = case.model.clone();
    let failures = run_case(&mut DuckDb, &case);
    assert!(
        failures.iter().any(|f| f.contains("N label is unproven")),
        "{failures:?}"
    );
}

#[test]
fn except_all_counts_duplicates_and_nulls() {
    let setup = "";
    let two = "select 1 as a union all select 1";
    let one = "select 1 as a";
    let diff = bag_equal(setup, two, one).expect("runs");
    assert_eq!(diff.except_all, Some((1, 0)));

    let nulls = "select cast(null as integer) as a union all select cast(null as integer)";
    let one_null = "select cast(null as integer) as a";
    assert_eq!(
        bag_equal(setup, nulls, nulls).expect("runs").except_all,
        Some((0, 0))
    );
    assert_eq!(
        bag_equal(setup, nulls, one_null).expect("runs").except_all,
        Some((1, 0))
    );
}

#[test]
fn a_retype_is_reported() {
    let diff = bag_equal("", "select 1::integer as a", "select 1::bigint as a").expect("runs");
    assert_ne!(diff.left_columns, diff.right_columns);
}

#[test]
fn a_column_count_mismatch_is_reported_not_an_error() {
    let diff = bag_equal("", "select 1 as a", "select 1 as a, 2 as b").expect("runs");
    assert_eq!(diff.except_all, None);
}

#[test]
fn engine_errors_surface() {
    assert!(explain_json("", "select * from missing").is_err());
    assert!(bag_equal("", "select 1", "select * from missing").is_err());
}

/// DuckDB does not document this JSON; a change must show up here first.
#[test]
fn the_explain_json_shape_is_pinned() {
    let setup = "create table orders as select * from \
                 (values (1, 'paid'), (2, 'void')) as v(id, status);";
    let plan = explain_json(
        setup,
        "select id, id + 1 as next_id from orders where status = 'paid'",
    )
    .expect("explain runs");
    let shape = plan_shape(&plan).expect("known shape");
    insta::assert_json_snapshot!(shape);
}

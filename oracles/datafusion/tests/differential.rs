//! The differential corpus against DataFusion; `just oracle-datafusion`.

use cogitator_oracle::corpus::{corpus_dir, discover, load_case, Case};
use cogitator_oracle::run::{run_case, Engine};
use cogitator_oracle_datafusion::DataFusion;

fn engine() -> DataFusion {
    DataFusion::new().expect("runtime starts")
}

fn case(name: &str) -> Case {
    load_case(&corpus_dir().join(name)).expect("case loads")
}

#[test]
fn the_corpus_passes() {
    let mut engine = engine();
    let cases = discover(&corpus_dir()).expect("corpus loads");
    let failures: Vec<String> = cases
        .iter()
        .flat_map(|case| run_case(&mut engine, case))
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
    assert!(!run_case(&mut engine(), &case).is_empty());
}

#[test]
fn an_n_case_without_a_real_difference_is_caught() {
    let mut case = case("join_left_nullified_on");
    case.after = case.model.clone();
    let failures = run_case(&mut engine(), &case);
    assert!(
        failures.iter().any(|f| f.contains("N label is unproven")),
        "{failures:?}"
    );
}

/// What SQL `EXCEPT ALL` gets wrong in this DataFusion (see the crate docs).
#[test]
fn duplicates_and_nulls_are_counted() {
    let setup = "CREATE TABLE dup AS SELECT * FROM (VALUES \
                 (1, 'a'), (1, 'a'), (CAST(NULL AS INT), 'b'), (CAST(NULL AS INT), 'b')) AS v(x, y); \
                 CREATE TABLE once AS SELECT * FROM (VALUES \
                 (1, 'a'), (CAST(NULL AS INT), 'b')) AS v(x, y);";
    let report = engine()
        .bag(setup, "select * from dup", "select * from once")
        .expect("runs");
    assert_eq!(report.except_all, Some((2, 0)));
}

#[test]
fn a_retype_is_reported() {
    let report = engine()
        .bag(
            "",
            "select cast(1 as int) as a",
            "select cast(1 as bigint) as a",
        )
        .expect("runs");
    assert!(!report.columns_match());
}

#[test]
fn engine_errors_surface() {
    assert!(engine()
        .bag("", "select * from missing", "select 1")
        .is_err());
    assert!(engine().reads("", "select * from missing").is_err());
}

#[test]
fn reads_list_only_the_projected_columns() {
    let setup = "CREATE TABLE t (a INT, b INT, c INT);";
    let reads = engine()
        .reads(setup, "select a from t where b > 1")
        .expect("plans");
    let columns: Vec<&str> = reads["t"].columns.iter().map(String::as_str).collect();
    assert_eq!(columns, ["a", "b"]);
}

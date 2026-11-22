use super::*;
use crate::segment::lex;

fn run(sql: &str) -> Parsed {
    let dialect = Dialect::new("duckdb");
    let lexed = lex(sql, &dialect).expect("lex");
    parse(lexed.tokens, &lexed.pos, &dialect, false)
}

#[test]
fn a_valid_statement_parses_whole() {
    assert!(matches!(run("select 1").outcome, Outcome::Whole(_)));
    assert!(matches!(run("select 1;").outcome, Outcome::Whole(_)));
}

#[test]
fn a_broken_cte_is_isolated() {
    let parsed =
        run("with a as (select 1), b as (select from where), c as (select 2) select * from c");
    let Outcome::Parts(parts) = parsed.outcome else {
        panic!("expected parts");
    };
    assert_eq!(parts.ctes.len(), 3);
    assert!(matches!(parts.ctes[0].body, Part::Parsed(_)));
    assert!(matches!(
        parts.ctes[1].body,
        Part::Hole(HoleKind::ParseError, _)
    ));
    assert!(matches!(parts.body, Part::Parsed(_)));
    assert_eq!(parsed.degraded.len(), 1);
    assert_eq!(parsed.degraded[0].kind, DegradationKind::ParseHole);
    assert!(parsed.degraded[0].span.is_some_and(|s| !s.is_empty()));
}

#[test]
fn multiple_statements_keep_the_first() {
    let parsed = run("select 1; select 2");
    assert!(matches!(parsed.outcome, Outcome::Whole(_)));
    assert_eq!(parsed.degraded[0].kind, DegradationKind::UnsupportedSyntax);
}

#[test]
fn empty_input_is_a_hole() {
    assert!(matches!(
        run("   ").outcome,
        Outcome::Hole(HoleKind::ParseError, _)
    ));
}

#[test]
fn a_deep_chain_is_truncated_not_parsed() {
    let sql = format!("select {} from t", vec!["a"; 2000].join(" + "));
    let parsed = run(&sql);
    assert_eq!(parsed.degraded[0].kind, DegradationKind::CapExceeded);
}

//! Tests for `plan::suppress`, split out to keep the module small.

use super::*;

fn scanned(text: &str) -> (Suppressions, LineTable) {
    (scan(text, &Dialect::new("duckdb")), LineTable::new(text))
}

#[test]
fn jinja_comment_covers_its_line_and_the_next() {
    let text =
        "select 1 {# cogitator: ignore join.cartesian -- no key needed #}\nselect 2\nselect 3\n";
    let (s, table) = scanned(text);
    let line_two = text.find("select 2").unwrap() as u32;
    let line_three = text.find("select 3").unwrap() as u32;
    assert!(s.suppresses(&table, "join.cartesian", 3));
    assert!(s.suppresses(&table, "join.cartesian", line_two));
    assert!(!s.suppresses(&table, "join.cartesian", line_three));
    assert!(!s.suppresses(&table, "filter.non_sargable", 3));
    assert!(s.unknown.is_empty());
}

#[test]
fn whitespace_controlled_jinja_comments_count() {
    let (s, table) = scanned("{#- cogitator: ignore join.cartesian -#}\nselect 1\n");
    assert!(s.suppresses(&table, "join.cartesian", 42));
}

#[test]
fn sql_comment_same_line_and_line_above() {
    let text = "select 1 -- cogitator: ignore join.cartesian, filter.non_sargable\n\
                -- cogitator: ignore set.union_distinct -- deliberate\n\
                select 2\n";
    let (s, table) = scanned(text);
    let second = text.find("select 2").unwrap() as u32;
    assert!(s.suppresses(&table, "join.cartesian", 0));
    assert!(s.suppresses(&table, "filter.non_sargable", 0));
    assert!(s.suppresses(&table, "set.union_distinct", second));
    assert!(!s.suppresses(&table, "join.cartesian", second));
}

#[test]
fn dashes_inside_a_string_are_not_a_comment() {
    let (s, table) = scanned("select '-- cogitator: ignore join.cartesian' as x\n");
    assert!(!s.suppresses(&table, "join.cartesian", 0));
}

#[test]
fn jinja_text_cannot_open_a_sql_comment() {
    let (s, table) = scanned("select {{ '-- cogitator: ignore join.cartesian' }} from t\n");
    assert!(!s.suppresses(&table, "join.cartesian", 0));
}

#[test]
fn multibyte_text_keeps_offsets_exact() {
    let text = "select 'é' as x -- cogitator: ignore join.cartesan\nselect 2\n";
    let (s, _) = scanned(text);
    let span = s.unknown[0].span;
    assert_eq!(
        &text[span.start as usize..span.end as usize],
        "join.cartesan"
    );
}

#[test]
fn ignore_file_with_ids_and_bare() {
    let (ids, table) = scanned("{# cogitator: ignore-file join.cartesian #}\nselect 1\n");
    assert!(ids.suppresses(&table, "join.cartesian", 2));
    assert!(!ids.suppresses(&table, "set.union_distinct", 2));
    let (all, table) = scanned("-- cogitator: ignore-file\nselect 1\n");
    assert!(all.suppresses(&table, "set.union_distinct", 3));
}

#[test]
fn ignore_with_no_id_suppresses_nothing() {
    let (s, table) = scanned("select 1 -- cogitator: ignore\n");
    assert!(!s.suppresses(&table, "join.cartesian", 0));
}

#[test]
fn unknown_ids_warn_with_did_you_mean_at_their_span() {
    let text = "select 1 -- cogitator: ignore join.cartesan -- typo\n";
    let (s, _) = scanned(text);
    assert_eq!(s.unknown.len(), 1);
    let unknown = &s.unknown[0];
    assert!(
        unknown.message.contains("join.cartesian"),
        "{}",
        unknown.message
    );
    let span = unknown.span;
    assert_eq!(
        &text[span.start as usize..span.end as usize],
        "join.cartesan"
    );
}

#[test]
fn unknown_ids_in_jinja_comments_warn_at_their_span() {
    let text = "{# cogitator: ignore join.cartesan #}\nselect 1\n";
    let (s, _) = scanned(text);
    let span = s.unknown[0].span;
    assert_eq!(
        &text[span.start as usize..span.end as usize],
        "join.cartesan"
    );
}

#[test]
fn not_a_directive_is_ignored() {
    let (s, _) = scanned("-- cogitator: ignored foo\nselect 1 -- plain note\n");
    assert_eq!(s, Suppressions::default());
}

//! Type inference over the lattice.

use super::*;

fn int() -> ColType {
    ColType::new(SqlType::Int { bits: Some(32) }, Nullability::NotNull)
}
fn dec() -> ColType {
    ColType::new(
        SqlType::Decimal {
            p: Some(10),
            s: Some(2),
        },
        Nullability::Nullable,
    )
}
fn float() -> ColType {
    ColType::new(SqlType::Float { bits: Some(64) }, Nullability::Unknown)
}

#[test]
fn numeric_join_widens_through_the_lattice() {
    let joined = binary(BinaryOp::Plus, &int(), &dec());
    assert!(matches!(joined.ty, SqlType::Decimal { .. }));
    let floated = binary(BinaryOp::Mul, &dec(), &float());
    assert!(matches!(floated.ty, SqlType::Float { .. }));
}

#[test]
fn incompatible_arithmetic_is_unknown() {
    let text = ColType::new(SqlType::Text, Nullability::NotNull);
    assert_eq!(binary(BinaryOp::Plus, &text, &int()).ty, SqlType::Unknown);
    assert_eq!(
        binary(BinaryOp::Plus, &ColType::UNKNOWN, &int()).ty,
        SqlType::Unknown
    );
}

#[test]
fn integer_division_is_left_unknown() {
    assert_eq!(binary(BinaryOp::Div, &int(), &int()).ty, SqlType::Unknown);
}

#[test]
fn null_is_the_identity_of_case_and_coalesce() {
    let null = ColType::new(SqlType::Null, Nullability::Nullable);
    assert_eq!(case(&[null.clone(), int()], true).ty, int().ty);
    assert_eq!(call("COALESCE", &[null, int()]).ty, int().ty);
}

#[test]
fn case_without_else_is_nullable() {
    assert_eq!(case(&[int()], false).null, Nullability::Nullable);
    assert_eq!(case(&[int()], true).null, Nullability::NotNull);
}

#[test]
fn coalesce_with_a_not_null_argument_is_not_null() {
    let args = [dec(), int()];
    assert_eq!(call("coalesce", &args).null, Nullability::NotNull);
}

#[test]
fn aggregates_are_recognised_case_insensitively() {
    assert!(is_aggregate("SUM"));
    assert!(!is_aggregate("my_udf"));
}

#[test]
fn count_is_not_null_and_avg_is_unknown() {
    assert_eq!(aggregate("count", &[]).null, Nullability::NotNull);
    assert_eq!(aggregate("avg", &[int()]), ColType::UNKNOWN);
}

#[test]
fn sum_of_integers_stays_unknown() {
    assert_eq!(aggregate("sum", &[int()]), ColType::UNKNOWN);
    assert!(matches!(
        aggregate("sum", &[dec()]).ty,
        SqlType::Decimal { p: None, s: None }
    ));
}

#[test]
fn number_literals_classify_by_form() {
    let text = |raw: &'static str| {
        let mut syms = cogitator_ir::Interner::new();
        let sym = syms.intern(raw);
        literal(&Lit::Number(sym), |s| syms.text(s).to_string()).ty
    };
    assert_eq!(text("1"), SqlType::Int { bits: None });
    assert_eq!(text("1.5"), SqlType::Decimal { p: None, s: None });
    assert_eq!(text("1e3"), SqlType::Float { bits: None });
}

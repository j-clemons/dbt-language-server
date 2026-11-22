//! SQL type inference over the `ColType` lattice. Pure, and tolerant of
//! `Unknown` everywhere: a result the dialects disagree on is `Unknown`.
//!
//! Nullability is stated only when it is derivable: literals, `IS NULL`
//! shapes and `count`. Outer joins and `not_null` facts are applied at
//! column references; everything else is `Unknown`.

use cogitator_ir::common::{BinaryOp, Lit, UnaryOp};
use cogitator_ir::types::{ColType, Nullability, SqlType, Tz};

const AGGREGATES: &[&str] = &[
    "any_value",
    "approx_count_distinct",
    "approx_distinct",
    "approx_percentile",
    "approx_quantile",
    "approx_top_count",
    "approx_top_k",
    "arbitrary",
    "array_agg",
    "avg",
    "bit_and",
    "bit_or",
    "bit_xor",
    "bool_and",
    "bool_or",
    "boolor_agg",
    "booland_agg",
    "corr",
    "count",
    "count_if",
    "countif",
    "covar_pop",
    "covar_samp",
    "every",
    "first",
    "group_concat",
    "hll",
    "json_agg",
    "jsonb_agg",
    "json_group_array",
    "last",
    "listagg",
    "logical_and",
    "logical_or",
    "max",
    "max_by",
    "median",
    "min",
    "min_by",
    "mode",
    "object_agg",
    "percentile_cont",
    "percentile_disc",
    "quantile",
    "quantile_cont",
    "quantile_disc",
    "stddev",
    "stddev_pop",
    "stddev_samp",
    "string_agg",
    "sum",
    "var_pop",
    "var_samp",
    "variance",
];

fn is(name: &str, set: &[&str]) -> bool {
    set.iter().any(|f| f.eq_ignore_ascii_case(name))
}

/// True for the built-in aggregates of the supported dialects.
pub(crate) fn is_aggregate(name: &str) -> bool {
    is(name, AGGREGATES)
}

fn bool_of(null: Nullability) -> ColType {
    ColType::new(SqlType::Bool, null)
}

fn known(ty: SqlType) -> ColType {
    ColType::new(ty, Nullability::Unknown)
}

fn is_numeric(ty: &SqlType) -> bool {
    matches!(
        ty,
        SqlType::Int { .. } | SqlType::Decimal { .. } | SqlType::Float { .. }
    )
}

/// Drops widths: arithmetic results depend on the dialect's promotion.
fn widen(ty: SqlType) -> SqlType {
    match ty {
        SqlType::Int { .. } => SqlType::Int { bits: None },
        SqlType::Decimal { .. } => SqlType::Decimal { p: None, s: None },
        SqlType::Float { .. } => SqlType::Float { bits: None },
        other => other,
    }
}

pub(crate) fn literal(lit: &Lit, text: impl Fn(cogitator_ir::Symbol) -> String) -> ColType {
    let not_null = |ty| ColType::new(ty, Nullability::NotNull);
    match lit {
        Lit::Null => ColType::new(SqlType::Null, Nullability::Nullable),
        Lit::Bool(_) => not_null(SqlType::Bool),
        Lit::Str(_) => not_null(SqlType::Text),
        Lit::Interval(_) => not_null(SqlType::Interval),
        Lit::Number(sym) => not_null(number_type(&text(*sym))),
        Lit::Typed { ty, .. } => not_null(typed_literal(&text(*ty))),
    }
}

fn number_type(raw: &str) -> SqlType {
    if raw.contains(['e', 'E']) {
        SqlType::Float { bits: None }
    } else if raw.contains('.') {
        SqlType::Decimal { p: None, s: None }
    } else {
        SqlType::Int { bits: None }
    }
}

fn typed_literal(name: &str) -> SqlType {
    match name.to_ascii_lowercase().as_str() {
        "date" => SqlType::Date,
        "time" => SqlType::Time,
        "timestamp" | "datetime" | "timestamp_ntz" => SqlType::Timestamp { tz: Tz::Without },
        "timestamptz" | "timestamp_tz" | "timestamp_ltz" => SqlType::Timestamp { tz: Tz::With },
        _ => SqlType::Unknown,
    }
}

pub(crate) fn unary(op: UnaryOp, e: &ColType) -> ColType {
    match op {
        UnaryOp::Not => bool_of(e.null),
        UnaryOp::Neg => ColType::new(widen(e.ty.clone()), e.null),
        UnaryOp::IsNull | UnaryOp::IsNotNull | UnaryOp::IsTrue | UnaryOp::IsFalse => {
            bool_of(Nullability::NotNull)
        }
        UnaryOp::Other(_) => ColType::UNKNOWN,
    }
}

pub(crate) fn binary(op: BinaryOp, l: &ColType, r: &ColType) -> ColType {
    use BinaryOp as B;
    match op {
        B::Eq
        | B::NotEq
        | B::Lt
        | B::LtEq
        | B::Gt
        | B::GtEq
        | B::And
        | B::Or
        | B::Like
        | B::NotLike
        | B::ILike
        | B::NotILike => bool_of(Nullability::Unknown),
        B::Plus | B::Minus | B::Mul | B::Mod if is_numeric(&l.ty) && is_numeric(&r.ty) => {
            known(widen(l.ty.join(&r.ty)))
        }
        B::Div
            if matches!(l.ty, SqlType::Float { .. }) || matches!(r.ty, SqlType::Float { .. }) =>
        {
            known(SqlType::Float { bits: None })
        }
        B::Concat if l.ty == SqlType::Text && r.ty == SqlType::Text => known(SqlType::Text),
        _ => ColType::UNKNOWN,
    }
}

/// The type of a `CASE`: arms joined, and nullable without an `ELSE`.
pub(crate) fn case(results: &[ColType], has_else: bool) -> ColType {
    let joined = join_all(results);
    if has_else {
        joined
    } else {
        ColType::new(joined.ty, joined.null.join(Nullability::Nullable))
    }
}

fn join_all(types: &[ColType]) -> ColType {
    let mut iter = types.iter();
    let Some(first) = iter.next() else {
        return ColType::UNKNOWN;
    };
    iter.fold(first.clone(), |acc, t| acc.join(t))
}

fn first_arg(args: &[ColType]) -> ColType {
    args.first().cloned().unwrap_or(ColType::UNKNOWN)
}

pub(crate) fn call(name: &str, args: &[ColType]) -> ColType {
    const TEXT: &[&str] = &[
        "concat",
        "initcap",
        "lower",
        "ltrim",
        "replace",
        "rtrim",
        "substr",
        "substring",
        "trim",
        "upper",
    ];
    const LENGTH: &[&str] = &["char_length", "character_length", "len", "length"];
    if is(name, &["coalesce", "ifnull", "nvl"]) {
        let joined = join_all(args);
        let any_not_null = args.iter().any(|a| a.null == Nullability::NotNull);
        let null = if any_not_null {
            Nullability::NotNull
        } else {
            joined.null
        };
        ColType::new(joined.ty, null)
    } else if is(name, TEXT) {
        known(SqlType::Text)
    } else if is(name, LENGTH) {
        known(SqlType::Int { bits: None })
    } else if is(name, &["current_date"]) {
        known(SqlType::Date)
    } else if is(name, &["abs"]) {
        ColType::new(widen(first_arg(args).ty), Nullability::Unknown)
    } else {
        ColType::UNKNOWN
    }
}

pub(crate) fn aggregate(name: &str, args: &[ColType]) -> ColType {
    let arg = first_arg(args);
    if is(name, &["count", "count_if", "countif"]) {
        ColType::new(SqlType::Int { bits: None }, Nullability::NotNull)
    } else if is(name, &["sum"]) {
        match arg.ty {
            SqlType::Decimal { .. } | SqlType::Float { .. } => known(widen(arg.ty)),
            _ => ColType::UNKNOWN,
        }
    } else if is(
        name,
        &[
            "min",
            "max",
            "any_value",
            "arbitrary",
            "first",
            "last",
            "max_by",
            "min_by",
        ],
    ) {
        known(arg.ty)
    } else if is(name, &["array_agg"]) {
        known(SqlType::Array(std::sync::Arc::new(arg.ty)))
    } else if is(name, &["string_agg", "listagg", "group_concat"]) {
        known(SqlType::Text)
    } else if is(
        name,
        &["bool_and", "bool_or", "every", "logical_and", "logical_or"],
    ) {
        known(SqlType::Bool)
    } else {
        ColType::UNKNOWN
    }
}

pub(crate) fn window(name: &str, args: &[ColType]) -> ColType {
    if is(name, &["row_number", "rank", "dense_rank", "ntile"]) {
        ColType::new(SqlType::Int { bits: None }, Nullability::NotNull)
    } else if is(
        name,
        &["lag", "lead", "first_value", "last_value", "nth_value"],
    ) {
        known(first_arg(args).ty)
    } else if is(name, &["percent_rank", "cume_dist"]) {
        known(SqlType::Float { bits: None })
    } else {
        aggregate(name, args)
    }
}

#[cfg(test)]
mod tests;

//! Shared text forms for the printers.

use crate::types::{ColType, Nullability, SqlType, Tz};

/// Caps printer recursion on pathological input.
pub(crate) const MAX_PRINT_DEPTH: usize = 256;

pub(crate) fn sql_type(ty: &SqlType) -> String {
    let width = |base: &str, bits: &Option<u8>| match bits {
        Some(b) => format!("{base}{b}"),
        None => base.to_string(),
    };
    match ty {
        SqlType::Unknown => "unknown".to_string(),
        SqlType::Null => "null".to_string(),
        SqlType::Bool => "bool".to_string(),
        SqlType::Int { bits } => width("int", bits),
        SqlType::Float { bits } => width("float", bits),
        SqlType::Decimal { p, s } => match (p, s) {
            (Some(p), Some(s)) => format!("decimal({p},{s})"),
            _ => "decimal".to_string(),
        },
        SqlType::Text => "text".to_string(),
        SqlType::Binary => "binary".to_string(),
        SqlType::Date => "date".to_string(),
        SqlType::Time => "time".to_string(),
        SqlType::Timestamp { tz } => match tz {
            Tz::With => "timestamptz".to_string(),
            Tz::Without => "timestamp".to_string(),
            Tz::Unknown => "timestamp?tz".to_string(),
        },
        SqlType::Interval => "interval".to_string(),
        SqlType::Json => "json".to_string(),
        SqlType::Array(inner) => format!("array<{}>", sql_type(inner)),
        SqlType::Struct(fields) => {
            let parts: Vec<String> = fields
                .iter()
                .map(|f| format!("{} {}", f.name, sql_type(&f.ty)))
                .collect();
            format!("struct<{}>", parts.join(", "))
        }
        SqlType::Map(kv) => format!("map<{}, {}>", sql_type(&kv.0), sql_type(&kv.1)),
        SqlType::Other(raw) => format!("other({raw})"),
    }
}

/// `int64!` not null, `int64?` nullable, `int64` unknown nullability.
pub(crate) fn col_type(ty: &ColType) -> String {
    let suffix = match ty.null {
        Nullability::NotNull => "!",
        Nullability::Nullable => "?",
        Nullability::Unknown => "",
    };
    format!("{}{suffix}", sql_type(&ty.ty))
}

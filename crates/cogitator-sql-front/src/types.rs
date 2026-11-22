//! Type strings to [`SqlType`], through the dialect's own parser. Never
//! fails: whatever the lattice does not model comes back as `Other(raw)`.

use std::sync::Arc;

use cogitator_dialects::Dialect;
use cogitator_ir::types::{Field, SqlType, Tz};
use cogitator_parser::backend::sql_dialect;
use sqlparser::ast::{ArrayElemTypeDef, DataType, ExactNumberInfo, TimezoneInfo};
use sqlparser::parser::Parser;
use sqlparser::tokenizer::Token;

/// Parses a dbt column `data_type` string. Unknown names, trailing text
/// and any parser failure yield `SqlType::Other(raw)`.
pub fn parse_sql_type(raw: &str, dialect: &Dialect) -> SqlType {
    let other = || SqlType::Other(Arc::from(raw));
    let dialect = sql_dialect(dialect);
    let Ok(mut parser) = Parser::new(dialect.as_ref()).try_with_sql(raw) else {
        return other();
    };
    match parser.parse_data_type() {
        Ok(data_type) if parser.peek_token_ref().token == Token::EOF => {
            match to_sql_type(&data_type) {
                // Keep the user's spelling rather than the parser's.
                SqlType::Other(_) => other(),
                known => known,
            }
        }
        _ => other(),
    }
}

/// A parsed type on the lattice; `Other` carries its display text.
pub(crate) fn to_sql_type(data_type: &DataType) -> SqlType {
    use DataType as D;
    use SqlType as T;
    match data_type {
        D::Bool | D::Boolean => T::Bool,
        D::TinyInt(_) | D::UTinyInt | D::TinyIntUnsigned(_) => int(8),
        D::SmallInt(_)
        | D::Int2(_)
        | D::SmallIntUnsigned(_)
        | D::USmallInt
        | D::Int16
        | D::UInt16 => int(16),
        D::Int(_)
        | D::Int4(_)
        | D::Integer(_)
        | D::IntUnsigned(_)
        | D::IntegerUnsigned(_)
        | D::Int4Unsigned(_)
        | D::Int32
        | D::UInt32
        | D::MediumInt(_) => int(32),
        D::BigInt(_)
        | D::Int8(_)
        | D::BigIntUnsigned(_)
        | D::UBigInt
        | D::Int64
        | D::UInt64
        | D::Int8Unsigned(_) => int(64),
        D::HugeInt | D::UHugeInt | D::Int128 | D::UInt128 => int(128),
        D::Int256
        | D::UInt256
        | D::Signed
        | D::SignedInteger
        | D::Unsigned
        | D::UnsignedInteger => T::Int { bits: None },
        D::Numeric(info)
        | D::Decimal(info)
        | D::BigNumeric(info)
        | D::BigDecimal(info)
        | D::Dec(info)
        | D::DecimalUnsigned(info)
        | D::DecUnsigned(info) => decimal(info),
        D::Float4 | D::Float32 | D::Real | D::RealUnsigned => float(32),
        D::Float8
        | D::Float64
        | D::DoublePrecision
        | D::DoublePrecisionUnsigned
        | D::Double(_)
        | D::DoubleUnsigned(_) => float(64),
        D::Float(info) | D::FloatUnsigned(info) => float_of(info),
        D::Character(_)
        | D::Char(_)
        | D::CharacterVarying(_)
        | D::CharVarying(_)
        | D::Varchar(_)
        | D::Nvarchar(_)
        | D::Text
        | D::TinyText
        | D::MediumText
        | D::LongText
        | D::String(_)
        | D::FixedString(_)
        | D::Clob(_)
        | D::CharacterLargeObject(_)
        | D::CharLargeObject(_) => T::Text,
        D::Binary(_)
        | D::Varbinary(_)
        | D::Blob(_)
        | D::TinyBlob
        | D::MediumBlob
        | D::LongBlob
        | D::Bytes(_)
        | D::Bytea => T::Binary,
        D::Date | D::Date32 => T::Date,
        D::Time(_, _) => T::Time,
        D::Datetime(_) | D::Datetime64(..) | D::TimestampNtz(_) => T::Timestamp { tz: Tz::Without },
        D::Timestamp(_, tz) => T::Timestamp {
            tz: timestamp_tz(tz),
        },
        D::Interval { .. } => T::Interval,
        D::JSON | D::JSONB => T::Json,
        D::Array(elem) => T::Array(Arc::new(array_elem(elem))),
        D::Map(k, v, _) => T::Map(Arc::new((to_sql_type(k), to_sql_type(v)))),
        D::Struct(fields, _) | D::Tuple(fields) => {
            let fields: Vec<Field> = fields
                .iter()
                .map(|f| Field {
                    name: Arc::from(f.field_name.as_ref().map_or("", |n| n.value.as_str())),
                    ty: to_sql_type(&f.field_type),
                })
                .collect();
            T::Struct(Arc::from(fields))
        }
        D::Nullable(inner) | D::LowCardinality(inner) => to_sql_type(inner),
        D::Custom(name, _) => custom(name.to_string()),
        other => T::Other(Arc::from(other.to_string())),
    }
}

fn int(bits: u8) -> SqlType {
    SqlType::Int { bits: Some(bits) }
}

fn float(bits: u8) -> SqlType {
    SqlType::Float { bits: Some(bits) }
}

fn decimal(info: &ExactNumberInfo) -> SqlType {
    let (p, s) = match info {
        ExactNumberInfo::None => (None, None),
        ExactNumberInfo::Precision(p) => (narrow(*p), Some(0)),
        ExactNumberInfo::PrecisionAndScale(p, s) => {
            (narrow(*p), u64::try_from(*s).ok().and_then(narrow))
        }
    };
    SqlType::Decimal { p, s }
}

fn float_of(info: &ExactNumberInfo) -> SqlType {
    match info {
        ExactNumberInfo::Precision(p) if *p <= 24 => float(32),
        ExactNumberInfo::Precision(_) | ExactNumberInfo::PrecisionAndScale(..) => float(64),
        ExactNumberInfo::None => SqlType::Float { bits: None },
    }
}

fn narrow(v: u64) -> Option<u8> {
    u8::try_from(v).ok()
}

fn timestamp_tz(tz: &TimezoneInfo) -> Tz {
    match tz {
        TimezoneInfo::WithTimeZone | TimezoneInfo::Tz => Tz::With,
        TimezoneInfo::WithoutTimeZone => Tz::Without,
        TimezoneInfo::None => Tz::Unknown,
    }
}

fn array_elem(elem: &ArrayElemTypeDef) -> SqlType {
    match elem {
        ArrayElemTypeDef::None => SqlType::Unknown,
        ArrayElemTypeDef::AngleBracket(t)
        | ArrayElemTypeDef::Parenthesis(t)
        | ArrayElemTypeDef::SquareBracket(t, _)
        | ArrayElemTypeDef::Qualified(t, _) => to_sql_type(t),
    }
}

/// Dialect names sqlparser leaves as custom types.
fn custom(name: String) -> SqlType {
    match name.to_ascii_uppercase().as_str() {
        "INT1" => int(8),
        "TIMESTAMP_TZ" | "TIMESTAMP_LTZ" | "TIMESTAMPTZ" => SqlType::Timestamp { tz: Tz::With },
        "TIMESTAMP_NTZ" => SqlType::Timestamp { tz: Tz::Without },
        "NUMBER" => SqlType::Decimal { p: None, s: None },
        "STRING" => SqlType::Text,
        "VARCHAR2" => SqlType::Text,
        _ => SqlType::Other(Arc::from(name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(raw: &str, dialect: &str) -> SqlType {
        parse_sql_type(raw, &Dialect::new(dialect))
    }

    #[test]
    fn integers_and_decimals() {
        assert_eq!(parsed("bigint", "duckdb"), int(64));
        assert_eq!(parsed("int", "postgres"), int(32));
        assert_eq!(parsed("smallint", "snowflake"), int(16));
        assert_eq!(
            parsed("numeric(12, 2)", "postgres"),
            SqlType::Decimal {
                p: Some(12),
                s: Some(2)
            }
        );
        assert_eq!(
            parsed("number", "snowflake"),
            SqlType::Decimal { p: None, s: None }
        );
    }

    #[test]
    fn text_and_time() {
        assert_eq!(parsed("varchar(32)", "postgres"), SqlType::Text);
        assert_eq!(parsed("string", "bigquery"), SqlType::Text);
        assert_eq!(parsed("date", "duckdb"), SqlType::Date);
        assert_eq!(
            parsed("timestamp", "duckdb"),
            SqlType::Timestamp { tz: Tz::Unknown }
        );
        assert_eq!(
            parsed("timestamptz", "postgres"),
            SqlType::Timestamp { tz: Tz::With }
        );
        assert_eq!(
            parsed("timestamp_ntz", "snowflake"),
            SqlType::Timestamp { tz: Tz::Without }
        );
    }

    #[test]
    fn nested_types() {
        assert_eq!(
            parsed("array<int64>", "bigquery"),
            SqlType::Array(Arc::new(int(64)))
        );
        assert!(matches!(
            parsed("struct<a int64>", "bigquery"),
            SqlType::Struct(_)
        ));
    }

    #[test]
    fn unknown_types_never_fail() {
        assert_eq!(
            parsed("not a type at all (", "duckdb"),
            SqlType::Other(Arc::from("not a type at all ("))
        );
        assert_eq!(parsed("", "duckdb"), SqlType::Other(Arc::from("")));
        assert!(matches!(
            parsed("geography", "snowflake"),
            SqlType::Other(_)
        ));
    }
}

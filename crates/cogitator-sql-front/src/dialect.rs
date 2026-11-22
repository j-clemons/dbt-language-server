//! Per-dialect identifier folding and alias-visibility rules.

use std::borrow::Cow;

use cogitator_dialects::Dialect;
use sqlparser::ast;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fold {
    /// Unquoted names fold up; quoted names are exact (Snowflake).
    Upper,
    /// Unquoted names fold down; quoted names are exact (Postgres).
    Lower,
    /// Every name, quoted or not, compares case-insensitively.
    AllLower,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct DialectRules {
    fold: Fold,
    /// Whether select-list aliases are visible to later select items.
    pub lateral_alias: bool,
}

impl DialectRules {
    pub(crate) fn for_dialect(dialect: &Dialect) -> Self {
        match dialect.as_str() {
            "snowflake" => DialectRules {
                fold: Fold::Upper,
                lateral_alias: true,
            },
            "duckdb" => DialectRules {
                fold: Fold::AllLower,
                lateral_alias: true,
            },
            "bigquery" => DialectRules {
                fold: Fold::AllLower,
                lateral_alias: false,
            },
            "postgres" => DialectRules {
                fold: Fold::Lower,
                lateral_alias: false,
            },
            // Redshift, Databricks and the generic fallback: permissive
            _ => DialectRules {
                fold: Fold::Lower,
                lateral_alias: true,
            },
        }
    }

    pub(crate) fn fold<'a>(&self, value: &'a str, quoted: bool) -> Cow<'a, str> {
        match self.fold {
            Fold::Upper if !quoted => Cow::Owned(value.to_uppercase()),
            Fold::Lower | Fold::AllLower
                if (!quoted || self.fold == Fold::AllLower)
                    && value.chars().any(char::is_uppercase) =>
            {
                Cow::Owned(value.to_lowercase())
            }
            _ => Cow::Borrowed(value),
        }
    }

    pub(crate) fn fold_ident<'a>(&self, ident: &'a ast::Ident) -> Cow<'a, str> {
        self.fold(&ident.value, ident.quote_style.is_some())
    }

    /// Folds a name from schema metadata, always treated as unquoted.
    pub(crate) fn fold_name<'a>(&self, name: &'a str) -> Cow<'a, str> {
        self.fold(name, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(name: &str) -> DialectRules {
        DialectRules::for_dialect(&Dialect::new(name))
    }

    #[test]
    fn folding_per_dialect() {
        assert_eq!(rules("snowflake").fold("Ab", false), "AB");
        assert_eq!(rules("snowflake").fold("Ab", true), "Ab");
        assert_eq!(rules("postgres").fold("Ab", false), "ab");
        assert_eq!(rules("postgres").fold("Ab", true), "Ab");
        assert_eq!(rules("duckdb").fold("Ab", true), "ab");
        assert_eq!(rules("bigquery").fold("Ab", false), "ab");
        assert_eq!(rules("").fold("Ab", false), "ab");
    }

    #[test]
    fn lateral_alias_support() {
        assert!(rules("snowflake").lateral_alias);
        assert!(rules("duckdb").lateral_alias);
        assert!(!rules("bigquery").lateral_alias);
        assert!(!rules("postgres").lateral_alias);
    }
}

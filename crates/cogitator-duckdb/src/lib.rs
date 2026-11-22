//! DuckDB 2.0 parse diagnostics (specs/rust-port/07).
//!
//! The engine integration is feature-gated (`duckdb-parser`) so the bundled
//! C++ build stays out of the dev loop (09 §1). The Jinja masking is always
//! compiled (and tested) — it is pure text work.

use cogitator_parser::jinja_spans::{blank_spans_in, jinja_spans, NOT_SQL};
use cogitator_parser::{JinjaKind, ParsedDocument, TokenKind};

/// Position-preserving Jinja masking (07 §3.1): every replacement has the
/// original byte length, so DuckDB error offsets map 1:1 onto the template.
/// Returns `None` when the confidence rule (07 §3.2) says not to diagnose:
/// the template contains `{% … %}` control blocks other than config/set.
pub fn mask_jinja(doc: &ParsedDocument) -> Option<String> {
    let text = doc.text.as_bytes();
    let mut out = text.to_vec();

    let mut i = 0;
    while i < doc.tokens.len() {
        let token = &doc.tokens[i];
        if let TokenKind::JinjaOpen(kind) = token.kind {
            // find the matching close
            let close = doc.tokens[i + 1..]
                .iter()
                .position(|t| matches!(t.kind, TokenKind::JinjaClose(k) if k == kind))
                .map(|off| i + 1 + off)?;
            let (start, end) = (token.start as usize, doc.tokens[close].end as usize);
            match kind {
                // config() renders to nothing, not to an identifier
                JinjaKind::Expr
                    if doc
                        .tokens
                        .get(i + 1)
                        .is_some_and(|t| t.kind == TokenKind::Config) =>
                {
                    blank(&mut out[start..end]);
                }
                JinjaKind::Expr => {
                    // `_` + `j` padding: a valid identifier of equal length
                    out[start] = b'_';
                    for b in &mut out[start + 1..end] {
                        *b = b'j';
                    }
                }
                JinjaKind::Stmt => {
                    let first_ident = doc.tokens[i + 1..close]
                        .iter()
                        .find(|t| t.kind == TokenKind::Ident || t.kind == TokenKind::Config);
                    let tag = first_ident
                        .map(|t| &doc.text[t.start as usize..t.end as usize])
                        .unwrap_or("");
                    if !matches!(tag, "config" | "set" | "endset") {
                        return None; // control block → stay silent (07 §3.2)
                    }
                    blank(&mut out[start..end]);
                }
            }
            i = close;
        }
        i += 1;
    }
    // Comments and raw tags produce no tokens; blank them separately.
    blank_spans_in(&mut out, &jinja_spans(&doc.text), NOT_SQL);
    String::from_utf8(out).ok()
}

fn blank(bytes: &mut [u8]) {
    for b in bytes {
        if *b != b'\n' {
            *b = b' ';
        }
    }
}

#[cfg(feature = "duckdb-oracle")]
pub mod oracle;

#[cfg(feature = "duckdb-parser")]
pub mod engine {
    //! In-process DuckDB 2.0 parser access (07 §2): one lazy in-memory
    //! connection on a blocking worker; `json_serialize_sql` for parse
    //! errors with positions.

    use cogitator_lsp::{diagnostic_severity, Diagnostic, Position, Range};

    pub struct DuckDbParser {
        conn: duckdb::Connection,
    }

    impl DuckDbParser {
        pub fn new() -> Result<Self, String> {
            duckdb::Connection::open_in_memory()
                .map(|conn| DuckDbParser { conn })
                .map_err(|e| e.to_string())
        }

        /// Parse `masked_sql`; a parse error becomes one diagnostic at the
        /// reported offset (07 §3.2).
        pub fn parse_diagnostic(&self, masked_sql: &str, template: &str) -> Option<Diagnostic> {
            let json: String = self
                .conn
                .query_row("SELECT json_serialize_sql(?)", [masked_sql], |row| {
                    row.get(0)
                })
                .ok()?;
            let value: serde_json::Value = serde_json::from_str(&json).ok()?;
            if value.get("error")?.as_bool() != Some(true) {
                return None;
            }
            let message = value
                .get("error_message")
                .and_then(|m| m.as_str())
                .unwrap_or("syntax error")
                .to_string();
            let offset = value
                .get("position")
                .and_then(|p| p.as_str()?.parse::<usize>().ok())
                .or_else(|| {
                    value
                        .get("position")
                        .and_then(|p| p.as_u64())
                        .map(|p| p as usize)
                })
                .unwrap_or(0);
            let (line, character) = line_col(template, offset);
            Some(Diagnostic::plain(
                Range::zero_width(Position { line, character }),
                message,
                diagnostic_severity::ERROR,
                "",
                "duckdb 2.0",
            ))
        }
    }

    fn line_col(text: &str, offset: usize) -> (u32, u32) {
        let upto = &text.as_bytes()[..offset.min(text.len())];
        let line = upto.iter().filter(|&&b| b == b'\n').count() as u32;
        let last = upto
            .iter()
            .rposition(|&b| b == b'\n')
            .map(|p| p + 1)
            .unwrap_or(0);
        (line, (offset.min(text.len()) - last) as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cogitator_dialects::Dialect;
    use cogitator_parser::{DocumentParser, JinjaSqlParser};

    fn parse(text: &str) -> ParsedDocument {
        JinjaSqlParser.parse(text, &Dialect::new("duckdb")).unwrap()
    }

    #[test]
    fn expr_masked_as_identifier_of_equal_length() {
        let text = "select * from {{ ref('stg_orders') }} where x = 1";
        let masked = mask_jinja(&parse(text)).expect("maskable");
        assert_eq!(masked.len(), text.len());
        assert!(masked.starts_with("select * from _jjjjj"));
        assert!(masked.ends_with("where x = 1"));
        assert!(!masked.contains("{{"));
    }

    #[test]
    fn standalone_config_masks_to_nothing() {
        let text = "{{ config(materialized='table') }}\n{# c #}\nselect 1";
        let masked = mask_jinja(&parse(text)).expect("maskable");
        assert_eq!(masked.len(), text.len());
        assert_eq!(masked.trim(), "select 1");
    }

    #[test]
    fn raw_tags_mask_and_the_body_stays() {
        let text = "select {% raw %}1{% endraw %}";
        let masked = mask_jinja(&parse(text)).expect("maskable");
        assert_eq!(
            masked.split_whitespace().collect::<Vec<_>>(),
            ["select", "1"]
        );
    }

    #[test]
    fn control_blocks_suppress_masking() {
        let text = "select 1 {% if flag %} ,2 {% endif %}";
        assert_eq!(mask_jinja(&parse(text)), None);
    }

    #[test]
    fn config_blocks_are_blanked_not_suppressed() {
        let text = "{% set x = 1 %}\nselect {{ x }}";
        let masked = mask_jinja(&parse(text)).expect("set block allowed");
        assert_eq!(masked.len(), text.len());
        assert!(masked.starts_with("               \n"));
    }
}

/// Read-only warehouse introspection (03 §10): source-column
/// completion for tables the YAML leaves undocumented. Doubly gated —
/// this cargo feature plus cogitator.yml `warehouse.columns`.
#[cfg(feature = "duckdb-parser")]
pub mod catalog {
    use std::path::Path;

    use cogitator_error::guidance::Guidance;

    #[derive(Debug, Clone)]
    pub struct CatalogColumn {
        pub name: String,
        pub data_type: String,
    }

    /// Columns of `schema.table` from information_schema, via a
    /// read-only connection that can never mutate the warehouse.
    pub fn table_columns(
        db_path: &Path,
        schema: &str,
        table: &str,
    ) -> Result<Vec<CatalogColumn>, Guidance> {
        let config = duckdb::Config::default()
            .access_mode(duckdb::AccessMode::ReadOnly)
            .map_err(|err| open_error(db_path, &err.to_string()))?;
        let conn = duckdb::Connection::open_with_flags(db_path, config)
            .map_err(|err| open_error(db_path, &err.to_string()))?;
        let mut statement = conn
            .prepare(
                "select column_name, data_type from information_schema.columns \
                 where table_schema = ? and table_name = ? order by ordinal_position",
            )
            .map_err(|err| open_error(db_path, &err.to_string()))?;
        let rows = statement
            .query_map(duckdb::params![schema, table], |row| {
                Ok(CatalogColumn {
                    name: row.get(0)?,
                    data_type: row.get(1)?,
                })
            })
            .map_err(|err| open_error(db_path, &err.to_string()))?;
        Ok(rows.flatten().collect())
    }

    fn open_error(path: &Path, detail: &str) -> Guidance {
        let mut guidance = Guidance::new("warehouse introspection failed.")
            .at(path.display().to_string())
            .because(detail.to_string());
        guidance = if detail.contains("lock") {
            guidance.fix(
                "another process holds the warehouse — close duckdb CLI/IDE \
                 connections, or disable warehouse.columns.",
            )
        } else if detail.contains("version") {
            guidance.fix(
                "the warehouse was written by an incompatible duckdb — re-run \
                 dbt with a matching adapter, or disable warehouse.columns.",
            )
        } else {
            guidance.fix("column completion for undeclared source columns stays off.")
        };
        guidance
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn reads_columns_read_only() {
            let dir =
                std::env::temp_dir().join(format!("cogitator-catalog-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let db = dir.join("wh.duckdb");
            std::fs::remove_file(&db).ok();
            {
                let conn = duckdb::Connection::open(&db).unwrap();
                conn.execute_batch(
                    "create schema raw; create table raw.payments (id integer, amount decimal(10,2));",
                )
                .unwrap();
            }
            let columns = table_columns(&db, "raw", "payments").unwrap();
            let names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
            assert_eq!(names, vec!["id", "amount"]);
            assert!(columns[1].data_type.to_lowercase().contains("decimal"));
            assert!(table_columns(&db, "raw", "nope").unwrap().is_empty());
            std::fs::remove_dir_all(&dir).ok();
        }
    }
}

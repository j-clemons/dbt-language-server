//! Diagnostic payload extensions (01 §6.1): tags, related information
//! and the TYPED `data` payload that rides did-you-mean suggestions to
//! the client and back through CodeActionContext. Deserialization is
//! deliberately lenient — foreign diagnostics in a codeAction context
//! must never fail the whole request.

use serde::{Deserialize, Deserializer, Serialize};

use super::core::{Location, Range};
use super::workspace::TextEdit;

/// LSP DiagnosticTag values.
pub mod diagnostic_tag {
    pub const UNNECESSARY: u8 = 1;
    #[allow(dead_code)] // deprecation marking is a natural follow-up
    pub const DEPRECATED: u8 = 2;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticRelatedInformation {
    pub location: Location,
    pub message: String,
}

/// What a quickfix should do with the carried range. The unit variants
/// serialize as their name (`"replace_token"`), as they always have;
/// `MultiEdit` serializes as `{"multi_edit": {...}}`, so older data on
/// the wire still decodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixKind {
    /// Replace the token at `token_range` with a suggestion.
    ReplaceToken,
    /// Insert a suggestion at `token_range.start`.
    InsertText,
    /// Alternative fixes, each a complete set of edits to apply together.
    MultiEdit { fixes: Vec<MultiEditFix> },
}

/// One self-contained fix: its title and all of its edits. `preserving`
/// (absent means false) marks a fix that keeps results unchanged, the
/// only kind offered as preferred or bundled into `source.fixAll`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiEditFix {
    pub title: String,
    pub edits: Vec<TextEdit>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub preserving: bool,
}

/// The typed Diagnostic.data payload (01 §6.1). Clients preserve `data`
/// verbatim, so the quickfix handler reads suggestions straight out of
/// the context diagnostics without recomputing them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticData {
    pub fix: FixKind,
    pub token_range: Range,
    pub suggestions: Vec<String>,
}

/// A client may hand back diagnostics that are not ours (other sources
/// in the same context) whose `data` has any shape at all; those decode
/// to `None` instead of failing the request.
pub(super) fn lenient_data<'de, D>(deserializer: D) -> Result<Option<DiagnosticData>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|v| serde_json::from_value(v).ok()))
}

/// LSP allows `code` as integer or string; ours are strings, but a
/// foreign diagnostic's integer code must not fail the decode.
pub(super) fn lenient_code<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Code {
        Text(String),
        Number(i64),
    }
    Ok(match Option::<Code>::deserialize(deserializer)? {
        Some(Code::Text(text)) => text,
        Some(Code::Number(n)) => n.to_string(),
        None => String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::results::{diagnostic_severity, Diagnostic};
    use super::*;
    use crate::Position;

    #[test]
    fn plain_diagnostic_serializes_without_extras() {
        let diag = Diagnostic::plain(
            Range::default(),
            "message".into(),
            diagnostic_severity::WARNING,
            "dangling-ref",
            "cogitator",
        );
        let json = serde_json::to_value(&diag).unwrap();
        assert!(json.get("data").is_none(), "{json}");
        assert!(json.get("tags").is_none(), "{json}");
        assert!(json.get("relatedInformation").is_none(), "{json}");
    }

    #[test]
    fn data_round_trips_through_the_wire() {
        let mut diag = Diagnostic::plain(
            Range::zero_width(Position {
                line: 2,
                character: 4,
            }),
            "model 'stg_order' not found".into(),
            diagnostic_severity::WARNING,
            "dangling-ref",
            "cogitator",
        );
        diag.data = Some(DiagnosticData {
            fix: FixKind::ReplaceToken,
            token_range: diag.range,
            suggestions: vec!["stg_orders".into()],
        });
        let wire = serde_json::to_string(&diag).unwrap();
        let back: Diagnostic = serde_json::from_str(&wire).unwrap();
        let data = back.data.expect("data survives");
        assert_eq!(data.fix, FixKind::ReplaceToken);
        assert_eq!(data.suggestions, vec!["stg_orders".to_string()]);
    }

    #[test]
    fn unit_fix_kinds_keep_their_wire_format() {
        assert_eq!(
            serde_json::to_string(&FixKind::ReplaceToken).unwrap(),
            r#""replace_token""#
        );
        let old: FixKind = serde_json::from_str(r#""insert_text""#).unwrap();
        assert_eq!(old, FixKind::InsertText);
    }

    #[test]
    fn multi_edit_round_trips_through_the_wire() {
        let fix = FixKind::MultiEdit {
            fixes: vec![MultiEditFix {
                title: "Delete".into(),
                edits: vec![
                    TextEdit {
                        range: Range::default(),
                        new_text: String::new(),
                    },
                    TextEdit {
                        range: Range::default(),
                        new_text: "x".into(),
                    },
                ],
                preserving: true,
            }],
        };
        let wire = serde_json::to_value(&fix).unwrap();
        assert_eq!(wire["multi_edit"]["fixes"][0]["edits"][1]["newText"], "x");
        assert_eq!(wire["multi_edit"]["fixes"][0]["preserving"], true);
        let back: FixKind = serde_json::from_value(wire).unwrap();
        assert_eq!(back, fix);
    }

    #[test]
    fn multi_edit_decodes_without_preserving() {
        let fix: FixKind =
            serde_json::from_str(r#"{"multi_edit":{"fixes":[{"title":"t","edits":[]}]}}"#).unwrap();
        let FixKind::MultiEdit { fixes } = fix else {
            panic!("multi edit");
        };
        assert!(!fixes[0].preserving);
    }

    #[test]
    fn foreign_diagnostics_decode_leniently() {
        // integer code, alien data shape, missing severity/source
        let wire = r#"{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},
                       "message":"x","code":7,"data":{"something":"else"}}"#;
        let diag: Diagnostic = serde_json::from_str(wire).unwrap();
        assert_eq!(diag.code, "7");
        assert!(diag.data.is_none());
    }
}

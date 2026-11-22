//! Physical relation names back to the `ref`/`source` they came from.

use std::collections::BTreeMap;

use cogitator_dialects::Dialect;
use sqlparser::tokenizer::Token;

use crate::dialect::DialectRules;
use crate::segment;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Target {
    Model(String),
    Source(String, String),
}

/// Lookup from rendered relation names. Names are folded for the dialect
/// on both sides, so a manifest's quoting and case need not match.
#[derive(Debug)]
pub struct RelationTable {
    rules: DialectRules,
    /// A model's whole rendered `relation_name`.
    exact: BTreeMap<Vec<String>, Target>,
    /// Sources by their last two parts: `schema.table`.
    by_schema: BTreeMap<(String, String), Target>,
    /// A last path part, for relations with no rendered name to compare.
    by_name: BTreeMap<String, Target>,
    /// Kept to parse relation names added later.
    dialect: Dialect,
}

impl RelationTable {
    pub fn new(dialect: &Dialect) -> Self {
        RelationTable {
            rules: DialectRules::for_dialect(dialect),
            exact: BTreeMap::new(),
            by_schema: BTreeMap::new(),
            by_name: BTreeMap::new(),
            // boundary: the table outlives the caller's borrow
            dialect: dialect.clone(),
        }
    }

    /// Adds a model. Call in sorted name order so duplicates resolve the
    /// same way every time. `relation_name` is the manifest's rendered
    /// value; without one (no manifest, or ephemeral) the model is found
    /// by its bare name instead.
    pub fn add_model(&mut self, name: &str, relation_name: Option<&str>) {
        let target = Target::Model(name.to_string());
        let parts = relation_name.and_then(|r| self.parse_parts(r));
        match parts {
            Some(parts) if !parts.is_empty() => {
                self.exact.entry(parts).or_insert(target);
            }
            _ => {
                let key = self.fold(name);
                self.by_name.entry(key).or_insert(target);
            }
        }
    }

    /// Adds a source table. `identifier` is its YAML override. Call in
    /// sorted order, as [`RelationTable::add_model`].
    pub fn add_source(
        &mut self,
        source: &str,
        table: &str,
        schema: Option<&str>,
        identifier: Option<&str>,
    ) {
        let target = Target::Source(source.to_string(), table.to_string());
        let name = self.fold(identifier.unwrap_or(table));
        match schema.map(|s| self.fold(s)) {
            Some(schema) => {
                self.by_schema.entry((schema, name)).or_insert(target);
            }
            None => {
                self.by_name.entry(name).or_insert(target);
            }
        }
    }

    /// `parts` as written in the compiled SQL, unfolded.
    pub(super) fn lookup(&self, parts: &[String]) -> Option<&Target> {
        let folded: Vec<String> = parts.iter().map(|p| self.fold(p)).collect();
        if let Some(target) = self.exact.get(&folded) {
            return Some(target);
        }
        if let [.., schema, name] = folded.as_slice() {
            // boundary: the BTreeMap key is an owned pair
            if let Some(target) = self.by_schema.get(&(schema.clone(), name.clone())) {
                return Some(target);
            }
        }
        self.by_name.get(folded.last()?)
    }

    fn fold(&self, value: &str) -> String {
        // Both sides render the same way, so quoting does not distinguish.
        self.rules.fold(value, false).into_owned()
    }

    /// The name parts of a rendered relation: `"db"."schema"."t"`.
    fn parse_parts(&self, relation: &str) -> Option<Vec<String>> {
        let lexed = segment::lex(relation, &self.dialect).ok()?;
        let mut parts = Vec::new();
        let mut expect_word = true;
        for token in lexed
            .tokens
            .iter()
            .filter(|t| segment::is_significant(&t.token))
        {
            match (&token.token, expect_word) {
                (Token::Word(word), true) => {
                    parts.extend(word_parts(word).into_iter().map(|p| self.fold(&p)));
                    expect_word = false;
                }
                (Token::Period, false) => expect_word = true,
                _ => return None,
            }
        }
        (!expect_word).then_some(parts)
    }
}

/// A backtick-quoted word may hold a whole dotted path (BigQuery).
pub(super) fn word_parts(word: &sqlparser::tokenizer::Word) -> Vec<String> {
    if word.quote_style == Some('`') {
        word.value.split('.').map(str::to_string).collect()
    } else {
        vec![word.value.clone()]
    }
}

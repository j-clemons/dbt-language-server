//! Tree-sitter document backend (specs/rust-port/06 §3): error-tolerant
//! segmentation of dbt SQL-with-Jinja via the tree-sitter-dbt grammar
//! (https://github.com/sirh3e/tree-sitter-dbt), behind the
//! `DocumentParser` seam.
//!
//! Division of labor: the tree decides *where* the Jinja constructs are —
//! and keeps deciding it on broken mid-edit buffers (ERROR nodes degrade one
//! region, not the document). Each well-formed construct is then tokenized
//! by minijinja and the gaps by sqlparser through `cogitator_parser::backend`, so
//! refinement, CTE tracking and the token model stay byte-for-byte identical
//! to the phase-1 pipeline. SQL stays on sqlparser per 06 §3.3.

mod regions;
mod sql_structure;

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Mutex;

use cogitator_dialects::Dialect;
use cogitator_error::Result;
use cogitator_parser::{backend, AppliedEdit, DocumentParser, LineTable, ParsedDocument, Token};

use regions::{collect_regions, Region, RegionKind};

/// Below this size a full reparse is µs-scale anyway; the incremental
/// path (06 §3.4) only engages past it — risk stays bounded.
const INCREMENTAL_MIN_BYTES: usize = 32 * 1024;

/// Phase-2 `DocumentParser` (06 §3.5). Unlike `JinjaSqlParser`, `parse`
/// practically never fails: malformed regions contribute no tokens instead
/// of staling the whole document.
pub struct TreeSitterParser {
    parser: Mutex<tree_sitter::Parser>,
    /// tree-sitter-sequel for SQL *structure* only (06 §3.6); `None`
    /// (grammar failed to load) just means token-based fallbacks.
    sql_parser: Option<Mutex<tree_sitter::Parser>>,
    /// Last tree per open document (06 §3.4) — fed back as the old
    /// tree on big ranged edits, dropped on didClose via `forget`.
    trees: Mutex<HashMap<String, tree_sitter::Tree>>,
}

impl TreeSitterParser {
    pub fn new() -> Result<Self> {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_dbt::LANGUAGE.into())
            .map_err(|e| cogitator_error::Error::Jinja {
                path: Default::default(),
                message: format!("tree-sitter language load failed: {e}"),
            })?;
        Ok(TreeSitterParser {
            parser: Mutex::new(parser),
            sql_parser: sql_structure::new_sql_parser(),
            trees: Mutex::new(HashMap::new()),
        })
    }
}

impl TreeSitterParser {
    /// The shared tail of every parse: tokens from the tree, SQL
    /// structure from tree-sitter-sequel.
    fn document_from(
        &self,
        tree: &tree_sitter::Tree,
        text: &str,
        dialect: &Dialect,
    ) -> ParsedDocument {
        let regions = collect_regions(tree.root_node());
        let mut doc = parse_regions(&regions, text, dialect);
        doc.sql_structure = sql_structure::collect(&self.sql_parser, text, &regions);
        doc
    }

    fn parse_tree(
        &self,
        text: &str,
        old_tree: Option<&tree_sitter::Tree>,
    ) -> Option<tree_sitter::Tree> {
        self.parser.lock().ok()?.parse(text, old_tree)
    }
}

impl DocumentParser for TreeSitterParser {
    fn parse(&self, text: &str, dialect: &Dialect) -> Result<ParsedDocument> {
        match self.parse_tree(text, None) {
            Some(tree) => Ok(self.document_from(&tree, text, dialect)),
            // parse() returns None only on cancellation/timeout; fall back to
            // the phase-1 pipeline rather than dropping the request.
            None => backend_parse(text, dialect),
        }
    }

    /// 06 §3.4: feed the previous tree + InputEdits back into
    /// tree-sitter for documents past the size gate; everything else is
    /// a full parse. Either way the fresh tree is remembered per uri.
    fn parse_incremental(
        &self,
        uri: &str,
        text: &str,
        dialect: &Dialect,
        edits: &[AppliedEdit],
    ) -> Result<ParsedDocument> {
        let old_tree = if !edits.is_empty() && text.len() > INCREMENTAL_MIN_BYTES {
            self.trees
                .lock()
                .ok()
                .and_then(|trees| trees.get(uri).cloned())
                .map(|mut tree| {
                    for edit in edits {
                        tree.edit(&input_edit(edit));
                    }
                    tree
                })
        } else {
            None
        };
        match self.parse_tree(text, old_tree.as_ref()) {
            Some(tree) => {
                let doc = self.document_from(&tree, text, dialect);
                if let Ok(mut trees) = self.trees.lock() {
                    trees.insert(uri.to_string(), tree);
                }
                Ok(doc)
            }
            None => backend_parse(text, dialect),
        }
    }

    fn forget(&self, uri: &str) {
        if let Ok(mut trees) = self.trees.lock() {
            trees.remove(uri);
        }
    }
}

fn input_edit(edit: &AppliedEdit) -> tree_sitter::InputEdit {
    let point = |(row, column): (u32, u32)| tree_sitter::Point {
        row: row as usize,
        column: column as usize,
    };
    tree_sitter::InputEdit {
        start_byte: edit.start_byte,
        old_end_byte: edit.old_end_byte,
        new_end_byte: edit.new_end_byte,
        start_position: point(edit.start_point),
        old_end_position: point(edit.old_end_point),
        new_end_position: point(edit.new_end_point),
    }
}

fn backend_parse(text: &str, dialect: &Dialect) -> Result<ParsedDocument> {
    cogitator_parser::pipeline::parse_document(text, dialect)
}

fn parse_regions(regions: &[Region], text: &str, dialect: &Dialect) -> ParsedDocument {
    let table = LineTable::new(text);
    let mut jinja: Vec<Token> = Vec::new();
    let mut segments: Vec<Range<usize>> = Vec::new();

    let mut pos = 0usize;
    for region in regions {
        if region.start > pos {
            segments.push(pos..region.start);
        }
        let slice = &text[region.start..region.end];
        match region.kind {
            RegionKind::Comment => {}
            RegionKind::Jinja => {
                tokenize_region(slice, region.start, &table, &mut jinja, &mut segments)
            }
            RegionKind::Error => {
                // A broken region that still looks like Jinja degrades to
                // nothing (06 §3.3); anything else is SQL the grammar
                // tripped over.
                if slice.contains("{{") || slice.contains("{%") || slice.contains("{#") {
                    tokenize_region(slice, region.start, &table, &mut jinja, &mut segments);
                } else {
                    segments.push(region.start..region.end);
                }
            }
        }
        pos = region.end.max(pos);
    }
    if pos < text.len() {
        segments.push(pos..text.len());
    }

    let tokens = backend::lex_document(text, segments, jinja, dialect, &table);
    backend::assemble(text, tokens)
}

/// Tokenize one Jinja region with minijinja; a malformed region contributes
/// nothing instead of failing the document.
fn tokenize_region(
    slice: &str,
    base: usize,
    table: &LineTable,
    jinja: &mut Vec<Token>,
    segments: &mut Vec<Range<usize>>,
) {
    let (mut region_tokens, mut region_segments) = (Vec::new(), Vec::new());
    match backend::tokenize_template(
        slice,
        base as u32,
        table,
        &mut region_tokens,
        &mut region_segments,
    ) {
        Ok(()) => {
            jinja.extend(region_tokens);
            segments.extend(region_segments);
        }
        Err(err) => tracing::trace!(%err, base, "malformed jinja region skipped"),
    }
}

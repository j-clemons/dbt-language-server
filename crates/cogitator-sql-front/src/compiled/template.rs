//! The template's literal SQL, classified by how it can reach the
//! compiled text. Built on minijinja's tokens (`tokenize_template`) and
//! the existing tag classifier; no new lexing.

use std::ops::Range;

use cogitator_parser::backend::tokenize_template;
use cogitator_parser::{JinjaKind, LineTable, TokenKind};

use crate::jinja::{classify_tag, Tag};

/// Blocks whose body is repeated, deferred or transformed: their SQL has
/// no one-to-one place in the compiled text.
const OPAQUE_BLOCKS: [&str; 5] = ["for", "macro", "call", "filter", "materialization"];

/// One run of literal SQL outside every opaque block.
#[derive(Debug)]
pub(super) struct Segment {
    pub span: Range<usize>,
    /// Enclosing `if` blocks, outermost first: `(if id, branch index)`.
    /// Empty when the SQL always reaches the compiled text.
    pub branches: Vec<(u32, u32)>,
}

impl Segment {
    pub(super) fn required(&self) -> bool {
        self.branches.is_empty()
    }
}

#[derive(Default)]
struct Blocks {
    opaque: u32,
    ifs: Vec<(u32, u32)>,
    next_if: u32,
}

impl Blocks {
    fn apply(&mut self, tag: Tag) {
        match tag {
            Tag::IfIncremental => self.open_if(),
            Tag::Other(name) if name == "if" => self.open_if(),
            Tag::Other(name) if name == "elif" || name == "else" => {
                if self.opaque == 0 {
                    if let Some(open) = self.ifs.last_mut() {
                        open.1 += 1;
                    }
                }
            }
            Tag::EndIf => {
                if self.opaque == 0 {
                    self.ifs.pop();
                }
            }
            Tag::SetBlock => self.opaque += 1,
            Tag::EndSet => self.opaque = self.opaque.saturating_sub(1),
            Tag::Other(name) if OPAQUE_BLOCKS.contains(&name.as_str()) => self.opaque += 1,
            Tag::Other(name)
                if name
                    .strip_prefix("end")
                    .is_some_and(|block| OPAQUE_BLOCKS.contains(&block)) =>
            {
                self.opaque = self.opaque.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn open_if(&mut self) {
        if self.opaque == 0 {
            self.ifs.push((self.next_if, 0));
            self.next_if += 1;
        }
    }
}

/// Literal SQL segments in document order, without whitespace-only ones.
/// A template minijinja cannot tokenize yields none.
pub(super) fn segments(text: &str) -> Vec<Segment> {
    let table = LineTable::new(text);
    let (mut tokens, mut spans) = (Vec::new(), Vec::new());
    if tokenize_template(text, 0, &table, &mut tokens, &mut spans).is_err() {
        return Vec::new();
    }
    let mut blocks = Blocks::default();
    let mut out = Vec::new();
    let mut pending = spans.into_iter().peekable();
    let mut i = 0;
    while let Some(open) = tokens.get(i) {
        let TokenKind::JinjaOpen(kind) = open.kind else {
            i += 1;
            continue;
        };
        let close = tokens
            .iter()
            .enumerate()
            .skip(i + 1)
            .find(|(_, t)| t.kind == TokenKind::JinjaClose(kind));
        let Some((close_at, close)) = close else {
            break;
        };
        while let Some(span) = pending.next_if(|s| s.start < open.start as usize) {
            push_segment(text, span, &blocks, &mut out);
        }
        if kind == JinjaKind::Stmt {
            let inner = text
                .get(open.end as usize..close.start as usize)
                .unwrap_or("");
            blocks.apply(classify_tag(inner));
        }
        i = close_at + 1;
    }
    for span in pending {
        push_segment(text, span, &blocks, &mut out);
    }
    out
}

fn push_segment(text: &str, span: Range<usize>, blocks: &Blocks, out: &mut Vec<Segment>) {
    let literal = text
        .get(span.clone())
        .is_some_and(|s| s.chars().any(|c| !c.is_ascii_whitespace()));
    if blocks.opaque == 0 && literal {
        out.push(Segment {
            span,
            // boundary: each segment owns its branch path
            branches: blocks.ifs.clone(),
        });
    }
}

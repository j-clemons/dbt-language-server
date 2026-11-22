//! Walks Jinja regions, classifying each and validating the control flow.

use cogitator_ir::Span;
use cogitator_parser::{JinjaKind, Token, TokenKind};

use super::{Region, RegionKind, Stop};
use crate::jinja::{classify, classify_tag, Class, ExprKindTag, Tag};

/// A block whose extent matters: `{% if is_incremental() %}` or a block set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenBlock {
    Incremental,
    Set,
}

pub(super) struct Walked {
    pub regions: Vec<Region>,
    /// `{% if is_incremental() %}` … `{% endif %}`, tags included.
    pub incremental: Vec<Span>,
    pub set_blocks: Vec<Span>,
}

pub(super) fn walk_regions(text: &str, tokens: &[Token]) -> Result<Walked, Stop> {
    let mut out = Walked {
        regions: Vec::new(),
        incremental: Vec::new(),
        set_blocks: Vec::new(),
    };
    let mut open: Vec<(OpenBlock, u32)> = Vec::new();
    let mut i = 0;
    while let Some(token) = tokens.get(i) {
        let TokenKind::JinjaOpen(kind) = token.kind else {
            i += 1;
            continue;
        };
        let close_at = tokens[i + 1..]
            .iter()
            .position(|t| t.kind == TokenKind::JinjaClose(kind))
            .map(|p| i + 1 + p);
        let Some(close_at) = close_at else {
            return Err(Stop::Malformed {
                detail: "unterminated Jinja region".to_string(),
            });
        };
        let close = &tokens[close_at];
        let span = Span::new(token.start, close.end);
        let inner = text
            .get(token.end as usize..close.start as usize)
            .unwrap_or("");
        let region = match kind {
            JinjaKind::Expr => expression_region(text, span, inner),
            JinjaKind::Stmt => statement_region(span, inner, &mut open, &mut out)?,
        };
        out.regions.push(region);
        i = close_at + 1;
    }
    if let Some((_, start)) = open.last() {
        return Err(Stop::ControlFlow {
            detail: "unbalanced control block".to_string(),
            span: Span::new(*start, *start),
        });
    }
    Ok(out)
}

fn expression_region(text: &str, span: Span, inner: &str) -> Region {
    let classified = classify(inner);
    let kind = match (&classified.class, classified.tag) {
        (Class::Relation(crate::jinja::RelInfo::Model { .. }), _) => RegionKind::Ref,
        (Class::Relation(crate::jinja::RelInfo::Source { .. }), _) => RegionKind::Source,
        (Class::Relation(crate::jinja::RelInfo::This), _) => RegionKind::This,
        (_, ExprKindTag::Var) => RegionKind::Var,
        (_, ExprKindTag::Config) => RegionKind::Config,
        (_, ExprKindTag::IsIncremental) => RegionKind::IsIncremental,
        (Class::Macro(name), _) if !name.is_empty() => RegionKind::Macro(name.clone()),
        _ => RegionKind::Expression,
    };
    let word = (classified.class != Class::Blank).then(|| first_line(text, span));
    Region {
        span,
        word,
        class: classified.class,
        kind,
        clause: false,
    }
}

/// The placeholder covers the region's first line: an identifier cannot
/// span a newline, and every newline must stay where the template has it.
fn first_line(text: &str, span: Span) -> Span {
    let line_len = text
        .get(span.start as usize..span.end as usize)
        .and_then(|region| region.find(['\n', '\r']))
        .unwrap_or(span.len() as usize);
    // `{{` is on the first line, so the word is never empty.
    Span::new(span.start, span.start + line_len.max(1) as u32)
}

fn statement_region(
    span: Span,
    inner: &str,
    open: &mut Vec<(OpenBlock, u32)>,
    out: &mut Walked,
) -> Result<Region, Stop> {
    let control = |detail: String| Stop::ControlFlow { detail, span };
    let tag = classify_tag(inner);
    let kind = match &tag {
        Tag::IfIncremental | Tag::EndIf => RegionKind::IsIncremental,
        _ => RegionKind::Statement,
    };
    match tag {
        Tag::IfIncremental => {
            if !open.is_empty() {
                return Err(control("nested is_incremental block".to_string()));
            }
            open.push((OpenBlock::Incremental, span.start));
        }
        Tag::EndIf => match open.pop() {
            Some((OpenBlock::Incremental, start)) => {
                out.incremental.push(Span::new(start, span.end));
            }
            _ => return Err(control("unbalanced endif".to_string())),
        },
        Tag::SetBlock => open.push((OpenBlock::Set, span.start)),
        Tag::EndSet => match open.pop() {
            Some((OpenBlock::Set, start)) => out.set_blocks.push(Span::new(start, span.end)),
            _ => return Err(control("unbalanced endset".to_string())),
        },
        Tag::SetInline | Tag::Do => {}
        Tag::Other(name) => {
            let what = if name.is_empty() {
                "statement"
            } else {
                name.as_str()
            };
            return Err(control(format!(
                "control flow `{what}` needs the compiled fallback"
            )));
        }
    }
    Ok(Region {
        span,
        word: None,
        class: Class::Blank,
        kind,
        clause: false,
    })
}

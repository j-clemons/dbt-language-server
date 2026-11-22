//! Same-length masking: every Jinja region is overwritten byte for byte,
//! newlines preserved, so offsets and sqlparser line/columns agree with
//! the template. Placeholders are named by offset, not by their text.

use std::ops::Range;

use cogitator_ir::hir::JinjaRole;
use cogitator_ir::origin::TemplateVariant;
use cogitator_ir::Span;
use cogitator_parser::backend::tokenize_template;
use cogitator_parser::LineTable;

use crate::jinja::{config_kwargs, Class, Raw};

mod walk;

use walk::walk_regions;

/// The HIR `JinjaKind` of a region, before its macro name is interned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RegionKind {
    Ref,
    Source,
    This,
    Var,
    Macro(String),
    IsIncremental,
    Config,
    Expression,
    Statement,
    Comment,
}

#[derive(Debug, Clone)]
pub(crate) struct Region {
    pub span: Span,
    /// The placeholder identifier's bytes; `None` when the region is blanked.
    pub word: Option<Span>,
    pub class: Class,
    pub kind: RegionKind,
    pub clause: bool,
}

impl Region {
    /// The role before position refines it. A region that never finds a
    /// consumer stays Blank, the role for regions inside holes and strings.
    pub fn initial_role(&self) -> JinjaRole {
        if self.clause {
            JinjaRole::ClausePosition
        } else {
            JinjaRole::Blank
        }
    }
}

#[derive(Debug)]
pub(crate) struct Scanned {
    pub mask: String,
    pub regions: Vec<Region>,
    pub variant: TemplateVariant,
    pub configs: Vec<Vec<(String, Raw)>>,
}

#[derive(Debug)]
pub(crate) enum Stop {
    /// Control flow outside the `is_incremental()` shape.
    ControlFlow { detail: String, span: Span },
    /// minijinja could not tokenize the template.
    Malformed { detail: String },
}

pub(crate) fn scan(text: &str, variant: TemplateVariant) -> Result<Scanned, Stop> {
    let table = LineTable::new(text);
    let (mut tokens, mut segments) = (Vec::new(), Vec::new());
    tokenize_template(text, 0, &table, &mut tokens, &mut segments).map_err(|e| {
        Stop::Malformed {
            detail: e.to_string(),
        }
    })?;
    let walked = walk_regions(text, &tokens)?;
    let variant = effective_variant(variant, &walked.incremental);
    let mut regions = walked.regions;
    let mut blank_ranges = walked.set_blocks;
    if variant == TemplateVariant::FullRefresh {
        blank_ranges.extend(walked.incremental.iter().copied());
    }
    blank_inner_regions(&mut regions, &blank_ranges);
    let mut configs = Vec::new();
    for region in &regions {
        if region.kind == RegionKind::Config && region.class == Class::Blank {
            if let Some(inner) = text.get(region.span.start as usize..region.span.end as usize) {
                configs.push(config_kwargs(strip_expr_markers(inner)));
            }
        }
    }
    add_comments(text, &segments, &mut regions);
    let mask = render_mask(text, &segments, &regions, &blank_ranges)?;
    Ok(Scanned {
        mask,
        regions,
        variant,
        configs,
    })
}

/// A template with incremental blocks has no `Plain` variant: it is the
/// variant rules run on, `Incremental`.
fn effective_variant(requested: TemplateVariant, incremental: &[Span]) -> TemplateVariant {
    if requested == TemplateVariant::Plain && !incremental.is_empty() {
        TemplateVariant::Incremental
    } else {
        requested
    }
}

fn blank_inner_regions(regions: &mut [Region], ranges: &[Span]) {
    let merged = merge_ranges(ranges);
    for region in regions {
        let after = merged.partition_point(|r| r.start <= region.span.start);
        let inside = after
            .checked_sub(1)
            .and_then(|i| merged.get(i))
            .is_some_and(|r| r.contains(region.span));
        if inside {
            region.word = None;
            region.class = Class::Blank;
        }
    }
}

/// Sorted, disjoint cover of `ranges` (set blocks may nest in `if`s).
fn merge_ranges(ranges: &[Span]) -> Vec<Span> {
    let mut sorted = ranges.to_vec();
    sorted.sort();
    let mut merged: Vec<Span> = Vec::with_capacity(sorted.len());
    for range in sorted {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// `{{ config(a=1) }}` → `config(a=1)` for the config parser.
fn strip_expr_markers(region: &str) -> &str {
    let inner = region.strip_prefix("{{").unwrap_or(region);
    let inner = inner.strip_prefix(['-', '+']).unwrap_or(inner);
    let inner = inner.strip_suffix("}}").unwrap_or(inner);
    inner.strip_suffix(['-', '+']).unwrap_or(inner)
}

/// Everything not covered by a segment or a region is a Jinja comment
/// or whitespace minijinja trimmed; comments become Blank nodes.
fn add_comments(text: &str, segments: &[Range<usize>], regions: &mut Vec<Region>) {
    let mut covered: Vec<Range<usize>> = segments.to_vec();
    covered.extend(
        regions
            .iter()
            .map(|r| r.span.start as usize..r.span.end as usize),
    );
    covered.sort_by_key(|r| r.start);
    let mut at = 0usize;
    let mut gaps = Vec::new();
    for range in covered
        .iter()
        .chain(std::iter::once(&(text.len()..text.len())))
    {
        if range.start > at {
            gaps.push(at..range.start);
        }
        at = at.max(range.end);
    }
    for gap in gaps {
        // A gap may start with whitespace trimmed by `{#-`; the node
        // spans the comment only.
        let comment = text.get(gap.clone()).and_then(|g| {
            let open = g.find("{#")?;
            let close = g.rfind("#}")? + 2;
            Some(gap.start + open..gap.start + close)
        });
        if let Some(comment) = comment {
            regions.push(Region {
                span: Span::new(comment.start as u32, comment.end as u32),
                word: None,
                class: Class::Blank,
                kind: RegionKind::Comment,
                clause: false,
            });
        }
    }
    regions.sort_by_key(|r| r.span.start);
}

fn render_mask(
    text: &str,
    segments: &[Range<usize>],
    regions: &[Region],
    blank_ranges: &[Span],
) -> Result<String, Stop> {
    // Start blank so comments and trimmed whitespace vanish; segments and
    // regions then restore or overwrite their own bytes.
    let source = text.as_bytes();
    let mut out: Vec<u8> = source
        .iter()
        .map(|b| if *b == b'\n' { b'\n' } else { b' ' })
        .collect();
    for segment in segments {
        if let (Some(dst), Some(src)) = (out.get_mut(segment.clone()), source.get(segment.clone()))
        {
            dst.copy_from_slice(src);
        }
    }
    for region in regions {
        blank(&mut out, region.span);
        if let Some(word) = region.word {
            write_placeholder(&mut out, word);
        }
    }
    for range in blank_ranges {
        blank(&mut out, *range);
    }
    String::from_utf8(out).map_err(|e| Stop::Malformed {
        detail: format!("mask is not valid UTF-8: {e}"),
    })
}

fn blank(out: &mut [u8], span: Span) {
    if let Some(bytes) = out.get_mut(span.start as usize..span.end as usize) {
        for b in bytes.iter_mut().filter(|b| **b != b'\n') {
            *b = b' ';
        }
    }
}

/// `_jjj` over the word; the region's later lines stay blank.
fn write_placeholder(out: &mut [u8], word: Span) {
    if let Some(bytes) = out.get_mut(word.start as usize..word.end as usize) {
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = if i == 0 { b'_' } else { b'j' };
        }
    }
}

#[cfg(test)]
mod tests;

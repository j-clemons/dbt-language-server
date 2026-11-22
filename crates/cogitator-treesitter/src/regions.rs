//! Where the Jinja constructs are, according to the tree-sitter-dbt CST
//! (06 §3.3).
//!
//! The grammar nests blocks (`if_statement` holds its body), but this
//! crate wants a flat list: one region per tag or `{{ … }}`, the text
//! between them SQL. So the walk goes into every block and takes each
//! run of tag tokens, `{%` up to `%}`, as one region; `content` stays a
//! gap.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegionKind {
    Jinja,
    Comment,
    Error,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Region {
    pub(crate) kind: RegionKind,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

const TAG_OPEN: [&str; 3] = ["{%", "{%-", "{%+"];
const TAG_CLOSE: [&str; 3] = ["%}", "-%}", "+%}"];

/// Children that are text, not Jinja: SQL, a docs block's Markdown and the
/// body of `{% raw %}` all go to the SQL side, as they did before.
const TEXT: [&str; 3] = ["content", "markdown_content", "raw_content"];

/// Every Jinja region of the document, sorted and non-overlapping.
/// Everything between regions is SQL text.
pub(crate) fn collect_regions(root: tree_sitter::Node<'_>) -> Vec<Region> {
    let mut regions = Vec::new();
    walk(root, false, &mut regions);
    regions.sort_by_key(|r| r.start);
    // defensive: a recovered tree may overlap
    regions.dedup_by(|b, a| {
        if b.start < a.end {
            a.end = a.end.max(b.end);
            true
        } else {
            false
        }
    });
    regions
}

/// Walks the children of `node`. Inside `raw_block` the tags are
/// comment regions: minijinja cannot tokenize `{% raw %}` alone, and the
/// tags carry nothing to complete or hover.
fn walk(node: tree_sitter::Node<'_>, in_raw: bool, regions: &mut Vec<Region>) {
    let tag_kind = if in_raw {
        RegionKind::Comment
    } else {
        RegionKind::Jinja
    };
    // the tag being read: its start, where its last token ended, and
    // whether an ERROR is inside it
    let mut tag: Option<Tag> = None;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let kind = child.kind();
        if TAG_OPEN.contains(&kind) {
            close(&mut tag, tag_kind, regions);
            tag = Some(Tag {
                start: child.start_byte(),
                end: child.end_byte(),
                broken: false,
            });
            continue;
        }
        if let Some(open) = tag.as_mut() {
            // anything between `{%` and `%}` is part of the tag
            open.end = child.end_byte();
            open.broken |= child.is_error() || child.is_missing();
            if TAG_CLOSE.contains(&kind) {
                close(&mut tag, tag_kind, regions);
            }
            continue;
        }
        if child.is_error() {
            regions.push(region(RegionKind::Error, child));
            continue;
        }
        match kind {
            "output" => regions.push(region(RegionKind::Jinja, child)),
            "comment" => regions.push(region(RegionKind::Comment, child)),
            _ if TEXT.contains(&kind) => {}
            "raw_block" => walk(child, true, regions),
            // a statement, or an elif/else clause: its tags and its body
            _ if child.is_named() => walk(child, in_raw, regions),
            // stray tokens of a recovered tree
            _ => {}
        }
    }
    // a tag the tree never closed (a MISSING `%}`) runs to its last token
    close(&mut tag, tag_kind, regions);
}

struct Tag {
    start: usize,
    end: usize,
    broken: bool,
}

/// A tag with an ERROR in it is one Error region, so the caller decides
/// about it whole rather than about two halves.
fn close(tag: &mut Option<Tag>, kind: RegionKind, regions: &mut Vec<Region>) {
    if let Some(Tag { start, end, broken }) = tag.take() {
        let kind = if broken { RegionKind::Error } else { kind };
        regions.push(Region { kind, start, end });
    }
}

fn region(kind: RegionKind, node: tree_sitter::Node<'_>) -> Region {
    Region {
        kind,
        start: node.start_byte(),
        end: node.end_byte(),
    }
}

//! What the logical rules share: every scalar the plan evaluates, and
//! small text readers for the fixes that edit around a subquery.

use std::collections::BTreeSet;

use cogitator_ir::mir::{Mir, RelId, ScalarId, ScalarKind};
use cogitator_ir::Span;

/// Every scalar node the plan evaluates, with the relation evaluating
/// it. Subquery bodies are relations of their own, so the walk stops at
/// them; each node appears once.
pub(crate) fn plan_scalars(mir: &Mir) -> Vec<(RelId, ScalarId)> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for id in mir.reachable() {
        let Some(rel) = mir.rels.get(id) else {
            continue;
        };
        let mut stack = rel.op.scalars();
        while let Some(scalar) = stack.pop() {
            if !seen.insert(scalar) {
                continue;
            }
            out.push((id, scalar));
            if let Some(node) = mir.scalars.get(scalar) {
                stack.extend(node.kind.children());
            }
        }
    }
    out
}

/// True when the tree under `root` holds an `Opaque` scalar, which can
/// read anything.
pub(crate) fn touches_opaque(mir: &Mir, root: ScalarId) -> bool {
    let mut seen = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        match mir.scalars.get(id).map(|s| &s.kind) {
            None | Some(ScalarKind::Opaque { .. }) => return true,
            Some(kind) => stack.extend(kind.children()),
        }
    }
    false
}

/// The position of the first non-whitespace byte at or after `pos`.
pub(crate) fn next_nonspace(text: &str, pos: u32) -> Option<u32> {
    let rest = text.get(pos as usize..)?;
    let skipped = rest.len() - rest.trim_start().len();
    (skipped < rest.len()).then_some(pos + skipped as u32)
}

/// The word starting at the next non-whitespace byte after `pos`, or
/// `None` when something else (a comment, punctuation) comes first.
pub(crate) fn word_after(text: &str, pos: u32) -> Option<Span> {
    let start = next_nonspace(text, pos)?;
    let rest = text.get(start as usize..)?;
    let len = rest
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    (len > 0).then_some(Span::new(start, start + len as u32))
}

/// True when `body` holds a comment or a Jinja delimiter, which an edit
/// over it would lose or break.
pub(crate) fn holds_comment_or_jinja(body: &str) -> bool {
    body.contains("--") || body.contains("/*") || body.contains('{') || body.contains('}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_next_word() {
        let text = "a  not in";
        let span = word_after(text, 1).expect("word");
        assert_eq!(&text[span.start as usize..span.end as usize], "not");
    }

    #[test]
    fn a_comment_before_the_word_is_no_word() {
        assert!(word_after("a -- c\n b", 1).is_none());
    }

    #[test]
    fn end_of_text_has_no_next_byte() {
        assert_eq!(next_nonspace("ab  ", 2), None);
        assert_eq!(next_nonspace("ab )", 2), Some(3));
    }
}

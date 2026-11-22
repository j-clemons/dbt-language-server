//! Line-level reading of YAML text for the actions that insert into it:
//! indents, the extent of a block, and an insertion point that stays a
//! valid position when the file does not end in a newline. Pure over the
//! text — the YAML stays as written, comments and all.

use cogitator_lsp::{Position, Range, TextEdit};

/// Spaces before the first non-space character.
pub(super) fn indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// A line that carries YAML content (not blank, not only a comment).
pub(super) fn is_content(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty() && !trimmed.starts_with('#')
}

/// The last content line of the block that starts at `start` and holds
/// every following line indented deeper than `parent_indent`.
pub(super) fn block_end(lines: &[&str], start: usize, parent_indent: usize) -> usize {
    let mut last = start;
    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        if !is_content(line) {
            continue;
        }
        if indent(line) <= parent_indent {
            break;
        }
        last = i;
    }
    last
}

/// A TextEdit inserting `body` (complete lines, each ending in `\n`) on
/// the line after `after`. At the end of a file without a final newline
/// the insertion goes at the end of the last line, behind a newline.
pub(super) fn insert_after_line(text: &str, after: usize, body: String) -> TextEdit {
    let lines: Vec<&str> = text.lines().collect();
    let at_end = after + 1 >= lines.len();
    if at_end && !text.is_empty() && !text.ends_with('\n') {
        let last = lines.len().saturating_sub(1);
        let character = lines.get(last).map_or(0, |l| l.encode_utf16().count()) as u32;
        let end = Position {
            line: last as u32,
            character,
        };
        return TextEdit {
            range: Range::zero_width(end),
            new_text: format!("\n{body}"),
        };
    }
    TextEdit {
        range: Range::zero_width(Position {
            line: (after + 1) as u32,
            character: 0,
        }),
        new_text: body,
    }
}

/// A plain YAML scalar for a name, or None when it would need quoting:
/// the actions only write names a reader would write unquoted.
pub(super) fn plain_name(name: &str) -> Option<&str> {
    let ok = !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit());
    ok.then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_end_skips_blank_and_comment_lines_inside() {
        let text = "a:\n  b: 1\n\n  # note\n  c: 2\nd: 3\n";
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(block_end(&lines, 0, 0), 4);
    }

    #[test]
    fn insertion_after_the_last_line_with_and_without_a_final_newline() {
        let edit = insert_after_line("a: 1\n", 0, "b: 2\n".into());
        assert_eq!(
            edit.range.start,
            Position {
                line: 1,
                character: 0
            }
        );
        assert_eq!(edit.new_text, "b: 2\n");
        let edit = insert_after_line("a: 1", 0, "b: 2\n".into());
        assert_eq!(
            edit.range.start,
            Position {
                line: 0,
                character: 4
            }
        );
        assert_eq!(edit.new_text, "\nb: 2\n");
    }

    #[test]
    fn only_plain_names_are_written() {
        assert_eq!(plain_name("order_id"), Some("order_id"));
        assert_eq!(plain_name("has space"), None);
        assert_eq!(plain_name("1st"), None);
        assert_eq!(plain_name(""), None);
    }
}

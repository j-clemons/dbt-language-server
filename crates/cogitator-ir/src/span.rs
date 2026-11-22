//! Byte spans into the text a plan was built from.

use serde::Serialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub const fn new(start: u32, end: u32) -> Self {
        Self { start, end }
    }

    pub const fn len(self) -> u32 {
        self.end.saturating_sub(self.start)
    }

    pub const fn is_empty(self) -> bool {
        self.end <= self.start
    }

    pub const fn contains(self, other: Span) -> bool {
        self.start <= other.start && other.end <= self.end
    }

    /// True when both ends are in range and on UTF-8 boundaries.
    pub fn is_valid_in(self, text: &str) -> bool {
        self.start <= self.end
            && text.is_char_boundary(self.start as usize)
            && text.is_char_boundary(self.end as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validity_is_char_boundary_aware() {
        let text = "é1";
        assert!(Span::new(0, 2).is_valid_in(text));
        assert!(!Span::new(1, 2).is_valid_in(text));
        assert!(!Span::new(0, 9).is_valid_in(text));
        assert!(!Span::new(2, 0).is_valid_in(text));
    }

    #[test]
    fn contains_and_len() {
        let outer = Span::new(2, 10);
        assert!(outer.contains(Span::new(2, 3)));
        assert!(!outer.contains(Span::new(1, 3)));
        assert_eq!(Span::new(5, 3).len(), 0);
        assert!(Span::new(5, 3).is_empty());
    }
}

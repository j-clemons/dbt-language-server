//! Count-based budgets. Clocks would make results non-deterministic, so
//! the pass manager counts instead; an overrun marks the run incomplete.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budgets {
    pub max_passes: u32,
    pub max_nodes: u32,
    pub max_applications: u32,
}

impl Default for Budgets {
    fn default() -> Self {
        Self::STANDARD
    }
}

impl Budgets {
    pub const STANDARD: Budgets = Budgets {
        max_passes: 8,
        max_nodes: 50_000,
        max_applications: 10_000,
    };

    /// The first limit reached, if any.
    pub fn exceeded(&self, passes: u32, nodes: u32, applications: u32) -> Option<Exceeded> {
        if passes >= self.max_passes {
            Some(Exceeded::Passes)
        } else if nodes >= self.max_nodes {
            Some(Exceeded::Nodes)
        } else if applications >= self.max_applications {
            Some(Exceeded::Applications)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exceeded {
    Passes,
    Nodes,
    Applications,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_are_inclusive_upper_bounds() {
        let b = Budgets::STANDARD;
        assert_eq!(b.exceeded(0, 0, 0), None);
        assert_eq!(b.exceeded(8, 0, 0), Some(Exceeded::Passes));
        assert_eq!(b.exceeded(1, 50_000, 0), Some(Exceeded::Nodes));
        assert_eq!(b.exceeded(1, 1, 10_000), Some(Exceeded::Applications));
    }
}

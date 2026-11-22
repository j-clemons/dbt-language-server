//! The rule traits. A rule implements `Rule` plus exactly the phase
//! traits it needs; the registry records which.

use cogitator_ir::mir::Mir;

use crate::cx::Cx;
use crate::findings::Findings;
use crate::meta::RuleMeta;

pub trait Rule: Sync + 'static {
    fn meta(&self) -> &'static RuleMeta;
}

/// Takes the MIR by value so a rewrite can append nodes and move `root`
/// without cloning; returns it either way.
pub trait Canonicalize: Rule {
    fn apply(&self, ir: Mir, cx: &Cx<'_>) -> Rewrite<Mir>;
}

/// Reads one IR level and records findings. `L` is `Hir`, `Mir` or `Lir`.
pub trait Detect<L>: Rule {
    fn check(&self, ir: &L, cx: &Cx<'_>, out: &mut Findings);
}

/// Result of a canonicaliser visit. `Changed` means `root` moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rewrite<T> {
    Unchanged(T),
    Changed(T),
}

impl<T> Rewrite<T> {
    pub fn changed(&self) -> bool {
        matches!(self, Rewrite::Changed(_))
    }

    pub fn into_inner(self) -> T {
        match self {
            Rewrite::Unchanged(t) | Rewrite::Changed(t) => t,
        }
    }
}

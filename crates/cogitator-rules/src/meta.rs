//! Rule metadata: what a rule needs, where it runs, how sure its fix is.

use cogitator_ir::common::RuleId;
use cogitator_ir::Level;

/// One prerequisite a rule can declare. `AllBranches` means every Jinja
/// branch is visible; a compiled-origin plan of a template with control
/// flow does not have it (specs/ir-pipeline/05).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Need {
    Schema,
    Types,
    Stats,
    Dialect,
    Mat,
    Dag,
    Tests,
    Phys,
    AllBranches,
}

const ALL_NEEDS: [Need; 9] = [
    Need::Schema,
    Need::Types,
    Need::Stats,
    Need::Dialect,
    Need::Mat,
    Need::Dag,
    Need::Tests,
    Need::Phys,
    Need::AllBranches,
];

/// A small set of `Need`s, cheap to copy and constructible in statics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Needs(u16);

impl Needs {
    pub const NONE: Needs = Needs(0);

    pub const fn with(self, need: Need) -> Needs {
        Needs(self.0 | 1 << need as u16)
    }

    pub const fn contains(self, need: Need) -> bool {
        self.0 & 1 << need as u16 != 0
    }

    /// Everything in `self` missing from `available`, in declaration order.
    pub fn unmet(self, available: Needs) -> impl Iterator<Item = Need> {
        ALL_NEEDS
            .into_iter()
            .filter(move |n| self.contains(*n) && !available.contains(*n))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    /// Disabled, or a canonicaliser (never reported).
    Off,
    Hint,
    Info,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuleClass {
    Canon,
    Advice,
}

/// Where a rule runs. A `HirAndLir` rule implements `Detect` for both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuleLevel {
    Hir,
    Mir,
    Lir,
    HirAndLir,
}

impl RuleLevel {
    pub const fn runs_at(self, level: Level) -> bool {
        matches!(
            (self, level),
            (RuleLevel::Hir | RuleLevel::HirAndLir, Level::Hir)
                | (RuleLevel::Mir, Level::Mir)
                | (RuleLevel::Lir | RuleLevel::HirAndLir, Level::Lir)
        )
    }
}

/// How safe a rule's fixes are. A rule may offer several fixes; each
/// `Fix` states its own safety. **P** fixes preserve bag semantics, **N**
/// fixes change intent and are never preferred or in `source.fixAll`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FixSafety {
    None,
    Preserving,
    ChangesIntent,
    /// Offers both, per fix or conditionally on a proof.
    Mixed,
}

#[derive(Debug)]
pub struct RuleMeta {
    pub id: RuleId,
    pub level: RuleLevel,
    pub class: RuleClass,
    pub needs: Needs,
    /// The highest severity the rule reports. Rules report per dialect
    /// (a Warning on BigQuery, a Hint elsewhere) but never above this,
    /// and catalog statistics never raise it.
    pub severity: Severity,
    pub fix: FixSafety,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_are_a_set() {
        let needs = Needs::NONE.with(Need::Schema).with(Need::Dag);
        assert!(needs.contains(Need::Dag));
        assert!(!needs.contains(Need::Mat));
        let have = Needs::NONE.with(Need::Dag);
        assert_eq!(needs.unmet(have).collect::<Vec<_>>(), vec![Need::Schema]);
    }

    #[test]
    fn levels() {
        assert!(RuleLevel::HirAndLir.runs_at(Level::Hir));
        assert!(RuleLevel::HirAndLir.runs_at(Level::Lir));
        assert!(!RuleLevel::HirAndLir.runs_at(Level::Mir));
    }
}

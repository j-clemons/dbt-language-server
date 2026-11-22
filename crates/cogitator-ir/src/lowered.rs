//! The result of building or lowering one level, and the verifier's
//! report vocabulary. A [`Degradation`] is an expected gap in the input
//! (a hole, an open schema); a [`Violation`] is our own bug.

use serde::Serialize;

use crate::common::Level;
use crate::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Fidelity {
    Complete,
    Partial,
    Declined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum DegradationKind {
    ParseHole,
    JinjaControlFlow,
    OpaqueExpression,
    UnsupportedSyntax,
    OpenSchema,
    RecursiveCte,
    /// A front-end cap; `detail` names which one.
    CapExceeded,
    StaleCompiled,
    NoCompiledFallback,
    /// A Wave 1 stub answered; removed as the stages land.
    NotImplemented,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Degradation {
    pub kind: DegradationKind,
    pub span: Option<Span>,
    pub detail: String,
}

impl Degradation {
    pub fn new(kind: DegradationKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            span: None,
            detail: detail.into(),
        }
    }

    pub fn at(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }
}

/// One row of the invariant tables in specs/ir-pipeline/00.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum Invariant {
    HirIdRange,
    HirScopeTree,
    HirColRefResolved,
    HirCteBackward,
    HirJinjaConsumed,
    HirSpanBoundary,
    MirIdRange,
    MirColDefinedOnce,
    MirColVisible,
    MirDeclaredOutput,
    MirCteAcyclic,
    MirProvenanceTotal,
    MirRootColumns,
    MirTypeSpecificity,
    LirIdRange,
    LirMapsToMir,
    LirRootColumns,
    LirPruningKeyOwned,
    LirHintSupported,
}

impl Invariant {
    pub const fn level(self) -> Level {
        use Invariant as I;
        match self {
            I::HirIdRange
            | I::HirScopeTree
            | I::HirColRefResolved
            | I::HirCteBackward
            | I::HirJinjaConsumed
            | I::HirSpanBoundary => Level::Hir,
            I::MirIdRange
            | I::MirColDefinedOnce
            | I::MirColVisible
            | I::MirDeclaredOutput
            | I::MirCteAcyclic
            | I::MirProvenanceTotal
            | I::MirRootColumns
            | I::MirTypeSpecificity => Level::Mir,
            I::LirIdRange
            | I::LirMapsToMir
            | I::LirRootColumns
            | I::LirPruningKeyOwned
            | I::LirHintSupported => Level::Lir,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Violation {
    pub invariant: Invariant,
    pub detail: String,
}

impl Violation {
    pub fn new(invariant: Invariant, detail: impl Into<String>) -> Self {
        Self {
            invariant,
            detail: detail.into(),
        }
    }
}

/// How often the pipeline verifies (specs/ir-pipeline/04).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum VerifyMode {
    EveryRewrite,
    Boundaries,
}

impl VerifyMode {
    /// `EveryRewrite` in debug builds, `Boundaries` in release.
    pub const fn for_build() -> Self {
        if cfg!(debug_assertions) {
            VerifyMode::EveryRewrite
        } else {
            VerifyMode::Boundaries
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Lowered<T> {
    pub value: Option<T>,
    pub fidelity: Fidelity,
    pub degraded: Vec<Degradation>,
    pub violations: Vec<Violation>,
}

impl<T> Lowered<T> {
    pub fn complete(value: T) -> Self {
        Self {
            value: Some(value),
            fidelity: Fidelity::Complete,
            degraded: Vec::new(),
            violations: Vec::new(),
        }
    }

    pub fn partial(value: T, degraded: Vec<Degradation>) -> Self {
        Self {
            value: Some(value),
            fidelity: Fidelity::Partial,
            degraded,
            violations: Vec::new(),
        }
    }

    pub fn declined(degraded: Vec<Degradation>) -> Self {
        Self {
            value: None,
            fidelity: Fidelity::Declined,
            degraded,
            violations: Vec::new(),
        }
    }

    /// Records verifier output. Any violation drops the value and
    /// declines the level, so nothing below it runs on a bad plan.
    pub fn with_violations(mut self, violations: Vec<Violation>) -> Self {
        if !violations.is_empty() {
            self.value = None;
            self.fidelity = Fidelity::Declined;
        }
        self.violations.extend(violations);
        self
    }

    /// For tests and fuzz targets only; the server reads `violations`.
    pub fn assert_clean(&self) {
        assert_clean(&self.violations);
    }
}

/// Panics listing every violation. Never called from the server.
pub fn assert_clean(violations: &[Violation]) {
    assert!(violations.is_empty(), "IR verifier: {violations:#?}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_violation_drops_the_value_and_declines() {
        let lowered = Lowered::complete(1)
            .with_violations(vec![Violation::new(Invariant::MirCteAcyclic, "x")]);
        assert_eq!(lowered.value, None);
        assert_eq!(lowered.fidelity, Fidelity::Declined);
        assert_eq!(Invariant::MirCteAcyclic.level(), Level::Mir);
    }

    #[test]
    fn no_violations_keep_the_value() {
        let lowered = Lowered::partial(1, vec![]).with_violations(vec![]);
        assert_eq!(lowered.value, Some(1));
        assert_eq!(lowered.fidelity, Fidelity::Partial);
        lowered.assert_clean();
    }
}

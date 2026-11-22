//! The cogitator query IR (specs/ir-pipeline/): arenas, the three
//! levels (`hir`, `mir`, `lir`), the SQL type lattice, the
//! lowering/verifier contract, and text printers.
//!
//! This crate depends only on serde and petgraph. It must never pull in
//! `cogitator-error` (which pulls sqlparser): sqlparser and minijinja
//! stay inside `cogitator-sql-front`.

pub mod arena;
pub mod common;
pub mod hir;
pub mod intern;
pub mod lir;
pub mod lowered;
pub mod mir;
pub mod origin;
#[cfg(test)]
mod print_snapshots;
mod print_util;
pub mod schema;
pub mod span;
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;
pub mod types;

pub use arena::{Arena, Idx};
pub use common::{Level, RuleId};
pub use intern::{Interner, Symbol};
pub use lowered::{
    assert_clean, Degradation, DegradationKind, Fidelity, Invariant, Lowered, VerifyMode, Violation,
};
pub use span::Span;
pub use types::{ColType, Nullability, SqlType};

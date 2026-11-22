//! Builders for hand-constructed plans: lowering, rule and oracle tests
//! code against these, and later waves can extend them. Behind the
//! `testkit` feature; never part of the default build.
//!
//! The builders keep the verifier happy by default (operands precede
//! users, provenance is populated) and expose the IR's public fields,
//! so a test can break an invariant on purpose.

pub mod hir;
pub mod lir;
pub mod mir;

use crate::types::{ColType, Nullability, SqlType};

pub use hir::HirBuilder;
pub use lir::LirBuilder;
pub use mir::MirBuilder;

/// `int64 not null`, the default test column.
pub fn int_col() -> ColType {
    ColType::new(SqlType::Int { bits: Some(64) }, Nullability::NotNull)
}

/// `text` of unknown nullability.
pub fn text_col() -> ColType {
    ColType::new(SqlType::Text, Nullability::Unknown)
}

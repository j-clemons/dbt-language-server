//! The canonicalising rules (specs/ir-pipeline/05). They run to fixpoint
//! before detection; each appends nodes and moves `root`, and preserves
//! the root's columns and every node's origin span.

mod join_sides;
mod pred;
pub mod predicates;
mod pushdown;
mod rebuild;
pub mod required_columns;
mod scalar_map;
pub mod simplify;

#[cfg(test)]
mod predicates_blockers_tests;
#[cfg(test)]
mod predicates_tests;
#[cfg(test)]
mod required_columns_tests;
#[cfg(test)]
mod simplify_tests;
#[cfg(test)]
mod test_util;

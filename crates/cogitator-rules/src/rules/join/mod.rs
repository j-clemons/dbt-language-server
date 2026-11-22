//! `join.*` rules.

pub mod cartesian;
pub mod distinct_fanout;
pub mod key_type_mismatch;
pub mod left_nullified;
pub mod redundant_left;

mod left_fix;
pub(crate) mod null_reject;
pub(crate) mod support;
pub(crate) mod text;

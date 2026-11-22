//! What the pass manager hands each rule.

use std::collections::BTreeMap;
use std::sync::Arc;

use cogitator_ir::common::{Materialization, ModelConfig};
use cogitator_ir::hir::Hir;
use cogitator_ir::lir::{Backend, PhysicalFacts};
use cogitator_ir::mir::Mir;
use cogitator_ir::schema::ProjectSchemas;

use crate::meta::{Need, Needs, RuleMeta};
use crate::passes::SkipReason;

/// A predicate a consumer provably applies to every row it reads of this
/// model, over one output column.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ColumnFilter {
    /// The output column on this model, as the consumer read it.
    pub column: Arc<str>,
    /// The predicate in canonical text with the column as `$`: two
    /// consumers filter alike exactly when these are equal.
    pub predicate: Arc<str>,
}

/// One direct downstream model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConsumerFacts {
    pub filters: Arc<[ColumnFilter]>,
}

/// Another model holds the same canonical subplan as this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedSubplan {
    /// The canonical text, for confirming against this model's own plan.
    pub text: Arc<str>,
    pub size: u32,
    /// The others, sorted and capped; `total` counts every one.
    pub models: Vec<Arc<str>>,
    pub total: usize,
}

/// Downstream demand for this model, for `cross.*` rules. Sorted maps
/// so iteration stays deterministic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DagFacts {
    /// Columns downstream models read from this model, by model name.
    pub downstream_columns: BTreeMap<Arc<str>, Vec<Arc<str>>>,
    /// Columns a dbt test on this model reads; not a downstream model.
    pub tested_columns: Vec<Arc<str>>,
    /// Set when this model's columns rest on a catalog snapshot: when it
    /// was generated, for `Evidence::Catalog`.
    pub catalog_generated_at: Option<Arc<str>>,
    /// Every direct downstream model, including one that reads no column.
    pub consumers: BTreeMap<Arc<str>, ConsumerFacts>,
    /// The non-ephemeral models this one is compiled into; filled only
    /// for an ephemeral model, chains of ephemerals looked through.
    pub inlined_into: Vec<Arc<str>>,
    /// Subplans this model shares with another, by canonical hash.
    pub shared_subplans: BTreeMap<u64, SharedSubplan>,
}

/// Read-only inputs. `available` states what is actually usable, so
/// `Skipped` reasons can name the need; the optional fields stay `None`
/// when their need is absent.
#[derive(Debug, Clone, Copy)]
pub struct Cx<'a> {
    pub available: Needs,
    pub hir: Option<&'a Hir>,
    /// The canonical plan, set by the driver for LIR detect: LIR scalars
    /// and columns are MIR ids.
    pub mir: Option<&'a Mir>,
    /// The text `hir` spans index, for rules that verify a fix's edit
    /// against what is written. `None` withholds those fixes.
    pub text: Option<&'a str>,
    pub dialect: Option<Backend>,
    pub mat: Option<Materialization>,
    pub config: Option<&'a ModelConfig>,
    pub schemas: Option<&'a ProjectSchemas>,
    pub phys: Option<&'a PhysicalFacts>,
    pub dag: Option<&'a DagFacts>,
}

impl<'a> Cx<'a> {
    pub fn new(available: Needs) -> Self {
        Self {
            available,
            hir: None,
            mir: None,
            text: None,
            dialect: None,
            mat: None,
            config: None,
            schemas: None,
            phys: None,
            dag: None,
        }
    }

    pub fn has(&self, need: Need) -> bool {
        self.available.contains(need)
    }

    /// Why `meta` cannot run here, naming the first unmet need.
    pub fn skip_reason(&self, meta: &RuleMeta) -> Option<SkipReason> {
        meta.needs
            .unmet(self.available)
            .next()
            .map(SkipReason::Unmet)
    }
}

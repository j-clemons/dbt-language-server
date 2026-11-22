//! BigQuery partition and block pruning.
//!
//! Partition column ([`PARTITION_DOC`]):
//!
//! | Predicate | Verdict | Doc says |
//! |---|---|---|
//! | column alone on one side | prunes | "isolate the partitioning column on one side" |
//! | subquery value | blocks | a dynamic value "prevents partition pruning" |
//! | `OR` with another column | blocks | "If `AND` is changed to `OR`, partition pruning won't work" |
//! | arithmetic on the column | blocks | "other functions and complex mathematical operations will require a full table scan" |
//! | compared to another column | blocks | such predicates do not prune |
//! | function not on the supported list | blocks | as for arithmetic |
//! | function on the supported list, or `CAST` | unknown | supported with constant arguments, which the verdict cannot see |
//!
//! Clustered column ([`CLUSTER_DOC`]):
//!
//! | Predicate | Verdict | Doc says |
//! |---|---|---|
//! | column alone | prunes | "use simple filter expressions" |
//! | `CAST`, function or arithmetic on it | blocks | a complex filter expression "is not optimized"; the page's example is `CAST(customer_id AS STRING)` |
//! | compared to another column | blocks | "block pruning cannot be applied" |
//! | filter skips an earlier clustered column | unknown | pruning is "typically less effective", not absent |
//! | subquery or `OR` | unknown | not mentioned |
//!
//! CTEs ([`CTE_DOC`]): non-recursive ones are not materialized and run
//! "once for each reference", so the policy is `Inline`.

use cogitator_ir::lir::{
    Backend, BlockReason, Capabilities, CtePolicy, KeyKind, Lir, PhysColumn, PhysicalFacts, Pred,
    PredShape, PruneKey, PruneUse, Pruning,
};
use cogitator_ir::mir::Mir;

use crate::{lower_plan, DialectBackend, PredContext, PredDetail};

pub const PARTITION_DOC: &str =
    "https://docs.cloud.google.com/bigquery/docs/querying-partitioned-tables";
pub const CLUSTER_DOC: &str =
    "https://docs.cloud.google.com/bigquery/docs/querying-clustered-tables";
pub const CTE_DOC: &str =
    "https://docs.cloud.google.com/bigquery/docs/reference/standard-sql/query-syntax";

/// Functions the partition page lists as still able to prune, given
/// constant arguments. `date` and `extract` appear in its examples;
/// `extract` and `format_timestamp` further depend on the part or format.
const PARTITION_FUNCTIONS: &[&str] = &[
    "date",
    "date_add",
    "date_diff",
    "date_sub",
    "date_trunc",
    "datetime_diff",
    "extract",
    "format_timestamp",
    "timestamp_add",
    "timestamp_diff",
    "timestamp_sub",
    "timestamp_trunc",
];

static CAPABILITIES: Capabilities = Capabilities {
    backend: Backend::BigQuery,
    join_hints: &[],
    cte_policies: &[CtePolicy::Inline],
    pruning_keys: &[KeyKind::Partition, KeyKind::Cluster],
    doc: PARTITION_DOC,
};

fn blocks(why: BlockReason, doc: &'static str) -> Pruning {
    Pruning::Blocks { why, doc }
}

pub struct BigQuery;

impl DialectBackend for BigQuery {
    fn capabilities(&self) -> &'static Capabilities {
        &CAPABILITIES
    }

    fn lower(&self, mir: &Mir, phys: &PhysicalFacts) -> Lir {
        lower_plan(mir, phys, self)
    }

    /// The verdict from shape alone. A `Function` is `Blocks` here only
    /// because `Symbol`s cannot be read without the plan; `refine`
    /// relaxes it for the documented functions.
    fn pruning(&self, pred: &Pred, col: &PhysColumn) -> Pruning {
        use BlockReason as R;
        match (col.key, pred.shape) {
            (_, PredShape::BareCompare | PredShape::Range) => Pruning::Prunes,
            (Some(KeyKind::Partition), PredShape::Function { .. }) => {
                blocks(R::FunctionWrapped, PARTITION_DOC)
            }
            (Some(KeyKind::Partition), PredShape::Subquery) => {
                blocks(R::SubqueryValue, PARTITION_DOC)
            }
            (Some(KeyKind::Partition), PredShape::OrAcrossColumns) => {
                blocks(R::OrAcrossColumns, PARTITION_DOC)
            }
            (Some(KeyKind::Cluster), PredShape::Function { .. }) => {
                blocks(R::FunctionWrapped, CLUSTER_DOC)
            }
            (Some(KeyKind::Cluster), PredShape::Cast) => blocks(R::CastWrapped, CLUSTER_DOC),
            _ => Pruning::Unknown,
        }
    }

    fn refine(&self, decided: Pruning, ctx: &PredContext<'_>) -> Pruning {
        let doc = match ctx.col.key {
            Some(KeyKind::Partition) => PARTITION_DOC,
            Some(KeyKind::Cluster) => CLUSTER_DOC,
            _ => return decided,
        };
        if matches!(
            ctx.detail,
            PredDetail::ArithmeticOnColumn | PredDetail::ComparedToColumn
        ) {
            return blocks(BlockReason::Other, doc);
        }
        let supported = ctx.function.is_some_and(|f| {
            PARTITION_FUNCTIONS
                .iter()
                .any(|s| s.eq_ignore_ascii_case(f))
        });
        if ctx.col.key == Some(KeyKind::Partition) && supported {
            return Pruning::Unknown;
        }
        decided
    }

    /// A filter that skips an earlier clustered column may still prune,
    /// "typically less effective" per the docs, so it is neither `Used`
    /// nor `Blocked`. Keys arrive in `TableFacts` order, which callers
    /// must supply as the `CLUSTER BY` order.
    fn finish_keys(&self, keys: &mut [PruneKey]) {
        let mut chain_intact = true;
        for key in keys.iter_mut().filter(|k| k.kind == KeyKind::Cluster) {
            match key.usage {
                PruneUse::Used(_) if !chain_intact => key.usage = PruneUse::Unknown,
                PruneUse::Used(_) => {}
                _ => chain_intact = false,
            }
        }
    }

    fn cte_policy(&self, _uses: u32) -> CtePolicy {
        CtePolicy::Inline
    }
}

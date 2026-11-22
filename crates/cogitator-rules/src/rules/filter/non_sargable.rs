//! `filter.non_sargable`: a wrapped column may block pruning.
//!
//! The backend has already judged each key (`PruneKey.usage`); this rule
//! finds the predicate behind the verdict and says no more than the
//! verdict and the vendor documentation allow.
//!
//! | Usage | Severity | Claim |
//! |---|---|---|
//! | `Blocked` on BigQuery | Warning | the form the docs say cannot prune, with the link |
//! | `Blocked` elsewhere (Postgres functions) | Hint | an index needs the expression |
//! | `Unknown`, wrapped column | Hint | "may reduce pruning" |
//!
//! A bare filter on a later clustering column is not wrapped either; that
//! belongs to `filter.cluster_order`. `SubqueryValue` and `OrAcrossColumns`
//! belong to `incr.subquery_watermark` and `filter.partition_or`. A
//! conjunct holding an `Opaque` (Jinja, `EXTRACT`,
//! `SUBSTRING`, `TRIM`) or a scan with an open schema makes no claim, and
//! neither does an `Unknown` key read only above other operators.

use cogitator_ir::lir::{Backend, BlockReason, KeyKind, Lir, PruneKey, PruneUse};
use cogitator_ir::mir::{ColId, Mir, ScalarId};
use cogitator_ir::types::SqlType;
use cogitator_ir::{Level, RuleId};

use super::predicate::{use_of, Use};
use super::range::range_rewrite;
use super::range_fix::range_fix;
use super::support::{
    col_name, plan_evidence, scalar_span, scan_conjuncts, scan_is_closed, scans, ScanView,
    BIGQUERY_CLUSTER_DOC, BIGQUERY_PARTITION_DOC, POSTGRES_EXPRESSION_DOC,
};
use crate::cx::Cx;
use crate::findings::{Evidence, Finding, Findings, Fix};
use crate::meta::{FixSafety, Need, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("filter.non_sargable"),
    level: RuleLevel::Lir,
    class: RuleClass::Advice,
    needs: Needs::NONE
        .with(Need::Dialect)
        .with(Need::Types)
        .with(Need::Phys),
    severity: Severity::Warning,
    fix: FixSafety::Preserving,
};

pub struct NonSargable;

impl Rule for NonSargable {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Lir> for NonSargable {
    fn check(&self, ir: &Lir, cx: &Cx<'_>, out: &mut Findings) {
        let Some(mir) = cx.mir else {
            return;
        };
        let conjuncts = scan_conjuncts(ir);
        for scan in scans(ir) {
            if !scan_is_closed(mir, scan.mir, scan.table) {
                continue;
            }
            let above = conjuncts.get(&scan.id).map_or(&[][..], Vec::as_slice);
            let check = Check {
                mir,
                backend: ir.backend,
                cx,
                scan: &scan,
            };
            for key in scan.keys {
                // A column that is also the partition key reports as that.
                let is_partition = |k: &PruneKey| k.kind == KeyKind::Partition && k.col == key.col;
                if key.kind == KeyKind::Cluster && scan.keys.iter().any(is_partition) {
                    continue;
                }
                let finding = match key.usage {
                    PruneUse::Blocked { by, reason } => check.blocked(key, by, reason),
                    PruneUse::Unknown => check.unknown(key, above),
                    PruneUse::Used(_) | PruneUse::NotFiltered => None,
                };
                if let Some(finding) = finding {
                    out.push(finding);
                }
            }
        }
    }
}

struct Check<'a> {
    mir: &'a Mir,
    backend: Backend,
    cx: &'a Cx<'a>,
    scan: &'a ScanView<'a>,
}

impl Check<'_> {
    fn blocked(&self, key: &PruneKey, by: ScalarId, reason: BlockReason) -> Option<Finding> {
        if matches!(
            reason,
            BlockReason::SubqueryValue | BlockReason::OrAcrossColumns
        ) {
            return None;
        }
        let used = use_of(self.mir, by, key.col)?;
        let name = col_name(self.mir, key.col);
        let form = describe(used)?;
        let (severity, stated) = match self.backend {
            Backend::BigQuery => (Severity::Warning, bigquery_blocked(key.kind)),
            Backend::Postgres => (Severity::Hint, postgres_blocked()),
            Backend::Snowflake | Backend::DuckDb | Backend::Generic => return None,
        };
        let message = format!(
            "{form} on {} column `{name}`: {stated}{}",
            kind_name(key.kind),
            self.rewrite_note(by, key.col, name),
        );
        let fixes = self.range_fixes(by, key.col, name);
        Some(self.finding(by, severity, message, plan_evidence(self.cx.hir), fixes))
    }

    fn unknown(&self, key: &PruneKey, above: &[ScalarId]) -> Option<Finding> {
        let (conjunct, used) = above
            .iter()
            .find_map(|c| use_of(self.mir, *c, key.col).map(|u| (*c, u)))?;
        if matches!(used, Use::Bare | Use::ComparedToColumn) {
            return None;
        }
        let name = col_name(self.mir, key.col);
        let message = format!(
            "{} on {} column `{name}` may reduce pruning.{}{}",
            describe(used)?,
            kind_name(key.kind),
            self.documents_forms(key.kind),
            self.rewrite_note(conjunct, key.col, name),
        );
        let fixes = self.range_fixes(conjunct, key.col, name);
        Some(self.finding(conjunct, Severity::Hint, message, Evidence::Unknown, fixes))
    }

    /// BigQuery's list of forms that can still prune is on the partition
    /// page only; no other vendor states one.
    fn documents_forms(&self, kind: KeyKind) -> String {
        if self.backend == Backend::BigQuery && kind == KeyKind::Partition {
            format!(
                " BigQuery can still prune some date and timestamp functions and casts \
                 with constant arguments (see {BIGQUERY_PARTITION_DOC})."
            )
        } else {
            String::new()
        }
    }

    fn rewrite_note(&self, conjunct: ScalarId, col: ColId, name: &str) -> String {
        if self.backend != Backend::BigQuery {
            return String::new();
        }
        let ty = self.column_type(col);
        range_rewrite(self.mir, conjunct, col, &ty, name)
            .map(|range| {
                format!(" Comparing the column directly selects the same rows: `{range}`.")
            })
            .unwrap_or_default()
    }

    /// The edit behind the message's range, when the document shows
    /// exactly the predicate the plan holds.
    fn range_fixes(&self, conjunct: ScalarId, col: ColId, name: &str) -> Vec<Fix> {
        if self.backend != Backend::BigQuery {
            return Vec::new();
        }
        let ty = self.column_type(col);
        range_fix(self.mir, self.cx, conjunct, col, &ty, name)
            .into_iter()
            .collect()
    }

    /// The plan's type, falling back to the physical column's when the
    /// plan has none.
    fn column_type(&self, col: ColId) -> SqlType {
        let planned = self
            .mir
            .cols
            .get(col)
            .map_or(SqlType::Unknown, |c| c.ty.ty.clone());
        if planned != SqlType::Unknown {
            return planned;
        }
        let name = self.mir.cols.get(col).and_then(|c| c.name);
        self.cx
            .phys
            .and_then(|p| p.table(self.scan.table))
            .and_then(|t| t.columns.iter().find(|c| Some(c.name) == name))
            .map_or(SqlType::Unknown, |c| c.ty.clone())
    }

    fn finding(
        &self,
        conjunct: ScalarId,
        severity: Severity,
        message: String,
        evidence: Evidence,
        fixes: Vec<Fix>,
    ) -> Finding {
        Finding {
            id: META.id,
            span: scalar_span(self.mir, conjunct),
            level: Level::Lir,
            severity,
            message,
            evidence,
            fixes,
        }
    }
}

fn postgres_blocked() -> String {
    format!(
        "PostgreSQL can only use an index here if it is defined on this expression \
         (see {POSTGRES_EXPRESSION_DOC})."
    )
}

fn bigquery_blocked(kind: KeyKind) -> String {
    let (stated, doc) = match kind {
        KeyKind::Partition => (
            "BigQuery documents that this form requires a full table scan instead of \
             partition pruning; isolate the column on one side of the comparison",
            BIGQUERY_PARTITION_DOC,
        ),
        _ => (
            "BigQuery documents that block pruning cannot be applied to this form; \
             compare the column directly",
            BIGQUERY_CLUSTER_DOC,
        ),
    };
    format!("{stated} (see {doc}).")
}

fn describe(used: Use<'_>) -> Option<String> {
    Some(match used {
        Use::Call(function) => format!("`{function}(...)`"),
        Use::Cast => "A cast".to_string(),
        Use::Arithmetic => "Arithmetic".to_string(),
        Use::ComparedToColumn => "A comparison with another column".to_string(),
        Use::Bare => return None,
    })
}

fn kind_name(kind: KeyKind) -> &'static str {
    match kind {
        KeyKind::Partition => "partition",
        KeyKind::Cluster => "clustering",
        KeyKind::Sort => "sort",
        KeyKind::Unique => "indexed",
    }
}

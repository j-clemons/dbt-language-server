//! `filter.cluster_order`: a filter that skips a leading clustering
//! column.
//!
//! BigQuery's page on clustered tables: "In most cases, the first
//! clustering column is the most effective in block pruning, then the
//! second column, then the third. You can still use the second or third
//! column alone in the query, but block pruning probably won't be as
//! effective." That is a soft claim, so the rule reports a Hint in the
//! same terms, on BigQuery only.
//!
//! The backend turns a `Used` cluster key after a broken chain into
//! `Unknown` (`finish_keys`), which is the case read here: a bare
//! comparison on a later key. A wrapped column belongs to
//! `filter.non_sargable`, so each conjunct is reported by one rule.

use cogitator_ir::lir::{Backend, KeyKind, Lir, PruneKey, PruneUse};
use cogitator_ir::mir::{ColId, Mir};
use cogitator_ir::{Level, RuleId};

use super::predicate::{use_of, Use};
use super::support::{
    col_name, scalar_span, scan_conjuncts, scan_is_closed, scans, ScanView, BIGQUERY_CLUSTER_DOC,
};
use crate::cx::Cx;
use crate::findings::{Evidence, Finding, Findings};
use crate::meta::{FixSafety, Need, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};

pub static META: RuleMeta = RuleMeta {
    id: RuleId("filter.cluster_order"),
    level: RuleLevel::Lir,
    class: RuleClass::Advice,
    needs: Needs::NONE.with(Need::Dialect).with(Need::Phys),
    severity: Severity::Info,
    fix: FixSafety::None,
};

pub struct ClusterOrder;

impl Rule for ClusterOrder {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Lir> for ClusterOrder {
    fn check(&self, ir: &Lir, cx: &Cx<'_>, out: &mut Findings) {
        let Some(mir) = cx.mir else {
            return;
        };
        if ir.backend != Backend::BigQuery {
            return;
        }
        let conjuncts = scan_conjuncts(ir);
        for scan in scans(ir) {
            if !scan_is_closed(mir, scan.mir, scan.table) {
                continue;
            }
            let above = conjuncts.get(&scan.id).map_or(&[][..], Vec::as_slice);
            for key in scan.keys {
                if key.kind != KeyKind::Cluster
                    || key.usage != PruneUse::Unknown
                    || !skips_cluster_key(mir, cx, &scan, key)
                {
                    continue;
                }
                // The same conjunct `filter.non_sargable` reads: a wrapped
                // one is reported there, not here.
                let Some(conjunct) = above
                    .iter()
                    .find_map(|c| use_of(mir, *c, key.col).map(|u| (*c, u)))
                    .and_then(|(c, used)| (used == Use::Bare).then_some(c))
                else {
                    continue;
                };
                let name = col_name(mir, key.col);
                out.push(Finding {
                    id: META.id,
                    span: scalar_span(mir, conjunct),
                    level: Level::Lir,
                    severity: Severity::Hint,
                    message: format!(
                        "`{name}` is a later clustering column and an earlier one has no \
                         usable filter; BigQuery documents that block pruning then \
                         probably won't be as effective. Filter the clustering columns in \
                         sort order, starting with the first. See {BIGQUERY_CLUSTER_DOC}"
                    ),
                    evidence: Evidence::Unknown,
                    fixes: Vec::new(),
                });
            }
        }
    }
}

/// True when a clustering column before `key` in `CLUSTER BY` order (the
/// `TableFacts` order) has no usable filter here.
fn skips_cluster_key(mir: &Mir, cx: &Cx<'_>, scan: &ScanView<'_>, key: &PruneKey) -> bool {
    let name_of = |col: ColId| mir.cols.get(col).and_then(|c| c.name);
    let Some(facts) = cx.phys.and_then(|p| p.table(scan.table)) else {
        return false;
    };
    let filtered = |name| {
        scan.keys.iter().any(|k| {
            k.kind == KeyKind::Cluster
                && matches!(k.usage, PruneUse::Used(_))
                && name_of(k.col) == Some(name)
        })
    };
    let target = name_of(key.col);
    facts
        .columns
        .iter()
        .filter(|c| c.key == Some(KeyKind::Cluster))
        .take_while(|c| Some(c.name) != target)
        .any(|c| !filtered(c.name))
}

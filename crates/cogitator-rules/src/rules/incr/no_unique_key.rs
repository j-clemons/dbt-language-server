//! `incr.no_unique_key`: a merge-style incremental strategy with no
//! `unique_key` can duplicate rows.
//!
//! dbt documents that "if you use `merge` without specifying a
//! `unique_key`, it behaves like the `append` strategy", and its
//! `delete+insert` macro generates no delete when no key is set. The rule
//! fires on either as an explicit strategy, and on no strategy where the
//! adapter documents merge as its default:
//!
//! | Adapter | Default | Doc | Fires by default? |
//! |---|---|---|---|
//! | BigQuery | `merge` | bigquery-configs | yes |
//! | Snowflake | `merge` | snowflake-configs | yes |
//! | Postgres | `append` without a `unique_key` | postgres-configs | no |
//! | DuckDB | not documented | duckdb-configs | no |
//! | others | unknown | none | no |
//!
//! A `generic` backend makes no claim at all, because adapter behaviour
//! is unknown. Neither does a model whose `config()` sets `unique_key` or
//! `incremental_strategy` to a value this pipeline cannot read (a
//! `var()`, a `**` expansion), since the value may exist. The manifest
//! fills what the template leaves out before this rule reads the config.

use cogitator_ir::common::Materialization;
use cogitator_ir::hir::{Hir, JinjaKind};
use cogitator_ir::lir::Backend;
use cogitator_ir::{Level, RuleId, Symbol};

use super::missing_guard::anchor;
use crate::cx::Cx;
use crate::findings::{Evidence, Finding, Findings};
use crate::meta::{FixSafety, Need, Needs, RuleClass, RuleLevel, RuleMeta, Severity};
use crate::rule::{Detect, Rule};
use crate::rules::filter::support::plan_evidence;
use crate::rules::join::text::slice;

const STRATEGY_DOC: &str = "https://docs.getdbt.com/docs/build/incremental-strategy";
const BIGQUERY_CONFIG_DOC: &str =
    "https://docs.getdbt.com/reference/resource-configs/bigquery-configs";
const SNOWFLAKE_CONFIG_DOC: &str =
    "https://docs.getdbt.com/reference/resource-configs/snowflake-configs";

pub static META: RuleMeta = RuleMeta {
    id: RuleId("incr.no_unique_key"),
    level: RuleLevel::Hir,
    class: RuleClass::Advice,
    needs: Needs::NONE.with(Need::Mat).with(Need::Dialect),
    severity: Severity::Warning,
    fix: FixSafety::None,
};

pub struct NoUniqueKey;

impl Rule for NoUniqueKey {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }
}

impl Detect<Hir> for NoUniqueKey {
    fn check(&self, ir: &Hir, cx: &Cx<'_>, out: &mut Findings) {
        if cx.mat != Some(Materialization::Incremental) || hides_a_key(ir, cx) {
            return;
        }
        let Some(backend) = cx.dialect.filter(|b| *b != Backend::Generic) else {
            return;
        };
        let (unique_key, strategy) = effective_config(ir, cx);
        if !unique_key.is_empty() {
            return;
        }
        let Some(claim) = claim_for(ir, backend, strategy) else {
            return;
        };
        out.push(Finding {
            id: META.id,
            span: anchor(ir),
            level: Level::Hir,
            severity: Severity::Warning,
            message: claim.message(),
            evidence: claim.evidence(plan_evidence(cx.hir)),
            fixes: Vec::new(),
        });
    }
}

/// `Hir.config`, falling back through what `Cx::config` merges.
fn effective_config<'a>(ir: &'a Hir, cx: &Cx<'a>) -> (&'a [Symbol], Option<Symbol>) {
    let unique_key = if ir.config.unique_key.is_empty() {
        cx.config.map_or(&[][..], |c| c.unique_key.as_slice())
    } else {
        ir.config.unique_key.as_slice()
    };
    let strategy = ir
        .config
        .incremental_strategy
        .or_else(|| cx.config.and_then(|c| c.incremental_strategy));
    (unique_key, strategy)
}

/// True when a `config()` in the template names one of the two keys with
/// a value the parser could not read, or expands `**kwargs`. Without the
/// text a `config()` call could say anything.
fn hides_a_key(ir: &Hir, cx: &Cx<'_>) -> bool {
    ir.jinja
        .values()
        .filter(|node| node.kind == JinjaKind::Config)
        .any(|node| {
            let Some(text) = cx.text.and_then(|t| slice(t, node.span)) else {
                return true;
            };
            text.contains("**")
                || (text.contains("unique_key") && ir.config.unique_key.is_empty())
                || (text.contains("incremental_strategy")
                    && ir.config.incremental_strategy.is_none())
        })
}

enum Claim {
    Merge,
    DeleteInsert,
    DefaultMerge {
        adapter: &'static str,
        doc: &'static str,
    },
}

fn claim_for(ir: &Hir, backend: Backend, strategy: Option<Symbol>) -> Option<Claim> {
    match strategy {
        Some(symbol) => {
            let name = ir.syms.resolve(symbol)?;
            if name.eq_ignore_ascii_case("merge") {
                Some(Claim::Merge)
            } else if name.eq_ignore_ascii_case("delete+insert") {
                Some(Claim::DeleteInsert)
            } else {
                None
            }
        }
        None => match backend {
            Backend::BigQuery => Some(Claim::DefaultMerge {
                adapter: "BigQuery",
                doc: BIGQUERY_CONFIG_DOC,
            }),
            Backend::Snowflake => Some(Claim::DefaultMerge {
                adapter: "Snowflake",
                doc: SNOWFLAKE_CONFIG_DOC,
            }),
            // Postgres documents `append` when no `unique_key` is set;
            // the DuckDB adapter documents no default.
            Backend::Postgres | Backend::DuckDb | Backend::Generic => None,
        },
    }
}

impl Claim {
    fn message(&self) -> String {
        match self {
            Claim::Merge => format!(
                "`incremental_strategy='merge'` with no `unique_key`: dbt documents that \
                 merge then behaves like `append`, so each re-processed row is inserted \
                 again. Set a `unique_key`, or use `append` if duplicates are acceptable. \
                 See {STRATEGY_DOC}"
            ),
            Claim::DeleteInsert => format!(
                "`incremental_strategy='delete+insert'` with no `unique_key`: delete+insert \
                 deletes target rows by `unique_key`, so without one nothing is deleted \
                 and re-processed rows can be inserted again. Set a `unique_key`. See \
                 {STRATEGY_DOC}"
            ),
            Claim::DefaultMerge { adapter, doc } => format!(
                "This incremental model sets no `unique_key`, and dbt's default strategy \
                 on {adapter} is `merge` (see {doc}), which without a `unique_key` \
                 behaves like `append`, so re-processed rows are inserted again (see \
                 {STRATEGY_DOC}). Set a `unique_key`, unless `dbt_project.yml` sets \
                 another strategy."
            ),
        }
    }

    fn evidence(&self, plan: Evidence) -> Evidence {
        match self {
            Claim::Merge | Claim::DeleteInsert => plan,
            // The default is an adapter fact the model does not state.
            Claim::DefaultMerge { .. } => Evidence::Heuristic,
        }
    }
}

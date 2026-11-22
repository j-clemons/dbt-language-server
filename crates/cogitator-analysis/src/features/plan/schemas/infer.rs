//! One model through HIR and MIR: its output columns and what it reads.

use std::collections::BTreeSet;
use std::sync::Arc;

use cogitator_dialects::Dialect;
use cogitator_ir::common::{Materialization, ModelConfig};
use cogitator_ir::hir::Hir;
use cogitator_ir::mir::Mir;
use cogitator_ir::origin::{SourceOrigin, TemplateVariant};
use cogitator_ir::{ColType, VerifyMode};
use cogitator_lower::lower_to_mir;
use cogitator_rules::passes::{run_passes, Budgets, PassInput};
use cogitator_rules::rules::cross::output::output_open;
use cogitator_rules::{Cx, Needs};
use cogitator_sql_front::{build_hir, fold_schema_name, HirInput, SchemaEnv};

use super::model_facts::{model_facts, ModelFacts};
use super::reads::{model_reads, ModelReads};
use crate::artifacts::manifest::ManifestIndex;

/// Whether a model's outputs are stored or computed for each consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Storage {
    Stored,
    /// Views and ephemerals: engines read through them.
    PassThrough,
}

pub(super) struct InferredCol {
    pub(super) name: Arc<str>,
    pub(super) ty: ColType,
}

/// Cached per model: nothing here depends on the manifest.
pub(super) struct Inferred {
    pub(super) columns: Vec<InferredCol>,
    /// The output list may be missing columns (see the module docs).
    pub(super) open: bool,
    pub(super) reads: ModelReads,
    /// From the template's own `config()`; the manifest fills the gap.
    pub(super) template_storage: Option<Storage>,
    /// Same, for whether the template declares itself ephemeral.
    pub(super) template_ephemeral: Option<bool>,
    pub(super) facts: ModelFacts,
}

pub(super) fn storage_of(raw: &str) -> Option<Storage> {
    match raw.to_ascii_lowercase().as_str() {
        "view" | "ephemeral" => Some(Storage::PassThrough),
        "table" | "incremental" | "materialized_view" | "snapshot" => Some(Storage::Stored),
        _ => None,
    }
}

pub(super) fn storage_for(
    model: &str,
    template: Option<Storage>,
    manifest: Option<&ManifestIndex>,
) -> Option<Storage> {
    template.or_else(|| {
        manifest?
            .models
            .get(model)?
            .config
            .materialized
            .as_deref()
            .and_then(storage_of)
    })
}

/// The plan rules compare: the same canonicalisers as a live plan. Their
/// detectors run too and their findings are discarded here.
fn canonicalise(hir: &Hir, mir: Mir) -> Mir {
    let mut cx = Cx::new(Needs::NONE);
    cx.hir = Some(hir);
    let input = PassInput {
        hir: Some(hir),
        mir,
        lir: None,
        lower_lir: None,
        verify: VerifyMode::Boundaries,
        budgets: Budgets::default(),
        snapshots: false,
    };
    run_passes(input, &cx).mir
}

/// `None` when the template cannot be planned or a verifier rejected it.
pub(super) fn infer(text: &str, dialect: &Dialect, env: &SchemaEnv) -> Option<Inferred> {
    let front = HirInput {
        text,
        dialect,
        origin: SourceOrigin::Template {
            variant: TemplateVariant::Plain,
        },
        config: ModelConfig::default(),
    };
    let lowered = build_hir(&front, env);
    if !lowered.violations.is_empty() {
        return None;
    }
    let hir = lowered.value?;
    let root = hir.root?;
    let mir_lowered = lower_to_mir(&hir);
    if !mir_lowered.violations.is_empty() {
        return None;
    }
    let mir = mir_lowered.value?;

    let mut open = output_open(&hir, root);
    let mut seen = BTreeSet::new();
    let mut columns = Vec::new();
    for col in mir.root_columns() {
        let Some(def) = mir.cols.get(*col) else {
            open = true;
            continue;
        };
        let Some(name) = def.name else {
            open = true;
            continue;
        };
        let text = mir.syms.text(name);
        if fold_schema_name(text, dialect) != text {
            // A quoted identifier: the binder would re-fold it on the way
            // back in, so downstream could not address it by this name.
            open = true;
        }
        if seen.insert(name) {
            columns.push(InferredCol {
                name: Arc::from(text),
                ty: def.ty.clone(), // boundary: the schema owns its types
            });
        }
    }
    let mut reads = model_reads(&mir);
    reads.unknown |= output_open(&hir, root);
    let facts = model_facts(&hir, &canonicalise(&hir, mir));
    Some(Inferred {
        columns,
        open,
        reads,
        facts,
        template_ephemeral: hir
            .config
            .materialized
            .as_ref()
            .map(|m| matches!(m, Materialization::Ephemeral)),
        template_storage: hir.config.materialized.as_ref().and_then(|m| match m {
            Materialization::View | Materialization::Ephemeral => Some(Storage::PassThrough),
            Materialization::Table
            | Materialization::Incremental
            | Materialization::MaterializedView
            | Materialization::Snapshot => Some(Storage::Stored),
            Materialization::Other(_) => None,
        }),
    })
}

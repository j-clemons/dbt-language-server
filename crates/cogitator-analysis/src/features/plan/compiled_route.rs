//! The compiled fallback for a template the HIR cannot express: pick the
//! compiled text the shell supplied, align it, and build its HIR. Pure:
//! the shell reads the file and its mtime (`CompiledInput`).

use cogitator_error::guidance::Guidance;
use cogitator_ir::hir::{Body, Hir};
use cogitator_ir::origin::{SourceOrigin, Staleness};
use cogitator_ir::{DegradationKind, Fidelity, Lowered};
use cogitator_sql_front::compiled::{
    align, build_compiled_hir, prepare, RelationTable, MIN_LITERAL_COVERAGE,
};
use cogitator_sql_front::HirInput;

use super::drive::{self, two_pass, BuiltHir};
use super::{CompiledInput, PlanInput};

/// A template plan needs the fallback only when it declined on control flow.
pub(super) fn declined_on_control_flow(lowered: &Lowered<Hir>) -> bool {
    lowered.value.is_none()
        && lowered.fidelity == Fidelity::Declined
        && lowered
            .degraded
            .iter()
            .any(|d| d.kind == DegradationKind::JinjaControlFlow)
}

/// A plan built from compiled SQL. `input.text` is the compiled text,
/// which every later stage must see: the spans index it.
pub(super) struct Route<'a> {
    pub(super) input: PlanInput<'a>,
    pub(super) built: BuiltHir,
    pub(super) banner: Option<String>,
}

/// Why the fallback produced no plan.
pub(super) enum Failure<'a> {
    NoFile {
        path: Option<&'a str>,
    },
    TooLarge {
        path: &'a str,
        bytes: u64,
        cap: u64,
    },
    Unusable {
        path: Option<&'a str>,
        detail: String,
    },
}

pub(super) fn route<'a>(input: &PlanInput<'a>) -> Result<Route<'a>, Failure<'a>> {
    let (text, path, mtime) = match input.compiled {
        CompiledInput::NotChecked => return Err(Failure::NoFile { path: None }),
        CompiledInput::Missing { path } => return Err(Failure::NoFile { path: Some(path) }),
        CompiledInput::TooLarge { path, bytes, cap } => {
            return Err(Failure::TooLarge { path, bytes, cap });
        }
        CompiledInput::Unreadable { path, detail } => {
            return Err(Failure::Unusable {
                path: Some(path),
                detail: detail.to_string(),
            });
        }
        CompiledInput::Available { text, path, mtime } => (text, Some(path), mtime),
    };
    let unusable = |detail: String| Failure::Unusable { path, detail };
    let relations = relation_table(input);
    let prepared = prepare(text, input.dialect, &relations, input.model).map_err(unusable)?;
    let alignment = align(input.text, &prepared, text);
    let (staleness, banner) = verdict(mtime, alignment.literal_coverage);
    let origin = SourceOrigin::Compiled {
        staleness,
        map: alignment.map,
    };
    let compiled_input = PlanInput { text, ..*input };
    let built = two_pass(&compiled_input, |env| {
        let front = HirInput {
            text,
            dialect: input.dialect,
            // boundary: each lowering owns its origin (08 §1.5)
            origin: origin.clone(),
            config: Default::default(),
        };
        build_compiled_hir(&front, env, &prepared)
    });
    if is_whole_hole(&built.hir) {
        return Err(unusable(
            "the parser could not read the compiled SQL".to_string(),
        ));
    }
    Ok(Route {
        input: compiled_input,
        built,
        banner,
    })
}

/// Coverage is checked on top of the shell's mtime comparison, because
/// unsaved edits never change a file's mtime.
fn verdict(mtime: Staleness, literal_coverage: u16) -> (Staleness, Option<String>) {
    if mtime == Staleness::Stale {
        return (
            Staleness::Stale,
            Some(
                "Stale compiled SQL: the compiled file is older than the model. \
                 Run `dbt compile` to refresh this plan."
                    .to_string(),
            ),
        );
    }
    if literal_coverage < MIN_LITERAL_COVERAGE {
        let found = f64::from(literal_coverage) / 100.0;
        return (
            Staleness::Stale,
            Some(format!(
                "Stale compiled SQL: only {found:.1}% of the model's SQL appears in \
                 the compiled file, so it has changed since it was compiled (unsaved \
                 edits count). Run `dbt compile` after saving to refresh this plan."
            )),
        );
    }
    match mtime {
        Staleness::Unknown => (
            Staleness::Unknown,
            Some(
                "Compiled SQL of unknown age: the file times could not be compared. \
                 Run `dbt compile` if this plan looks out of date."
                    .to_string(),
            ),
        ),
        _ => (Staleness::Fresh, None),
    }
}

/// True when the compiled SQL parsed to nothing the plan can use.
fn is_whole_hole(lowered: &Lowered<Hir>) -> bool {
    let root = lowered
        .value
        .as_ref()
        .and_then(|hir| hir.queries.get(hir.root?));
    root.is_some_and(|query| matches!(query.body, Body::Hole(_)) && query.ctes.is_empty())
}

/// Names sorted before insertion, so a name collision resolves the same
/// way each time regardless of `HashMap` order.
fn relation_table(input: &PlanInput<'_>) -> RelationTable {
    let mut table = RelationTable::new(input.dialect);
    let manifest = input.manifest.map(|m| &m.models);
    let mut names: Vec<&str> = input
        .models
        .keys()
        .chain(manifest.into_iter().flat_map(|m| m.keys()))
        .map(String::as_str)
        .collect();
    names.sort_unstable();
    names.dedup();
    for name in names {
        let relation = manifest
            .and_then(|m| m.get(name))
            .and_then(|node| node.relation_name.as_deref());
        table.add_model(name, relation);
    }
    let mut sources: Vec<(&str, &str)> = input
        .sources
        .iter()
        .flat_map(|(source, details)| {
            details
                .tables
                .keys()
                .map(move |table| (source.as_str(), table.as_str()))
        })
        .collect();
    sources.sort_unstable();
    for (source, name) in sources {
        let details = input.sources.get(source);
        let identifier = details
            .and_then(|d| d.tables.get(name))
            .and_then(|t| t.identifier.as_deref());
        let schema = details.and_then(|d| d.schema.as_deref());
        table.add_source(source, name, schema, identifier);
    }
    table
}

/// A verifier violation is a bug report whatever the fallback did; every
/// other decline says what the fallback lacked.
pub(super) fn declined_guidance(
    input: &PlanInput<'_>,
    lowered: &Lowered<Hir>,
    failure: Option<Failure<'_>>,
) -> Guidance {
    match failure {
        Some(failure) if lowered.violations.is_empty() => {
            failure_guidance(input, failure, drive::decline_reason(lowered))
        }
        _ => drive::guidance(input, lowered),
    }
}

/// The `Declined` guidance when the template needs the fallback and the
/// fallback has nothing to offer.
pub(super) fn failure_guidance(
    input: &PlanInput<'_>,
    failure: Failure<'_>,
    reason: &str,
) -> Guidance {
    let model = input.model.unwrap_or("this model");
    let title = format!("No query plan for {model}.");
    match failure {
        Failure::NoFile { path } => {
            let guidance = Guidance::new(title)
                .because(format!(
                    "{reason}, and no compiled file exists for it; the template alone cannot be planned."
                ));
            let guidance = match path {
                Some(path) => guidance.at(path),
                None => guidance,
            };
            guidance.fix("run `dbt compile` so the model can be planned from its compiled SQL.")
        }
        Failure::TooLarge { path, bytes, cap } => Guidance::new(title)
            .at(path)
            .because(format!(
                "{reason}, and the compiled file is {bytes} bytes, over the {cap}-byte limit."
            ))
            .fix("raise `limits.compiled_sql_mib` in cogitator.yml, then run `dbt compile` again."),
        Failure::Unusable { path, detail } => {
            let guidance = Guidance::new(title).because(format!(
                "{reason}, and the compiled file could not be used: {detail}."
            ));
            let guidance = match path {
                Some(path) => guidance.at(path),
                None => guidance,
            };
            guidance.fix(
                "run `dbt compile` again; if the error stays, report it with the compiled SQL.",
            )
        }
    }
}

//! Groups the project's canonical subplans for `cross.duplicate_logic`.
//!
//! The hash only narrows candidates: entries sharing a hash are split by
//! text, so a collision cannot make two different subplans look shared.
//! Whole-project text is bounded by `max_subplan_bytes`; past it, later
//! models are left out and the first of them is reported.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use cogitator_rules::rules::cross::subplan::Subplan;
use cogitator_rules::SharedSubplan;

use super::infer::Inferred;

/// A group this size still only names the first few of its members.
const MAX_NAMED: usize = 16;

#[derive(Debug, Default)]
pub(super) struct SharedIndex {
    pub(super) by_model: BTreeMap<Arc<str>, BTreeMap<u64, SharedSubplan>>,
    /// The first model the byte budget left out.
    pub(super) over_budget: Option<Arc<str>>,
    /// Models a per-model count bound left partly or wholly uncompared.
    pub(super) incomplete: Vec<Arc<str>>,
}

/// Groups every inferred model's candidates, in `order`.
pub(super) fn for_job(
    order: &[&str],
    inferred: &HashMap<Arc<str>, Option<Arc<Inferred>>>,
    max_bytes: u64,
) -> SharedIndex {
    let models = || {
        order.iter().filter_map(|name| {
            let result = inferred.get(*name)?.as_ref()?;
            Some((Arc::<str>::from(*name), &result.facts))
        })
    };
    let mut out = group(
        models().map(|(name, facts)| (name, facts.subplans.as_slice())),
        max_bytes,
    );
    out.incomplete = models()
        .filter(|(_, facts)| !facts.subplans_complete)
        .map(|(name, _)| name)
        .collect();
    out
}

/// `models` in a fixed (upstream-first) order; the result does not depend
/// on it except for which models the budget would drop.
pub(super) fn group<'a>(
    models: impl Iterator<Item = (Arc<str>, &'a [Subplan])>,
    max_bytes: u64,
) -> SharedIndex {
    let mut out = SharedIndex::default();
    let mut bytes = 0u64;
    let mut buckets: BTreeMap<u64, Vec<(Arc<str>, &'a Subplan)>> = BTreeMap::new();
    for (model, subplans) in models {
        let cost: u64 = subplans.iter().map(|s| s.text.len() as u64).sum();
        if bytes + cost > max_bytes {
            out.over_budget.get_or_insert(model);
            continue;
        }
        bytes += cost;
        for subplan in subplans {
            buckets
                .entry(subplan.hash)
                .or_default()
                .push((model.clone(), subplan));
        }
    }
    for mut bucket in buckets.into_values() {
        // equal text, then model: a run of one text is a group
        bucket.sort_by(|a, b| a.1.text.cmp(&b.1.text).then_with(|| a.0.cmp(&b.0)));
        for run in bucket.chunk_by(|a, b| a.1.text == b.1.text) {
            record(run, &mut out);
        }
    }
    out
}

fn record(run: &[(Arc<str>, &Subplan)], out: &mut SharedIndex) {
    let Some((_, sample)) = run.first() else {
        return;
    };
    let mut models: Vec<&Arc<str>> = run.iter().map(|(m, _)| m).collect();
    models.dedup();
    if models.len() < 2 {
        return;
    }
    for model in &models {
        let others: Vec<Arc<str>> = models
            .iter()
            .filter(|m| **m != *model)
            .take(MAX_NAMED)
            .map(|m| (**m).clone())
            .collect();
        out.by_model.entry((*model).clone()).or_default().insert(
            sample.hash,
            SharedSubplan {
                text: sample.text.clone(),
                size: sample.size,
                models: others,
                total: models.len() - 1,
            },
        );
    }
}

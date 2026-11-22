//! Downstream column demand, folded consumers-first.
//!
//! Stored models read what they read: the build computes every output.
//! Views and ephemerals are looked through, because an engine prunes
//! across them: such a model only demands what its own consumers demand
//! of it. A view with no consumer in the project is a leaf someone reads
//! from outside, so it demands everything. Anything that cannot be
//! analysed makes every upstream's demand unknown, and unknown demand
//! withholds `Need::Dag`, which keeps `cross.*` silent.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use cogitator_rules::{ConsumerFacts, DagFacts, SharedSubplan};

use super::cross_sites::{inline_sites, record_consumer};
use super::infer::{storage_for, Inferred, Storage};
use super::shared::SharedIndex;
use super::JobInput;
use crate::context::types::ModelDetails;
use crate::graph::NodeKind;

type Columns = BTreeSet<Arc<str>>;

#[derive(Debug, Default, Clone)]
pub(super) struct ModelDemand {
    pub(super) by_downstream: BTreeMap<Arc<str>, Columns>,
    tested: Columns,
    unknown: bool,
    pub(super) consumers: BTreeMap<Arc<str>, ConsumerFacts>,
}

/// Lookup-only: rules get one model's [`DagFacts`] at a time.
#[derive(Debug, Default)]
pub struct DagIndex {
    by_model: HashMap<Arc<str>, ModelDemand>,
    catalog_backed: HashSet<Arc<str>>,
    generated_at: Option<Arc<str>>,
    /// Non-ephemeral compile sites of each ephemeral model.
    inlined: BTreeMap<Arc<str>, Vec<Arc<str>>>,
    shared: BTreeMap<Arc<str>, BTreeMap<u64, SharedSubplan>>,
}

impl DagIndex {
    /// The demand on `model`; `None` when it is not known.
    pub(in crate::features::plan) fn facts(&self, model: &str) -> Option<DagFacts> {
        let demand = self.by_model.get(model)?;
        if demand.unknown {
            return None;
        }
        Some(DagFacts {
            downstream_columns: demand
                .by_downstream
                .iter()
                .map(|(name, cols)| (name.clone(), cols.iter().cloned().collect()))
                .collect(),
            tested_columns: demand.tested.iter().cloned().collect(),
            catalog_generated_at: self
                .catalog_backed
                .contains(model)
                .then(|| self.generated_at.clone())
                .flatten(),
            consumers: demand.consumers.clone(),
            inlined_into: self.inlined.get(model).cloned().unwrap_or_default(),
            shared_subplans: self.shared.get(model).cloned().unwrap_or_default(),
        })
    }

    /// Feeds the revision: sorted, so independent of map order.
    pub(super) fn digest(&self, hasher: &mut impl Hasher) {
        let mut models: Vec<_> = self.by_model.iter().collect();
        models.sort_unstable_by(|a, b| a.0.cmp(b.0));
        for (name, demand) in models {
            name.hash(hasher);
            demand.unknown.hash(hasher);
            demand.tested.hash(hasher);
            for (down, cols) in &demand.by_downstream {
                down.hash(hasher);
                cols.hash(hasher);
            }
            for (consumer, facts) in &demand.consumers {
                consumer.hash(hasher);
                for filter in facts.filters.iter() {
                    filter.column.hash(hasher);
                    filter.predicate.hash(hasher);
                }
            }
        }
        self.inlined.hash(hasher);
        for (model, shared) in &self.shared {
            model.hash(hasher);
            for (hash, subplan) in shared {
                hash.hash(hasher);
                subplan.text.hash(hasher);
                subplan.models.hash(hasher);
                subplan.total.hash(hasher);
            }
        }
    }

    #[cfg(test)]
    pub(super) fn is_unknown(&self, model: &str) -> bool {
        self.by_model.get(model).is_none_or(|d| d.unknown)
    }
}

/// `order` is upstream-first; demand flows the other way.
pub(super) fn fold(
    order: &[&str],
    input: &JobInput<'_>,
    inferred: &HashMap<Arc<str>, Option<Arc<Inferred>>>,
    catalog_backed: HashSet<Arc<str>>,
    generated_at: Arc<str>,
    shared: SharedIndex,
) -> DagIndex {
    let mut by_model: HashMap<Arc<str>, ModelDemand> = order
        .iter()
        .map(|name| (Arc::<str>::from(*name), ModelDemand::default()))
        .collect();
    for &name in order.iter().rev() {
        let own = by_model.remove(name).unwrap_or_default();
        let (own, needed) = settle(name, own, input, inferred);
        let upstream: Vec<&str> = input
            .graph
            .model_neighbors(name)
            .0
            .iter()
            .filter(|n| n.kind == NodeKind::Model)
            .map(|n| n.name.as_str())
            .collect();
        let found = inferred.get(name).and_then(|i| i.as_deref());
        match found.filter(|i| !i.reads.unknown) {
            Some(inferred) => {
                apply_reads(inferred, needed.as_ref(), name, &mut by_model);
                record_consumer(inferred, name, &upstream, &mut by_model);
            }
            None => mark_unknown(&upstream, &mut by_model),
        }
        by_model.insert(Arc::from(name), own);
    }
    let inlined = inline_sites(order, input, inferred, &by_model);
    DagIndex {
        by_model,
        catalog_backed,
        generated_at: Some(generated_at),
        inlined,
        shared: shared.by_model,
    }
}

/// Finishes one model's own demand, and returns which of its outputs it
/// passes demand on for: `None` means all of them.
fn settle(
    name: &str,
    mut own: ModelDemand,
    input: &JobInput<'_>,
    inferred: &HashMap<Arc<str>, Option<Arc<Inferred>>>,
) -> (ModelDemand, Option<Columns>) {
    let details = input.models.get(name);
    own.tested = tested_columns(details);
    // A declared exposure, test or analysis reads an unknown column set.
    let (_, downstream) = input.graph.model_neighbors(name);
    own.unknown |= downstream.iter().any(|n| n.kind != NodeKind::Model);
    let template = inferred
        .get(name)
        .and_then(|i| i.as_deref())
        .and_then(|i| i.template_storage);
    let storage = storage_for(name, template, input.manifest);
    let needed =
        if storage == Some(Storage::PassThrough) && !own.unknown && !own.by_downstream.is_empty() {
            let mut needed: Columns = own.tested.clone();
            for cols in own.by_downstream.values() {
                needed.extend(cols.iter().cloned());
            }
            Some(needed)
        } else {
            None
        };
    (own, needed)
}

fn tested_columns(details: Option<&ModelDetails>) -> Columns {
    details
        .map(|d| {
            d.columns
                .iter()
                .filter(|c| !c.tests.is_empty())
                .map(|c| Arc::from(c.name.as_str()))
                .collect()
        })
        .unwrap_or_default()
}

/// Records what `consumer` reads of each upstream, for the outputs asked.
fn apply_reads(
    inferred: &Inferred,
    needed: Option<&Columns>,
    consumer: &str,
    by_model: &mut HashMap<Arc<str>, ModelDemand>,
) {
    let reads = &inferred.reads;
    let mut wanted: BTreeSet<u32> = reads.always.clone();
    for (output, deps) in &reads.outputs {
        if needed.is_none_or(|n| n.contains(output)) {
            wanted.extend(deps.iter().copied());
        }
    }
    let consumer: Arc<str> = Arc::from(consumer);
    for id in wanted {
        let Some((upstream, column)) = reads.cols.get(id as usize) else {
            continue;
        };
        by_model
            .entry(upstream.clone())
            .or_default()
            .by_downstream
            .entry(consumer.clone())
            .or_default()
            .insert(column.clone());
    }
}

fn mark_unknown(upstream: &[&str], by_model: &mut HashMap<Arc<str>, ModelDemand>) {
    for name in upstream {
        by_model.entry(Arc::from(*name)).or_default().unknown = true;
    }
}

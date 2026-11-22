//! Consumer lists and ephemeral compile sites for the `cross.*` rules.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use cogitator_rules::ConsumerFacts;

use super::dag::ModelDemand;
use super::infer::Inferred;
use super::JobInput;

/// Bound on the walk through chains of ephemerals.
const MAX_INLINE_VISITS: usize = 4_096;

/// Adds `consumer` to each upstream's consumers, with what it filters.
pub(super) fn record_consumer(
    inferred: &Inferred,
    consumer: &str,
    upstream: &[&str],
    by_model: &mut HashMap<Arc<str>, ModelDemand>,
) {
    let name: Arc<str> = Arc::from(consumer);
    for up in upstream {
        let facts = ConsumerFacts {
            filters: inferred
                .facts
                .filters
                .get(*up)
                .cloned()
                .unwrap_or_else(|| Arc::from(Vec::new())),
        };
        by_model
            .entry(Arc::from(*up))
            .or_default()
            .consumers
            .insert(name.clone(), facts);
    }
}

/// For each ephemeral model, the non-ephemeral models it is compiled
/// into. A model with no known materialisation counts as non-ephemeral:
/// `dbt_project.yml` could make it ephemeral, but it is a compile site
/// either way more often than not, and the rule is only a hint.
pub(super) fn inline_sites(
    order: &[&str],
    input: &JobInput<'_>,
    inferred: &HashMap<Arc<str>, Option<Arc<Inferred>>>,
    by_model: &HashMap<Arc<str>, ModelDemand>,
) -> BTreeMap<Arc<str>, Vec<Arc<str>>> {
    let ephemeral = |name: &str| -> bool {
        let template = inferred
            .get(name)
            .and_then(|i| i.as_deref())
            .and_then(|i| i.template_ephemeral);
        template.unwrap_or_else(|| {
            input
                .manifest
                .and_then(|m| m.models.get(name))
                .and_then(|m| m.config.materialized.as_deref())
                .is_some_and(|raw| raw.eq_ignore_ascii_case("ephemeral"))
        })
    };
    let mut out = BTreeMap::new();
    for name in order.iter().filter(|name| ephemeral(name)) {
        let mut sites = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut stack: Vec<&str> = vec![name];
        let mut visits = 0;
        while let Some(at) = stack.pop() {
            visits += 1;
            if visits > MAX_INLINE_VISITS {
                break;
            }
            let Some(demand) = by_model.get(at) else {
                continue;
            };
            for consumer in demand.consumers.keys() {
                if !seen.insert(consumer.clone()) {
                    continue;
                }
                if ephemeral(consumer) {
                    stack.push(consumer);
                } else {
                    sites.insert(consumer.clone());
                }
            }
        }
        if !sites.is_empty() {
            out.insert(Arc::from(*name), sites.into_iter().collect());
        }
    }
    out
}

//! The `dbt.queryPlan` surface of the derived cache: typed plans (with
//! their serialized form) keyed by (uri, document generation, schema
//! revision). The push diagnostics path only ever reads this cache.

use std::sync::Arc;

use cogitator_analysis::features::plan::suppress::Suppressions;
use cogitator_analysis::features::plan::QueryPlan;

use super::DerivedCache;

/// One computed plan: the typed value diagnostics read, the suppressions
/// marked on it, and the JSON `dbt.queryPlan` answers with.
#[derive(Debug)]
pub struct CachedPlan {
    pub plan: QueryPlan,
    pub suppressions: Suppressions,
    pub json: Arc<str>,
}

impl DerivedCache {
    /// The cached plan for `uri`, computed on a miss. One entry per uri:
    /// an edit or a new schema revision replaces it in place.
    pub fn query_plan(
        &self,
        uri: &str,
        generation: u64,
        revision: u64,
        compute: impl FnOnce() -> Option<CachedPlan>,
    ) -> Option<Arc<CachedPlan>> {
        let key = (generation, revision);
        if let Some(plan) = self.cached_plan(uri, key) {
            self.plans_stats.hit("plans");
            return Some(plan);
        }
        self.plans_stats.miss("plans");
        let plan = Arc::new(compute()?);
        self.plans
            .lock()
            .expect("plans cache lock")
            .insert(uri.to_string(), (key, plan.clone()));
        Some(plan)
    }

    /// The cached plan for `uri` at exactly this key, never computing.
    /// The push path's only access: it must not plan on the dispatcher,
    /// and a probe is not a cache hit or miss in the counters.
    pub fn plan_cached(
        &self,
        uri: &str,
        generation: u64,
        revision: u64,
    ) -> Option<Arc<CachedPlan>> {
        self.cached_plan(uri, (generation, revision))
    }

    fn cached_plan(&self, uri: &str, key: (u64, u64)) -> Option<Arc<CachedPlan>> {
        let map = self.plans.lock().expect("plans cache lock");
        let (cached, plan) = map.get(uri)?;
        (*cached == key).then(|| plan.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cogitator_analysis::artifacts::ArtifactSet;
    use cogitator_analysis::features::plan::{query_plan, PlanInput};
    use cogitator_analysis::DbtContext;

    fn cached(body: &str) -> CachedPlan {
        let ctx = DbtContext::default();
        let artifacts = ArtifactSet::default();
        let input = PlanInput::from_context(&ctx, "select 1", Some("m"), &artifacts, 0);
        CachedPlan {
            plan: query_plan(&input),
            suppressions: Suppressions::default(),
            json: Arc::from(body),
        }
    }

    fn plan(cache: &DerivedCache, generation: u64, revision: u64, body: &str) -> Arc<CachedPlan> {
        cache
            .query_plan("file:///m.sql", generation, revision, || Some(cached(body)))
            .expect("plan")
    }

    #[test]
    fn hits_until_the_generation_or_revision_moves() {
        let cache = DerivedCache::default();
        let first = plan(&cache, 1, 9, "{\"a\":1}");
        assert!(Arc::ptr_eq(&first, &plan(&cache, 1, 9, "{\"a\":2}")));
        assert_eq!(&*plan(&cache, 2, 9, "{\"a\":2}").json, "{\"a\":2}");
        assert_eq!(&*plan(&cache, 2, 10, "{\"a\":3}").json, "{\"a\":3}");
        let report = cache.report();
        assert_eq!(report.plans.entries, 1, "one entry per uri");
        assert_eq!((report.plans.hits, report.plans.misses), (1, 3));
    }

    #[test]
    fn a_failed_compute_is_not_cached() {
        let cache = DerivedCache::default();
        assert!(cache.query_plan("u", 1, 1, || None).is_none());
        assert_eq!(cache.report().plans.entries, 0);
    }

    #[test]
    fn plan_cached_never_computes_and_misses_on_a_new_key() {
        let cache = DerivedCache::default();
        assert!(cache.plan_cached("file:///m.sql", 1, 9).is_none());
        let stored = plan(&cache, 1, 9, "{}");
        let probed = cache.plan_cached("file:///m.sql", 1, 9).expect("stored");
        assert!(Arc::ptr_eq(&stored, &probed));
        // a newer generation or revision is a different plan: no answer
        assert!(cache.plan_cached("file:///m.sql", 2, 9).is_none());
        assert!(cache.plan_cached("file:///m.sql", 1, 10).is_none());
        // a probe counts neither as a hit nor a miss
        let report = cache.report();
        assert_eq!((report.plans.hits, report.plans.misses), (0, 1));
    }
}

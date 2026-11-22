//! `dbt.queryPlan`: snapshot here on the dispatcher task, plan off it.

use std::sync::atomic::Ordering;

use super::Dispatcher;

impl Dispatcher {
    pub(super) fn query_plan(&mut self, id: i64, uri: String) {
        let snap = self.snapshot_for(&uri);
        let derived = self.derived.clone();
        let cap = self.limits.artifact_cap_bytes.load(Ordering::Relaxed);
        let catalog_stats = self.perf_settings().catalog_stats;
        self.offload_blocking(id, "dbt.queryPlan", move || {
            crate::server::handlers::plan::query_plan(&snap, &derived, &uri, cap, catalog_stats)
        });
    }
}

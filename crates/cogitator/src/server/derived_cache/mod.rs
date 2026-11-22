//! In-memory derived-data caches for work the request path repeats:
//! profiles.yml parsing (done in 4+ places per request today), column
//! lineage per document, and the serialized dbt.modelInfo/modelTests
//! results (whose inputs are the context snapshot plus schema/compiled
//! file stamps). Everything is Sync — the offload architecture's tasks
//! consult it concurrently. THESE derived caches stay memory-only by
//! design; the SCAN cache now persists separately (03 §10,
//! server/cache_persist.rs) — that supersedes the old blanket
//! no-persistence note.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cogitator_analysis::artifacts::ArtifactSet;
use cogitator_analysis::lineage::ModelLineage;
use cogitator_analysis::profiles::{load_inventory, ProfileScope, ProfilesInventory};
use serde::Serialize;

mod artifacts;
mod lineage;
mod plan;
mod schemas;

pub use plan::CachedPlan;

/// (mtime secs, mtime nanos, size) — the same freshness idea as the scan
/// cache; a file that changed under us gets a different stamp.
pub(crate) type FileStamp = (i64, u32, u64);

pub fn stamp(path: &Path) -> Option<FileStamp> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let dur = modified
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    Some((dur.as_secs() as i64, dur.subsec_nanos(), meta.len()))
}

#[derive(Default)]
struct Counters {
    hits: AtomicU64,
    misses: AtomicU64,
}

impl Counters {
    fn hit(&self, surface: &'static str) {
        tracing::trace!(surface, "derived cache hit");
        crate::metrics::record_derived_cache(surface, true);
        self.hits.fetch_add(1, Ordering::Relaxed);
    }
    fn miss(&self, surface: &'static str) {
        tracing::trace!(surface, "derived cache miss");
        crate::metrics::record_derived_cache(surface, false);
        self.misses.fetch_add(1, Ordering::Relaxed);
    }
}

/// One cache surface in the dbt.cache / dbt.status report.
#[derive(Debug, Serialize)]
pub struct CacheSurface {
    pub entries: usize,
    pub hits: u64,
    pub misses: u64,
}

/// The dbt.cache report (also dbt.status's `caches` section).
#[derive(Debug, Serialize)]
pub struct CacheReport {
    pub profiles: CacheSurface,
    pub lineage: CacheSurface,
    pub results: CacheSurface,
    pub artifacts: CacheSurface,
    pub plans: CacheSurface,
}

/// (path, scope fingerprint) → (stamp, parsed inventory).
type ProfilesMap = HashMap<(PathBuf, String), (FileStamp, Arc<ProfilesInventory>)>;
/// uri → (document generation, lineage).
type LineageMap = HashMap<String, (u64, Arc<Option<ModelLineage>>)>;
/// (context snapshot pointer, key → (input fingerprint, serialized result)).
type ResultsSlot = (usize, HashMap<String, (String, Arc<str>)>);
/// root → ([run_results, manifest, sources] stamps, the parsed set).
type ArtifactsMap = HashMap<PathBuf, ([Option<FileStamp>; 4], ArtifactSet)>;
/// uri → ((document generation, plan key), typed lineage).
type TypedLineageMap = HashMap<String, ((u64, u64), Arc<Option<ModelLineage>>)>;
/// uri → ((document generation, schema revision), typed plan).
type PlansMap = HashMap<String, ((u64, u64), Arc<CachedPlan>)>;

#[derive(Default)]
pub struct DerivedCache {
    /// profiles.yml inventories keyed by (path, scope fingerprint).
    profiles: Mutex<ProfilesMap>,
    profiles_stats: Counters,
    /// Column lineage per uri; only the latest document generation lives.
    lineage: Mutex<LineageMap>,
    lineage_stats: Counters,
    /// Typed column lineage; only the latest key per uri lives.
    typed_lineage: Mutex<TypedLineageMap>,
    /// Serialized command results scoped to ONE context snapshot (the
    /// pointer identity of the Arc): a refresh wipes the slot wholesale.
    results: Mutex<ResultsSlot>,
    results_stats: Counters,
    /// target/ artifact payloads per root, stamp-guarded like profiles.
    artifacts: Mutex<ArtifactsMap>,
    artifacts_stats: Counters,
    /// `dbt.queryPlan` results; only the latest revision per uri lives.
    plans: Mutex<PlansMap>,
    plans_stats: Counters,
    /// The project-schema job's snapshot and state, per project root.
    schemas: schemas::SchemaSlot,
}

impl DerivedCache {
    /// The scoped inventory for `path`, re-parsed only when the file
    /// stamp or the (project, allowlist) scope changed.
    pub fn profiles_inventory(
        &self,
        path: &Path,
        project: &str,
        allow: &[String],
    ) -> Arc<ProfilesInventory> {
        let key = (
            path.to_path_buf(),
            format!("{project}\u{1f}{}", allow.join("\u{1f}")),
        );
        let current = stamp(path);
        if let Some(current) = current {
            let map = self.profiles.lock().expect("profiles cache lock");
            if let Some((cached, inventory)) = map.get(&key) {
                if *cached == current {
                    self.profiles_stats.hit("profiles");
                    return inventory.clone();
                }
            }
        }
        self.profiles_stats.miss("profiles");
        let inventory = Arc::new(load_inventory(
            path,
            &ProfileScope::Allowed {
                project,
                extra: allow,
            },
        ));
        if let Some(current) = current {
            self.profiles
                .lock()
                .expect("profiles cache lock")
                .insert(key, (current, inventory.clone()));
        }
        inventory
    }

    /// Column lineage for an OPEN document, keyed by its generation —
    /// an edit bumps the generation, so stale entries are unreachable
    /// and replaced in place (one entry per uri).
    pub fn lineage(
        &self,
        uri: &str,
        generation: u64,
        compute: impl FnOnce() -> Option<ModelLineage>,
    ) -> Arc<Option<ModelLineage>> {
        {
            let map = self.lineage.lock().expect("lineage cache lock");
            if let Some((cached, lineage)) = map.get(uri) {
                if *cached == generation {
                    self.lineage_stats.hit("lineage");
                    return lineage.clone();
                }
            }
        }
        self.lineage_stats.miss("lineage");
        let lineage = Arc::new(compute());
        self.lineage
            .lock()
            .expect("lineage cache lock")
            .insert(uri.to_string(), (generation, lineage.clone()));
        lineage
    }

    /// A serialized command result valid for one context snapshot
    /// (`ctx_ptr`) and one input fingerprint (`fingerprint` — file
    /// stamps etc. encoded by the caller). A new snapshot wipes the slot.
    pub fn result(
        &self,
        ctx_ptr: usize,
        key: &str,
        fingerprint: String,
        compute: impl FnOnce() -> Option<String>,
    ) -> Option<Arc<str>> {
        {
            let mut slot = self.results.lock().expect("results cache lock");
            if slot.0 != ctx_ptr {
                slot.0 = ctx_ptr;
                slot.1.clear();
            } else if let Some((cached, json)) = slot.1.get(key) {
                if *cached == fingerprint {
                    self.results_stats.hit("results");
                    return Some(json.clone());
                }
            }
        }
        self.results_stats.miss("results");
        let json: Arc<str> = Arc::from(compute()?.as_str());
        self.results
            .lock()
            .expect("results cache lock")
            .1
            .insert(key.to_string(), (fingerprint, json.clone()));
        Some(json)
    }

    pub fn clear(&self) {
        self.profiles.lock().expect("profiles cache lock").clear();
        self.lineage.lock().expect("lineage cache lock").clear();
        self.typed_lineage
            .lock()
            .expect("typed lineage lock")
            .clear();
        let mut slot = self.results.lock().expect("results cache lock");
        slot.0 = 0;
        slot.1.clear();
        self.artifacts.lock().expect("artifacts cache lock").clear();
        self.plans.lock().expect("plans cache lock").clear();
    }

    pub fn report(&self) -> CacheReport {
        let surface = |entries: usize, c: &Counters| CacheSurface {
            entries,
            hits: c.hits.load(Ordering::Relaxed),
            misses: c.misses.load(Ordering::Relaxed),
        };
        CacheReport {
            profiles: surface(
                self.profiles.lock().expect("profiles cache lock").len(),
                &self.profiles_stats,
            ),
            lineage: surface(
                self.lineage.lock().expect("lineage cache lock").len()
                    + self.typed_lineage.lock().expect("typed lineage lock").len(),
                &self.lineage_stats,
            ),
            results: surface(
                self.results.lock().expect("results cache lock").1.len(),
                &self.results_stats,
            ),
            artifacts: surface(
                self.artifacts.lock().expect("artifacts cache lock").len(),
                &self.artifacts_stats,
            ),
            plans: surface(
                self.plans.lock().expect("plans cache lock").len(),
                &self.plans_stats,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_profiles(name: &str, body: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cogitator-derived-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("profiles.yml");
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn profiles_cache_hits_until_the_file_changes() {
        let cache = DerivedCache::default();
        let path = temp_profiles(
            "hit",
            "p:\n  target: dev\n  outputs:\n    dev:\n      type: duckdb\n",
        );
        let first = cache.profiles_inventory(&path, "p", &[]);
        let second = cache.profiles_inventory(&path, "p", &[]);
        assert!(Arc::ptr_eq(&first, &second), "stamp unchanged → same Arc");
        // a rewrite with different content/size misses and re-parses
        std::fs::write(
            &path,
            "p:\n  target: prod\n  outputs:\n    prod:\n      type: snowflake\n",
        )
        .unwrap();
        let third = cache.profiles_inventory(&path, "p", &[]);
        assert_eq!(third.profiles[0].default_target, "prod");
        let report = cache.report();
        assert_eq!(report.profiles.hits, 1);
        assert_eq!(report.profiles.misses, 2);
    }

    #[test]
    fn scope_is_part_of_the_profiles_key() {
        let cache = DerivedCache::default();
        let path = temp_profiles(
            "scope",
            "p:\n  outputs:\n    dev:\n      type: duckdb\nq:\n  outputs:\n    dev:\n      type: duckdb\n",
        );
        let narrow = cache.profiles_inventory(&path, "p", &[]);
        let wide = cache.profiles_inventory(&path, "p", &["q".to_string()]);
        assert!(!Arc::ptr_eq(&narrow, &wide));
        assert!(!narrow
            .profiles
            .iter()
            .any(|pr| pr.name == "q" && pr.allowed));
        assert!(wide.profiles.iter().any(|pr| pr.name == "q" && pr.allowed));
    }

    #[test]
    fn lineage_generation_replaces_in_place() {
        let cache = DerivedCache::default();
        let a = cache.lineage("file:///m.sql", 1, || None);
        let b = cache.lineage("file:///m.sql", 1, || None);
        assert!(Arc::ptr_eq(&a, &b));
        let c = cache.lineage("file:///m.sql", 2, || None);
        assert!(!Arc::ptr_eq(&a, &c));
        assert_eq!(cache.report().lineage.entries, 1, "one entry per uri");
    }

    #[test]
    fn results_slot_wipes_on_a_new_snapshot() {
        let cache = DerivedCache::default();
        let one = cache.result(1, "modelInfo:customers", "s1".into(), || {
            Some("{\"a\":1}".to_string())
        });
        assert!(one.is_some());
        let hit = cache.result(1, "modelInfo:customers", "s1".into(), || None);
        assert!(hit.is_some(), "same snapshot + fingerprint → cached");
        // new context snapshot: the whole slot resets
        let miss = cache.result(2, "modelInfo:customers", "s1".into(), || {
            Some("{\"a\":2}".to_string())
        });
        assert_eq!(&*miss.unwrap(), "{\"a\":2}");
        assert_eq!(cache.report().results.entries, 1);
    }
}

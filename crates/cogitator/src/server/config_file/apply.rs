//! The hot half of cogitator.yml (03 §10): the changed-keys-only diff
//! between two file versions, launch-precedence filtering, and the one
//! shell helper that writes the live state.

use cogitator_analysis::features::plan::diagnostics_config::PerfSettings;

use super::super::{ProfilesConfig, SharedLimits, SharedToggles};
use super::FileConfig;

/// One applied difference between two file versions. `None` values in
/// the NEW file revert the key to the launch-time `baseline`.
#[derive(Debug, PartialEq)]
pub enum Change {
    ProfilesDir(String),
    Profile(String),
    Target(String),
    Allow(Vec<String>),
    CompiledCapMib(u64),
    DebounceMs(u64),
    ArtifactCapMib(u64),
    LintFlags(u32),
    WarehouseColumns(bool),
    ClientLogLevel(u8),
    Perf(PerfSettings),
}

/// The changed-keys-only diff: a key participates only when its file
/// value CHANGED between `old` and `new`; a key that disappeared
/// reverts to `baseline`'s launch value.
pub fn diff(old: &FileConfig, new: &FileConfig, baseline: &Baseline) -> Vec<Change> {
    let mut out = Vec::new();
    let mut field = |old_v: &Option<String>,
                     new_v: &Option<String>,
                     fallback: &str,
                     make: fn(String) -> Change| {
        if old_v != new_v {
            out.push(make(new_v.clone().unwrap_or_else(|| fallback.to_string())));
        }
    };
    field(
        &old.profiles.dir,
        &new.profiles.dir,
        &baseline.profiles.dir_override,
        Change::ProfilesDir,
    );
    field(
        &old.profiles.profile,
        &new.profiles.profile,
        &baseline.profiles.profile,
        Change::Profile,
    );
    field(
        &old.profiles.target,
        &new.profiles.target,
        &baseline.profiles.target,
        Change::Target,
    );
    if old.profiles.allow != new.profiles.allow {
        out.push(Change::Allow(
            new.profiles
                .allow
                .clone()
                .unwrap_or_else(|| baseline.profiles.allow.clone()),
        ));
    }
    if old.limits.compiled_sql_mib != new.limits.compiled_sql_mib {
        out.push(Change::CompiledCapMib(
            new.limits
                .compiled_sql_mib
                .unwrap_or(baseline.compiled_cap_mib),
        ));
    }
    if old.refresh.debounce_ms != new.refresh.debounce_ms {
        out.push(Change::DebounceMs(
            new.refresh.debounce_ms.unwrap_or(baseline.debounce_ms),
        ));
    }
    if old.limits.artifact_mib != new.limits.artifact_mib {
        out.push(Change::ArtifactCapMib(
            new.limits.artifact_mib.unwrap_or(baseline.artifact_mib),
        ));
    }
    if old.lint != new.lint {
        out.push(Change::LintFlags(new.lint.flags(baseline.lint_flags)));
    }
    if old.warehouse != new.warehouse {
        out.push(Change::WarehouseColumns(
            new.warehouse.columns.unwrap_or(baseline.warehouse_columns),
        ));
    }
    if old.perf != new.perf
        || old.perf_rules != new.perf_rules
        || old.lint.select_star != new.lint.select_star
    {
        out.push(Change::Perf(new.perf_settings(&baseline.perf)));
    }
    if old.log != new.log {
        // an unrecognized level name keeps the launch value — the
        // cogitator.yml diagnostics flag the typo separately
        out.push(Change::ClientLogLevel(
            new.log
                .client_level
                .as_deref()
                .and_then(crate::client_log::level_from_name)
                .unwrap_or(baseline.client_log_level),
        ));
    }
    out
}

/// Launch precedence (defaults < file < CLI < initializationOptions):
/// on the FIRST load of the root file, a profiles key that a higher
/// layer already set (non-empty in the baseline) stays with that layer.
/// Later edits apply unconditionally — last writer wins.
pub fn launch_filter(changes: Vec<Change>, baseline: &Baseline) -> Vec<Change> {
    changes
        .into_iter()
        .filter(|change| match change {
            Change::ProfilesDir(_) => baseline.profiles.dir_override.is_empty(),
            Change::Profile(_) => baseline.profiles.profile.is_empty(),
            Change::Target(_) => baseline.profiles.target.is_empty(),
            Change::Allow(_) => baseline.profiles.allow.is_empty(),
            Change::CompiledCapMib(_)
            | Change::DebounceMs(_)
            | Change::ArtifactCapMib(_)
            | Change::LintFlags(_)
            | Change::WarehouseColumns(_)
            | Change::ClientLogLevel(_)
            | Change::Perf(_) => true,
        })
        .collect()
}

/// The launch-time values keys revert to when removed from the file.
#[derive(Debug, Clone)]
pub struct Baseline {
    pub profiles: ProfilesConfig,
    pub compiled_cap_mib: u64,
    pub debounce_ms: u64,
    pub artifact_mib: u64,
    pub lint_flags: u32,
    pub warehouse_columns: bool,
    pub client_log_level: u8,
    pub perf: PerfSettings,
}

/// Apply changes to the live state; returns the key names for the one
/// INFO message. State writes stay in this shell helper (05 §6.2).
pub fn apply(
    changes: &[Change],
    profiles: &std::sync::Mutex<ProfilesConfig>,
    limits: &SharedLimits,
    toggles: &SharedToggles,
) -> Vec<&'static str> {
    use std::sync::atomic::Ordering;

    let mut applied = Vec::new();
    for change in changes {
        match change {
            Change::ProfilesDir(dir) => {
                profiles.lock().expect("profiles config lock").dir_override = dir.clone();
                applied.push("profiles.dir");
            }
            Change::Profile(profile) => {
                profiles.lock().expect("profiles config lock").profile = profile.clone();
                applied.push("profiles.profile");
            }
            Change::Target(target) => {
                profiles.lock().expect("profiles config lock").target = target.clone();
                applied.push("profiles.target");
            }
            Change::Allow(allow) => {
                profiles.lock().expect("profiles config lock").allow = allow.clone();
                applied.push("profiles.allow");
            }
            Change::CompiledCapMib(mib) => {
                limits
                    .compiled_cap_bytes
                    .store(mib * 1024 * 1024, Ordering::Relaxed);
                applied.push("limits.compiled_sql_mib");
            }
            Change::DebounceMs(ms) => {
                limits.refresh_debounce_ms.store(*ms, Ordering::Relaxed);
                applied.push("refresh.debounce_ms");
            }
            Change::ArtifactCapMib(mib) => {
                limits
                    .artifact_cap_bytes
                    .store(mib * 1024 * 1024, Ordering::Relaxed);
                applied.push("limits.artifact_mib");
            }
            Change::LintFlags(flags) => {
                toggles.lints.store(*flags, Ordering::Relaxed);
                applied.push("lint");
            }
            Change::WarehouseColumns(on) => {
                toggles.warehouse_columns.store(*on, Ordering::Relaxed);
                applied.push("warehouse.columns");
            }
            Change::ClientLogLevel(level) => {
                crate::client_log::set_level(*level);
                applied.push("log.client_level");
            }
            Change::Perf(settings) => {
                // a Mutex-free swap, kept tight: readers clone the Arc
                *toggles.perf.write().expect("perf settings lock") =
                    std::sync::Arc::new(settings.clone());
                applied.push("perf");
            }
        }
    }
    applied
}

#[cfg(test)]
mod tests {
    use super::super::parse;
    use super::*;
    use cogitator_analysis::config_schema::lint_flags;
    use std::path::Path;

    fn baseline() -> Baseline {
        Baseline {
            profiles: ProfilesConfig::default(),
            compiled_cap_mib: 2,
            debounce_ms: 200,
            artifact_mib: 64,
            lint_flags: 0,
            warehouse_columns: false,
            client_log_level: crate::client_log::level::INFO,
            perf: PerfSettings::default(),
        }
    }

    #[test]
    fn lint_section_diffs_into_flags() {
        let old = parse("lint:\n  select_star: true\n", Path::new("x")).unwrap();
        let new = parse(
            "lint:\n  select_star: true\n  dead_models: true\n",
            Path::new("x"),
        )
        .unwrap();
        let changes = diff(&old, &new, &baseline());
        assert_eq!(
            changes,
            vec![Change::LintFlags(
                lint_flags::SELECT_STAR | lint_flags::DEAD_MODELS
            )]
        );
        // removing the whole section reverts to the baseline (all off)
        let gone = parse("", Path::new("x")).unwrap();
        let reverted = diff(&new, &gone, &baseline());
        // ...and, since select_star aliases a plan rule, the perf settings
        assert_eq!(
            reverted,
            vec![Change::LintFlags(0), Change::Perf(PerfSettings::default())]
        );
    }

    #[test]
    fn diff_applies_changed_keys_only_and_reverts_removals() {
        let old = parse("profiles:\n  target: dev\n  dir: /a\n", Path::new("x")).unwrap();
        let new = parse("profiles:\n  target: prod\n", Path::new("x")).unwrap();
        let changes = diff(&old, &new, &baseline());
        // target changed; dir was REMOVED → reverts to the baseline (empty)
        assert!(
            changes.contains(&Change::Target("prod".into())),
            "{changes:?}"
        );
        assert!(
            changes.contains(&Change::ProfilesDir(String::new())),
            "{changes:?}"
        );
        assert_eq!(changes.len(), 2, "untouched keys stay out: {changes:?}");
    }

    #[test]
    fn launch_filter_keeps_higher_layers() {
        let mut base = baseline();
        base.profiles.target = "prod".into(); // set via CLI/initOptions
        let kept = launch_filter(
            vec![
                Change::Target("dev".into()),
                Change::ProfilesDir("/x".into()),
                Change::CompiledCapMib(4),
            ],
            &base,
        );
        // the CLI-set target survives the first file load; the rest applies
        assert_eq!(
            kept,
            vec![Change::ProfilesDir("/x".into()), Change::CompiledCapMib(4)]
        );
    }

    #[test]
    fn apply_writes_the_live_state() {
        use std::sync::atomic::Ordering;
        let profiles = std::sync::Mutex::new(ProfilesConfig::default());
        let limits = SharedLimits::default();
        let toggles = SharedToggles::default();
        let applied = apply(
            &[
                Change::Target("prod".into()),
                Change::CompiledCapMib(4),
                Change::DebounceMs(50),
                Change::LintFlags(lint_flags::SELECT_STAR),
            ],
            &profiles,
            &limits,
            &toggles,
        );
        assert_eq!(
            applied,
            vec![
                "profiles.target",
                "limits.compiled_sql_mib",
                "refresh.debounce_ms",
                "lint"
            ]
        );
        assert_eq!(profiles.lock().unwrap().target, "prod");
        assert_eq!(
            limits.compiled_cap_bytes.load(Ordering::Relaxed),
            4 * 1024 * 1024
        );
        assert_eq!(limits.refresh_debounce_ms.load(Ordering::Relaxed), 50);
        assert_eq!(
            toggles.lints.load(Ordering::Relaxed),
            lint_flags::SELECT_STAR
        );
    }
}

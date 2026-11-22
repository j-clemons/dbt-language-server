//! `perf`/`perf_rules` → the resolved [`PerfSettings`]. Per-key layering:
//! a value the file does not set (or sets to something unrecognized)
//! keeps the launch value, so changed-keys-only semantics hold.

use cogitator_analysis::features::plan::diagnostics_config::{
    advice_rule_ids, PerfProfile, PerfSettings, RuleOverride, SELECT_STAR_RULE,
};

use super::FileConfig;

impl FileConfig {
    /// The settings this file asks for, layered over `base`. An
    /// unrecognized profile or rule value keeps `base`; the cogitator.yml
    /// diagnostics flag the typo. Unknown rule ids never enter the map.
    pub fn perf_settings(&self, base: &PerfSettings) -> PerfSettings {
        let mut settings = base.clone();
        if let Some(profile) = self.perf.profile.as_deref().and_then(PerfProfile::parse) {
            settings.profile = profile;
        }
        if let Some(on) = self.perf.catalog_stats {
            settings.catalog_stats = on;
        }
        let known: Vec<&str> = advice_rule_ids().collect();
        for (id, value) in &self.perf_rules {
            if !known.contains(&id.as_str()) {
                continue;
            }
            if let Some(entry) = RuleOverride::parse(value) {
                settings.rules.insert(id.clone(), entry);
            }
        }
        // `lint.select_star` aliases the plan rule; a perf_rules entry
        // for the rule is more specific and wins
        if !self.perf_rules.contains_key(SELECT_STAR_RULE) {
            match self.lint.select_star {
                Some(true) => {
                    settings
                        .rules
                        .insert(SELECT_STAR_RULE.to_string(), RuleOverride::On);
                }
                Some(false) => {
                    settings
                        .rules
                        .insert(SELECT_STAR_RULE.to_string(), RuleOverride::Off);
                }
                None => {}
            }
        }
        settings
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::{apply, diff, parse, Baseline};
    use super::*;
    use crate::server::{ProfilesConfig, SharedLimits, SharedToggles};
    use cogitator_analysis::features::plan::diagnostics_config::Severity;

    fn file(text: &str) -> FileConfig {
        parse(text, Path::new("cogitator.yml")).unwrap()
    }

    #[test]
    fn dotted_rule_ids_and_values_parse() {
        let settings = file(
            "perf:\n  profile: recommended\n  catalog_stats: false\nperf_rules:\n  join.cartesian: warning\n  filter.non_sargable: off\n",
        )
        .perf_settings(&PerfSettings::default());
        assert_eq!(settings.profile, PerfProfile::Recommended);
        assert!(!settings.catalog_stats);
        assert_eq!(
            settings.rules["join.cartesian"],
            RuleOverride::Set(Severity::Warning)
        );
        assert_eq!(settings.rules["filter.non_sargable"], RuleOverride::Off);
    }

    #[test]
    fn bad_values_keep_the_launch_value() {
        let settings = file(
            "perf:\n  profile: loud\nperf_rules:\n  not.a_rule: warning\n  join.cartesian: nope\n",
        )
        .perf_settings(&PerfSettings::default());
        assert_eq!(settings, PerfSettings::default());
    }

    #[test]
    fn select_star_aliases_the_plan_rule() {
        let on = file("lint:\n  select_star: true\n").perf_settings(&PerfSettings::default());
        assert_eq!(on.rules[SELECT_STAR_RULE], RuleOverride::On);
        let off = file("lint:\n  select_star: false\n").perf_settings(&PerfSettings::default());
        assert_eq!(off.rules[SELECT_STAR_RULE], RuleOverride::Off);
        // an explicit perf_rules entry is more specific
        let explicit =
            file("lint:\n  select_star: true\nperf_rules:\n  cross.select_star_chain: off\n")
                .perf_settings(&PerfSettings::default());
        assert_eq!(explicit.rules[SELECT_STAR_RULE], RuleOverride::Off);
    }

    #[test]
    fn perf_rules_hot_reload_through_diff_and_apply() {
        let baseline = Baseline {
            profiles: ProfilesConfig::default(),
            compiled_cap_mib: 2,
            debounce_ms: 200,
            artifact_mib: 64,
            lint_flags: 0,
            warehouse_columns: false,
            client_log_level: crate::client_log::level::INFO,
            perf: PerfSettings::default(),
        };
        let toggles = SharedToggles::default();
        let limits = SharedLimits::default();
        let profiles = std::sync::Mutex::new(ProfilesConfig::default());
        let first = file("perf_rules:\n  join.cartesian: warning\n");
        let changes = diff(&FileConfig::default(), &first, &baseline);
        assert_eq!(apply(&changes, &profiles, &limits, &toggles), vec!["perf"]);
        assert_eq!(
            toggles.perf.read().unwrap().rules["join.cartesian"],
            RuleOverride::Set(Severity::Warning)
        );
        // an unrelated edit does not touch the live settings
        let same = file("perf_rules:\n  join.cartesian: warning\nlimits:\n  compiled_sql_mib: 4\n");
        assert!(diff(&first, &same, &baseline)
            .iter()
            .all(|c| !matches!(c, super::super::apply::Change::Perf(_))));
        // the section vanishing reverts to the launch value
        let changes = diff(&first, &FileConfig::default(), &baseline);
        apply(&changes, &profiles, &limits, &toggles);
        assert!(toggles.perf.read().unwrap().rules.is_empty());
    }
}

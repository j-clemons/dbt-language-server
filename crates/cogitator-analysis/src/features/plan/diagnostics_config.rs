//! The `perf`/`perf_rules` settings (specs/ir-pipeline/08): which rules
//! report as diagnostics, at what severity. Plain values, built by the
//! config layer and read wherever diagnostics assemble.

use std::collections::BTreeMap;

use cogitator_rules::meta::RuleClass;
pub use cogitator_rules::meta::Severity;
use cogitator_rules::REGISTRY;

/// The named rule set (`perf.profile`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PerfProfile {
    #[default]
    Off,
    Recommended,
    Strict,
}

impl PerfProfile {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "off" => Some(PerfProfile::Off),
            "recommended" => Some(PerfProfile::Recommended),
            "strict" => Some(PerfProfile::Strict),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            PerfProfile::Off => "off",
            PerfProfile::Recommended => "recommended",
            PerfProfile::Strict => "strict",
        }
    }
}

/// One `perf_rules` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleOverride {
    /// The rule stays silent, whatever the profile.
    Off,
    /// On, at the severity the finding reports (the `lint.select_star`
    /// alias: that key never carried a severity).
    On,
    /// On, always at this severity.
    Set(Severity),
}

impl RuleOverride {
    /// A `perf_rules` value; `None` for anything else.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "off" => Some(RuleOverride::Off),
            "hint" => Some(RuleOverride::Set(Severity::Hint)),
            "info" => Some(RuleOverride::Set(Severity::Info)),
            "warning" => Some(RuleOverride::Set(Severity::Warning)),
            _ => None,
        }
    }
}

/// The rule `lint.select_star` aliases.
pub const SELECT_STAR_RULE: &str = "cross.select_star_chain";

/// What `recommended` reports. Everything else needs `strict` or a
/// `perf_rules` entry: heuristics stay opt-in until false positives are
/// measured (specs/ir-pipeline/08).
pub const RECOMMENDED_RULES: &[&str] = &[
    "join.left_nullified",
    "join.cartesian",
    "model.unused_cte",
    "incr.missing_guard",
    "incr.subquery_watermark",
    "filter.non_sargable",
];

/// The registry's advice rules: the ids that can be reported and
/// configured (canonicalisers never report).
pub fn advice_rule_ids() -> impl Iterator<Item = &'static str> {
    REGISTRY
        .iter()
        .filter(|rule| rule.meta.class == RuleClass::Advice)
        .map(|rule| rule.meta.id.as_str())
}

/// The highest severity a rule reports; findings never report above it.
fn rule_ceiling(id: &str) -> Severity {
    REGISTRY
        .iter()
        .find(|rule| rule.meta.id.as_str() == id)
        .map_or(Severity::Off, |rule| rule.meta.severity)
}

/// Resolved settings. Default: profile off, no overrides, catalog stats
/// used (they only matter once a profile or rule is on).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerfSettings {
    pub profile: PerfProfile,
    pub catalog_stats: bool,
    pub rules: BTreeMap<String, RuleOverride>,
}

impl Default for PerfSettings {
    fn default() -> Self {
        PerfSettings {
            profile: PerfProfile::Off,
            catalog_stats: true,
            rules: BTreeMap::new(),
        }
    }
}

impl PerfSettings {
    /// Whether any rule can report at all. When false, diagnostics skip
    /// the plan entirely.
    pub fn active(&self) -> bool {
        advice_rule_ids().any(|id| self.enabled(id))
    }

    /// The number of advice rules currently enabled.
    pub fn enabled_rule_count(&self) -> usize {
        advice_rule_ids().filter(|id| self.enabled(id)).count()
    }

    fn enabled(&self, id: &str) -> bool {
        match self.rules.get(id) {
            Some(RuleOverride::Off) => false,
            Some(RuleOverride::On | RuleOverride::Set(_)) => true,
            None => match self.profile {
                PerfProfile::Off => false,
                PerfProfile::Recommended => RECOMMENDED_RULES.contains(&id),
                PerfProfile::Strict => true,
            },
        }
    }

    /// The severity to report a finding of rule `id` at, or `None` when
    /// the rule is silent under these settings. `reported` is what the
    /// finding carries; it is capped at the rule's declared ceiling so
    /// evidence (catalog statistics above all) can never raise it. An
    /// explicit `perf_rules` severity is the user's decision and wins.
    pub fn severity(&self, id: &str, reported: Severity) -> Option<Severity> {
        if let Some(RuleOverride::Set(severity)) = self.rules.get(id) {
            return Some(*severity);
        }
        if !self.enabled(id) {
            return None;
        }
        let capped = reported.min(rule_ceiling(id));
        (capped != Severity::Off).then_some(capped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(profile: PerfProfile, rules: &[(&str, RuleOverride)]) -> PerfSettings {
        PerfSettings {
            profile,
            catalog_stats: true,
            rules: rules.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
        }
    }

    #[test]
    fn recommended_list_names_real_rules() {
        let advice: Vec<&str> = advice_rule_ids().collect();
        for rule in RECOMMENDED_RULES {
            assert!(advice.contains(rule), "{rule}");
        }
        assert!(advice.contains(&SELECT_STAR_RULE));
    }

    #[test]
    fn off_profile_reports_nothing_unless_a_rule_opts_in() {
        let off = PerfSettings::default();
        assert!(!off.active());
        assert_eq!(off.severity("join.cartesian", Severity::Warning), None);
        let opted = with(
            PerfProfile::Off,
            &[("join.cartesian", RuleOverride::Set(Severity::Hint))],
        );
        assert!(opted.active());
        assert_eq!(opted.enabled_rule_count(), 1);
        assert_eq!(
            opted.severity("join.cartesian", Severity::Warning),
            Some(Severity::Hint)
        );
        assert_eq!(
            opted.severity("join.left_nullified", Severity::Warning),
            None
        );
    }

    #[test]
    fn recommended_is_a_subset_of_strict() {
        let recommended = with(PerfProfile::Recommended, &[]);
        let strict = with(PerfProfile::Strict, &[]);
        assert_eq!(recommended.enabled_rule_count(), RECOMMENDED_RULES.len());
        assert_eq!(strict.enabled_rule_count(), advice_rule_ids().count());
        // the dialect-specific severity survives both
        assert_eq!(
            recommended.severity("filter.non_sargable", Severity::Hint),
            Some(Severity::Hint)
        );
        // a heuristic rule waits for strict
        assert_eq!(
            recommended.severity("set.union_distinct", Severity::Info),
            None
        );
        assert_eq!(
            strict.severity("set.union_distinct", Severity::Info),
            Some(Severity::Info)
        );
    }

    #[test]
    fn overrides_turn_rules_off_and_on() {
        let settings = with(
            PerfProfile::Strict,
            &[
                ("filter.non_sargable", RuleOverride::Off),
                (SELECT_STAR_RULE, RuleOverride::On),
            ],
        );
        assert_eq!(
            settings.severity("filter.non_sargable", Severity::Warning),
            None
        );
        assert_eq!(
            settings.severity(SELECT_STAR_RULE, Severity::Info),
            Some(Severity::Info)
        );
    }

    #[test]
    fn evidence_never_raises_above_the_rule_ceiling() {
        let strict = with(PerfProfile::Strict, &[]);
        // join.distinct_fanout declares Info; a finding claiming more
        // (catalog stats, say) stays at the ceiling
        assert_eq!(
            strict.severity("join.distinct_fanout", Severity::Warning),
            Some(Severity::Info)
        );
        // an explicit override is the user's call, not evidence's
        let raised = with(
            PerfProfile::Strict,
            &[("join.distinct_fanout", RuleOverride::Set(Severity::Warning))],
        );
        assert_eq!(
            raised.severity("join.distinct_fanout", Severity::Hint),
            Some(Severity::Warning)
        );
    }

    #[test]
    fn parses_names_and_rejects_the_rest() {
        assert_eq!(PerfProfile::parse("strict"), Some(PerfProfile::Strict));
        assert_eq!(PerfProfile::parse("loud"), None);
        assert_eq!(
            RuleOverride::parse("warning"),
            Some(RuleOverride::Set(Severity::Warning))
        );
        assert_eq!(RuleOverride::parse("off"), Some(RuleOverride::Off));
        assert_eq!(RuleOverride::parse("error"), None);
    }
}

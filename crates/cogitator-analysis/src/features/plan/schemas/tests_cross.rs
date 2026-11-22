//! Per-model facts behind the `cross.*` rules, through real SQL.

use std::sync::Arc;

use cogitator_ir::mir::RelId;
use cogitator_rules::rules::cross::subplan::Subplan;
use cogitator_rules::{ColumnFilter, DagFacts};

use super::shared::group;
use super::tests::{run, Files, Project};
use super::JobState;

fn facts_of(project: &Project, model: &str) -> Option<DagFacts> {
    let (ctx, files) = (project.ctx(), Files::default());
    run(&ctx, &files, &mut JobState::default()).dag.facts(model)
}

const WIDE: &str = "select 1 as id, 'x' as status, 2 as amount";

fn filters(facts: &DagFacts, consumer: &str) -> Vec<(String, String)> {
    facts.consumers[consumer]
        .filters
        .iter()
        .map(|f: &ColumnFilter| (f.column.to_string(), f.predicate.to_string()))
        .collect()
}

#[test]
fn a_plain_filter_and_one_through_a_cte_read_the_same() {
    let project = Project::new()
        .sql("up", WIDE)
        .sql(
            "direct",
            "select id from {{ ref('up') }} where status = 'done'",
        )
        .sql(
            "via_cte",
            "with src as (select * from {{ ref('up') }})\nselect id from src where status = 'done'",
        );
    let facts = facts_of(&project, "up").expect("known");
    let direct = filters(&facts, "direct");
    assert_eq!(direct.len(), 1, "{direct:?}");
    assert_eq!(direct[0].0, "status");
    assert_eq!(direct, filters(&facts, "via_cte"));
}

#[test]
fn a_renamed_column_still_filters_the_upstream_column() {
    let project = Project::new().sql("up", WIDE).sql(
        "renamed",
        "with src as (select status as st, id from {{ ref('up') }})\nselect id from src where st = 'done'",
    );
    let facts = facts_of(&project, "up").expect("known");
    assert_eq!(filters(&facts, "renamed")[0].0, "status");
}

#[test]
fn a_filter_above_a_left_join_proves_nothing_about_this_input() {
    let project = Project::new()
        .sql("up", WIDE)
        .sql("other", "select 1 as id, 'y' as kind")
        .sql(
            "joined",
            "select u.id from {{ ref('up') }} u left join {{ ref('other') }} o \
             on u.id = o.id where o.kind = 'a'",
        );
    let facts = facts_of(&project, "other").expect("known");
    assert!(filters(&facts, "joined").is_empty());
}

#[test]
fn a_predicate_with_a_function_is_not_provably_the_same_filter() {
    let project = Project::new().sql("up", WIDE).sql(
        "volatile",
        "select id from {{ ref('up') }} where amount > current_date",
    );
    let facts = facts_of(&project, "up").expect("known");
    assert!(filters(&facts, "volatile").is_empty());
}

#[test]
fn jinja_in_a_predicate_is_not_provably_the_same_filter() {
    for (name, predicate) in [
        ("scalar", "status = {{ var('s') }}"),
        ("in_string", "status = '{{ var(\"s\") }}'"),
    ] {
        let project = Project::new().sql("up", WIDE).sql(
            name,
            &format!("select id from {{{{ ref('up') }}}} where {predicate}"),
        );
        let facts = facts_of(&project, "up");
        // A scalar hole makes this consumer's reads unknown, withholding
        // `Need::Dag`; otherwise the predicate must not be recorded.
        assert!(facts.is_none_or(|f| filters(&f, name).is_empty()), "{name}");
    }
}

#[test]
fn an_incremental_guard_is_not_a_filter_every_build_applies() {
    let project = Project::new().sql("up", WIDE).sql(
        "incr",
        "{{ config(materialized='incremental') }}\nselect id from {{ ref('up') }}\n\
         {% if is_incremental() %}where status = 'done'{% endif %}",
    );
    let facts = facts_of(&project, "up").expect("known");
    assert!(filters(&facts, "incr").is_empty());
}

#[test]
fn a_model_read_twice_proves_nothing_from_either_read() {
    let project = Project::new().sql("up", WIDE).sql(
        "twice",
        "select a.id from {{ ref('up') }} a join {{ ref('up') }} b on a.id = b.id \
         where a.status = 'done'",
    );
    let facts = facts_of(&project, "up").expect("known");
    assert!(filters(&facts, "twice").is_empty());
}

#[test]
fn an_unreadable_consumer_withholds_every_fact() {
    let project = Project::new().sql("up", WIDE).sql(
        "branchy",
        "{% if target.name == 'x' %}select id from {{ ref('up') }}{% else %}select 1 as id{% endif %}",
    );
    assert!(facts_of(&project, "up").is_none());
}

#[test]
fn every_direct_consumer_is_listed_even_one_reading_nothing() {
    let project = Project::new()
        .sql("up", WIDE)
        .sql("reader", "select id from {{ ref('up') }}")
        .sql("counter", "select count(*) as n from {{ ref('up') }}");
    let facts = facts_of(&project, "up").expect("known");
    let names: Vec<&str> = facts.consumers.keys().map(|k| &**k).collect();
    assert_eq!(names, ["counter", "reader"]);
}

#[test]
fn an_ephemeral_is_counted_where_it_is_compiled_through_chains() {
    let project = Project::new()
        .sql(
            "eph",
            "{{ config(materialized='ephemeral') }}\nselect 1 as id",
        )
        .sql(
            "mid",
            "{{ config(materialized='ephemeral') }}\nselect id from {{ ref('eph') }}",
        )
        .sql(
            "a",
            "{{ config(materialized='table') }}\nselect id from {{ ref('mid') }}",
        )
        .sql(
            "b",
            "{{ config(materialized='view') }}\nselect id from {{ ref('mid') }}",
        )
        .sql("c", "select id from {{ ref('eph') }}");
    let facts = facts_of(&project, "eph").expect("known");
    let sites: Vec<&str> = facts.inlined_into.iter().map(|s| &**s).collect();
    assert_eq!(sites, ["a", "b", "c"], "chains looked through, sorted");
    let table = facts_of(&project, "a").expect("known");
    assert!(table.inlined_into.is_empty(), "only ephemerals are counted");
}

const JOIN: &str = "select a.id, sum(b.amount) as total\nfrom {{ ref('x') }} as a\n\
                    join {{ ref('y') }} as b on a.id = b.id\ngroup by a.id";

fn upstreams(project: Project) -> Project {
    project
        .sql("x", "select 1 as id")
        .sql("y", "select 1 as id, 2 as amount")
}

#[test]
fn two_models_with_the_same_join_share_a_subplan() {
    let project = upstreams(Project::new()).sql("m1", JOIN).sql("m2", JOIN);
    let facts = facts_of(&project, "m1").expect("known");
    let shared: Vec<_> = facts.shared_subplans.values().collect();
    assert!(
        shared
            .iter()
            .any(|s| s.models.iter().map(|m| &**m).eq(["m2"]) && s.size >= 4),
        "{shared:#?}"
    );
}

#[test]
fn a_trivial_star_never_counts_as_shared_logic() {
    let project = upstreams(Project::new())
        .sql("s1", "select * from {{ ref('x') }}")
        .sql("s2", "select * from {{ ref('x') }}");
    assert!(facts_of(&project, "s1")
        .expect("known")
        .shared_subplans
        .is_empty());
}

#[test]
fn a_different_window_partition_is_different_logic() {
    let base = |partition: &str| {
        format!(
            "select id, sum(amount) over (partition by {partition}) as t \
             from {{{{ ref('y') }}}} join {{{{ ref('x') }}}} using (id)"
        )
    };
    let project = upstreams(Project::new())
        .sql("w1", &base("id"))
        .sql("w2", &base("amount"));
    let shared = facts_of(&project, "w1").expect("known").shared_subplans;
    assert!(
        shared
            .values()
            .all(|s| !s.models.iter().any(|m| &**m == "w2")),
        "{shared:#?}"
    );
}

#[test]
fn a_model_edit_updates_facts_and_early_cut_off_reuses_the_rest() {
    let project = upstreams(Project::new())
        .sql("m1", "select id from {{ ref('x') }} where id > 1")
        .sql("m2", "select id from {{ ref('x') }} where id > 1");
    let (ctx, files) = (project.ctx(), Files::default());
    let mut state = JobState::default();
    let first = run(&ctx, &files, &mut state);
    let before = first.dag.facts("x").expect("known");
    assert_eq!(filters(&before, "m1"), filters(&before, "m2"));

    let unchanged = run(&ctx, &files, &mut state);
    assert_eq!(unchanged.report.rebuilt, 0);
    assert_eq!(unchanged.revision, first.revision, "no change, no bump");

    project.write_sql("m2", "select id from {{ ref('x') }} where id > 2");
    files.bump(&project.root.join("models/m2.sql"));
    let after = run(&ctx, &files, &mut state);
    assert_eq!(after.report.rebuilt, 1, "only m2's file changed");
    assert!(after.report.reused >= 3, "{:?}", after.report);
    assert_ne!(after.revision, first.revision, "the facts moved");
    let facts = after.dag.facts("x").expect("known");
    assert_ne!(filters(&facts, "m1"), filters(&facts, "m2"));
}

#[test]
fn same_length_jinja_string_literals_are_not_shared_logic() {
    let text = |v: &str| {
        format!(
            "select id, sum(amount) as t from {{{{ ref('y') }}}} join {{{{ ref('x') }}}} using (id) \
             where amount = '{{{{ var(\"{v}\") }}}}' group by id"
        )
    };
    let project = upstreams(Project::new())
        .sql("j1", &text("a"))
        .sql("j2", &text("b"));
    let shared = facts_of(&project, "j1").expect("known").shared_subplans;
    assert!(
        shared
            .values()
            .all(|s| !s.models.iter().any(|m| &**m == "j2")),
        "{shared:#?}"
    );
}

/// The real path: the job's facts reach the rules through `query_plan`.
#[test]
fn the_rules_see_the_jobs_facts_on_a_live_plan() {
    let project = upstreams(Project::new())
        .sql("m1", JOIN)
        .sql("m2", JOIN)
        .sql("big", &format!("{{{{ config(materialized='table') }}}}\n{WIDE}"))
        .sql(
            "r1",
            "select id from {{ ref('big') }} where status = 'done'",
        )
        .sql(
            "r2",
            "with s as (select * from {{ ref('big') }})\nselect amount from s where status = 'done'",
        );
    let (ctx, files) = (project.ctx(), Files::default());
    let snapshot = run(&ctx, &files, &mut JobState::default());
    let artifacts = crate::artifacts::ArtifactSet::default();
    let findings = |model: &str, text: &str| {
        let input = crate::features::plan::PlanInput::from_context(
            &ctx,
            text,
            Some(model),
            &artifacts,
            snapshot.revision,
        )
        .with_project(Some(&*snapshot));
        crate::features::plan::query_plan(&input).findings
    };
    let dup = findings("m1", JOIN);
    assert!(
        dup.iter().any(|f| f.id == "cross.duplicate_logic"),
        "{dup:#?}"
    );
    let big = format!("{{{{ config(materialized='table') }}}}\n{WIDE}");
    let filtered = findings("big", &big);
    assert!(
        filtered.iter().any(|f| f.id == "cross.filter_late"),
        "{filtered:#?}"
    );
}

fn subplan(hash: u64, text: &str) -> Subplan {
    Subplan {
        at: RelId::from_raw(0),
        hash,
        size: 6,
        text: Arc::from(text),
    }
}

#[test]
fn a_hash_collision_is_not_shared_logic() {
    let collided = group(
        [
            (Arc::<str>::from("m1"), vec![subplan(7, "join(a,b)")]),
            (Arc::<str>::from("m2"), vec![subplan(7, "join(c,d)")]),
        ]
        .iter()
        .map(|(m, s)| (m.clone(), s.as_slice())),
        u64::MAX,
    );
    assert!(collided.by_model.is_empty(), "{:?}", collided.by_model);

    let confirmed = group(
        [
            (Arc::<str>::from("m1"), vec![subplan(7, "join(a,b)")]),
            (Arc::<str>::from("m2"), vec![subplan(7, "join(a,b)")]),
            (Arc::<str>::from("m3"), vec![subplan(7, "join(a,b)x")]),
        ]
        .iter()
        .map(|(m, s)| (m.clone(), s.as_slice())),
        u64::MAX,
    );
    let shared = &confirmed.by_model[&Arc::<str>::from("m1")][&7];
    assert_eq!(shared.models, vec![Arc::<str>::from("m2")]);
    assert_eq!(shared.total, 1, "the collider is not counted");
}

#[test]
fn the_subplan_budget_degrades_explicitly() {
    let subject = |bytes| {
        group(
            [(Arc::<str>::from("m"), vec![subplan(1, "abcdef")])]
                .iter()
                .map(|(m, s)| (m.clone(), s.as_slice())),
            bytes,
        )
    };
    assert!(subject(100).over_budget.is_none());
    assert_eq!(subject(2).over_budget.as_deref(), Some("m"));
}

#[test]
fn group_size_is_bounded_but_the_total_stays_honest() {
    let many: Vec<(Arc<str>, Vec<Subplan>)> = (0..40)
        .map(|i| {
            (
                Arc::<str>::from(format!("m{i:02}").as_str()),
                vec![subplan(1, "shared")],
            )
        })
        .collect();
    let grouped = group(
        many.iter().map(|(m, s)| (m.clone(), s.as_slice())),
        u64::MAX,
    );
    let shared = &grouped.by_model[&Arc::<str>::from("m00")][&1];
    assert_eq!(shared.models.len(), 16);
    assert_eq!(shared.total, 39);
}

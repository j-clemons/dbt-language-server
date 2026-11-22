//! Plans closer to a real project than `plan_budgets`: a Jinja-heavy mart,
//! control flow routed through compiled SQL, and a large mart, measured
//! against the 15 ms miss and 1 ms cached budgets.
//!
//! No Cargo.toml target, so this runs through the test harness:
//! `cargo test --release -p cogitator-analysis --bench plan_realistic -- --ignored --nocapture`

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use cogitator_analysis::artifacts::ArtifactSet;
use cogitator_analysis::features::plan::diagnostics::plan_diagnostics;
use cogitator_analysis::features::plan::diagnostics_config::{PerfProfile, PerfSettings};
use cogitator_analysis::features::plan::schemas::{
    run_schema_job, JobInput, JobLimits, JobState, SavedFiles, SchemaSnapshot,
};
use cogitator_analysis::features::plan::suppress::Suppressions;
use cogitator_analysis::features::plan::{
    query_plan, CompiledInput, PlanInput, QueryPlan, Staleness,
};
use cogitator_analysis::{build_context, DbtContext};

const SAMPLES: usize = 300;
/// `stg_*` models the marts read.
const STAGING: usize = 12;
/// Columns on each staging model.
const WIDTH: usize = 40;

struct Project {
    root: PathBuf,
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn staging_columns() -> Vec<String> {
    (0..WIDTH).map(|c| format!("col_{c}")).collect()
}

fn write(dir: &Path, name: &str, sql: &str) {
    std::fs::write(dir.join(format!("{name}.sql")), sql).expect("model");
}

fn staging(dir: &Path, i: usize) {
    let list = staging_columns().join(",\n        ");
    write(
        dir,
        &format!("stg_{i}"),
        &format!(
            "{{{{ config(materialized='view', tags=['staging']) }}}}\n\nwith source as (\n    select * from {{{{ source('raw', 't_{i}') }}}}\n),\nrenamed as (\n    select\n        id,\n        customer_id,\n        {list},\n        {{{{ dbt_utils.generate_surrogate_key(['id', 'customer_id']) }}}} as sk,\n        cast(loaded_at as date) as load_date\n    from source\n    where deleted_at is null\n)\nselect * from renamed\n"
        ),
    );
}

/// A mart in the usual shape: config, a var, macro calls, several CTEs
/// and joins over `ref`s. No control flow, so the template route plans it.
fn jinja_mart(dir: &Path) -> String {
    let aggregates: Vec<String> = (0..15)
        .map(|c| format!("sum(col_{c}) as total_{c}"))
        .collect();
    let sql = format!(
        "{{{{ config(materialized='incremental', unique_key='customer_id', tags=['mart', 'finance']) }}}}\n\n{{# customers joined to the last payment, one row each #}}\nwith orders as (\n    select * from {{{{ ref('stg_0') }}}} where load_date >= '{{{{ var(\"start\", \"2020-01-01\") }}}}'\n),\npayments as (\n    select * from {{{{ ref('stg_1') }}}}\n),\nagg as (\n    select\n        customer_id,\n        {aggs},\n        count(*) as n_orders\n    from orders\n    group by customer_id\n),\nlatest as (\n    select customer_id, col_3, row_number() over (partition by customer_id order by load_date desc) as rn\n    from payments\n    qualify rn = 1\n),\njoined as (\n    select\n        agg.*,\n        latest.col_3 as last_amount,\n        {{{{ my_macro('agg.customer_id') }}}} as derived\n    from agg\n    left join latest on agg.customer_id = latest.customer_id\n    where agg.total_0 > 0\n)\nselect * from joined\n",
        aggs = aggregates.join(",\n        ")
    );
    write(dir, "mart_jinja", &sql);
    sql
}

/// A `{% for %}` over methods: planned from `target/compiled`.
fn control_flow_mart(dir: &Path) -> (String, String) {
    let methods = ["card", "voucher", "cash", "transfer", "wallet", "invoice"];
    let list = methods
        .iter()
        .map(|m| format!("'{m}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let template = format!(
        "{{{{ config(materialized='table') }}}}\n{{% set methods = [{list}] %}}\n\nwith payments as (\n    select * from {{{{ ref('stg_2') }}}}\n),\npivoted as (\n    select\n        order_id,\n        {{% for m in methods %}}\n        sum(case when method = '{{{{ m }}}}' then amount else 0 end) as {{{{ m }}}}_amount,\n        {{% endfor %}}\n        sum(amount) as total\n    from payments\n    group by order_id\n)\nselect * from pivoted\n{{% if is_incremental() %}}\nwhere order_id > (select max(order_id) from {{{{ this }}}})\n{{% endif %}}\n"
    );
    let arms: String = methods
        .iter()
        .map(|m| {
            format!(
                "\n        sum(case when method = '{m}' then amount else 0 end) as {m}_amount,\n        "
            )
        })
        .collect();
    let compiled = format!(
        "\n\n\nwith payments as (\n    select * from \"db\".\"main\".\"stg_2\"\n),\npivoted as (\n    select\n        order_id,\n        {arms}\n        sum(amount) as total\n    from payments\n    group by order_id\n)\nselect * from pivoted\n\n\n"
    );
    write(dir, "mart_control", &template);
    (template, compiled)
}

/// Eight CTEs over `ref`s and a 200-column final select.
fn large_mart(dir: &Path) -> String {
    let columns = staging_columns();
    let mut sql = String::from("{{ config(materialized='table') }}\n\nwith ");
    for i in 0..STAGING.min(8) {
        let list = columns.join(", ");
        sql.push_str(&format!(
            "c{i} as (\n    select id, customer_id, {list}\n    from {{{{ ref('stg_{i}') }}}}\n    where col_0 > {i}\n),\n"
        ));
    }
    sql.push_str("wide as (\n    select\n        c0.id,\n        c0.customer_id");
    for i in 0..8usize {
        for (n, c) in columns.iter().enumerate().take(25) {
            let expr = match n % 5 {
                0 => format!("coalesce(c{i}.{c}, 0)"),
                1 => format!("case when c{i}.{c} > 10 then 'high' else 'low' end"),
                2 => format!("cast(c{i}.{c} as varchar)"),
                3 => format!("c{i}.{c} + 1"),
                _ => format!("c{i}.{c}"),
            };
            sql.push_str(&format!(",\n        {expr} as c{i}_{c}"));
        }
    }
    sql.push_str("\n    from c0\n");
    for i in 1..8 {
        sql.push_str(&format!(
            "    left join c{i} on c0.id = c{i}.id and c0.customer_id = c{i}.customer_id\n"
        ));
    }
    sql.push_str(")\nselect * from wide where c0_col_0 is not null\n");
    write(dir, "mart_large", &sql);
    sql
}

struct Disk;

impl SavedFiles for Disk {
    fn stamp(&self, path: &Path) -> Option<u64> {
        let meta = std::fs::metadata(path).ok()?;
        let modified = meta.modified().ok()?;
        let nanos = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
        Some(nanos.as_nanos() as u64 ^ meta.len())
    }

    fn read(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
}

fn snapshot(ctx: &DbtContext) -> Arc<SchemaSnapshot> {
    let never = || false;
    let input = JobInput {
        dialect: &ctx.dialect,
        models: &ctx.model_detail_map,
        sources: &ctx.source_detail_map,
        graph: &ctx.graph,
        manifest: None,
        catalog: None,
        files: &Disk,
        limits: JobLimits::default(),
        stale: &never,
    };
    run_schema_job(&input, &mut JobState::default()).expect("job")
}

fn percentile(samples: &mut [f64], p: f64) -> f64 {
    samples.sort_by(|a, b| a.total_cmp(b));
    samples[((samples.len() - 1) as f64 * p) as usize]
}

fn time(mut sample: impl FnMut()) -> (f64, f64, f64) {
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        sample();
        samples.push(started.elapsed().as_secs_f64() * 1e3);
    }
    let p50 = percentile(&mut samples, 0.5);
    let p95 = percentile(&mut samples, 0.95);
    (p50, p95, samples[samples.len() - 1])
}

fn report(name: &str, text: &str, input: &PlanInput<'_>) {
    let (p50, p95, max) = time(|| {
        std::hint::black_box(query_plan(input));
    });
    let plan = query_plan(input);
    println!(
        "{name}: {} bytes, {} findings; miss p50 {p50:.2} ms, p95 {p95:.2} ms, max {max:.2} ms (budget p95 15 ms)",
        text.len(),
        plan.findings.len(),
    );
    cached(name, text, plan);
}

/// What a request does on a plan-cache hit: the probe the push path runs,
/// then turning the cached plan into diagnostics.
fn cached(name: &str, text: &str, plan: QueryPlan) {
    let suppressions = Suppressions::default();
    let settings = PerfSettings {
        profile: PerfProfile::Recommended,
        catalog_stats: true,
        rules: BTreeMap::new(),
    };
    let mut map: HashMap<String, ((u64, u64), Arc<QueryPlan>)> = HashMap::new();
    map.insert("file:///m.sql".to_string(), ((1, 1), Arc::new(plan)));
    let (p50, p95, _) = time(|| {
        let (key, plan) = map.get("file:///m.sql").expect("cached");
        assert_eq!(*key, (1, 1));
        std::hint::black_box(plan_diagnostics(text, plan, &suppressions, &settings));
    });
    println!("{name}: cached probe + diagnostics p50 {p50:.4} ms, p95 {p95:.4} ms (budget 1 ms)");
}

#[test]
#[ignore = "a bench: run with --ignored --nocapture in release"]
fn realistic_plan_budgets() {
    let root = std::env::temp_dir().join(format!("cogitator_realistic_{}", std::process::id()));
    let dir = root.join("models");
    std::fs::create_dir_all(&dir).expect("temp project");
    std::fs::write(
        root.join("dbt_project.yml"),
        "name: bench\nversion: '1.0'\nprofile: p\nmodel-paths: ['models']\n",
    )
    .expect("project file");
    let project = Project { root };
    for i in 0..STAGING {
        staging(&dir, i);
    }
    let jinja = jinja_mart(&dir);
    let (control, compiled) = control_flow_mart(&dir);
    let large = large_mart(&dir);

    let ctx = build_context(&project.root.to_string_lossy());
    let snapshot = snapshot(&ctx);
    let artifacts = ArtifactSet::default();
    let input = |text, model| {
        PlanInput::from_context(&ctx, text, Some(model), &artifacts, snapshot.revision)
            .with_project(Some(&snapshot))
    };

    report("mart_jinja", &jinja, &input(jinja.as_str(), "mart_jinja"));
    let compiled_input =
        input(control.as_str(), "mart_control").with_compiled(CompiledInput::Available {
            text: &compiled,
            path: "target/compiled/bench/models/mart_control.sql",
            mtime: Staleness::Fresh,
        });
    let compiled_plan = query_plan(&compiled_input);
    println!(
        "mart_control: origin {:?}, banner {:?}",
        compiled_plan.origin.is_some(),
        compiled_plan.banner
    );
    report("mart_control (compiled route)", &control, &compiled_input);
    report("mart_large", &large, &input(large.as_str(), "mart_large"));
}

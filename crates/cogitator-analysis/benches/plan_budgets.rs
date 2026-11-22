//! Measures the project-schema job and one full plan against the budgets
//! in the pipeline plan (full plan p95 15 ms on a cache miss, schema job
//! about 2 ms per model). A plain binary rather than criterion, because the
//! budget is a percentile over one fixed-size run. `just bench`.
//!
//! Usage: `cargo bench -p cogitator-analysis --bench plan_budgets [models]`

use std::path::{Path, PathBuf};
use std::time::Instant;

use cogitator_analysis::artifacts::ArtifactSet;
use cogitator_analysis::features::plan::schemas::{
    run_schema_job, JobInput, JobLimits, JobState, SavedFiles,
};
use cogitator_analysis::features::plan::{query_plan, PlanInput};
use cogitator_analysis::{build_context, DbtContext};
use cogitator_ir::schema::Completeness;

const COLUMNS: usize = 30;

struct Project {
    root: PathBuf,
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// `models` models in three layers, each stg wide and starred through.
fn generate(models: usize) -> Project {
    let root = std::env::temp_dir().join(format!("cogitator_bench_{}", std::process::id()));
    let dir = root.join("models");
    std::fs::create_dir_all(&dir).expect("temp project");
    std::fs::write(
        root.join("dbt_project.yml"),
        "name: bench\nversion: '1.0'\nprofile: p\nmodel-paths: ['models']\n",
    )
    .expect("project file");
    let (stg, rest) = (models / 3, models - models / 3);
    for i in 0..stg.max(1) {
        write_stg(&dir, i);
    }
    for i in 0..rest / 2 {
        write_mart(&dir, i, stg.max(1));
    }
    for i in 0..(rest - rest / 2) {
        write_report(&dir, i, (rest / 2).max(1));
    }
    // a reader so `report_0`'s star has downstream demand to compare with
    std::fs::write(
        dir.join("final_0.sql"),
        "select id, col_0 from {{ ref('report_0') }}\n",
    )
    .expect("model");
    Project { root }
}

fn columns() -> Vec<String> {
    (0..COLUMNS).map(|c| format!("col_{c}")).collect()
}

fn write_stg(dir: &Path, i: usize) {
    let list = columns().join(",\n        ");
    let sql = format!(
        "with source as (\n    select * from {{{{ source('raw', 't_{i}') }}}}\n),\nrenamed as (\n    select\n        id,\n        {list}\n    from source\n)\nselect * from renamed\n"
    );
    std::fs::write(dir.join(format!("stg_{i}.sql")), sql).expect("model");
}

fn write_mart(dir: &Path, i: usize, stg: usize) {
    let a = i % stg;
    let b = (i + 1) % stg;
    let list: Vec<String> = columns().iter().map(|c| format!("a.{c}")).collect();
    let list = list.join(",\n    ");
    let sql = format!(
        "{{{{ config(materialized='table') }}}}\nselect\n    a.id,\n    {list},\n    b.col_0 as other_col,\n    sum(b.col_1) over (partition by a.id) as total\nfrom {{{{ ref('stg_{a}') }}}} as a\nleft join {{{{ ref('stg_{b}') }}}} as b on a.id = b.id\nwhere a.col_2 > 0\n"
    );
    std::fs::write(dir.join(format!("mart_{i}.sql")), sql).expect("model");
}

fn write_report(dir: &Path, i: usize, marts: usize) {
    let mart = i % marts;
    let sql = format!(
        "{{{{ config(materialized='table') }}}}\nselect * from {{{{ ref('mart_{mart}') }}}} where id > {i}\n"
    );
    std::fs::write(dir.join(format!("report_{i}.sql")), sql).expect("model");
}

struct Disk;

impl SavedFiles for Disk {
    fn stamp(&self, path: &Path) -> Option<u64> {
        let meta = std::fs::metadata(path).ok()?;
        let modified = meta
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?;
        Some(modified.as_nanos() as u64 ^ meta.len())
    }

    fn read(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
}

fn job_input<'a>(ctx: &'a DbtContext, stale: &'a dyn Fn() -> bool) -> JobInput<'a> {
    JobInput {
        dialect: &ctx.dialect,
        models: &ctx.model_detail_map,
        sources: &ctx.source_detail_map,
        graph: &ctx.graph,
        manifest: None,
        catalog: None,
        files: &Disk,
        limits: JobLimits::default(),
        stale,
    }
}

fn percentile(samples: &mut [f64], p: f64) -> f64 {
    samples.sort_by(|a, b| a.total_cmp(b));
    samples[((samples.len() - 1) as f64 * p) as usize]
}

fn main() {
    let models: usize = std::env::args()
        .filter_map(|a| a.parse().ok())
        .next()
        .unwrap_or(3000);
    let project = generate(models);
    let root = project.root.to_string_lossy().into_owned();
    let start = Instant::now();
    let ctx = build_context(&root);
    let actual = ctx.model_detail_map.len();
    println!(
        "project: {actual} models, context build {:.0} ms",
        start.elapsed().as_secs_f64() * 1e3
    );

    let never = || false;
    let mut cold = Vec::new();
    let mut state = JobState::default();
    for _ in 0..3 {
        state = JobState::default();
        let started = Instant::now();
        let snapshot = run_schema_job(&job_input(&ctx, &never), &mut state).expect("job");
        let ms = started.elapsed().as_secs_f64() * 1e3;
        cold.push(ms);
        assert_eq!(
            snapshot.report.rebuilt, actual,
            "cold run rebuilds everything"
        );
    }
    let cold_ms = percentile(&mut cold, 0.5);
    println!(
        "schema job (cold): {cold_ms:.0} ms total, {:.3} ms/model (budget about 2 ms/model)",
        cold_ms / actual as f64
    );

    let mut warm = Vec::new();
    for _ in 0..5 {
        let started = Instant::now();
        let snapshot = run_schema_job(&job_input(&ctx, &never), &mut state).expect("job");
        warm.push(started.elapsed().as_secs_f64() * 1e3);
        assert_eq!(snapshot.report.reused, actual, "warm run reuses everything");
    }
    println!(
        "schema job (warm, nothing changed): {:.1} ms total",
        percentile(&mut warm, 0.5)
    );

    let snapshot = run_schema_job(&job_input(&ctx, &never), &mut state).expect("job");
    let closed = snapshot
        .schemas
        .models
        .values()
        .filter(|s| s.completeness == Completeness::Closed)
        .count();
    println!(
        "schemas: {closed} of {} models Closed; {} degraded",
        snapshot.schemas.models.len(),
        snapshot.report.degraded.len()
    );
    let artifacts = ArtifactSet::default();
    for model in ["mart_0", "report_0"] {
        let text = std::fs::read_to_string(project.root.join(format!("models/{model}.sql")))
            .expect("model");
        let mut samples = Vec::with_capacity(300);
        let mut findings = 0;
        for _ in 0..300 {
            let input =
                PlanInput::from_context(&ctx, &text, Some(model), &artifacts, snapshot.revision)
                    .with_project(Some(&snapshot));
            let started = Instant::now();
            let plan = query_plan(&input);
            samples.push(started.elapsed().as_secs_f64() * 1e3);
            findings = plan.findings.len();
        }
        let p50 = percentile(&mut samples, 0.5);
        let p95 = percentile(&mut samples, 0.95);
        println!(
            "one full plan ({model}, cache miss): p50 {p50:.2} ms, p95 {p95:.2} ms over {} samples ({findings} findings; budget p95 15 ms)",
            samples.len()
        );
    }
}

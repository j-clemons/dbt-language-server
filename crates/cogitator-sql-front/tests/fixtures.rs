//! Snapshots of the HIR for every jaffle_shop SQL file, plus `assert_clean`
//! on each (specs/ir-pipeline/05: every lowering verifies).

mod common;

use std::path::{Path, PathBuf};

use cogitator_ir::origin::TemplateVariant;
use cogitator_sql_front::SchemaEnv;

use common::{build_in, report};

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/jaffle_shop_duckdb")
}

fn sql_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            !name.starts_with('.') && name != "dbt_packages" && name != "target"
        })
        .collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            sql_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "sql") {
            out.push(path);
        }
    }
}

fn snapshot_name(path: &Path) -> String {
    let rel = path
        .strip_prefix(fixture_root())
        .unwrap_or(path)
        .with_extension("");
    rel.to_string_lossy().replace(['/', '\\'], "__")
}

#[test]
fn every_fixture_builds_clean_and_is_pinned() {
    let mut files = Vec::new();
    for dir in ["models", "snapshots", "tests", "analysis"] {
        sql_files(&fixture_root().join(dir), &mut files);
    }
    assert!(files.len() >= 9, "fixtures not found: {files:?}");
    for path in files {
        // Windows checkouts convert fixtures to CRLF; the pinned spans are LF.
        let text = std::fs::read_to_string(&path)
            .expect("read fixture")
            .replace("\r\n", "\n");
        let lowered = build_in(
            &text,
            "duckdb",
            TemplateVariant::Plain,
            &SchemaEnv::default(),
        );
        insta::assert_snapshot!(snapshot_name(&path), report(&text, &lowered));
    }
}

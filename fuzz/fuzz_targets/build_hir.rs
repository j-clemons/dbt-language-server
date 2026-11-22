//! The HIR front end must never panic or produce a plan its verifier
//! rejects, whatever text it is handed, in any dialect or variant.

#![no_main]

use libfuzzer_sys::fuzz_target;

use cogitator_dialects::Dialect;
use cogitator_ir::common::ModelConfig;
use cogitator_ir::origin::{SourceOrigin, TemplateVariant};
use cogitator_sql_front::{build_hir, parse_sql_type, HirInput, SchemaEnv};

const DIALECTS: [&str; 5] = ["duckdb", "snowflake", "bigquery", "postgres", ""];
const VARIANTS: [TemplateVariant; 3] = [
    TemplateVariant::Plain,
    TemplateVariant::Incremental,
    TemplateVariant::FullRefresh,
];

fuzz_target!(|input: (u8, &str)| {
    let (selector, text) = input;
    let dialect = Dialect::new(DIALECTS[usize::from(selector) % DIALECTS.len()]);
    let variant = VARIANTS[usize::from(selector >> 4) % VARIANTS.len()];
    let hir_input = HirInput {
        text,
        dialect: &dialect,
        origin: SourceOrigin::Template { variant },
        config: ModelConfig::default(),
    };
    build_hir(&hir_input, &SchemaEnv::default()).assert_clean();
    let _ = parse_sql_type(text, &dialect);
});

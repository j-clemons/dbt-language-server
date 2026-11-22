use super::*;

fn run(text: &str) -> Scanned {
    scan(text, TemplateVariant::Plain).expect("scan")
}

#[test]
fn mask_is_same_length_and_keeps_newlines() {
    let text = "select {{\n ref('é')\n}} from t -- x\n";
    let s = run(text);
    assert_eq!(s.mask.len(), text.len());
    assert_eq!(
        s.mask.as_bytes().iter().filter(|b| **b == b'\n').count(),
        text.as_bytes().iter().filter(|b| **b == b'\n').count()
    );
    assert!(s.mask.contains("from t"));
    assert!(s.mask.starts_with("select _j\n"));
}

#[test]
fn regions_are_sorted_with_kinds() {
    let s = run("{# c #}select {{ var('a') }}, {{ m() }} {{ config(x=1) }} from {{ ref('b') }}");
    assert_eq!(s.regions[0].span, Span::new(0, 7));
    let kinds: Vec<_> = s.regions.iter().map(|r| r.kind.clone()).collect();
    assert_eq!(
        kinds,
        vec![
            RegionKind::Comment,
            RegionKind::Var,
            RegionKind::Macro("m".into()),
            RegionKind::Config,
            RegionKind::Ref,
        ]
    );
    assert_eq!(s.configs.len(), 1);
}

#[test]
fn incremental_variants() {
    let text = "select * from t {% if is_incremental() %}where a > 1{% endif %}";
    let inc = scan(text, TemplateVariant::Incremental).expect("scan");
    assert!(inc.mask.contains("where a > 1"));
    assert_eq!(inc.variant, TemplateVariant::Incremental);
    let full = scan(text, TemplateVariant::FullRefresh).expect("scan");
    assert!(!full.mask.contains("where"));
    // Plain on an incremental model behaves as Incremental
    let plain = scan(text, TemplateVariant::Plain).expect("scan");
    assert!(plain.mask.contains("where a > 1"));
    assert_eq!(plain.variant, TemplateVariant::Incremental);
}

#[test]
fn other_control_flow_stops() {
    for text in [
        "{% for x in y %}{{ x }}{% endfor %}",
        "{% if x %}a{% endif %}",
        "{% if is_incremental() %}a{% else %}b{% endif %}",
        "{% if is_incremental() %}{% if is_incremental() %}a{% endif %}{% endif %}",
        "{% macro m() %}x{% endmacro %}",
    ] {
        let stopped = matches!(
            scan(text, TemplateVariant::Plain),
            Err(Stop::ControlFlow { .. })
        );
        assert!(stopped, "{text}");
    }
}

#[test]
fn set_blocks_blank_their_body() {
    let s = run("{% set a %}select 'x'{% endset %}select 1");
    assert!(!s.mask.contains("'x'"));
    assert!(s.mask.contains("select 1"));
}

#[test]
fn inline_set_is_blanked_but_allowed() {
    let s = run("{% set p = ['a'] %}select 1");
    assert!(s.mask.trim_start().starts_with("select 1"));
}

#[test]
fn malformed_jinja_is_reported() {
    assert!(matches!(
        scan("select {{ a", TemplateVariant::Plain),
        Err(Stop::Malformed { .. })
    ));
}

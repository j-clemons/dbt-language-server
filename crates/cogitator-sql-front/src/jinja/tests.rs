use super::*;

#[test]
fn ref_variants() {
    let model = |inner: &str| classify(inner).class;
    assert_eq!(
        model("ref('a')"),
        Class::Relation(RelInfo::Model {
            package: None,
            name: "a".into(),
            version: None
        })
    );
    assert_eq!(
        model("ref('p', 'a', v=2)"),
        Class::Relation(RelInfo::Model {
            package: Some("p".into()),
            name: "a".into(),
            version: Some("2".into())
        })
    );
    assert_eq!(model("ref(var('x'))"), Class::Relation(RelInfo::Dynamic));
    assert_eq!(
        model("source('s', 't')"),
        Class::Relation(RelInfo::Source {
            source: "s".into(),
            table: "t".into()
        })
    );
}

#[test]
fn other_expressions() {
    assert_eq!(classify("var('x')").class, Class::Scalar);
    assert_eq!(classify("var('x')").tag, ExprKindTag::Var);
    assert_eq!(classify("config(a=1)").class, Class::Blank);
    assert_eq!(classify("this").class, Class::Relation(RelInfo::This));
    assert_eq!(
        classify("dbt_utils.star(from=x)").class,
        Class::Macro("dbt_utils.star".into())
    );
    assert_eq!(classify("target.name").class, Class::Scalar);
    assert_eq!(classify("!!!").class, Class::Macro(String::new()));
}

#[test]
fn statement_tags() {
    assert_eq!(classify_tag(" if is_incremental() "), Tag::IfIncremental);
    assert_eq!(
        classify_tag(" if not is_incremental() "),
        Tag::Other("if".into())
    );
    assert_eq!(classify_tag(" if x "), Tag::Other("if".into()));
    assert_eq!(classify_tag(" set a = [1,2] "), Tag::SetInline);
    assert_eq!(classify_tag(" set a "), Tag::SetBlock);
    assert_eq!(classify_tag(" for x in y "), Tag::Other("for".into()));
    assert_eq!(classify_tag(" do x.append(1) "), Tag::Do);
}

#[test]
fn config_constants() {
    let kw = config_kwargs("config(materialized='table', unique_key=['a','b'], x=var('y'))");
    assert_eq!(kw[0], ("materialized".into(), Raw::Str("table".into())));
    assert_eq!(
        kw[1].1,
        Raw::List(vec![Raw::Str("a".into()), Raw::Str("b".into())])
    );
    assert_eq!(kw[2].1, Raw::Other);
}

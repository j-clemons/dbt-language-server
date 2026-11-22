//! Everything that touches `minijinja::machinery`, an unstable API: if it
//! changes, this module is the only one to rewrite. Classifies expression
//! and statement regions and reads the constant parts of `config()`.

use minijinja::machinery::ast::{CallArg, Expr};
use minijinja::machinery::{parse_expr, tokenize, Token};
use minijinja::syntax::SyntaxConfig;

/// What a `ref`/`source`-shaped expression names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RelInfo {
    Model {
        package: Option<String>,
        name: String,
        version: Option<String>,
    },
    Source {
        source: String,
        table: String,
    },
    This,
    /// Relation-shaped but not statically known (`ref(var('x'))`).
    Dynamic,
}

/// How one `{{ … }}` participates in the SQL, before position is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Class {
    Relation(RelInfo),
    Scalar,
    Macro(String),
    /// Renders to nothing (`config`, `log`, …).
    Blank,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExprKindTag {
    Var,
    Config,
    IsIncremental,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Classified {
    pub class: Class,
    pub tag: ExprKindTag,
}

/// Classifies the text between `{{` and `}}`. Unparsable expressions are
/// a macro of no name: position still decides how they are lowered.
pub(crate) fn classify(inner: &str) -> Classified {
    match parse_expr(inner) {
        Ok(expr) => classify_expr(&expr),
        Err(_) => Classified {
            class: Class::Macro(String::new()),
            tag: ExprKindTag::Other,
        },
    }
}

fn classify_expr(expr: &Expr<'_>) -> Classified {
    let scalar = |tag| Classified {
        class: Class::Scalar,
        tag,
    };
    match expr {
        Expr::Var(var) if var.id == "this" => Classified {
            class: Class::Relation(RelInfo::This),
            tag: ExprKindTag::Other,
        },
        Expr::Call(call) => match callee_name(&call.expr).as_deref() {
            Some("ref") => relation(rel_model(&call.args)),
            Some("source") => relation(rel_source(&call.args)),
            Some("config") => Classified {
                class: Class::Blank,
                tag: ExprKindTag::Config,
            },
            Some("var" | "env_var") => scalar(ExprKindTag::Var),
            Some("is_incremental") => scalar(ExprKindTag::IsIncremental),
            Some("log" | "print") => Classified {
                class: Class::Blank,
                tag: ExprKindTag::Other,
            },
            Some(name) if name.starts_with("exceptions.") => Classified {
                class: Class::Blank,
                tag: ExprKindTag::Other,
            },
            Some(name) => Classified {
                class: Class::Macro(name.to_string()),
                tag: ExprKindTag::Other,
            },
            None => Classified {
                class: Class::Macro(String::new()),
                tag: ExprKindTag::Other,
            },
        },
        _ => scalar(ExprKindTag::Other),
    }
}

fn relation(info: RelInfo) -> Classified {
    Classified {
        class: Class::Relation(info),
        tag: ExprKindTag::Other,
    }
}

/// `f` or `pkg.f`; `None` for a computed callee.
fn callee_name(expr: &Expr<'_>) -> Option<String> {
    match expr {
        Expr::Var(var) => Some(var.id.to_string()),
        Expr::GetAttr(attr) => callee_name(&attr.expr).map(|base| format!("{base}.{}", attr.name)),
        _ => None,
    }
}

fn const_str(expr: &Expr<'_>) -> Option<String> {
    match expr {
        Expr::Const(c) => c.value.as_str().map(str::to_string),
        _ => None,
    }
}

fn const_version(expr: &Expr<'_>) -> Option<String> {
    match expr {
        Expr::Const(c) if !c.value.is_undefined() && !c.value.is_none() => {
            Some(c.value.to_string())
        }
        _ => None,
    }
}

fn rel_model(args: &[CallArg<'_>]) -> RelInfo {
    let (mut positional, mut version) = (Vec::new(), None);
    for arg in args {
        match arg {
            CallArg::Pos(e) => match const_str(e) {
                Some(s) => positional.push(s),
                None => return RelInfo::Dynamic,
            },
            CallArg::Kwarg("v" | "version", e) => version = const_version(e),
            _ => return RelInfo::Dynamic,
        }
    }
    let mut parts = positional.into_iter();
    match (parts.next(), parts.next(), parts.next()) {
        (Some(name), None, None) => RelInfo::Model {
            package: None,
            name,
            version,
        },
        (Some(package), Some(name), None) => RelInfo::Model {
            package: Some(package),
            name,
            version,
        },
        _ => RelInfo::Dynamic,
    }
}

fn rel_source(args: &[CallArg<'_>]) -> RelInfo {
    let mut parts = args.iter().map(|arg| match arg {
        CallArg::Pos(e) => const_str(e),
        _ => None,
    });
    match (parts.next(), parts.next(), parts.next()) {
        (Some(Some(source)), Some(Some(table)), None) => RelInfo::Source { source, table },
        _ => RelInfo::Dynamic,
    }
}

/// A statement tag, by name and the shape of its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Tag {
    IfIncremental,
    EndIf,
    SetInline,
    SetBlock,
    EndSet,
    Do,
    /// Any other control flow or dbt block; the name is for the report.
    Other(String),
}

/// Classifies the text between `{%` and `%}` from minijinja's own tokens.
pub(crate) fn classify_tag(inner: &str) -> Tag {
    let mut stream = tokenize(inner, true, SyntaxConfig::default());
    let (name, rest_at) = match stream.next() {
        Some(Ok((Token::Ident(name), span))) => (name, span.end_offset as usize),
        _ => return Tag::Other(String::new()),
    };
    match name {
        "if" => match parse_expr(inner.get(rest_at..).unwrap_or("")) {
            Ok(expr) if is_incremental_call(&expr) => Tag::IfIncremental,
            _ => Tag::Other("if".to_string()),
        },
        "endif" => Tag::EndIf,
        "set" => {
            if has_top_level_assign(stream) {
                Tag::SetInline
            } else {
                Tag::SetBlock
            }
        }
        "endset" => Tag::EndSet,
        "do" => Tag::Do,
        other => Tag::Other(other.to_string()),
    }
}

/// `is_incremental()` with no arguments, nothing around it.
fn is_incremental_call(expr: &Expr<'_>) -> bool {
    matches!(expr, Expr::Call(call)
        if call.args.is_empty() && matches!(&call.expr, Expr::Var(v) if v.id == "is_incremental"))
}

fn has_top_level_assign<'a>(
    stream: impl Iterator<Item = Result<(Token<'a>, minijinja::machinery::Span), minijinja::Error>>,
) -> bool {
    let mut depth = 0i32;
    for item in stream {
        let Ok((token, _)) = item else { return false };
        match token {
            Token::ParenOpen | Token::BracketOpen | Token::BraceOpen => depth += 1,
            Token::ParenClose | Token::BracketClose | Token::BraceClose => depth -= 1,
            Token::Assign if depth <= 0 => return true,
            _ => {}
        }
    }
    false
}

/// A constant Jinja value in a config argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Raw {
    Str(String),
    Bool(bool),
    Other,
    List(Vec<Raw>),
    Map(Vec<(String, Raw)>),
}

/// `(keyword, constant)` pairs of a `config(...)` expression, in source
/// order. Non-constant values come back as [`Raw::Other`].
pub(crate) fn config_kwargs(inner: &str) -> Vec<(String, Raw)> {
    let Ok(Expr::Call(call)) = parse_expr(inner) else {
        return Vec::new();
    };
    call.args
        .iter()
        .filter_map(|arg| match arg {
            CallArg::Kwarg(name, e) => Some((name.to_string(), to_raw(e, 0))),
            _ => None,
        })
        .collect()
}

fn to_raw(expr: &Expr<'_>, depth: usize) -> Raw {
    if depth > 16 {
        return Raw::Other;
    }
    match expr {
        Expr::Const(c) => c
            .value
            .as_str()
            .map(|s| Raw::Str(s.to_string()))
            .or_else(|| {
                if c.value.kind() == minijinja::value::ValueKind::Bool {
                    Some(Raw::Bool(c.value.is_true()))
                } else {
                    None
                }
            })
            .unwrap_or(Raw::Other),
        Expr::List(list) => Raw::List(list.items.iter().map(|i| to_raw(i, depth + 1)).collect()),
        Expr::Map(map) => Raw::Map(
            map.keys
                .iter()
                .zip(map.values.iter())
                .filter_map(|(k, v)| Some((const_str(k)?, to_raw(v, depth + 1))))
                .collect(),
        ),
        _ => Raw::Other,
    }
}

#[cfg(test)]
mod tests;

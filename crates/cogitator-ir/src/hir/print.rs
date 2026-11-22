//! Deterministic text form of a HIR: for snapshots and the plan view.
//! Total over unverified input — bad ids print as `<bad>`.

use crate::common::Lit;
use crate::hir::{
    Body, ExprId, ExprKind, FromId, FromKind, GroupBy, Hir, JoinCond, QueryId, RelTarget,
    SelectItem,
};
use crate::Span;

/// Caps the walk on pathological input; the verifier reports the cause.
const MAX_PRINT_DEPTH: usize = 256;

pub fn print_hir(hir: &Hir) -> String {
    let mut out = String::new();
    match hir.root {
        Some(root) => query(hir, root, 0, &mut out),
        None => out.push_str("<no root>\n"),
    }
    out
}

fn line(depth: usize, text: &str, out: &mut String) {
    out.push_str(&"  ".repeat(depth));
    out.push_str(text);
    out.push('\n');
}

fn span_text(span: Span) -> String {
    format!("[{}..{}]", span.start, span.end)
}

fn query(hir: &Hir, id: QueryId, depth: usize, out: &mut String) {
    if depth > MAX_PRINT_DEPTH {
        line(depth, "...", out);
        return;
    }
    let Some(q) = hir.queries.get(id) else {
        line(depth, "<bad query>", out);
        return;
    };
    line(depth, &format!("query {}", span_text(q.span)), out);
    for &cte_id in &q.ctes {
        match hir.ctes.get(cte_id) {
            Some(cte) => {
                let rec = if cte.recursive { " recursive" } else { "" };
                let name = hir.syms.text(cte.name.folded);
                line(depth + 1, &format!("cte {name}{rec}"), out);
                query(hir, cte.query, depth + 2, out);
            }
            None => line(depth + 1, "<bad cte>", out),
        }
    }
    match &q.body {
        Body::Select(s) => {
            line(depth + 1, &format!("select distinct={:?}", s.distinct), out);
            for item in &s.items {
                line(depth + 2, &select_item(hir, item), out);
            }
            for &f in &s.from {
                from(hir, f, depth + 2, out);
            }
            if let Some(e) = s.filter {
                line(depth + 2, &format!("where {}", expr(hir, e, 0)), out);
            }
            if !matches!(s.group_by, GroupBy::None) {
                line(depth + 2, &format!("group_by {:?}", s.group_by), out);
            }
            if let Some(e) = s.having {
                line(depth + 2, &format!("having {}", expr(hir, e, 0)), out);
            }
            if let Some(e) = s.qualify {
                line(depth + 2, &format!("qualify {}", expr(hir, e, 0)), out);
            }
        }
        Body::SetOp {
            op,
            all,
            by_name,
            l,
            r,
        } => {
            line(
                depth + 1,
                &format!("setop {op:?} all={all} by_name={by_name}"),
                out,
            );
            query(hir, *l, depth + 2, out);
            query(hir, *r, depth + 2, out);
        }
        Body::Values(rows) => line(depth + 1, &format!("values rows={}", rows.len()), out),
        Body::Hole(kind) => line(depth + 1, &format!("hole {kind:?}"), out),
    }
    for item in &q.order_by {
        line(
            depth + 1,
            &format!("order_by {}", expr(hir, item.expr, 0)),
            out,
        );
    }
    if let Some(e) = q.limit {
        line(depth + 1, &format!("limit {}", expr(hir, e, 0)), out);
    }
}

fn select_item(hir: &Hir, item: &SelectItem) -> String {
    match item {
        SelectItem::Expr { expr: e, alias } => {
            let text = expr(hir, *e, 0);
            match alias {
                Some(a) => format!("item {text} as {}", hir.syms.text(a.folded)),
                None => format!("item {text}"),
            }
        }
        SelectItem::Star {
            qualifier, exclude, ..
        } => {
            let qual = qualifier
                .map(|q| format!("{}.", hir.syms.text(q.folded)))
                .unwrap_or_default();
            format!("item {qual}* exclude={}", exclude.len())
        }
        SelectItem::Jinja(id) => format!("item jinja {:?}", id.raw()),
        SelectItem::Hole(span) => format!("item hole {}", span_text(*span)),
    }
}

fn from(hir: &Hir, id: FromId, depth: usize, out: &mut String) {
    if depth > MAX_PRINT_DEPTH {
        line(depth, "...", out);
        return;
    }
    let Some(item) = hir.from.get(id) else {
        line(depth, "<bad from>", out);
        return;
    };
    let alias_of = |a: &Option<crate::hir::Ident>| {
        a.map(|a| format!(" as {}", hir.syms.text(a.folded)))
            .unwrap_or_default()
    };
    match &item.kind {
        FromKind::Base { target, alias } => {
            let t = rel_target(hir, target);
            line(depth, &format!("from {t}{}", alias_of(alias)), out);
        }
        FromKind::Derived { q, lateral, alias } => {
            let lat = if *lateral { " lateral" } else { "" };
            line(depth, &format!("derived{lat}{}", alias_of(alias)), out);
            query(hir, *q, depth + 1, out);
        }
        FromKind::Join { kind, l, r, on } => {
            let cond = match on {
                JoinCond::None => "no-condition".to_string(),
                JoinCond::On(e) => format!("on {}", expr(hir, *e, 0)),
                JoinCond::Using(cols) => format!("using {}", cols.len()),
                JoinCond::Natural => "natural".to_string(),
            };
            line(depth, &format!("join {kind:?} {cond}"), out);
            from(hir, *l, depth + 1, out);
            from(hir, *r, depth + 1, out);
        }
        FromKind::Unnest { exprs, alias, .. } => {
            line(
                depth,
                &format!("unnest args={}{}", exprs.len(), alias_of(alias)),
                out,
            );
        }
        FromKind::TableFn { name, alias, .. } => {
            let path: Vec<&str> = name.iter().map(|n| hir.syms.text(n.folded)).collect();
            line(
                depth,
                &format!("tablefn {}{}", path.join("."), alias_of(alias)),
                out,
            );
        }
        FromKind::Hole => line(depth, "from hole", out),
    }
}

fn rel_target(hir: &Hir, target: &RelTarget) -> String {
    match target {
        RelTarget::Model {
            package,
            name,
            version,
        } => {
            let pkg = package
                .map(|p| format!("{}.", hir.syms.text(p)))
                .unwrap_or_default();
            let ver = version
                .map(|v| format!(" v{}", hir.syms.text(v)))
                .unwrap_or_default();
            format!("model {pkg}{}{ver}", hir.syms.text(*name))
        }
        RelTarget::Source { source, table } => {
            format!(
                "source {}.{}",
                hir.syms.text(*source),
                hir.syms.text(*table)
            )
        }
        RelTarget::This => "this".to_string(),
        RelTarget::Cte(c) => hir
            .ctes
            .get(*c)
            .map(|cte| format!("cte {}", hir.syms.text(cte.name.folded)))
            .unwrap_or_else(|| "<bad cte>".to_string()),
        RelTarget::Physical(path) => {
            let parts: Vec<&str> = path.iter().map(|n| hir.syms.text(n.folded)).collect();
            format!("physical {}", parts.join("."))
        }
        RelTarget::Jinja(j) => format!("jinja {}", j.raw()),
    }
}

fn expr(hir: &Hir, id: ExprId, depth: usize) -> String {
    if depth > MAX_PRINT_DEPTH {
        return "...".to_string();
    }
    let Some(e) = hir.exprs.get(id) else {
        return "<bad>".to_string();
    };
    let sub = |x: ExprId| expr(hir, x, depth + 1);
    let list = |xs: &[ExprId]| xs.iter().map(|x| sub(*x)).collect::<Vec<_>>().join(", ");
    match &e.kind {
        ExprKind::Col(c) => match hir.cols.get(*c) {
            Some(col) => hir.syms.text(col.name.folded).to_string(),
            None => "<bad col>".to_string(),
        },
        ExprKind::Lit(l) => lit(hir, l),
        ExprKind::Unary { op, e } => format!("{op:?}({})", sub(*e)),
        ExprKind::Binary { op, l, r } => format!("({} {op:?} {})", sub(*l), sub(*r)),
        ExprKind::Call { name, args, .. } => {
            format!("{}({})", hir.syms.text(*name), list(args))
        }
        ExprKind::Case { .. } => "case".to_string(),
        ExprKind::Cast { e, ty } => format!("cast({} as {ty:?})", sub(*e)),
        ExprKind::InList { e, list: xs, .. } => format!("{} in ({})", sub(*e), list(xs)),
        ExprKind::InQuery { e, .. } => format!("{} in <query>", sub(*e)),
        ExprKind::Between { e, lo, hi, .. } => {
            format!("{} between {} and {}", sub(*e), sub(*lo), sub(*hi))
        }
        ExprKind::Exists(_) => "exists <query>".to_string(),
        ExprKind::Scalar(_) => "<scalar query>".to_string(),
        ExprKind::Jinja(j) => format!("jinja#{}", j.raw()),
        ExprKind::Opaque { children } => format!("opaque({})", list(children)),
        ExprKind::Hole => "hole".to_string(),
    }
}

fn lit(hir: &Hir, l: &Lit) -> String {
    match l {
        Lit::Null => "null".to_string(),
        Lit::Bool(b) => b.to_string(),
        Lit::Number(s) => hir.syms.text(*s).to_string(),
        Lit::Str(s) => format!("'{}'", hir.syms.text(*s)),
        Lit::Typed { ty, value } => format!("{} '{}'", hir.syms.text(*ty), hir.syms.text(*value)),
        Lit::Interval(s) => format!("interval {}", hir.syms.text(*s)),
    }
}

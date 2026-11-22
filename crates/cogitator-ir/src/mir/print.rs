//! Deterministic MIR text and a stable plan hash.
//!
//! Relation and scalar ids never appear: they change on every append,
//! so a rewrite that lands on the same plan prints identically and
//! hashes equal — the canonicaliser's fixpoint test.

use crate::common::Lit;
use crate::mir::{ColId, Mir, RelId, RelOp, ScalarId, ScalarKind, TableId, TableRef};
use crate::print_util::{col_type, MAX_PRINT_DEPTH};

/// FNV-1a over the printed plan: stable across builds and platforms.
pub fn plan_hash(mir: &Mir) -> u64 {
    print_mir(mir).bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

pub fn print_mir(mir: &Mir) -> String {
    let mut out = String::new();
    rel(mir, mir.root, 0, &mut out);
    let reachable = mir.reachable();
    let mut printed = false;
    for (id, cte) in mir.ctes.iter() {
        if reachable.contains(&cte.body) {
            if !printed {
                out.push_str("-- ctes\n");
                printed = true;
            }
            let name = cte.name.map_or("<unnamed>", |n| mir.syms.text(n));
            out.push_str(&format!(
                "cte#{} {name} out=[{}]\n",
                id.raw(),
                cols(mir, &cte.out)
            ));
            rel(mir, cte.body, 1, &mut out);
        }
    }
    out
}

fn col(mir: &Mir, id: ColId) -> String {
    match mir.cols.get(id) {
        Some(def) => {
            let name = def.name.map_or("?", |n| mir.syms.text(n));
            format!("{name}#{}:{}", id.raw(), col_type(&def.ty))
        }
        None => format!("<bad col {}>", id.raw()),
    }
}

pub(crate) fn cols(mir: &Mir, ids: &[ColId]) -> String {
    ids.iter()
        .map(|c| col(mir, *c))
        .collect::<Vec<_>>()
        .join(", ")
}

fn rel(mir: &Mir, id: RelId, depth: usize, out: &mut String) {
    let pad = "  ".repeat(depth);
    if depth > MAX_PRINT_DEPTH {
        out.push_str(&format!("{pad}...\n"));
        return;
    }
    let Some(node) = mir.rels.get(id) else {
        out.push_str(&format!("{pad}<bad rel>\n"));
        return;
    };
    let scalars = |xs: &[ScalarId]| {
        xs.iter()
            .map(|s| scalar(mir, *s))
            .collect::<Vec<_>>()
            .join(" AND ")
    };
    let pairs = |xs: &[(ColId, ScalarId)]| {
        xs.iter()
            .map(|(c, s)| format!("{} := {}", col(mir, *c), scalar(mir, *s)))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let head = match &node.op {
        RelOp::Scan {
            table,
            cols: c,
            open,
        } => {
            let t = table_name(mir, *table);
            let suffix = if *open { " open" } else { "" };
            format!("Scan {t} [{}]{suffix}", cols(mir, c))
        }
        RelOp::Filter { conjuncts, .. } => format!("Filter {}", scalars(conjuncts)),
        RelOp::Project { items, .. } => format!("Project {}", pairs(items)),
        RelOp::Join { kind, on, .. } => format!("Join {kind:?} on {}", scalars(on)),
        RelOp::Aggregate {
            keys, aggs, sets, ..
        } => {
            let a = aggs
                .iter()
                .map(|(c, call)| {
                    let args = call
                        .args
                        .iter()
                        .map(|s| scalar(mir, *s))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{} := {}({args})", col(mir, *c), mir.syms.text(call.func))
                })
                .collect::<Vec<_>>()
                .join(", ");
            let sets = sets.as_ref().map_or("", |_| " grouping_sets");
            format!("Aggregate keys=[{}] aggs=[{a}]{sets}", pairs(keys))
        }
        RelOp::Window { funcs, .. } => {
            let f: Vec<String> = funcs
                .iter()
                .map(|(c, w)| format!("{} := {}", col(mir, *c), mir.syms.text(w.func)))
                .collect();
            format!("Window {}", f.join(", "))
        }
        RelOp::Distinct(_) => "Distinct".to_string(),
        RelOp::SetOp {
            op, all, out: o, ..
        } => format!("SetOp {op:?} all={all} [{}]", cols(mir, o)),
        RelOp::Sort { keys, .. } => {
            let k: Vec<String> = keys.iter().map(|k| scalar(mir, k.expr)).collect();
            format!("Sort {}", k.join(", "))
        }
        RelOp::Limit { limit, offset, .. } => format!(
            "Limit {} offset {}",
            limit.map_or("none".to_string(), |s| scalar(mir, s)),
            offset.map_or("none".to_string(), |s| scalar(mir, s)),
        ),
        RelOp::Unnest { exprs, out: o, .. } => {
            format!("Unnest {} -> [{}]", scalars(exprs), cols(mir, o))
        }
        RelOp::CteRef { cte, out: o } => format!("CteRef #{} [{}]", cte.raw(), cols(mir, o)),
        RelOp::Opaque {
            out: o, open, hole, ..
        } => format!("Opaque {:?} open={open} [{}]", hole.kind, cols(mir, o)),
    };
    out.push_str(&format!("{pad}{head}\n"));
    for input in node.op.inputs() {
        rel(mir, input, depth + 1, out);
    }
}

pub(crate) fn table_name(mir: &Mir, id: TableId) -> String {
    let Some(table) = mir.tables.get(id) else {
        return "<bad table>".to_string();
    };
    match &table.target {
        TableRef::Model { package, name, .. } => match package {
            Some(p) => format!("model {}.{}", mir.syms.text(*p), mir.syms.text(*name)),
            None => format!("model {}", mir.syms.text(*name)),
        },
        TableRef::Source { source, table } => {
            format!(
                "source {}.{}",
                mir.syms.text(*source),
                mir.syms.text(*table)
            )
        }
        TableRef::This => "this".to_string(),
        TableRef::Physical(path) => {
            let parts: Vec<&str> = path.iter().map(|s| mir.syms.text(*s)).collect();
            format!("physical {}", parts.join("."))
        }
        TableRef::Jinja => "jinja".to_string(),
    }
}

pub(crate) fn scalar(mir: &Mir, id: ScalarId) -> String {
    scalar_at(mir, id, 0)
}

fn scalar_at(mir: &Mir, id: ScalarId, depth: usize) -> String {
    if depth > MAX_PRINT_DEPTH {
        return "...".to_string();
    }
    let Some(s) = mir.scalars.get(id) else {
        return "<bad>".to_string();
    };
    let sub = |x: ScalarId| scalar_at(mir, x, depth + 1);
    let list = |xs: &[ScalarId]| xs.iter().map(|x| sub(*x)).collect::<Vec<_>>().join(", ");
    match &s.kind {
        ScalarKind::Col(c) => mir.cols.get(*c).map_or("<bad col>".to_string(), |d| {
            format!("{}#{}", d.name.map_or("?", |n| mir.syms.text(n)), c.raw())
        }),
        ScalarKind::Outer(c) => format!("outer#{}", c.raw()),
        ScalarKind::Lit(l) => lit(mir, l),
        ScalarKind::Unary { op, e } => format!("{op:?}({})", sub(*e)),
        ScalarKind::Binary { op, l, r } => format!("({} {op:?} {})", sub(*l), sub(*r)),
        ScalarKind::Call { name, args, .. } => {
            format!("{}({})", mir.syms.text(*name), list(args))
        }
        ScalarKind::Case {
            operand,
            arms,
            else_,
        } => format!(
            "case(op={} arms={} else={})",
            operand.map_or("-".to_string(), sub),
            arms.len(),
            else_.map_or("-".to_string(), sub)
        ),
        ScalarKind::Cast { e, ty } => format!("cast({} as {ty:?})", sub(*e)),
        ScalarKind::InList {
            e,
            list: xs,
            negated,
        } => {
            let not = if *negated { "not " } else { "" };
            format!("{} {not}in ({})", sub(*e), list(xs))
        }
        ScalarKind::Subquery { kind, .. } => format!("subquery {kind:?}"),
        ScalarKind::Opaque { children, hole } => {
            format!("opaque[{:?}]({})", hole.kind, list(children))
        }
    }
}

fn lit(mir: &Mir, l: &Lit) -> String {
    match l {
        Lit::Null => "null".to_string(),
        Lit::Bool(b) => b.to_string(),
        Lit::Number(s) => mir.syms.text(*s).to_string(),
        Lit::Str(s) => format!("'{}'", mir.syms.text(*s)),
        Lit::Typed { ty, value } => format!("{} '{}'", mir.syms.text(*ty), mir.syms.text(*value)),
        Lit::Interval(s) => format!("interval {}", mir.syms.text(*s)),
    }
}

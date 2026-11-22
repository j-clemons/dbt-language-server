//! Shared builders for the lowering tests: a thin layer over the
//! `cogitator-ir` testkit that adds the HIR shapes it does not build.
#![allow(dead_code)]

use cogitator_ir::common::Lit;
use cogitator_ir::hir::{
    Body, CteId, Expr, ExprId, ExprKind, From, FromCol, FromId, FromKind, Hir, QueryId, Resolution,
    Select, SelectItem,
};
use cogitator_ir::mir::{Mir, RelId, RelOp};
use cogitator_ir::testkit::HirBuilder;
use cogitator_ir::{ColType, Lowered, Nullability, Span, SqlType};
use cogitator_lower::lower_to_mir;

pub const SPAN: Span = Span::new(0, 0);

pub fn builder() -> HirBuilder {
    HirBuilder::new("select 1")
}

pub fn item(expr: ExprId, alias: &str, b: &mut HirBuilder) -> SelectItem {
    let alias = (!alias.is_empty()).then(|| b.ident(SPAN, alias));
    SelectItem::Expr { expr, alias }
}

pub fn star() -> SelectItem {
    SelectItem::Star {
        span: SPAN,
        from: None,
        qualifier: None,
        exclude: Vec::new(),
        replace: Vec::new(),
    }
}

pub fn select(
    b: &mut HirBuilder,
    ctes: Vec<CteId>,
    items: Vec<SelectItem>,
    from: Vec<FromId>,
    f: impl FnOnce(&mut Select),
) -> QueryId {
    let mut body = Select {
        items,
        from,
        ..Select::default()
    };
    f(&mut body);
    b.query(SPAN, ctes, Body::Select(Box::new(body)))
}

pub fn expr(b: &mut HirBuilder, kind: ExprKind) -> ExprId {
    b.hir.exprs.alloc(Expr { span: SPAN, kind })
}

pub fn call(b: &mut HirBuilder, name: &str, args: Vec<ExprId>) -> ExprId {
    let name = b.sym(name);
    expr(
        b,
        ExprKind::Call {
            name,
            args,
            distinct: false,
            filter: None,
            over: None,
        },
    )
}

pub fn over(b: &mut HirBuilder, name: &str, partition: Vec<ExprId>) -> ExprId {
    let name = b.sym(name);
    let spec = cogitator_ir::hir::WindowSpec {
        partition_by: partition,
        ..Default::default()
    };
    expr(
        b,
        ExprKind::Call {
            name,
            args: Vec::new(),
            distinct: false,
            filter: None,
            over: Some(spec),
        },
    )
}

pub fn string(b: &mut HirBuilder, text: &str) -> ExprId {
    let sym = b.sym(text);
    expr(b, ExprKind::Lit(Lit::Str(sym)))
}

pub fn typed_from(b: &mut HirBuilder, name: &str, cols: &[(&str, ColType)]) -> FromId {
    let names: Vec<&str> = cols.iter().map(|(n, _)| *n).collect();
    let id = b.base_model(SPAN, name, None, &names);
    let output: Vec<FromCol> = cols
        .iter()
        .map(|(n, ty)| FromCol {
            name: b.sym(n),
            ty: ty.clone(),
        })
        .collect();
    if let Some(item) = b.hir.from.get_mut(id) {
        item.output = output;
    }
    id
}

pub fn open(b: &mut HirBuilder, id: FromId) {
    if let Some(item) = b.hir.from.get_mut(id) {
        item.open = true;
    }
}

pub fn add_from(b: &mut HirBuilder, kind: FromKind, output: &[&str]) -> FromId {
    let output = output
        .iter()
        .map(|n| FromCol {
            name: b.sym(n),
            ty: ColType::UNKNOWN,
        })
        .collect();
    b.hir.from.alloc(From {
        span: SPAN,
        kind,
        output,
        open: false,
    })
}

/// A reference `OpenSchema` resolves, by name, on `from`.
pub fn open_col(b: &mut HirBuilder, name: &str, from: FromId) -> ExprId {
    b.col_expr(SPAN, name, Some(Resolution::OpenSchema(from)))
}

pub fn outer_col(b: &mut HirBuilder, name: &str, from: FromId, col: u32) -> ExprId {
    b.col_expr(
        SPAN,
        name,
        Some(Resolution::Outer {
            depth: 1,
            from,
            col,
        }),
    )
}

pub fn int_type() -> ColType {
    ColType::new(SqlType::Int { bits: Some(64) }, Nullability::NotNull)
}

/// Lowers, requiring the verifier found nothing and a value exists.
pub fn clean(hir: &Hir) -> Mir {
    let lowered: Lowered<Mir> = lower_to_mir(hir);
    lowered.assert_clean();
    lowered.value.expect("a plan")
}

/// Every reachable relation is the input of at most one other: sharing
/// happens only through `CteRef`.
pub fn assert_tree(mir: &Mir) {
    let mut seen: Vec<RelId> = Vec::new();
    for id in mir.reachable() {
        for input in mir.rels[id].op.inputs() {
            assert!(!seen.contains(&input), "{input:?} has two parents");
            seen.push(input);
        }
    }
}

pub fn count_ops(mir: &Mir, f: impl Fn(&RelOp) -> bool) -> usize {
    mir.reachable()
        .into_iter()
        .filter(|id| f(&mir.rels[*id].op))
        .count()
}

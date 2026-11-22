//! Function calls, window specs, and the opaque fallback for every
//! expression form the IR does not model.

use cogitator_ir::hir::{ExprId, ExprKind, JinjaRole, OrderItem, WindowSpec};
use cogitator_ir::Span;
use sqlparser::ast::{self, Expr};

use super::from::name_idents;
use super::Lowerer;

impl Lowerer<'_> {
    pub(super) fn function(&mut self, f: &ast::Function) -> ExprId {
        let path = name_idents(&f.name);
        let name_span = self.path_span(&path);
        if let Some(region) = self.region_in_path(&path) {
            let jinja = self.jinja_expr(region, JinjaRole::Scalar);
            let (mut args, _) = self.call_args(&f.args);
            let mut children = vec![jinja];
            children.append(&mut args);
            let span = self.union_spans(&children);
            return self.alloc_expr(span, ExprKind::Opaque { children });
        }
        let name_text = path
            .iter()
            .map(|p| p.value.to_lowercase())
            .collect::<Vec<_>>()
            .join(".");
        let name = self.intern(&name_text);
        let (mut args, distinct) = self.call_args(&f.args);
        for item in self.within_group_exprs(&f.within_group) {
            args.push(item);
        }
        let filter = f.filter.as_ref().map(|e| self.expr(e));
        let over = f.over.as_ref().map(|w| self.window_type(w));
        let span = call_span(self, name_span, &args);
        self.alloc_expr(
            span,
            ExprKind::Call {
                name,
                args,
                distinct,
                filter,
                over,
            },
        )
    }

    fn within_group_exprs(&mut self, items: &[ast::OrderByExpr]) -> Vec<ExprId> {
        items.iter().map(|o| self.expr(&o.expr)).collect()
    }

    /// Lowered arguments and whether `DISTINCT` was present. `count(*)`'s
    /// star is not an expression: it comes back as an empty list.
    fn call_args(&mut self, args: &ast::FunctionArguments) -> (Vec<ExprId>, bool) {
        match args {
            ast::FunctionArguments::None => (Vec::new(), false),
            ast::FunctionArguments::Subquery(q) => (vec![self.scalar_subquery(q)], false),
            ast::FunctionArguments::List(list) => {
                let distinct = matches!(
                    list.duplicate_treatment,
                    Some(ast::DuplicateTreatment::Distinct)
                );
                let mut out = self.function_arg_list(&list.args);
                for clause in &list.clauses {
                    if let ast::FunctionArgumentClause::OrderBy(items) = clause {
                        out.extend(self.within_group_exprs(items));
                    }
                }
                (out, distinct)
            }
        }
    }

    pub(super) fn function_arg_list(&mut self, args: &[ast::FunctionArg]) -> Vec<ExprId> {
        let mut out = Vec::with_capacity(args.len());
        for arg in args {
            let inner = match arg {
                ast::FunctionArg::Named { arg, .. } | ast::FunctionArg::Unnamed(arg) => arg,
                ast::FunctionArg::ExprNamed { name, arg, .. } => {
                    out.push(self.expr(name));
                    arg
                }
            };
            if let ast::FunctionArgExpr::Expr(e) = inner {
                out.push(self.expr(e));
            }
        }
        out
    }

    fn window_type(&mut self, window: &ast::WindowType) -> WindowSpec {
        match window {
            ast::WindowType::WindowSpec(spec) => self.window_spec(spec),
            ast::WindowType::NamedWindow(name) => WindowSpec {
                name: Some(self.ident(name)),
                ..WindowSpec::default()
            },
        }
    }

    pub(super) fn window_spec(&mut self, spec: &ast::WindowSpec) -> WindowSpec {
        let name = spec.window_name.as_ref().map(|n| self.ident(n));
        let partition_by: Vec<ExprId> = spec.partition_by.iter().map(|e| self.expr(e)).collect();
        let order_by: Vec<OrderItem> = self.order_items(&spec.order_by);
        let frame = spec.window_frame.as_ref().map(|f| {
            let text = match &f.end_bound {
                Some(end) => format!("{} BETWEEN {} AND {end}", f.units, f.start_bound),
                None => format!("{} {}", f.units, f.start_bound),
            };
            self.intern(&text)
        });
        WindowSpec {
            name,
            partition_by,
            order_by,
            frame,
        }
    }

    /// Unknown expression forms: keep their children, lose only the shape.
    pub(super) fn opaque_expr(&mut self, e: &Expr) -> ExprId {
        if let Expr::CompoundFieldAccess { root, access_chain } = e {
            if let Some(access) = self.field_access(root, access_chain) {
                return access;
            }
        }
        let children = opaque_children(e);
        if let Expr::Lambda(lambda) = e {
            return self.lambda(lambda);
        }
        self.opaque_of(&children)
    }

    /// `s.g[1]` and `t.arr[1]`: the leading identifiers are one column path
    /// (`g` is a field, not a sibling column); `None` for any other root.
    fn field_access(&mut self, root: &Expr, chain: &[ast::AccessExpr]) -> Option<ExprId> {
        let mut path: Vec<ast::Ident> = match root {
            // boundary: `column` takes owned idents; one short path per access
            Expr::Identifier(ident) => vec![ident.clone()],
            Expr::CompoundIdentifier(idents) => idents.clone(),
            _ => return None,
        };
        let mut rest = chain;
        while let [ast::AccessExpr::Dot(Expr::Identifier(field)), tail @ ..] = rest {
            path.push(field.clone()); // boundary: as above
            rest = tail;
        }
        let mut children = vec![self.column(&path)];
        for access in rest {
            for inner in access_children(access) {
                children.push(self.expr(inner));
            }
        }
        let span = self.union_spans(&children);
        Some(self.alloc_expr(span, ExprKind::Opaque { children }))
    }

    fn lambda(&mut self, lambda: &ast::LambdaFunction) -> ExprId {
        let params: Vec<_> = lambda
            .params
            .iter()
            .map(|p| self.sym_for(&p.name))
            .collect();
        let mark = self.lambda.len();
        self.lambda.extend(params);
        let body = self.expr(&lambda.body);
        self.lambda.truncate(mark);
        let span = self.union_spans(&[body]);
        self.alloc_expr(
            span,
            ExprKind::Opaque {
                children: vec![body],
            },
        )
    }
}

fn call_span(this: &Lowerer<'_>, name_span: Span, args: &[ExprId]) -> Span {
    let args_span = this.union_spans(args);
    if name_span.is_empty() {
        args_span
    } else if args_span.is_empty() {
        name_span
    } else {
        super::union(name_span, args_span)
    }
}

/// What one access step reads: a subscript's bounds, or the expressions in
/// a `.call()` step. A plain `.field` names a field and reads nothing.
fn access_children(access: &ast::AccessExpr) -> Vec<&Expr> {
    match access {
        ast::AccessExpr::Dot(Expr::Identifier(_)) => Vec::new(),
        ast::AccessExpr::Dot(inner) => vec![inner],
        ast::AccessExpr::Subscript(ast::Subscript::Index { index }) => vec![index],
        ast::AccessExpr::Subscript(ast::Subscript::Slice {
            lower_bound,
            upper_bound,
            stride,
        }) => [lower_bound.as_ref(), upper_bound.as_ref(), stride.as_ref()]
            .into_iter()
            .flatten()
            .collect(),
    }
}

/// The expression children of forms the IR has no node for. Exhaustive
/// over what can hold column references; a form with none yields nothing.
fn opaque_children(e: &Expr) -> Vec<&Expr> {
    match e {
        Expr::CompoundFieldAccess { root, access_chain } => {
            let mut out = vec![root.as_ref()];
            out.extend(access_chain.iter().flat_map(access_children));
            out
        }
        Expr::JsonAccess { value, .. } => vec![value.as_ref()],
        Expr::IsJson { expr, .. }
        | Expr::IsNormalized { expr, .. }
        | Expr::Convert { expr, .. } => {
            vec![expr.as_ref()]
        }
        Expr::InUnnest {
            expr, array_expr, ..
        } => vec![expr.as_ref(), array_expr.as_ref()],
        Expr::Extract { expr, .. }
        | Expr::Ceil { expr, .. }
        | Expr::Floor { expr, .. }
        | Expr::OuterJoin(expr)
        | Expr::Prior(expr) => vec![expr.as_ref()],
        Expr::Position { expr, r#in } => vec![expr.as_ref(), r#in.as_ref()],
        Expr::Substring {
            expr,
            substring_from,
            substring_for,
            ..
        } => [
            Some(expr.as_ref()),
            substring_from.as_deref(),
            substring_for.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect(),
        Expr::Trim {
            expr,
            trim_what,
            trim_characters,
            ..
        } => {
            let mut out = vec![expr.as_ref()];
            out.extend(trim_what.as_deref());
            out.extend(trim_characters.iter().flatten());
            out
        }
        Expr::Overlay {
            expr,
            overlay_what,
            overlay_from,
            overlay_for,
        } => {
            let mut out = vec![expr.as_ref(), overlay_what.as_ref(), overlay_from.as_ref()];
            out.extend(overlay_for.as_deref());
            out
        }
        Expr::Tuple(items) => items.iter().collect(),
        Expr::Struct { values, .. } => values.iter().collect(),
        Expr::Array(array) => array.elem.iter().collect(),
        Expr::Map(map) => map
            .entries
            .iter()
            .flat_map(|entry| [entry.key.as_ref(), entry.value.as_ref()])
            .collect(),
        Expr::Dictionary(fields) => fields.iter().map(|f| f.value.as_ref()).collect(),
        Expr::GroupingSets(sets) | Expr::Cube(sets) | Expr::Rollup(sets) => {
            sets.iter().flatten().collect()
        }
        Expr::MemberOf(member) => vec![member.value.as_ref(), member.array.as_ref()],
        // Constant-only forms: no column can hide in them.
        _ => Vec::new(),
    }
}

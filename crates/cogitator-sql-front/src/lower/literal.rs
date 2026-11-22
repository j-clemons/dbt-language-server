//! Literals, typed strings and intervals.

use cogitator_ir::common::Lit;
use cogitator_ir::hir::{ExprId, ExprKind};
use sqlparser::ast::{self, Expr};

use super::Lowerer;

impl Lowerer<'_> {
    pub(super) fn literal(&mut self, value: &ast::ValueWithSpan) -> ExprId {
        let span = self.sql_span(value.span).unwrap_or_default();
        let lit = match &value.value {
            ast::Value::Number(text, _) => Lit::Number(self.intern(&text.to_string())),
            ast::Value::Boolean(b) => Lit::Bool(*b),
            ast::Value::Null => Lit::Null,
            ast::Value::Placeholder(_) => {
                return self.alloc_expr(
                    span,
                    ExprKind::Opaque {
                        children: Vec::new(),
                    },
                );
            }
            other => match string_literal(other) {
                Some(text) => Lit::Str(self.intern(&text)),
                None => {
                    return self.alloc_expr(
                        span,
                        ExprKind::Opaque {
                            children: Vec::new(),
                        },
                    );
                }
            },
        };
        self.alloc_expr(span, ExprKind::Lit(lit))
    }

    pub(super) fn typed_string(&mut self, ts: &ast::TypedString) -> ExprId {
        let span = self.sql_span(ts.value.span).unwrap_or_default();
        let ty = self.intern(&ts.data_type.to_string());
        let text = string_literal(&ts.value.value).unwrap_or_default();
        let value = self.intern(&text);
        self.alloc_expr(span, ExprKind::Lit(Lit::Typed { ty, value }))
    }

    pub(super) fn interval(&mut self, interval: &ast::Interval) -> ExprId {
        if let Expr::Value(v) = interval.value.as_ref() {
            let span = self.sql_span(v.span).unwrap_or_default();
            let text = interval.to_string();
            let sym = self.intern(&text);
            return self.alloc_expr(span, ExprKind::Lit(Lit::Interval(sym)));
        }
        let child = self.expr(&interval.value);
        let span = self.union_spans(&[child]);
        self.alloc_expr(
            span,
            ExprKind::Opaque {
                children: vec![child],
            },
        )
    }
}

/// The text of a string-valued literal, without quotes.
fn string_literal(value: &ast::Value) -> Option<String> {
    use ast::Value as V;
    match value {
        V::SingleQuotedString(s)
        | V::DoubleQuotedString(s)
        | V::TripleSingleQuotedString(s)
        | V::TripleDoubleQuotedString(s)
        | V::EscapedStringLiteral(s)
        | V::UnicodeStringLiteral(s)
        | V::NationalStringLiteral(s)
        | V::HexStringLiteral(s)
        | V::SingleQuotedByteStringLiteral(s)
        | V::DoubleQuotedByteStringLiteral(s)
        | V::TripleSingleQuotedByteStringLiteral(s)
        | V::TripleDoubleQuotedByteStringLiteral(s)
        | V::SingleQuotedRawStringLiteral(s)
        | V::DoubleQuotedRawStringLiteral(s)
        | V::TripleSingleQuotedRawStringLiteral(s)
        | V::TripleDoubleQuotedRawStringLiteral(s) => Some(s.clone()), // boundary: interned next
        V::DollarQuotedString(d) => Some(d.value.clone()), // boundary: interned next
        _ => None,
    }
}

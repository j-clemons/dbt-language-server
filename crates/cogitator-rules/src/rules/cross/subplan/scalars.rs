//! Renders scalar expressions as canonical text.

use cogitator_ir::mir::{ColId, RelId, ScalarId, ScalarKind, SortKey, SubqueryKind};

use super::lit_text;
use super::render::{Render, MAX_DEPTH};

impl<'m> Render<'m> {
    pub(super) fn scalars(&mut self, ids: &[ScalarId]) {
        for id in ids {
            self.scalar(*id);
            self.out.push(',');
        }
    }

    /// Conjuncts and join keys are unordered, so they compare as a set.
    pub(super) fn sorted(&mut self, ids: &[ScalarId]) {
        let saved = std::mem::take(&mut self.out);
        let mut parts: Vec<String> = ids
            .iter()
            .map(|id| {
                self.out.clear();
                self.scalar(*id);
                self.out.clone()
            })
            .collect();
        parts.sort_unstable();
        self.out = saved;
        self.out.push('[');
        self.out.push_str(&parts.join("&"));
        self.out.push(']');
    }

    pub(super) fn sort_keys(&mut self, keys: &[SortKey]) {
        for key in keys {
            self.scalar(key.expr);
            self.out
                .push_str(&format!("/{:?}/{:?},", key.desc, key.nulls_first));
        }
    }

    pub(super) fn scalar(&mut self, id: ScalarId) {
        if self.over_budget() || self.depth >= MAX_DEPTH {
            self.ok = false;
            return;
        }
        let mir = self.mir;
        let Some(node) = mir.scalars.get(id) else {
            self.ok = false;
            return;
        };
        self.depth += 1;
        self.scalar_kind(&node.kind);
        self.depth -= 1;
    }

    fn scalar_kind(&mut self, kind: &'m ScalarKind) {
        match kind {
            ScalarKind::Col(c) | ScalarKind::Outer(c) => self.write_col(*c),
            ScalarKind::Lit(lit) => match lit_text(self.mir, lit) {
                Some(text) => self.out.push_str(&text),
                None => self.ok = false,
            },
            ScalarKind::Unary { op, e } => {
                self.out.push_str(&format!("{op:?}("));
                self.scalar(*e);
                self.out.push(')');
            }
            ScalarKind::Binary { op, l, r } => {
                self.out.push('(');
                self.scalar(*l);
                self.out.push_str(&format!(" {op:?} "));
                self.scalar(*r);
                self.out.push(')');
            }
            ScalarKind::Call {
                name,
                args,
                distinct,
            } => {
                self.out.push_str(&format!(
                    "{}/{distinct}(",
                    self.mir.syms.text(*name).to_ascii_lowercase()
                ));
                self.scalars(args);
                self.out.push(')');
            }
            ScalarKind::Case {
                operand,
                arms,
                else_,
            } => {
                self.out.push_str("case(");
                self.scalars(&operand.iter().copied().collect::<Vec<_>>());
                for (when, then) in arms {
                    self.out.push_str("when:");
                    self.scalar(*when);
                    self.out.push_str("then:");
                    self.scalar(*then);
                }
                self.out.push_str("else:");
                self.scalars(&else_.iter().copied().collect::<Vec<_>>());
                self.out.push(')');
            }
            ScalarKind::Cast { e, ty } => {
                self.out.push_str("cast(");
                self.scalar(*e);
                self.out.push_str(&format!(" as {ty:?})"));
            }
            ScalarKind::InList { e, list, negated } => {
                self.out.push_str(&format!("in/{negated}("));
                self.scalar(*e);
                self.out.push('|');
                self.scalars(list);
                self.out.push(')');
            }
            ScalarKind::Subquery {
                kind,
                rel,
                correlated,
            } => self.subquery(kind, *rel, correlated),
            ScalarKind::Opaque { .. } => self.ok = false,
        }
    }

    fn subquery(&mut self, kind: &SubqueryKind, rel: RelId, correlated: &[ColId]) {
        match kind {
            SubqueryKind::Exists => self.out.push_str("exists("),
            SubqueryKind::Scalar => self.out.push_str("scalar("),
            SubqueryKind::In { e, negated } => {
                self.out.push_str(&format!("in_sub/{negated}("));
                self.scalar(*e);
                self.out.push('|');
            }
        }
        self.rel(rel);
        self.out.push_str(")corr:");
        self.write_cols(correlated);
    }
}

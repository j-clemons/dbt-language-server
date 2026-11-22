//! Renders relational operators as canonical text.

use std::collections::BTreeMap;

use cogitator_ir::mir::{ColId, GroupingSets, Mir, RelId, RelOp, ScalarId, TableRef};

use super::{holds_masked_jinja, is_work};

/// One render's bound; a larger subplan is not compared.
const MAX_RENDER_STEPS: usize = 4_096;
const MAX_TEXT_BYTES: usize = 16 * 1024;
pub(super) const MAX_DEPTH: usize = 128;

pub(super) struct Render<'m> {
    pub(super) mir: &'m Mir,
    /// One frame per inlined CTE, so two references get distinct columns.
    pub(super) scopes: Vec<BTreeMap<u32, u32>>,
    pub(super) next: u32,
    pub(super) out: String,
    pub(super) steps: usize,
    pub(super) rels: u32,
    pub(super) work: bool,
    pub(super) ok: bool,
    pub(super) depth: usize,
}

impl<'m> Render<'m> {
    pub(super) fn new(mir: &'m Mir) -> Self {
        Self {
            mir,
            scopes: vec![BTreeMap::new()],
            next: 0,
            out: String::new(),
            steps: 0,
            rels: 0,
            work: false,
            ok: true,
            depth: 0,
        }
    }

    fn ord(&mut self, col: ColId) -> u32 {
        for scope in self.scopes.iter().rev() {
            if let Some(found) = scope.get(&col.raw()) {
                return *found;
            }
        }
        let fresh = self.next;
        self.next += 1;
        if let Some(top) = self.scopes.last_mut() {
            top.insert(col.raw(), fresh);
        }
        fresh
    }

    fn bind(&mut self, col: ColId, ord: u32) {
        if let Some(top) = self.scopes.last_mut() {
            top.insert(col.raw(), ord);
        }
    }

    pub(super) fn write_col(&mut self, col: ColId) {
        let ord = self.ord(col);
        self.out.push_str(&format!("#{ord}"));
    }

    pub(super) fn write_cols(&mut self, cols: &[ColId]) {
        for col in cols {
            self.write_col(*col);
            self.out.push(',');
        }
    }

    pub(super) fn over_budget(&mut self) -> bool {
        self.steps += 1;
        if self.steps > MAX_RENDER_STEPS || self.out.len() > MAX_TEXT_BYTES {
            self.ok = false;
        }
        !self.ok
    }

    pub(super) fn rel(&mut self, id: RelId) {
        if self.over_budget() || self.depth >= MAX_DEPTH {
            self.ok = false;
            return;
        }
        let mir = self.mir;
        let Some(node) = mir.rels.get(id) else {
            self.ok = false;
            return;
        };
        self.depth += 1;
        self.rels += 1;
        self.work |= is_work(&node.op);
        self.op(&node.op);
        self.depth -= 1;
    }

    fn op(&mut self, op: &'m RelOp) {
        match op {
            RelOp::Scan { table, cols, .. } => self.scan(*table, cols),
            RelOp::Filter { input, conjuncts } => {
                self.input("filter", *input);
                self.sorted(conjuncts);
            }
            RelOp::Project { input, items } => {
                self.input("project", *input);
                self.assignments(items);
            }
            RelOp::Join { kind, l, r, on } => {
                self.out.push_str(&format!("join.{kind:?}("));
                self.rel(*l);
                self.out.push('|');
                self.rel(*r);
                self.out.push(')');
                self.sorted(on);
            }
            RelOp::Aggregate {
                input,
                keys,
                aggs,
                sets,
            } => {
                self.input("agg", *input);
                self.assignments(keys);
                for (col, call) in aggs {
                    self.write_col(*col);
                    self.out.push_str(&format!(
                        ":={}/{}(",
                        self.mir.syms.text(call.func).to_ascii_lowercase(),
                        call.distinct
                    ));
                    self.scalars(&call.args);
                    self.out.push_str(")f:");
                    self.scalars(&call.filter.iter().copied().collect::<Vec<_>>());
                    self.out.push(';');
                }
                self.sets(sets.as_ref());
            }
            RelOp::Window { input, funcs } => {
                self.input("window", *input);
                for (col, call) in funcs {
                    self.write_col(*col);
                    self.out.push_str(&format!(
                        ":={}(",
                        self.mir.syms.text(call.func).to_ascii_lowercase()
                    ));
                    self.scalars(&call.args);
                    self.out.push_str(")p:");
                    self.scalars(&call.partition_by);
                    self.out.push_str("o:");
                    self.sort_keys(&call.order_by);
                    let frame = call.frame.map_or("", |f| self.mir.syms.text(f));
                    self.out.push_str(&format!("w:{frame};"));
                }
            }
            RelOp::Distinct(input) => self.input("distinct", *input),
            RelOp::SetOp {
                op,
                all,
                inputs,
                out,
            } => {
                self.out.push_str(&format!("set.{op:?}/{all}("));
                for arm in inputs {
                    self.rel(*arm);
                    self.out.push('|');
                }
                self.out.push(')');
                self.write_cols(out);
            }
            RelOp::Sort { input, keys } => {
                self.input("sort", *input);
                self.sort_keys(keys);
            }
            RelOp::Limit {
                input,
                limit,
                offset,
            } => {
                self.input("limit", *input);
                self.scalars(&limit.iter().copied().collect::<Vec<_>>());
                self.out.push('+');
                self.scalars(&offset.iter().copied().collect::<Vec<_>>());
            }
            RelOp::Unnest { input, exprs, out } => {
                self.input("unnest", *input);
                self.scalars(exprs);
                self.write_cols(out);
            }
            RelOp::CteRef { cte, out } => self.cte_ref(*cte, out),
            RelOp::Opaque { .. } => self.ok = false,
        }
    }

    fn input(&mut self, name: &str, input: RelId) {
        self.out.push_str(name);
        self.out.push('(');
        self.rel(input);
        self.out.push(')');
    }

    /// A CTE reads as if written inline, so a CTE-first model and a
    /// subquery-first model that compute the same thing compare equal.
    fn cte_ref(&mut self, cte: cogitator_ir::mir::CteId, out: &[ColId]) {
        let Some(def) = self.mir.ctes.get(cte) else {
            self.ok = false;
            return;
        };
        self.scopes.push(BTreeMap::new());
        self.rel(def.body);
        let ords: Vec<u32> = def.out.iter().map(|c| self.ord(*c)).collect();
        self.scopes.pop();
        if ords.len() != out.len() {
            self.ok = false;
            return;
        }
        for (col, ord) in out.iter().zip(ords) {
            self.bind(*col, ord);
        }
    }

    fn scan(&mut self, table: cogitator_ir::mir::TableId, cols: &[ColId]) {
        let Some(def) = self.mir.tables.get(table) else {
            self.ok = false;
            return;
        };
        let syms = &self.mir.syms;
        let target = match &def.target {
            TableRef::Model {
                package,
                name,
                version,
            } => format!(
                "m:{}/{}/{}",
                package.map_or("", |p| syms.text(p)),
                syms.text(*name),
                version.map_or("", |v| syms.text(v))
            ),
            TableRef::Source { source, table } => {
                format!("s:{}/{}", syms.text(*source), syms.text(*table))
            }
            TableRef::Physical(path) => {
                let parts: Vec<&str> = path.iter().map(|p| syms.text(*p)).collect();
                format!("p:{}", parts.join("."))
            }
            TableRef::This | TableRef::Jinja => {
                self.ok = false;
                return;
            }
        };
        let mut named = Vec::with_capacity(cols.len());
        for col in cols {
            let name = self.mir.cols.get(*col).and_then(|d| d.name);
            let Some(name) = name else {
                self.ok = false;
                return;
            };
            let name = self.mir.syms.text(name);
            if holds_masked_jinja(name) {
                self.ok = false;
                return;
            }
            named.push((name.to_lowercase(), *col));
        }
        named.sort_by(|a, b| a.0.cmp(&b.0));
        self.out.push_str("scan(");
        self.out.push_str(&target);
        self.out.push(')');
        for (name, col) in named {
            self.write_col(col);
            self.out.push('=');
            self.out.push_str(&name);
            self.out.push(',');
        }
    }

    fn assignments(&mut self, items: &[(ColId, ScalarId)]) {
        for (col, scalar) in items {
            self.scalar(*scalar);
            self.out.push_str(">>");
            self.write_col(*col);
            self.out.push(';');
        }
    }

    fn sets(&mut self, sets: Option<&GroupingSets>) {
        let Some(GroupingSets(sets)) = sets else {
            return;
        };
        for set in sets {
            self.out.push_str("set[");
            self.write_cols(set);
            self.out.push(']');
        }
    }
}

//! Which upstream columns a model's outputs need, walked bottom-up over
//! its MIR. Column ids are stable from definition to use, so a `(c, Col(c))`
//! item adds nothing and renames share their source's set.

use std::collections::BTreeSet;
use std::sync::Arc;

use cogitator_ir::mir::{ColId, Mir, RelOp, ScalarId, ScalarKind, TableRef};
use cogitator_ir::Span;

/// Past this many (output, upstream column) pairs the model reads as
/// unknown rather than holding an arbitrarily large table.
const MAX_ENTRIES: usize = 200_000;

/// Interned (upstream model, column) pairs and who needs them.
#[derive(Debug, Default)]
pub(super) struct ModelReads {
    pub(super) cols: Vec<(Arc<str>, Arc<str>)>,
    /// Read regardless of which outputs are wanted: filters, join keys,
    /// grouping keys, DISTINCT and set-op deduplication.
    pub(super) always: BTreeSet<u32>,
    /// Upstream columns behind each named output.
    pub(super) outputs: Vec<(Arc<str>, BTreeSet<u32>)>,
    /// Something could not be modelled; assume every upstream column.
    pub(super) unknown: bool,
}

struct Walker<'m> {
    mir: &'m Mir,
    deps: Vec<Option<BTreeSet<u32>>>,
    interned: std::collections::HashMap<(cogitator_ir::Symbol, cogitator_ir::Symbol), u32>,
    out: ModelReads,
    entries: usize,
}

pub(super) fn model_reads(mir: &Mir) -> ModelReads {
    let mut walker = Walker {
        mir,
        deps: vec![None; mir.cols.len()],
        interned: std::collections::HashMap::new(),
        out: ModelReads::default(),
        entries: 0,
    };
    for id in mir.reachable() {
        let Some(rel) = mir.rels.get(id) else {
            walker.out.unknown = true;
            continue;
        };
        walker.visit(&rel.op, id.index());
    }
    for col in mir.root_columns() {
        let name = mir.cols.get(*col).and_then(|c| c.name);
        let Some(name) = name else { continue };
        let deps = walker.dep(*col).cloned().unwrap_or_default();
        walker.entries += deps.len();
        walker
            .out
            .outputs
            .push((Arc::from(mir.syms.text(name)), deps));
    }
    if walker.entries > MAX_ENTRIES {
        walker.out.unknown = true;
    }
    walker.out
}

impl Walker<'_> {
    fn dep(&self, col: ColId) -> Option<&BTreeSet<u32>> {
        self.deps.get(col.index())?.as_ref()
    }

    fn set(&mut self, col: ColId, deps: BTreeSet<u32>) {
        self.entries += deps.len();
        if let Some(slot) = self.deps.get_mut(col.index()) {
            *slot = Some(deps);
        }
    }

    fn visit(&mut self, op: &RelOp, at: usize) {
        match op {
            RelOp::Scan { table, cols, .. } => self.scan(*table, cols),
            RelOp::Filter { conjuncts, .. } => self.always(conjuncts),
            RelOp::Join { on, .. } => self.always(on),
            RelOp::Project { items, .. } => {
                for (col, scalar) in items {
                    if !self.is_self(*col, *scalar) {
                        let deps = self.collect(*scalar);
                        self.set(*col, deps);
                    }
                }
            }
            RelOp::Aggregate { keys, aggs, .. } => {
                for (col, scalar) in keys {
                    let deps = self.collect(*scalar);
                    self.out.always.extend(deps.iter().copied());
                    if !self.is_self(*col, *scalar) {
                        self.set(*col, deps);
                    }
                }
                for (col, call) in aggs {
                    let deps = self.collect_all(&call.scalars());
                    self.set(*col, deps);
                }
            }
            RelOp::Window { funcs, .. } => {
                for (col, call) in funcs {
                    let deps = self.collect_all(&call.scalars());
                    self.set(*col, deps);
                }
            }
            RelOp::Distinct(input) => self.require_outputs(*input),
            RelOp::Sort { keys, .. } => {
                for key in keys {
                    let deps = self.collect(key.expr);
                    self.out.always.extend(deps);
                }
            }
            RelOp::Limit { .. } => {}
            RelOp::SetOp {
                all, inputs, out, ..
            } => self.set_op(*all, inputs, out),
            RelOp::Unnest { exprs, out, .. } => {
                let deps = self.collect_all(exprs);
                self.out.always.extend(deps.iter().copied());
                for col in out {
                    self.set(*col, deps.clone());
                }
            }
            RelOp::CteRef { cte, out } => self.cte_ref(*cte, out, at),
            RelOp::Opaque {
                inputs,
                out,
                open,
                hole,
            } => {
                let singleton =
                    inputs.is_empty() && out.is_empty() && !open && hole.span == Span::default();
                self.out.unknown |= !singleton;
            }
        }
    }

    fn is_self(&self, col: ColId, scalar: ScalarId) -> bool {
        matches!(
            self.mir.scalars.get(scalar).map(|s| &s.kind),
            Some(ScalarKind::Col(c)) if *c == col
        )
    }

    fn scan(&mut self, table: cogitator_ir::mir::TableId, cols: &[ColId]) {
        let Some(def) = self.mir.tables.get(table) else {
            self.out.unknown = true;
            return;
        };
        let TableRef::Model { name, .. } = &def.target else {
            return; // only model columns have demand to fold
        };
        for col in cols {
            let Some(column) = self.mir.cols.get(*col).and_then(|c| c.name) else {
                continue;
            };
            let id = self.intern(*name, column);
            self.set(*col, BTreeSet::from([id]));
        }
    }

    fn intern(&mut self, model: cogitator_ir::Symbol, column: cogitator_ir::Symbol) -> u32 {
        let key = (model, column);
        if let Some(id) = self.interned.get(&key) {
            return *id;
        }
        let id = self.out.cols.len() as u32;
        self.out.cols.push((
            Arc::from(self.mir.syms.text(model)),
            Arc::from(self.mir.syms.text(column)),
        ));
        self.interned.insert(key, id);
        id
    }

    fn always(&mut self, scalars: &[ScalarId]) {
        let deps = self.collect_all(scalars);
        self.out.always.extend(deps);
    }

    /// DISTINCT compares every column of its input.
    fn require_outputs(&mut self, input: cogitator_ir::mir::RelId) {
        let Some(rel) = self.mir.rels.get(input) else {
            self.out.unknown = true;
            return;
        };
        let cols: Vec<ColId> = rel.out.clone();
        for col in cols {
            if let Some(deps) = self.dep(col).cloned() {
                self.out.always.extend(deps);
            }
        }
    }

    fn set_op(&mut self, all: bool, inputs: &[cogitator_ir::mir::RelId], out: &[ColId]) {
        for (i, col) in out.iter().enumerate() {
            let mut deps = BTreeSet::new();
            for arm in inputs {
                let source = self.mir.rels.get(*arm).and_then(|r| r.out.get(i)).copied();
                if let Some(found) = source.and_then(|c| self.dep(c)) {
                    deps.extend(found.iter().copied());
                }
            }
            if !all {
                self.out.always.extend(deps.iter().copied());
            }
            self.set(*col, deps);
        }
    }

    fn cte_ref(&mut self, cte: cogitator_ir::mir::CteId, out: &[ColId], at: usize) {
        let Some(def) = self.mir.ctes.get(cte) else {
            self.out.unknown = true;
            return;
        };
        if def.body.index() > at {
            // Rewrites may reorder; lowering never does. Past that, give up.
            self.out.unknown = true;
            return;
        }
        for (i, col) in out.iter().enumerate() {
            let source = def.out.get(i).copied();
            if let Some(deps) = source.and_then(|c| self.dep(c)).cloned() {
                self.set(*col, deps);
            }
        }
    }

    fn collect_all(&mut self, scalars: &[ScalarId]) -> BTreeSet<u32> {
        let mut out = BTreeSet::new();
        for scalar in scalars {
            out.extend(self.collect(*scalar));
        }
        out
    }

    /// Every upstream column under `root`; a subquery contributes its
    /// outputs and any correlated column it reads.
    fn collect(&mut self, root: ScalarId) -> BTreeSet<u32> {
        let mut out = BTreeSet::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let Some(node) = self.mir.scalars.get(id) else {
                self.out.unknown = true;
                continue;
            };
            match &node.kind {
                ScalarKind::Col(c) | ScalarKind::Outer(c) => {
                    if let Some(deps) = self.dep(*c) {
                        out.extend(deps.iter().copied());
                    }
                }
                ScalarKind::Opaque { .. } => self.out.unknown = true,
                ScalarKind::Subquery {
                    rel, correlated, ..
                } => {
                    let cols = self
                        .mir
                        .rels
                        .get(*rel)
                        .map_or_else(Vec::new, |r| r.out.clone());
                    for col in cols.iter().chain(correlated) {
                        if let Some(deps) = self.dep(*col) {
                            out.extend(deps.iter().copied());
                        }
                    }
                }
                _ => {}
            }
            stack.extend(node.kind.children());
        }
        out
    }
}

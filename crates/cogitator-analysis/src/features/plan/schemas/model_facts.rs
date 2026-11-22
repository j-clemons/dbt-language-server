//! Per-model facts for the `cross.*` rules, drawn from the model's
//! canonical plan and cached with its inference.

use std::collections::BTreeMap;
use std::sync::Arc;

use cogitator_ir::hir::{Hir, JinjaKind};
use cogitator_ir::mir::{ColId, Mir, RelId, RelOp, ScalarId, ScalarKind, TableRef};
use cogitator_rules::rules::cross::subplan::{bounded_subplans, lit_text, Subplan};
use cogitator_rules::ColumnFilter;

/// Per upstream: more predicates than this are not compared.
const MAX_FILTERS: usize = 16;
const MAX_PREDICATE_BYTES: usize = 512;
const MAX_SCALAR_DEPTH: usize = 32;

#[derive(Debug, Default)]
pub(super) struct ModelFacts {
    /// Predicates proven over each upstream model this one scans.
    pub(super) filters: BTreeMap<Arc<str>, Arc<[ColumnFilter]>>,
    /// Largest-first candidates for `cross.duplicate_logic`.
    pub(super) subplans: Vec<Subplan>,
    /// False when a count bound left candidates out.
    pub(super) subplans_complete: bool,
}

pub(super) fn model_facts(hir: &Hir, canonical: &Mir) -> ModelFacts {
    let (subplans, subplans_complete) = bounded_subplans(canonical);
    ModelFacts {
        filters: upstream_filters(hir, canonical),
        subplans,
        subplans_complete,
    }
}

/// A filter under `{% if is_incremental() %}` is not one the full build
/// applies, and the template variant cannot tell them apart here.
fn has_incremental_guard(hir: &Hir) -> bool {
    hir.jinja
        .iter()
        .any(|(_, n)| n.kind == JinjaKind::IsIncremental)
}

fn upstream_filters(hir: &Hir, mir: &Mir) -> BTreeMap<Arc<str>, Arc<[ColumnFilter]>> {
    let mut out = BTreeMap::new();
    if has_incremental_guard(hir) {
        return out;
    }
    let graph = Graph::of(mir);
    let mut scans: BTreeMap<&str, Vec<RelId>> = BTreeMap::new();
    for id in &graph.reachable {
        if let Some(name) = model_scan(mir, *id) {
            scans.entry(name).or_default().push(*id);
        }
    }
    for (name, sites) in scans {
        let [scan] = sites.as_slice() else {
            continue; // two reads of a model: neither proves anything alone
        };
        let filters = scan_filters(mir, &graph, *scan);
        out.insert(Arc::from(name), Arc::from(filters));
    }
    out
}

fn model_scan(mir: &Mir, id: RelId) -> Option<&str> {
    let RelOp::Scan { table, .. } = &mir.rels.get(id)?.op else {
        return None;
    };
    match &mir.tables.get(*table)?.target {
        TableRef::Model { name, .. } => Some(mir.syms.text(*name)),
        _ => None,
    }
}

/// Who uses each rel: operator parents, and the references of CTE bodies.
struct Graph {
    reachable: Vec<RelId>,
    parents: Vec<Vec<RelId>>,
    cte_refs: Vec<Vec<RelId>>,
}

impl Graph {
    fn of(mir: &Mir) -> Self {
        let reachable = mir.reachable();
        let mut parents = vec![Vec::new(); mir.rels.len()];
        let mut cte_refs = vec![Vec::new(); mir.rels.len()];
        for id in &reachable {
            let Some(rel) = mir.rels.get(*id) else {
                continue;
            };
            for input in rel.op.inputs() {
                if let Some(slot) = parents.get_mut(input.index()) {
                    slot.push(*id);
                }
            }
            if let RelOp::CteRef { cte, .. } = rel.op {
                let body = mir.ctes.get(cte).map(|def| def.body);
                if let Some(slot) = body.and_then(|b| cte_refs.get_mut(b.index())) {
                    slot.push(*id);
                }
            }
        }
        Graph {
            reachable,
            parents,
            cte_refs,
        }
    }
}

/// Walks upward from a scan through Filter, row-preserving Project and a
/// single-use CTE boundary, collecting predicates that apply to every row
/// the consumer reads. Stops at anything else: a filter above a join,
/// aggregate or Unnest does not bind every row of this input.
fn scan_filters(mir: &Mir, graph: &Graph, scan: RelId) -> Vec<ColumnFilter> {
    let Some(RelOp::Scan { cols, .. }) = mir.rels.get(scan).map(|r| &r.op) else {
        return Vec::new();
    };
    let mut names: BTreeMap<ColId, Arc<str>> = BTreeMap::new();
    for col in cols {
        if let Some(name) = mir.cols.get(*col).and_then(|c| c.name) {
            names.insert(*col, Arc::from(mir.syms.text(name).to_lowercase()));
        }
    }
    let mut at = scan;
    let mut found = Vec::new();
    while found.len() < MAX_FILTERS {
        let Some(next) = step_up(mir, graph, at, &mut names, &mut found) else {
            break;
        };
        at = next;
    }
    found.sort();
    found.dedup();
    found.truncate(MAX_FILTERS);
    found
}

fn step_up(
    mir: &Mir,
    graph: &Graph,
    at: RelId,
    names: &mut BTreeMap<ColId, Arc<str>>,
    found: &mut Vec<ColumnFilter>,
) -> Option<RelId> {
    if let [only] = graph.cte_refs.get(at.index())?.as_slice() {
        // `at` is a CTE body with one reference: continue from there.
        let def = cte_of(mir, *only)?;
        let RelOp::CteRef { out, .. } = &mir.rels.get(*only)?.op else {
            return None;
        };
        let mut mapped = BTreeMap::new();
        for (inner, outer) in def.out.iter().zip(out) {
            if let Some(name) = names.remove(inner) {
                mapped.insert(*outer, name);
            }
        }
        *names = mapped;
        return Some(*only);
    }
    let [parent] = graph.parents.get(at.index())?.as_slice() else {
        return None;
    };
    match &mir.rels.get(*parent)?.op {
        RelOp::Filter { conjuncts, .. } => {
            for conjunct in conjuncts {
                if let Some(filter) = column_filter(mir, *conjunct, names) {
                    found.push(filter);
                }
            }
            Some(*parent)
        }
        RelOp::Project { items, .. } => {
            let mut mapped = BTreeMap::new();
            for (col, scalar) in items {
                if let Some(ScalarKind::Col(source)) = mir.scalars.get(*scalar).map(|s| &s.kind) {
                    if let Some(name) = names.get(source) {
                        mapped.insert(*col, name.clone());
                    }
                }
            }
            *names = mapped;
            Some(*parent)
        }
        _ => None,
    }
}

fn cte_of(mir: &Mir, cte_ref: RelId) -> Option<&cogitator_ir::mir::CteDef> {
    let RelOp::CteRef { cte, .. } = &mir.rels.get(cte_ref)?.op else {
        return None;
    };
    mir.ctes.get(*cte)
}

/// The predicate's canonical text when it reads exactly one known output
/// column and nothing but that column, literals and pure operators.
/// Calls are refused: `date(ts) > current_date` must not move into a stored
/// model, and the rule cannot tell a pure function from a volatile one.
fn column_filter(
    mir: &Mir,
    root: ScalarId,
    names: &BTreeMap<ColId, Arc<str>>,
) -> Option<ColumnFilter> {
    let mut printer = Printer {
        mir,
        names,
        column: None,
        out: String::new(),
    };
    printer.expr(root, 0)?;
    let column = printer.column?;
    (printer.out.len() <= MAX_PREDICATE_BYTES).then(|| ColumnFilter {
        column,
        predicate: Arc::from(printer.out),
    })
}

struct Printer<'a> {
    mir: &'a Mir,
    names: &'a BTreeMap<ColId, Arc<str>>,
    column: Option<Arc<str>>,
    out: String,
}

impl Printer<'_> {
    fn expr(&mut self, id: ScalarId, depth: usize) -> Option<()> {
        if depth >= MAX_SCALAR_DEPTH || self.out.len() > MAX_PREDICATE_BYTES {
            return None;
        }
        let mir = self.mir;
        match &mir.scalars.get(id)?.kind {
            ScalarKind::Col(col) => {
                let name = self.names.get(col)?;
                if self.column.as_ref().is_some_and(|known| known != name) {
                    return None;
                }
                self.column = Some(name.clone());
                self.out.push('$');
            }
            ScalarKind::Lit(lit) => self.out.push_str(&lit_text(mir, lit)?),
            ScalarKind::Unary { op, e } => {
                self.out.push_str(&format!("{op:?}("));
                self.expr(*e, depth + 1)?;
                self.out.push(')');
            }
            ScalarKind::Binary { op, l, r } => {
                self.out.push('(');
                self.expr(*l, depth + 1)?;
                self.out.push_str(&format!(" {op:?} "));
                self.expr(*r, depth + 1)?;
                self.out.push(')');
            }
            ScalarKind::Cast { e, ty } => {
                self.out.push_str("cast(");
                self.expr(*e, depth + 1)?;
                self.out.push_str(&format!(" as {ty:?})"));
            }
            ScalarKind::InList { e, list, negated } => {
                self.out.push_str(&format!("in/{negated}("));
                self.expr(*e, depth + 1)?;
                for item in list {
                    self.out.push('|');
                    self.expr(*item, depth + 1)?;
                }
                self.out.push(')');
            }
            _ => return None,
        }
        Some(())
    }
}

//! MIR invariants (specs/ir-pipeline/00).
//!
//! `ids` runs first and stops the run: it also proves every operand
//! precedes its user, so everything after can walk bottom-up by id.

mod ids;
mod rewrite;
mod scoping;
#[cfg(test)]
mod tests;

use petgraph::algo::is_cyclic_directed;
use petgraph::graph::DiGraph;

use super::{Mir, Origin, RelId, RelOp};
use crate::hir::Hir;
use crate::{Invariant, Violation};

pub use rewrite::verify_rewrite;

/// Checks the plan rooted at `mir.root`. With `hir` supplied, also
/// checks provenance references against it.
pub fn verify_mir(mir: &Mir, hir: Option<&Hir>) -> Vec<Violation> {
    let mut out = Vec::new();
    ids::check(mir, &mut out);
    if !out.is_empty() {
        return out;
    }
    let reachable = mir.reachable();
    declared_output(mir, &reachable, &mut out);
    scoping::defined_once(mir, &reachable, &mut out);
    scoping::visibility(mir, &reachable, &mut out);
    scoping::column_types(mir, &reachable, &mut out);
    cte_acyclic(mir, &reachable, &mut out);
    provenance(mir, hir, &reachable, &mut out);
    out
}

fn declared_output(mir: &Mir, reachable: &[RelId], out: &mut Vec<Violation>) {
    let bad = |detail: String| Violation::new(Invariant::MirDeclaredOutput, detail);
    for &id in reachable {
        let rel = &mir.rels[id];
        let computed = rel.op.computed_output(&mir.rels);
        if computed != rel.out {
            out.push(bad(format!(
                "rel {id:?} declares {:?}, computed {computed:?}",
                rel.out
            )));
        }
        if let RelOp::CteRef { cte, out: refs } = &rel.op {
            let declared = mir.ctes[*cte].out.len();
            if refs.len() != declared {
                out.push(bad(format!(
                    "cte ref {id:?} has {} columns, cte {cte:?} declares {declared}",
                    refs.len()
                )));
            }
        }
    }
    for (cte_id, cte) in mir.ctes.iter() {
        if reachable.contains(&cte.body) && mir.rels[cte.body].out != cte.out {
            out.push(bad(format!(
                "cte {cte_id:?} body output differs from declared"
            )));
        }
    }
}

fn cte_acyclic(mir: &Mir, reachable: &[RelId], out: &mut Vec<Violation>) {
    let mut graph = DiGraph::<(), ()>::new();
    let nodes: Vec<_> = mir.ctes.ids().map(|_| graph.add_node(())).collect();
    for (id, cte) in mir.ctes.iter() {
        if !reachable.contains(&cte.body) {
            continue;
        }
        for rel in cte_body_rels(mir, cte.body) {
            if let RelOp::CteRef { cte: target, .. } = mir.rels[rel].op {
                graph.add_edge(nodes[id.index()], nodes[target.index()], ());
            }
        }
    }
    if is_cyclic_directed(&graph) {
        out.push(Violation::new(
            Invariant::MirCteAcyclic,
            "cte reference graph has a cycle",
        ));
    }
}

/// `body` and its operands, stopping at CTE references.
fn cte_body_rels(mir: &Mir, body: RelId) -> Vec<RelId> {
    let mut seen = vec![false; mir.rels.len()];
    let mut stack = vec![body];
    let mut out = Vec::new();
    while let Some(id) = stack.pop() {
        if std::mem::replace(&mut seen[id.index()], true) {
            continue;
        }
        out.push(id);
        let rel = &mir.rels[id];
        stack.extend(rel.op.inputs());
        for scalar in rel.op.scalars() {
            scoping::subquery_rels(mir, scalar, &mut stack);
        }
    }
    out
}

fn provenance(mir: &Mir, hir: Option<&Hir>, reachable: &[RelId], out: &mut Vec<Violation>) {
    let bad = |detail: String| Violation::new(Invariant::MirProvenanceTotal, detail);
    let mut check = |id: super::ProvId, what: String| match mir.prov.entries.get(id) {
        None => out.push(bad(format!("{what} has provenance {id:?} out of range"))),
        Some(Origin::Hir(r)) => {
            if let Some(hir) = hir {
                if !hir_node_in_range(hir, r.node) {
                    out.push(bad(format!(
                        "{what} refers to missing HIR node {:?}",
                        r.node
                    )));
                }
            }
        }
        Some(Origin::Derived { from, .. }) => {
            if from.is_empty() || from.iter().any(|f| f.raw() >= id.raw()) {
                out.push(bad(format!("{what} derivation {id:?} is broken")));
            }
        }
    };
    for &id in reachable {
        check(mir.rels[id].prov, format!("rel {id:?}"));
        for scalar in mir.rels[id].op.scalars() {
            scoping::for_scalar_tree(mir, scalar, |sid| {
                check(mir.scalars[sid].prov, format!("scalar {sid:?}"));
            });
        }
    }
}

fn hir_node_in_range(hir: &Hir, node: super::HirNode) -> bool {
    use super::HirNode as N;
    match node {
        N::Query(id) => hir.queries.contains(id),
        N::From(id) => hir.from.contains(id),
        N::Expr(id) => hir.exprs.contains(id),
        N::Col(id) => hir.cols.contains(id),
        N::Jinja(id) => hir.jinja.contains(id),
        N::None => true,
    }
}

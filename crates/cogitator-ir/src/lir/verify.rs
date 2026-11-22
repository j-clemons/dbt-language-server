//! LIR invariants (specs/ir-pipeline/00). A `Lir` is verified against
//! the `Mir` it came from and its backend's capability table.

use super::{Capabilities, JoinHint, Lir, PhysOp, PruneUse};
use crate::mir::{ColId, Mir, ScalarId};
use crate::{Invariant, Violation};

pub fn verify_lir(lir: &Lir, mir: &Mir, caps: &Capabilities) -> Vec<Violation> {
    let mut out = Vec::new();
    ids(lir, mir, &mut out);
    if !out.is_empty() {
        return out;
    }
    maps_to_mir(lir, mir, &mut out);
    root_columns(lir, mir, &mut out);
    pruning_keys(lir, &mut out);
    hints(lir, caps, &mut out);
    out
}

fn ids(lir: &Lir, mir: &Mir, out: &mut Vec<Violation>) {
    let range = |detail: String| Violation::new(Invariant::LirIdRange, detail);
    if !lir.nodes.contains(lir.root) {
        out.push(range(format!("root {:?} is out of range", lir.root)));
    }
    let col_ok = |c: &ColId| mir.cols.contains(*c);
    let scalar_ok = |s: &ScalarId| mir.scalars.contains(*s);
    for (id, node) in lir.nodes.iter() {
        for input in node.op.inputs() {
            if !lir.nodes.contains(input) || input >= id {
                out.push(range(format!("input {input:?} of {id:?} is invalid")));
            }
        }
        if !node.op.scalars().iter().all(scalar_ok) || !node.out.iter().all(col_ok) {
            out.push(range(format!("node {id:?} has a bad scalar or column")));
        }
        match &node.op {
            PhysOp::TableScan {
                table,
                cols,
                pruning,
            } => {
                if !mir.tables.contains(*table) || !cols.iter().all(col_ok) {
                    out.push(range(format!("scan {id:?} has a bad table or column")));
                }
                for key in pruning {
                    let scalar = match key.usage {
                        PruneUse::Used(s) | PruneUse::Blocked { by: s, .. } => Some(s),
                        PruneUse::NotFiltered | PruneUse::Unknown => None,
                    };
                    if !col_ok(&key.col) || !scalar.iter().all(scalar_ok) {
                        out.push(range(format!("scan {id:?} has a bad pruning key")));
                    }
                }
            }
            PhysOp::Cte { cte, out: o, .. } => {
                if !mir.ctes.contains(*cte) || !o.iter().all(col_ok) {
                    out.push(range(format!("cte {id:?} has a bad cte or column")));
                }
            }
            PhysOp::Union { out: o, .. } | PhysOp::Opaque { out: o, .. }
                if !o.iter().all(col_ok) =>
            {
                out.push(range(format!("node {id:?} has a bad column")));
            }
            _ => {}
        }
    }
}

fn maps_to_mir(lir: &Lir, mir: &Mir, out: &mut Vec<Violation>) {
    let reachable = mir.reachable();
    for (id, node) in lir.nodes.iter() {
        if reachable.binary_search(&node.mir).is_err() {
            out.push(Violation::new(
                Invariant::LirMapsToMir,
                format!("node {id:?} maps to {:?}, not in the MIR plan", node.mir),
            ));
        }
    }
}

fn root_columns(lir: &Lir, mir: &Mir, out: &mut Vec<Violation>) {
    let Some(root) = lir.nodes.get(lir.root) else {
        return;
    };
    if root.out != mir.root_columns() {
        out.push(Violation::new(
            Invariant::LirRootColumns,
            format!(
                "root output {:?} differs from MIR {:?}",
                root.out,
                mir.root_columns()
            ),
        ));
    }
}

fn pruning_keys(lir: &Lir, out: &mut Vec<Violation>) {
    for (id, node) in lir.nodes.iter() {
        if let PhysOp::TableScan { cols, pruning, .. } = &node.op {
            for key in pruning {
                if !cols.contains(&key.col) {
                    out.push(Violation::new(
                        Invariant::LirPruningKeyOwned,
                        format!("scan {id:?} prunes on {:?} it does not read", key.col),
                    ));
                }
            }
        }
    }
}

fn hints(lir: &Lir, caps: &Capabilities, out: &mut Vec<Violation>) {
    let bad = |detail: String| Violation::new(Invariant::LirHintSupported, detail);
    for (id, node) in lir.nodes.iter() {
        match &node.op {
            PhysOp::Join { hint, .. }
                if *hint != JoinHint::None && !caps.join_hints.contains(hint) =>
            {
                out.push(bad(format!("join {id:?} uses unsupported hint {hint:?}")));
            }
            PhysOp::Cte { policy, .. } if !caps.cte_policies.contains(policy) => {
                out.push(bad(format!(
                    "cte {id:?} uses unsupported policy {policy:?}"
                )));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::JoinKind;
    use crate::testkit::{int_col, lir::generic_caps, LirBuilder, MirBuilder};

    fn plan() -> (Mir, Lir) {
        let mut b = MirBuilder::new();
        let scan = b.scan("t", &[("a", int_col()), ("b", int_col())]);
        let mir = b.finish(scan);
        let mut lb = LirBuilder::new();
        let root = lb.scan(&mir, scan);
        let lir = lb.finish(root);
        (mir, lir)
    }

    fn assert_invariant(violations: &[Violation], invariant: Invariant) {
        assert!(
            violations.iter().any(|v| v.invariant == invariant),
            "expected {invariant:?} in {violations:#?}"
        );
    }

    #[test]
    fn clean_plan_verifies() {
        let (mir, lir) = plan();
        crate::assert_clean(&verify_lir(&lir, &mir, &generic_caps()));
    }

    #[test]
    fn bad_root_is_an_id_range_violation() {
        let (mir, mut lir) = plan();
        lir.root = super::super::PhysId::from_raw(9);
        assert_invariant(
            &verify_lir(&lir, &mir, &generic_caps()),
            Invariant::LirIdRange,
        );
    }

    #[test]
    fn unmapped_node_is_a_maps_to_mir_violation() {
        let (mir, mut lir) = plan();
        // a RelId in range but not in the plan
        let mut mir2 = mir;
        let orphan = mir2.rels.iter().next().map(|(id, r)| (id, r.clone()));
        let (_, rel) = orphan.expect("plan has a rel");
        let dangling = mir2.rels.alloc(rel);
        let root = lir.root;
        lir.nodes[root].mir = dangling;
        assert_invariant(
            &verify_lir(&lir, &mir2, &generic_caps()),
            Invariant::LirMapsToMir,
        );
    }

    #[test]
    fn dropped_root_column_is_detected() {
        let (mir, mut lir) = plan();
        let root = lir.root;
        lir.nodes[root].out.pop();
        assert_invariant(
            &verify_lir(&lir, &mir, &generic_caps()),
            Invariant::LirRootColumns,
        );
    }

    #[test]
    fn foreign_pruning_key_is_detected() {
        let (mir, mut lir) = plan();
        let root = lir.root;
        let other = mir.cols.iter().last().map(|(id, _)| id).expect("cols");
        if let PhysOp::TableScan { cols, pruning, .. } = &mut lir.nodes[root].op {
            cols.retain(|c| *c != other);
            pruning.push(super::super::PruneKey {
                col: other,
                kind: super::super::KeyKind::Partition,
                usage: PruneUse::NotFiltered,
            });
        }
        assert_invariant(
            &verify_lir(&lir, &mir, &generic_caps()),
            Invariant::LirPruningKeyOwned,
        );
    }

    #[test]
    fn unsupported_hint_is_detected() {
        let (mir, lir) = plan();
        let root = lir.root;
        let mut lb = LirBuilder::new();
        let scan = lir.nodes[root].clone();
        let l = lb.node(scan.op.clone(), scan.mir, scan.out.clone());
        let r = lb.node(scan.op.clone(), scan.mir, vec![]);
        let join = lb.join(
            JoinKind::Inner,
            l,
            r,
            vec![],
            JoinHint::Broadcast,
            None,
            scan.mir,
        );
        let lir = lb.finish(join);
        let caps = generic_caps();
        assert_invariant(&verify_lir(&lir, &mir, &caps), Invariant::LirHintSupported);
    }
}

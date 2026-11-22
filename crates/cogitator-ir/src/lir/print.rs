//! Deterministic LIR text, read together with its `Mir`.

use super::{Estimate, Lir, PhysId, PhysOp, PruneKey, PruneUse};
use crate::mir::print::{cols, scalar, table_name};
use crate::mir::{Mir, ScalarId};
use crate::print_util::MAX_PRINT_DEPTH;

pub fn print_lir(lir: &Lir, mir: &Mir) -> String {
    let mut out = format!(
        "backend {:?} sink {:?} partition=[{}] cluster=[{}] contract={}\n",
        lir.backend,
        lir.sink.mat,
        cols(mir, &lir.sink.partition_by),
        cols(mir, &lir.sink.cluster_by),
        lir.sink.contract,
    );
    node(lir, mir, lir.root, 0, &mut out);
    out
}

fn est(e: &Estimate) -> String {
    match e {
        Estimate::Unknown => "est=unknown".to_string(),
        Estimate::Stats { rows } => format!("est=stats({rows})"),
        Estimate::Heuristic(b) => format!("est=heuristic({b:?})"),
    }
}

fn prune(mir: &Mir, key: &PruneKey) -> String {
    let usage = match key.usage {
        PruneUse::Used(s) => format!("used {}", scalar(mir, s)),
        PruneUse::Blocked { by, reason } => format!("blocked {reason:?} by {}", scalar(mir, by)),
        PruneUse::NotFiltered => "not-filtered".to_string(),
        PruneUse::Unknown => "unknown".to_string(),
    };
    format!("{} {:?} {usage}", cols(mir, &[key.col]), key.kind)
}

fn node(lir: &Lir, mir: &Mir, id: PhysId, depth: usize, out: &mut String) {
    let pad = "  ".repeat(depth);
    if depth > MAX_PRINT_DEPTH {
        out.push_str(&format!("{pad}...\n"));
        return;
    }
    let Some(n) = lir.nodes.get(id) else {
        out.push_str(&format!("{pad}<bad node>\n"));
        return;
    };
    let scalars = |xs: &[ScalarId]| {
        xs.iter()
            .map(|s| scalar(mir, *s))
            .collect::<Vec<_>>()
            .join(" AND ")
    };
    let head = match &n.op {
        PhysOp::TableScan {
            table,
            cols: c,
            pruning,
        } => {
            let keys: Vec<String> = pruning.iter().map(|k| prune(mir, k)).collect();
            format!(
                "TableScan {} [{}] prune=[{}]",
                table_name(mir, *table),
                cols(mir, c),
                keys.join("; ")
            )
        }
        PhysOp::Filter { conjuncts, .. } => format!("Filter {}", scalars(conjuncts)),
        PhysOp::Project { items, .. } => {
            let ids: Vec<_> = items.iter().map(|(c, _)| *c).collect();
            format!("Project [{}]", cols(mir, &ids))
        }
        PhysOp::Join {
            kind,
            on,
            hint,
            build,
            ..
        } => format!(
            "Join {kind:?} hint={hint:?} build={build:?} on {}",
            scalars(on)
        ),
        PhysOp::Aggregate { keys, aggs, .. } => {
            let ks: Vec<_> = keys.iter().map(|(c, _)| *c).collect();
            format!(
                "Aggregate keys=[{}] aggs=[{}]",
                cols(mir, &ks),
                cols(mir, aggs)
            )
        }
        PhysOp::Window { funcs, .. } => format!("Window [{}]", cols(mir, funcs)),
        PhysOp::Sort { keys, .. } => {
            let ks: Vec<ScalarId> = keys.iter().map(|k| k.expr).collect();
            format!("Sort {}", scalars(&ks))
        }
        PhysOp::Limit { limit, offset, .. } => format!(
            "Limit {} offset {}",
            limit.map_or("none".to_string(), |s| scalar(mir, s)),
            offset.map_or("none".to_string(), |s| scalar(mir, s)),
        ),
        PhysOp::Union { all, out: o, .. } => format!("Union all={all} [{}]", cols(mir, o)),
        PhysOp::Cte {
            policy,
            uses,
            cte,
            out: o,
            ..
        } => format!(
            "Cte #{} policy={policy:?} uses={uses} [{}]",
            cte.raw(),
            cols(mir, o)
        ),
        PhysOp::Opaque { out: o, .. } => format!("Opaque [{}]", cols(mir, o)),
    };
    out.push_str(&format!("{pad}{head} {}\n", est(&n.est)));
    for input in n.op.inputs() {
        node(lir, mir, input, depth + 1, out);
    }
}

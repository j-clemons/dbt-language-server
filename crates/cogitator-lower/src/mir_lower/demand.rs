//! Which columns of open schemas the query reads. A `Scan`'s columns are
//! fixed when it is allocated, so this runs first, over the arenas.

use std::collections::BTreeMap;

use cogitator_ir::hir::{CteId, FromId, FromKind, Hir, JoinCond, RelTarget, Resolution};
use cogitator_ir::Symbol;

#[derive(Default)]
pub(super) struct Demand {
    from: BTreeMap<FromId, Vec<Symbol>>,
    cte: BTreeMap<CteId, Vec<Symbol>>,
}

impl Demand {
    pub(super) fn of(hir: &Hir) -> Self {
        let mut demand = Demand::default();
        for col in hir.cols.values() {
            match &col.res {
                Some(Resolution::OpenSchema(from)) => demand.add_from(*from, col.name.folded),
                // A named column of an open item is still read through
                // it: an open CTE only exposes what is demanded of it.
                Some(Resolution::Bound { from, .. } | Resolution::Outer { from, .. })
                    if hir.from.get(*from).is_some_and(|item| item.open) =>
                {
                    demand.add_from(*from, col.name.folded);
                }
                _ => {}
            }
        }
        for item in hir.from.values() {
            if let FromKind::Join {
                l,
                r,
                on: JoinCond::Using(names),
                ..
            } = &item.kind
            {
                for side in [*l, *r] {
                    for name in names {
                        demand.add_using(hir, side, name.folded);
                    }
                }
            }
        }
        for (id, item) in hir.from.iter() {
            if let FromKind::Base {
                target: RelTarget::Cte(cte),
                ..
            } = &item.kind
            {
                let names = demand.from.get(&id).cloned().unwrap_or_default();
                let slot = demand.cte.entry(*cte).or_default();
                for name in names {
                    push_unique(slot, name);
                }
            }
        }
        demand
    }

    /// A USING column on a side where exactly one open leaf can supply it.
    fn add_using(&mut self, hir: &Hir, side: FromId, name: Symbol) {
        let leaves = leaves(hir, side);
        let known = leaves.iter().any(|leaf| {
            hir.from
                .get(*leaf)
                .is_some_and(|item| item.output.iter().any(|col| col.name == name))
        });
        if known {
            return;
        }
        let mut open = leaves
            .iter()
            .filter(|leaf| hir.from.get(**leaf).is_some_and(|item| item.open));
        if let (Some(leaf), None) = (open.next(), open.next()) {
            self.add_from(*leaf, name);
        }
    }

    fn add_from(&mut self, from: FromId, name: Symbol) {
        push_unique(self.from.entry(from).or_default(), name);
    }

    pub(super) fn for_from(&self, from: FromId) -> &[Symbol] {
        self.from.get(&from).map_or(&[], Vec::as_slice)
    }

    pub(super) fn for_cte(&self, cte: CteId) -> &[Symbol] {
        self.cte.get(&cte).map_or(&[], Vec::as_slice)
    }
}

fn push_unique(slot: &mut Vec<Symbol>, name: Symbol) {
    if !slot.contains(&name) {
        slot.push(name);
    }
}

/// The non-join items under `from`, left to right. Iterative and bounded
/// by the arena length, so a cycle in unverified HIR cannot hang it.
pub(super) fn leaves(hir: &Hir, from: FromId) -> Vec<FromId> {
    let mut out = Vec::new();
    let mut stack = vec![from];
    let mut seen = 0;
    while let Some(id) = stack.pop() {
        seen += 1;
        if seen > hir.from.len() {
            break;
        }
        match hir.from.get(id).map(|item| &item.kind) {
            Some(FromKind::Join { l, r, .. }) => {
                stack.push(*r);
                stack.push(*l);
            }
            Some(_) => out.push(id),
            None => {}
        }
    }
    out
}

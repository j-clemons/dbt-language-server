//! The heaviest ordered chain of segment occurrences (specs/ir-pipeline/01).

use std::cmp::Reverse;

use super::align::Normalized;

const MAX_OCCURRENCES_PER_SEGMENT: usize = 4096;
const MAX_OCCURRENCES: usize = 400_000;

#[derive(Debug, Clone, Copy)]
pub(super) struct Occurrence {
    pub(super) segment: u32,
    pub(super) pos: u32,
    pub(super) len: u32,
}

/// The heaviest chain of occurrences, increasing in both segment and
/// position, without overlap.
pub(super) fn best_chain(needles: &[Normalized], haystack: &Normalized) -> Vec<Occurrence> {
    let occurrences = occurrences(needles, haystack);
    let n = haystack.text.len();
    let mut tree = BestFrom::new(n);
    // Per occurrence: its chain's weight and the next occurrence in it.
    let mut links: Vec<(u64, Option<u32>)> = vec![(0, None); occurrences.len()];
    // Later segments first; a segment's own occurrences never chain.
    let mut end = occurrences.len();
    for group in occurrences.chunk_by(|a, b| a.segment == b.segment).rev() {
        let base = end - group.len();
        end = base;
        let mut scored = Vec::with_capacity(group.len());
        for (i, occurrence) in group.iter().enumerate() {
            let from = (occurrence.pos + occurrence.len) as usize;
            let next = tree.query(from);
            let weight = u64::from(occurrence.len) + next.map_or(0, |(w, _, _)| w);
            let id = (base + i) as u32;
            links[id as usize] = (weight, next.map(|(_, _, id)| id));
            scored.push((
                occurrence.pos as usize,
                (weight, Reverse(occurrence.pos), id),
            ));
        }
        for (pos, best) in scored {
            tree.update(pos, best);
        }
    }
    let mut chain = Vec::new();
    let mut at = tree.query(0).map(|(_, _, id)| id);
    while let Some(id) = at {
        let Some(&occurrence) = occurrences.get(id as usize) else {
            break;
        };
        chain.push(occurrence);
        at = links.get(id as usize).and_then(|&(_, next)| next);
    }
    chain
}

fn occurrences(needles: &[Normalized], haystack: &Normalized) -> Vec<Occurrence> {
    let mut out = Vec::new();
    'segments: for (segment, needle) in needles.iter().enumerate() {
        if needle.text.is_empty() {
            continue;
        }
        let found = haystack
            .text
            .match_indices(needle.text.as_str())
            .take(MAX_OCCURRENCES_PER_SEGMENT);
        for (pos, _) in found {
            if out.len() >= MAX_OCCURRENCES {
                break 'segments;
            }
            out.push(Occurrence {
                segment: segment as u32,
                pos: pos as u32,
                len: needle.text.len() as u32,
            });
        }
    }
    out
}

/// Maximum over positions at or after a query point: a Fenwick tree on
/// reversed positions. Ties favour the earlier position.
struct BestFrom {
    tree: Vec<Option<Best>>,
}

type Best = (u64, Reverse<u32>, u32);

impl BestFrom {
    fn new(positions: usize) -> Self {
        BestFrom {
            tree: vec![None; positions + 1],
        }
    }

    fn update(&mut self, pos: usize, value: Best) {
        let mut i = self.tree.len().saturating_sub(1).saturating_sub(pos);
        while let Some(slot) = self.tree.get_mut(i).filter(|_| i > 0) {
            if *slot < Some(value) {
                *slot = Some(value);
            }
            i += i.isolate_lowest_one();
        }
    }

    fn query(&self, from: usize) -> Option<Best> {
        let mut i = self.tree.len().saturating_sub(1).saturating_sub(from);
        let mut best = None;
        while i > 0 {
            best = best.max(self.tree.get(i).copied().flatten());
            i &= i - 1;
        }
        best
    }
}

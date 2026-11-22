//! Canonical text of a MIR subplan, for `cross.duplicate_logic`.
//!
//! `plan_hash` is the right tool for "did this plan change" but not for
//! "are these two subplans the same": it prints `ColId`s, so a subplan
//! embedded in another model prints differently, and it elides window
//! partitions, sort directions and aggregate DISTINCT. This renders a
//! subtree with columns numbered by first occurrence (scans by column
//! name, so scan column order does not matter), inlines CTEs and keeps
//! everything a comparison must keep. Two subplans with equal text read
//! the same inputs and compute the same result. The FNV-1a hash only
//! narrows candidates; callers confirm on the text.
//!
//! A subplan with Jinja, a hole, `{{ this }}` or an opaque scalar has no
//! provable meaning, so it never yields text.

use std::sync::Arc;

use cogitator_ir::common::Lit;
use cogitator_ir::mir::{Mir, RelId, RelOp};

mod render;
mod scalars;

use render::Render;

/// Below this many relational operators a shared subplan is a coincidence
/// of shape: one scan, a filter and a projection look alike by accident.
pub const MIN_RELS: u32 = 4;

/// A model keeps its largest candidates only: nested ones add little.
const MAX_CANDIDATES: usize = 8;
const MAX_MODEL_STEPS: usize = 200_000;
/// Past this the model is left out rather than rendered quadratically.
const MAX_RELS: usize = 1_024;

/// One comparable subtree of a model's canonical plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subplan {
    /// Where it sits in the plan it came from; meaningless in any other.
    pub at: RelId,
    pub hash: u64,
    /// Relational operators, CTE bodies counted where they are used.
    pub size: u32,
    pub text: Arc<str>,
}

/// FNV-1a, the same function `plan_hash` uses.
pub fn fnv1a(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The model's comparable subplans, largest first, in a fixed order.
pub fn subplans_of(mir: &Mir) -> Vec<Subplan> {
    bounded_subplans(mir).0
}

/// The candidates, and whether the count bounds left any out, for the
/// schema job's explicit degradation.
pub fn bounded_subplans(mir: &Mir) -> (Vec<Subplan>, bool) {
    let reachable = mir.reachable();
    if reachable.len() > MAX_RELS {
        return (Vec::new(), false);
    }
    let mut spent = 0;
    let mut complete = true;
    let mut found: Vec<Subplan> = Vec::new();
    for at in reachable {
        let leaf = matches!(
            mir.rels.get(at).map(|r| &r.op),
            None | Some(RelOp::Scan { .. } | RelOp::CteRef { .. } | RelOp::Opaque { .. })
        );
        if leaf {
            continue;
        }
        if spent >= MAX_MODEL_STEPS {
            complete = false;
            continue;
        }
        let mut render = Render::new(mir);
        render.rel(at);
        spent += render.steps;
        if render.ok && render.work && render.rels >= MIN_RELS {
            found.push(Subplan {
                at,
                hash: fnv1a(&render.out),
                size: render.rels,
                text: Arc::from(render.out),
            });
        }
    }
    found.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.text.cmp(&b.text)));
    found.dedup_by(|a, b| a.text == b.text);
    found.truncate(MAX_CANDIDATES);
    (found, complete)
}

/// True when the subtree does work an engine pays for each time.
pub fn is_work(op: &RelOp) -> bool {
    matches!(
        op,
        RelOp::Join { .. } | RelOp::Aggregate { .. } | RelOp::Window { .. }
    )
}

/// True for text the front end masked over a Jinja region (`_jjj`). Jinja
/// inside a string literal reaches the plan as a literal of that shape,
/// so two different `'{{ var(...) }}'` would otherwise compare equal.
fn holds_masked_jinja(text: &str) -> bool {
    text.contains("_jj")
}

/// A literal's canonical text, `None` when it is masked Jinja rather
/// than a value. Strings carry their length so an odd literal cannot
/// imitate the surrounding structure.
pub fn lit_text(mir: &Mir, lit: &Lit) -> Option<String> {
    let sized = |s: &str| -> Option<String> {
        (!holds_masked_jinja(s)).then(|| format!("{}:{s}", s.len()))
    };
    Some(match lit {
        Lit::Null => "null".to_string(),
        Lit::Bool(b) => b.to_string(),
        Lit::Number(n) => format!("n{}", mir.syms.text(*n)),
        Lit::Str(s) => format!("s{}", sized(mir.syms.text(*s))?),
        Lit::Typed { ty, value } => format!(
            "t{}={}",
            mir.syms.text(*ty).to_ascii_lowercase(),
            sized(mir.syms.text(*value))?
        ),
        Lit::Interval(s) => format!("i{}", sized(mir.syms.text(*s))?),
    })
}

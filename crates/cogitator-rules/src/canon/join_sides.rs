//! Which inputs of a join a predicate may move into, by join kind.

use cogitator_ir::common::JoinKind;

/// Which join inputs a predicate may move into.
#[derive(Debug, Clone, Copy)]
pub(super) struct Sides {
    pub left: bool,
    pub right: bool,
}

/// A WHERE above the join. It sees null-extended rows, so moving it into
/// a null-supplying input would stop it rejecting them.
pub(super) fn where_sides(kind: JoinKind) -> Sides {
    match kind {
        JoinKind::Inner | JoinKind::Cross => Sides {
            left: true,
            right: true,
        },
        JoinKind::Left | JoinKind::Semi | JoinKind::Anti => Sides {
            left: true,
            right: false,
        },
        JoinKind::Right => Sides {
            left: false,
            right: true,
        },
        JoinKind::Full => Sides {
            left: false,
            right: false,
        },
    }
}

/// An ON condition. It decides matching, and a preserved input keeps its
/// row when the match fails, so only the other input may be filtered.
pub(super) fn on_sides(kind: JoinKind) -> Sides {
    match kind {
        JoinKind::Inner | JoinKind::Cross | JoinKind::Semi => Sides {
            left: true,
            right: true,
        },
        JoinKind::Left | JoinKind::Anti => Sides {
            left: false,
            right: true,
        },
        JoinKind::Right => Sides {
            left: true,
            right: false,
        },
        JoinKind::Full => Sides {
            left: false,
            right: false,
        },
    }
}

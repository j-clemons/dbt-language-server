//! Name resolution: frames mirror the query nesting, the innermost last.

use std::collections::HashMap;

use cogitator_ir::hir::{FromId, Resolution, ScopeId};
use cogitator_ir::Symbol;

use super::{Lowerer, INDEXED_WIDTH};

/// Whether and how select-list aliases are visible in the clause being
/// lowered. Input columns win except in ORDER BY.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AliasMode {
    Off,
    /// Earlier select aliases, where the dialect allows them (WHERE, the
    /// select list itself, HAVING, QUALIFY).
    Lateral,
    /// GROUP BY: every dialect accepts a select alias after inputs.
    GroupBy,
    /// ORDER BY: output names win over input columns.
    OrderBy,
}

#[derive(Debug)]
pub(super) struct Frame {
    pub scope: ScopeId,
    /// FROM leaves in source order; only these are name-bearing.
    pub items: Vec<FromId>,
    /// Columns merged by JOIN … USING: unambiguous when unqualified.
    pub using: Vec<Symbol>,
    /// `(name, select-item index)` for aliases already visible.
    pub aliases: Vec<(Symbol, u32)>,
    /// Name index over a wide `aliases`; lookup only, first entry wins.
    pub alias_index: Option<HashMap<Symbol, u32>>,
    /// Stops the upward walk: CTE bodies and non-lateral derived tables.
    pub barrier: bool,
    /// A set operation's ORDER BY sees outputs that may be incomplete.
    pub open_outputs: bool,
}

impl Frame {
    pub fn new(scope: ScopeId, barrier: bool) -> Self {
        Frame {
            scope,
            items: Vec::new(),
            using: Vec::new(),
            aliases: Vec::new(),
            alias_index: None,
            barrier,
            open_outputs: false,
        }
    }

    /// Replaces the aliases wholesale, indexing them when wide.
    pub fn set_aliases(&mut self, aliases: Vec<(Symbol, u32)>) {
        self.alias_index = (aliases.len() > INDEXED_WIDTH).then(|| {
            let mut index = HashMap::with_capacity(aliases.len());
            for (name, item) in &aliases {
                index.entry(*name).or_insert(*item);
            }
            index
        });
        self.aliases = aliases;
    }
}

impl Lowerer<'_> {
    /// Resolves one reference against the frame stack.
    pub(super) fn resolve(&self, qualifier: &[Symbol], name: Symbol) -> Resolution {
        for (depth, frame) in self.frames.iter().rev().enumerate() {
            let hit = if qualifier.is_empty() {
                self.resolve_unqualified(frame, name, depth)
            } else {
                self.resolve_qualified(frame, qualifier, name, depth)
            };
            if let Some(res) = hit {
                return res;
            }
            if frame.barrier {
                break;
            }
        }
        Resolution::Unresolved
    }

    fn at_depth(&self, from: FromId, col: u32, depth: usize) -> Resolution {
        if depth == 0 {
            Resolution::Bound { from, col }
        } else {
            Resolution::Outer {
                depth: depth.min(u8::MAX as usize) as u8,
                from,
                col,
            }
        }
    }

    /// `true` when `qualifier` names `from`, matching path tails so
    /// `schema.t.col` finds `from schema.t`.
    pub(super) fn names_from(&self, from: FromId, qualifier: &[Symbol]) -> bool {
        !qualifier.is_empty()
            && self
                .from_names
                .get(from.index())
                .is_some_and(|names| names.ends_with(qualifier))
    }

    /// Index of `name` in the output of `from`.
    fn col_of(&self, from: FromId, name: Symbol) -> Option<u32> {
        if let Some(Some(index)) = self.from_index.get(from.index()) {
            return index.get(&name).copied();
        }
        let item = self.hir.from.get(from)?;
        item.output
            .iter()
            .position(|c| c.name == name)
            .map(|i| i as u32)
    }

    fn is_open(&self, from: FromId) -> bool {
        self.hir.from.get(from).is_some_and(|f| f.open)
    }

    fn resolve_qualified(
        &self,
        frame: &Frame,
        qualifier: &[Symbol],
        name: Symbol,
        depth: usize,
    ) -> Option<Resolution> {
        let from = frame
            .items
            .iter()
            .copied()
            .find(|f| self.names_from(*f, qualifier))?;
        Some(match self.col_of(from, name) {
            Some(col) => self.at_depth(from, col, depth),
            None if self.is_open(from) => Resolution::OpenSchema(from),
            None => Resolution::Unresolved,
        })
    }

    fn resolve_unqualified(&self, frame: &Frame, name: Symbol, depth: usize) -> Option<Resolution> {
        if depth == 0 && self.mode == AliasMode::OrderBy {
            if let Some(hit) = self.alias_hit(frame, name) {
                return Some(hit);
            }
        }
        let hits: Vec<(FromId, u32)> = frame
            .items
            .iter()
            .filter_map(|&f| self.col_of(f, name).map(|col| (f, col)))
            .collect();
        // Open items that did not already answer: an open item that has
        // the name is a hit, not a second possibility.
        let opens: Vec<FromId> = frame
            .items
            .iter()
            .copied()
            .filter(|f| self.is_open(*f) && !hits.iter().any(|(hit, _)| hit == f))
            .collect();
        if frame.using.contains(&name) {
            if let Some(&(from, col)) = hits.first() {
                return Some(self.at_depth(from, col, depth));
            }
        }
        match (hits.as_slice(), opens.as_slice()) {
            ([(from, col)], []) => Some(self.at_depth(*from, *col, depth)),
            ([], []) => self.fallback_alias(frame, name, depth),
            ([], [from]) => Some(Resolution::OpenSchema(*from)),
            _ => {
                let mut candidates: Vec<FromId> = hits.iter().map(|(f, _)| *f).collect();
                candidates.extend(opens);
                Some(Resolution::Candidates(candidates))
            }
        }
    }

    /// Aliases are consulted last, and only in the frame being lowered.
    fn fallback_alias(&self, frame: &Frame, name: Symbol, depth: usize) -> Option<Resolution> {
        if depth != 0 {
            return None;
        }
        let allowed = match self.mode {
            AliasMode::Off => false,
            AliasMode::Lateral => self.rules.lateral_alias,
            AliasMode::GroupBy | AliasMode::OrderBy => true,
        };
        if !allowed {
            return None;
        }
        self.alias_hit(frame, name).or_else(|| {
            // ORDER BY over a set operation whose outputs are incomplete
            frame
                .open_outputs
                .then(|| Resolution::Candidates(Vec::new()))
        })
    }

    fn alias_hit(&self, frame: &Frame, name: Symbol) -> Option<Resolution> {
        if let Some(index) = &frame.alias_index {
            return index.get(&name).map(|item| Resolution::LateralAlias(*item));
        }
        frame
            .aliases
            .iter()
            .find(|(alias, _)| *alias == name)
            .map(|(_, item)| Resolution::LateralAlias(*item))
    }
}

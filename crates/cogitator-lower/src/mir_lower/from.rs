//! FROM items: scans, CTE references, derived tables, joins and UNNEST.
//! Each item records a `Binding`.

use cogitator_ir::common::{HoleKind, JoinKind};
use cogitator_ir::hir::{self, From, FromId, FromKind, QueryId, RelTarget};
use cogitator_ir::mir::{ColId, HirNode, ProvId, RelId, RelOp, TableDef, TableRef};
use cogitator_ir::schema::Completeness;
use cogitator_ir::{ColType, DegradationKind, Span, Symbol};

use super::{Binding, Ctx, OpaqueSpec, MAX_FROM_DEPTH};

impl Ctx<'_> {
    /// Comma-separated items are cross joins, in source order.
    pub(super) fn lower_from_list(&mut self, items: &[FromId], prov: ProvId) -> RelId {
        let hir = self.hir;
        let mut acc: Option<RelId> = None;
        for &item in items {
            let unnest = hir
                .from
                .get(item)
                .is_some_and(|f| matches!(f.kind, FromKind::Unnest { .. }));
            acc = Some(match acc {
                Some(left) if unnest => self.unnest(left, item, JoinKind::Cross, false),
                Some(left) => {
                    let right = self.from(item, false);
                    self.mir.add_rel(
                        RelOp::Join {
                            kind: JoinKind::Cross,
                            l: left,
                            r: right,
                            on: Vec::new(),
                        },
                        prov,
                    )
                }
                None => self.from(item, false),
            });
        }
        match acc {
            Some(rel) => rel,
            None => self.singleton(prov),
        }
    }

    pub(super) fn from(&mut self, id: FromId, nullable: bool) -> RelId {
        let hir = self.hir;
        let Some(item) = hir.from.get(id) else {
            self.bug(format!("FROM item {id:?} is out of range"));
            let prov = self.prov(HirNode::None, Span::default());
            return self.singleton(prov);
        };
        let prov = self.prov(HirNode::From(id), item.span);
        if self.from_depth >= MAX_FROM_DEPTH || !self.tick() {
            self.cap(item.span, "this FROM clause");
            return self.opaque_from(id, item, HoleKind::Truncated, nullable, prov);
        }
        self.from_depth += 1;
        let rel = match &item.kind {
            FromKind::Base { target, .. } => self.base(id, item, target, nullable, prov),
            FromKind::Derived { q, lateral, .. } => {
                self.derived(id, item, *q, *lateral, nullable, prov)
            }
            FromKind::Join { kind, l, r, on } => self.join(id, *kind, *l, *r, on, nullable, prov),
            FromKind::Unnest { .. } | FromKind::TableFn { .. } => {
                self.degrade(
                    DegradationKind::UnsupportedSyntax,
                    item.span,
                    "a table function with no input relation is not modelled",
                );
                self.opaque_from(id, item, HoleKind::Unsupported, nullable, prov)
            }
            FromKind::Hole => {
                self.degrade(
                    DegradationKind::ParseHole,
                    item.span,
                    "this FROM item did not parse",
                );
                self.opaque_from(id, item, HoleKind::ParseError, nullable, prov)
            }
        };
        self.from_depth -= 1;
        rel
    }

    /// Known outputs plus what the query reads from an open item.
    pub(super) fn scan_columns(&self, id: FromId, item: &From) -> Vec<(Option<Symbol>, ColType)> {
        let mut cols: Vec<(Option<Symbol>, ColType)> = item
            .output
            .iter()
            .map(|c| (Some(c.name), c.ty.clone()))
            .collect();
        if item.open {
            for name in self.demand.for_from(id) {
                if !item.output.iter().any(|c| c.name == *name) {
                    cols.push((Some(*name), ColType::UNKNOWN));
                }
            }
        }
        cols
    }

    fn opaque_from(
        &mut self,
        id: FromId,
        item: &From,
        kind: HoleKind,
        nullable: bool,
        prov: ProvId,
    ) -> RelId {
        let spec = OpaqueSpec {
            cols: self.scan_columns(id, item),
            open: item.open,
            kind,
            span: item.span,
        };
        let (rel, cols) = self.opaque_rel(Vec::new(), spec, prov);
        self.bind(id, cols, nullable);
        rel
    }

    pub(super) fn bind(&mut self, id: FromId, cols: Vec<ColId>, nullable: bool) {
        let depth = self.frames.len();
        self.bindings.insert(
            id,
            Binding {
                cols,
                depth,
                nullable,
            },
        );
    }

    fn base(
        &mut self,
        id: FromId,
        item: &From,
        target: &RelTarget,
        nullable: bool,
        prov: ProvId,
    ) -> RelId {
        let table = match target {
            RelTarget::Cte(cte) => return self.cte_ref(id, item, *cte, nullable, prov),
            RelTarget::Model {
                package,
                name,
                version,
            } => TableRef::Model {
                package: *package,
                name: *name,
                version: *version,
            },
            RelTarget::Source { source, table } => TableRef::Source {
                source: *source,
                table: *table,
            },
            RelTarget::This => TableRef::This,
            RelTarget::Physical(path) => {
                TableRef::Physical(path.iter().map(|ident| ident.folded).collect())
            }
            RelTarget::Jinja(_) => TableRef::Jinja,
        };
        let completeness = if item.open {
            Completeness::Open
        } else {
            Completeness::Closed
        };
        let table = self.mir.tables.alloc(TableDef {
            target: table,
            completeness,
        });
        let cols: Vec<ColId> = self
            .scan_columns(id, item)
            .into_iter()
            .map(|(name, ty)| self.mir.add_col(name, ty))
            .collect();
        let rel = self.mir.add_rel(
            RelOp::Scan {
                table,
                cols: cols.clone(),
                open: item.open,
            },
            prov,
        );
        self.bind(id, cols, nullable);
        rel
    }

    fn cte_ref(
        &mut self,
        id: FromId,
        item: &From,
        cte: hir::CteId,
        nullable: bool,
        prov: ProvId,
    ) -> RelId {
        let def = self
            .ctes
            .get(&cte)
            .copied()
            .and_then(|mir_cte| self.mir.ctes.get(mir_cte).map(|d| (mir_cte, d.out.clone())));
        let Some((mir_cte, defs)) = def else {
            self.degrade(
                DegradationKind::UnsupportedSyntax,
                item.span,
                "this CTE reference has no lowered definition",
            );
            return self.opaque_from(id, item, HoleKind::Unsupported, nullable, prov);
        };
        let out: Vec<ColId> = defs
            .iter()
            .map(|col| {
                let name = self.col_name(*col);
                let ty = self.col_type(*col);
                self.mir.add_col(name, ty)
            })
            .collect();
        let rel = self.mir.add_rel(
            RelOp::CteRef {
                cte: mir_cte,
                out: out.clone(),
            },
            prov,
        );
        self.bind(id, out, nullable);
        rel
    }

    fn derived(
        &mut self,
        id: FromId,
        item: &From,
        q: QueryId,
        lateral: bool,
        nullable: bool,
        prov: ProvId,
    ) -> RelId {
        // boundary: the demand list outlives the mutable walk below
        let demand: Vec<Symbol> = self.demand.for_from(id).to_vec();
        let out = if lateral {
            self.isolated(q, &demand, item.span, prov)
        } else {
            self.query(q, &demand)
        };
        self.bind(id, out.cols, nullable);
        out.rel
    }
}

//! Compiled-SQL fallback (specs/ir-pipeline/01): HIR built from
//! `target/compiled`, with spans mapped back to the template.
//!
//! Stages: [`prepare`] finds the relations dbt rendered and the CTEs it
//! injected, [`align`] anchors the template's literal SQL to the compiled
//! text, and [`build_compiled_hir`] lowers the result. Physical relations
//! become the `RelTarget::{Model, Source, This}` a template's `ref` would
//! have lowered to, via regions the lowerer already understands.

use cogitator_ir::hir::Hir;
use cogitator_ir::origin::TemplateVariant;
use cogitator_ir::{Lowered, Span};

use crate::mask::Scanned;
use crate::{build_scanned, HirInput, SchemaEnv};

mod align;
mod chain;
mod ephemeral;
mod prepare;
mod relations;
mod template;
#[cfg(test)]
mod tests;

pub use align::{align, Alignment, MIN_LITERAL_COVERAGE};
pub use prepare::{prepare, Prepared};
pub use relations::RelationTable;

/// Lowers compiled SQL for which [`prepare`] succeeded. `input.text` must
/// be the text `prepared` was made from, and `input.origin` its
/// `SourceOrigin::Compiled`.
pub fn build_compiled_hir(
    input: &HirInput<'_>,
    env: &SchemaEnv,
    prepared: &Prepared,
) -> Lowered<Hir> {
    let scanned = Scanned {
        // boundary: `build_scanned` consumes the mask; the driver builds twice
        mask: prepared.mask().to_string(),
        regions: prepared.regions().to_vec(),
        variant: TemplateVariant::Plain,
        configs: Vec::new(),
    };
    build_scanned(input, env, scanned)
}

/// Trims ASCII whitespace from both ends of `span` in `text`.
fn trim_span(text: &str, span: Span) -> Span {
    let Some(inner) = text.get(span.start as usize..span.end as usize) else {
        return Span::new(span.start, span.start);
    };
    let leading = inner.len() - inner.trim_start_matches(is_ws).len();
    let trailing = inner.len() - inner.trim_end_matches(is_ws).len();
    if leading == inner.len() {
        return Span::new(span.start, span.start);
    }
    Span::new(span.start + leading as u32, span.end - trailing as u32)
}

fn is_ws(c: char) -> bool {
    c.is_ascii_whitespace()
}

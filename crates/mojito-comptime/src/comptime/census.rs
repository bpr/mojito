//! Classifies the bodies one elaboration minted, for the instantiation
//! census (`mojito_checked::census`).

use super::{DefInstanceTrace, block_has_statement};
use mojito_ast::ast::{Stmt, StmtKind};
use mojito_checked::census::{CloneCensus, CloneClass};
use mojito_common::token::Span;
use std::collections::HashMap;

/// What one elaboration produced, and the prepared program it read.
pub(super) struct Minted<'a> {
    pub prepared: &'a [Stmt],
    pub def_traces: &'a [DefInstanceTrace],
}

/// Count the `def` clones the cloner minted. Nested `def` clones and the
/// clones of a compile-time evaluation's subprogram are not in the list;
/// their mint sites count them. No method body is minted: every method's
/// template serves its instances.
pub(super) fn clone_census(minted: &Minted<'_>) -> CloneCensus {
    let mut census = CloneCensus::default();
    let def_templates: HashMap<(Option<&str>, &str, Span), &[Stmt]> = minted
        .prepared
        .iter()
        .filter_map(|statement| match &statement.kind {
            StmtKind::Def { name, body, .. } => Some((
                (statement.module.as_deref(), name.as_str(), statement.span),
                body.as_slice(),
            )),
            _ => None,
        })
        .collect();
    for trace in minted.def_traces {
        let template = def_templates
            .get(&(
                trace.template_module.as_deref(),
                trace.template_name.as_str(),
                trace.template_span,
            ))
            .copied()
            .unwrap_or_default();
        census.add(def_class(trace, template), 1);
        census.name(trace.clone_name.clone());
    }
    census
}

fn def_class(trace: &DefInstanceTrace, template: &[Stmt]) -> CloneClass {
    if !trace.pack_bindings.is_empty() {
        CloneClass::PackDef
    } else if block_has_statement(template, &|kind| {
        matches!(kind, StmtKind::ComptimeFor { .. })
    }) {
        CloneClass::ComptimeForDef
    } else if block_has_statement(template, &|kind| {
        matches!(kind, StmtKind::ComptimeIf { .. })
    }) {
        CloneClass::ComptimeIfDef
    } else if trace.value_bindings.is_empty() {
        CloneClass::TypeDef
    } else {
        CloneClass::ValueDef
    }
}

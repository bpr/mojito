//! Classifies the bodies one elaboration minted, for the instantiation
//! census (`mojito_checked::census`).

use super::{
    DefInstanceTrace, GeneratedDeclarations, MethodInstanceTrace, block_has_comptime,
    block_has_statement,
};
use mojito_ast::ast::{Stmt, StmtKind};
use mojito_checked::census::{CloneCensus, CloneClass};
use mojito_common::token::Span;
use mojito_types::ct::CtValue;
use std::collections::HashMap;

/// What one elaboration produced, and the prepared program it read.
pub(super) struct Minted<'a> {
    pub prepared: &'a [Stmt],
    pub elaborated: &'a [Stmt],
    pub def_traces: &'a [DefInstanceTrace],
    pub method_traces: &'a [MethodInstanceTrace],
    pub generated: &'a GeneratedDeclarations,
}

/// Count the `def` clones and the method bodies of the elaborated program
/// the cloner minted. Nested `def` clones and the clones of a compile-time
/// evaluation's subprogram appear in neither list; their mint sites count
/// them.
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
    }
    let method_templates: HashMap<(&str, Span), &[Stmt]> = minted
        .prepared
        .iter()
        .filter_map(|statement| match &statement.kind {
            StmtKind::Struct { name, methods, .. } => Some((name, methods)),
            _ => None,
        })
        .flat_map(|(name, methods)| {
            methods.iter().filter_map(|method| {
                let first = method.body.first()?;
                Some(((name.as_str(), first.span), method.body.as_slice()))
            })
        })
        .collect();
    let struct_traces: HashMap<&str, &MethodInstanceTrace> = minted
        .method_traces
        .iter()
        .map(|trace| (trace.owner.as_str(), trace))
        .collect();
    let clone_traces: HashMap<(&str, &str, Span), &MethodInstanceTrace> = minted
        .method_traces
        .iter()
        .map(|trace| {
            (
                (
                    trace.owner.as_str(),
                    trace.clone_name.as_str(),
                    trace.clone_body,
                ),
                trace,
            )
        })
        .collect();
    for statement in minted.elaborated {
        let StmtKind::Struct { name, methods, .. } = &statement.kind else {
            continue;
        };
        if minted.generated.structs.contains(name) {
            let class = struct_traces
                .get(name.as_str())
                .map_or(CloneClass::ValueStruct, |trace| struct_class(trace));
            census.add(class, methods.len());
            continue;
        }
        for method in methods {
            let class = if method.self_ty.is_some() {
                let template = method
                    .body
                    .first()
                    .and_then(|first| clone_traces.get(&(name.as_str(), &*method.name, first.span)))
                    .and_then(|trace| {
                        method_templates.get(&(trace.template_owner.as_str(), trace.body))
                    })
                    .copied()
                    .unwrap_or_default();
                if block_has_comptime(template) {
                    CloneClass::InstanceMethodComptime
                } else {
                    CloneClass::InstanceMethod
                }
            } else if minted
                .generated
                .methods
                .iter()
                .any(|(owner, clone)| owner == name && *clone == method.name)
            {
                CloneClass::PerCallMethod
            } else {
                continue;
            };
            census.add(class, 1);
        }
    }
    census
}

fn def_class(trace: &DefInstanceTrace, template: &[Stmt]) -> CloneClass {
    if has_lane_value(&trace.value_bindings) {
        CloneClass::DTypeVectorDef
    } else if !trace.pack_bindings.is_empty() {
        CloneClass::PackDef
    } else if block_has_statement(template, |kind| {
        matches!(kind, StmtKind::ComptimeFor { .. })
    }) {
        CloneClass::ComptimeForDef
    } else if block_has_statement(template, |kind| matches!(kind, StmtKind::ComptimeIf { .. })) {
        CloneClass::ComptimeIfDef
    } else if trace.value_bindings.is_empty() {
        CloneClass::TypeDef
    } else {
        CloneClass::ValueDef
    }
}

fn struct_class(trace: &MethodInstanceTrace) -> CloneClass {
    if has_lane_value(&trace.value_bindings) {
        CloneClass::DTypeVectorStruct
    } else if trace.pack_bindings.is_empty() {
        CloneClass::ValueStruct
    } else {
        CloneClass::VariadicStruct
    }
}

fn has_lane_value(bindings: &[(String, CtValue)]) -> bool {
    bindings
        .iter()
        .any(|(_, value)| matches!(value, CtValue::Dtype(_) | CtValue::Simd { .. }))
}

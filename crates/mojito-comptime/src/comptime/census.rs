//! Classifies the bodies one elaboration minted, for the instantiation
//! census (`mojito_checked::census`).

use super::specialize::method_parameter_is_baked;
use super::{
    DefInstanceTrace, GeneratedDeclarations, MethodInstanceTrace, block_has_comptime,
    block_has_statement,
};
use mojito_ast::ast::{Method, Stmt, StmtKind};
use mojito_checked::census::{CloneCensus, CloneClass};
use mojito_common::token::Span;
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
        census.name(trace.clone_name.clone());
    }
    let method_templates: HashMap<(&str, Span), &Method> = minted
        .prepared
        .iter()
        .filter_map(|statement| match &statement.kind {
            StmtKind::Struct { name, methods, .. } => Some((name, methods)),
            _ => None,
        })
        .flat_map(|(name, methods)| {
            methods.iter().filter_map(|method| {
                let first = method.body.first()?;
                Some(((name.as_str(), first.span), method))
            })
        })
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
        let template = |method: &Method| {
            method
                .body
                .first()
                .and_then(|first| clone_traces.get(&(name.as_str(), &*method.name, first.span)))
                .and_then(|trace| method_templates.get(&(trace.owner.as_str(), trace.body)))
                .copied()
        };
        // A per-call clone counts as one wherever it was minted: on a
        // non-generic struct or on a generic struct's instance.
        let per_call = |method: &Method| {
            template(method).is_some_and(|template| bakes_own_parameters(template, method))
                || minted
                    .generated
                    .methods
                    .iter()
                    .any(|(owner, clone)| owner == name && *clone == method.name)
        };
        for method in methods {
            let class = if per_call(method) {
                CloneClass::PerCallMethod
            } else if method.self_ty.is_some() {
                let template = template(method).map_or(&[][..], |template| &template.body);
                if block_has_comptime(template) {
                    CloneClass::InstanceMethodComptime
                } else {
                    CloneClass::InstanceMethod
                }
            } else {
                continue;
            };
            census.add(class, 1);
            census.name(method_source_name(name, method));
        }
    }
    census
}

/// The source name MIR lowers `method` of `owner` under, before any
/// overload qualifier.
fn method_source_name(owner: &str, method: &mojito_ast::ast::Method) -> String {
    format!(
        "{owner}.{}",
        mojito_symbol::symbol::lifecycle_method_name(method)
    )
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

/// Whether `clone` bakes the compile-time parameters `template` declares of
/// its own: a per-call clone. A per-instantiation clone keeps them.
fn bakes_own_parameters(template: &Method, clone: &Method) -> bool {
    let baked: Vec<&str> = template
        .type_params
        .iter()
        .filter(|parameter| method_parameter_is_baked(parameter, &template.type_params))
        .map(|parameter| parameter.name.as_str())
        .collect();
    !baked.is_empty()
        && !clone
            .type_params
            .iter()
            .any(|parameter| baked.contains(&parameter.name.as_str()))
}

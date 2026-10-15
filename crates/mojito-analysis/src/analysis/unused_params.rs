//! `remove-unused-params`: upstream's `RemoveUnusedParams`, which its
//! pre-elaboration pipeline runs over its generators callees first, so that
//! instances differing only in a parameter the generator never reads are
//! one.
//!
//! The pass records on the body the function's own compile-time parameters
//! that nothing in its body or signature reads
//! ([`mojito_mir::mir::unread_parameters`]), and the elaborator leaves
//! their arguments out of an instance's identity. Upstream also rewrites the
//! generator's signature and every call of it without those arguments, and
//! a caller it processes later no longer reads a parameter it only passed
//! on to a removed one. Mojito keeps both the signature and the calls, and
//! the instance binds the argument its body never reads; so the pass reads
//! each body as that rewrite leaves it
//! ([`mojito_mir::mir::without_removed_forwards`]), and records the callees
//! whose removals it relied on, which the verifier reads the body through.
//!
//! A nested body is a generator over the binders of its enclosing scope,
//! as upstream lifts a closure to one: the pass removes those it never
//! reads, the elaborator keys its instances on the rest, and the enclosing
//! function reads through a nested body it names only the binders that
//! body kept. Those removals join the snapshot the verifier reads.
//!
//! The pass reads a callee's removals only from a finished body of the same
//! unit, as upstream skips an external generator, and only off a cycle
//! ([`Callees::unit_body`]): a forward around a recursive call stays a
//! read, as the pin keeps the parameter. Each run records the body's unread
//! parameters afresh, and the pipeline's last round runs it after every
//! cleanup pass, so a read they removed keeps no parameter.

use super::pre_elaboration::{CallGraphPass, Callees};
use mojito_mir::mir::{
    Const, MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr,
    instruction_regions, unread_parameters, without_removed_forwards,
};
use mojito_types::param_expr::ParamId;
use std::collections::{BTreeMap, BTreeSet};

pub const REMOVE_UNUSED_PARAMS: CallGraphPass = CallGraphPass {
    name: "remove-unused-params",
    follows: has_own_parameters,
    run: remove_unused_params,
};

/// Record `function`'s unread parameters as its `removed_params`; returns
/// whether they or its `callee_removed_params` changed.
///
/// A forward to a callee's removed parameter is no read, and the callees
/// whose removals discounted one become its `callee_removed_params`, as do
/// the nested bodies it names that removed a parameter.
pub fn remove_unused_params(
    name: &str,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    callees: &Callees<'_>,
) -> bool {
    let mut targets = BTreeSet::new();
    call_targets(&function.blocks, &mut targets);
    let removals: BTreeMap<&str, &[ParamId]> = targets
        .iter()
        .filter_map(|&target| {
            callees
                .unit_body(target)
                .filter(|body| !body.removed_params.is_empty())
                .map(|body| (target, body.removed_params.as_slice()))
        })
        .collect();
    let mut nested_targets = BTreeSet::new();
    nested_bodies(&function.blocks, declarations, &mut nested_targets);
    let nested: BTreeMap<&str, &[ParamId]> = nested_targets
        .into_iter()
        .filter_map(|target| {
            callees
                .unit_body(target)
                .map(|body| (target, body.removed_params.as_slice()))
        })
        .collect();
    let nested_removals = |body: &str| nested.get(body).copied();
    let view = without_removed_forwards(function, declarations, &|callee| {
        removals.get(callee).copied()
    });
    let (removed, mut discounted) = match &view {
        Some((view, discounted)) => (
            unread_parameters(name, view, declarations, &nested_removals),
            discounted
                .iter()
                .map(|callee| (callee.clone(), removals[callee.as_str()].to_vec()))
                .collect(),
        ),
        None => (
            unread_parameters(name, function, declarations, &nested_removals),
            Vec::new(),
        ),
    };
    let discounted = if removed.is_empty() {
        Vec::new()
    } else {
        for (body, ids) in nested {
            if !ids.is_empty() && !discounted.iter().any(|(callee, _)| callee == body) {
                discounted.push((body.to_string(), ids.to_vec()));
            }
        }
        discounted.sort();
        discounted
    };
    if removed == function.removed_params && discounted == function.callee_removed_params {
        return false;
    }
    function.removed_params = removed;
    function.callee_removed_params = discounted;
    true
}

/// Whether the function so declared has a compile-time parameter of its
/// own, which a call of it may pass on, or is nested in another, whose
/// parameters it may read.
fn has_own_parameters(declaration: &MirFunctionDeclaration) -> bool {
    declaration.enclosing.is_some()
        || declaration
            .param_decls
            .iter()
            .any(|decl| *decl.id().owner == *declaration.lowered_name)
}

/// The callee of each call and resolved method call in `blocks` and their
/// regions.
fn call_targets<'a>(blocks: &'a [MirBlock], targets: &mut BTreeSet<&'a str>) {
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        match instruction {
            MirInstr::Call { func, .. } => {
                targets.insert(&func.0);
            }
            MirInstr::MethodCall {
                resolved: Some(target),
                ..
            } => {
                targets.insert(target);
            }
            _ => {}
        }
        for region in instruction_regions(instruction) {
            call_targets(region, targets);
        }
    }
}

/// Each nested function `blocks` and their regions name: by a call, a
/// closure, or a function value.
fn nested_bodies<'a>(
    blocks: &'a [MirBlock],
    declarations: &MirDeclarations,
    bodies: &mut BTreeSet<&'a str>,
) {
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        let named = match instruction {
            MirInstr::Call { func, .. } => Some(func.0.as_str()),
            MirInstr::MakeClosure { function, .. }
            | MirInstr::Const {
                k: Const::Function(function),
                ..
            } => Some(function.as_str()),
            _ => None,
        };
        if let Some(body) = named
            && declarations
                .function(body)
                .is_some_and(|declaration| declaration.enclosing.is_some())
        {
            bodies.insert(body);
        }
        for region in instruction_regions(instruction) {
            nested_bodies(region, declarations, bodies);
        }
    }
}

//! `remove-unused-params`: upstream's `RemoveUnusedParams`, which its
//! pre-elaboration pipeline runs over its generators callees first, so that
//! instances differing only in a parameter the generator never reads are
//! one.
//!
//! It first records the runtime arguments the body never uses but to drop
//! a value whose drop runs no code ([`mojito_mir::mir::unused_arguments`],
//! upstream's `identifyUnusedArguments`), and reads the rest of the body
//! with them gone ([`mojito_mir::mir::without_removed_arguments`]), so a
//! parameter only an unused argument's type named is unread too. Upstream
//! clones the generator without them (`_REMOVED_ARG`) for every direct
//! call and keeps the original for any other use; the elaborator does the
//! same, instantiating the reduced generator for a direct call, which
//! passes no argument for them, and the full one, keeping every parameter,
//! for a function value or any other demand. An argument the body only
//! passes on to a callee's removed argument is unused too, as upstream
//! rewrote that call before reaching the caller: the pass reads the body
//! with those operands gone
//! ([`mojito_mir::mir::without_removed_argument_forwards`]), and records the
//! callees it relied on, which the verifier and the elaborator read the
//! body through.
//!
//! The pass records on the body the function's own compile-time parameters,
//! and a method's struct's, that nothing in its body or signature reads
//! ([`mojito_mir::mir::unread_parameters`]), and the elaborator leaves
//! their arguments out of an instance's identity. Upstream also rewrites the
//! generator's signature and every call of it without those arguments, and
//! a caller it processes later no longer reads a parameter it only passed
//! on to a removed one. Mojito keeps both the signature and the calls, and
//! the instance binds the argument its body never reads; so the pass reads
//! each body as that rewrite leaves it
//! ([`mojito_mir::mir::without_removed_forwards`]), and records the callees
//! whose removals it relied on, which the verifier reads the body through.
//! A call of a member (a method call, a subscript, a resolved operator, an
//! iteration step) passes the member's parameters as values on the pin, so
//! it reads only the receiver's arguments at the struct parameters that
//! member kept; each such member joins the snapshot too, empty removals
//! included, since the verifier reads a call of a member it has none for
//! whole. A drop, copy, or move leaves a call of the lifecycle member it
//! runs on the pin, so it reads the same way, and each declared lifecycle
//! member it runs ([`mojito_mir::mir::lifecycle_callees`]) joins the
//! snapshot as well.
//!
//! A nested body is a generator over the binders of its enclosing scope,
//! as upstream lifts a closure to one: the pass removes those it never
//! reads, the elaborator keys its instances on the rest, and the enclosing
//! function reads through a nested body it names only the binders that
//! body kept. Those removals join the snapshot the verifier reads.
//!
//! A lifted compile-time function (`$comptime$f$…`, `$default$f$…`) is a
//! generator over every binder it declares, whoever owns it: the
//! elaborator keys its instances, and the evaluations cached by instance
//! name, on the binders it kept. A function that names one reads only
//! those, and its removals join the snapshot, empty or not, since the
//! verifier reads a lifted body it has none for as reading every binder.
//!
//! A method is a generator over its struct's parameters as well: the
//! elaborator keys and names one that removed any of them off the struct
//! template rather than the owner instance, and a static call's spelled
//! receiver `S[n]` forwards `n` to the struct parameter it fills.
//!
//! The pass reads a callee's removals only from a finished body of the same
//! unit, as upstream skips an external generator, and only off a cycle
//! ([`Callees::unit_body`]): a forward around a recursive call stays a
//! read, as the pin keeps the parameter. Each run records the body's unread
//! parameters afresh, and the pipeline's last round runs it after every
//! cleanup pass, so a read they removed keeps no parameter.

use super::pre_elaboration::{CallGraphPass, Callees};
use mojito_hir::hir::VarId;
use mojito_mir::mir::{
    Const, MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr,
    instruction_regions, instruction_subscript_call, lifecycle_callees, method_owner_params,
    named_lifted_bodies, unread_parameters_of, unused_arguments, without_removed_argument_forwards,
    without_removed_arguments, without_removed_forwards,
};
use mojito_types::param_expr::ParamId;
use std::collections::{BTreeMap, BTreeSet};

pub const REMOVE_UNUSED_PARAMS: CallGraphPass = CallGraphPass {
    name: "remove-unused-params",
    follows: has_parameters,
    run: remove_unused_params,
};

/// Record `function`'s unused arguments and unread parameters; returns
/// whether they or its `callee_removed_params` changed.
///
/// The unused arguments are its `removed_args`, and the parameters the
/// body reads none of without them its `removed_params`.
///
/// A forward to a callee's removed argument is no use, and the callees
/// whose removed arguments discounted one become its
/// `callee_removed_args` when it removed anything.
///
/// A forward to a callee's removed parameter is no read, and the callees
/// whose removals discounted one become its `callee_removed_params`, as do
/// the nested bodies it names that removed a parameter and the lifted
/// bodies it names.
pub fn remove_unused_params(
    name: &str,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    callees: &Callees<'_>,
) -> bool {
    let mut forwarded = BTreeSet::new();
    call_targets(&function.blocks, &mut forwarded);
    let argument_removals: BTreeMap<&str, &[VarId]> = forwarded
        .into_iter()
        .filter_map(|target| {
            callees
                .unit_body(target)
                .filter(|body| !body.removed_args.is_empty())
                .map(|body| (target, body.removed_args.as_slice()))
        })
        .collect();
    let arguments_view = without_removed_argument_forwards(function, declarations, &|callee| {
        argument_removals.get(callee).copied()
    });
    let source = arguments_view.as_ref().map_or(&*function, |(view, _)| view);
    let removed_args = unused_arguments(name, source, declarations);
    let reduced = declarations
        .function(name)
        .filter(|_| !removed_args.is_empty())
        .map(|declaration| without_removed_arguments(source, declaration, &removed_args));
    let declaration = reduced
        .as_ref()
        .map(|(_, declaration)| declaration)
        .or_else(|| declarations.function(name));
    let body = reduced.as_ref().map_or(source, |(view, _)| view);
    let lifecycle = lifecycle_callees(body, declarations);
    let mut targets: BTreeSet<&str> = lifecycle.iter().map(String::as_str).collect();
    call_targets(&body.blocks, &mut targets);
    let removals: BTreeMap<&str, &[ParamId]> = targets
        .iter()
        .filter_map(|&target| {
            callees
                .unit_body(target)
                .map(|body| (target, body.removed_params.as_slice()))
        })
        .collect();
    let callee_removals = |callee: &str| removals.get(callee).copied();
    let mut nested_targets = BTreeSet::new();
    nested_bodies(&body.blocks, declarations, &mut nested_targets);
    let lifted_targets = named_lifted_bodies(name, body, declarations);
    let nested: BTreeMap<&str, &[ParamId]> = nested_targets
        .into_iter()
        .chain(lifted_targets.iter().map(String::as_str))
        .filter_map(|target| {
            callees
                .unit_body(target)
                .map(|body| (target, body.removed_params.as_slice()))
        })
        .collect();
    let nested_removals = |body: &str| nested.get(body).copied();
    let view = without_removed_forwards(name, body, declarations, &callee_removals);
    let unread = |body: &MirFunction| {
        declaration
            .map(|declaration| {
                unread_parameters_of(
                    name,
                    body,
                    declaration,
                    declarations,
                    &nested_removals,
                    &callee_removals,
                )
            })
            .unwrap_or_default()
    };
    let (unread, mut discounted): (_, Vec<(String, Vec<ParamId>)>) = match &view {
        Some((view, discounted)) => (
            unread(view),
            discounted
                .iter()
                .map(|callee| (callee.clone(), removals[callee.as_str()].to_vec()))
                .collect(),
        ),
        None => (unread(body), Vec::new()),
    };
    let removed = unread.params;
    let discounted = if removed.is_empty() {
        Vec::new()
    } else {
        for callee in unread.member_callees {
            if !discounted.iter().any(|(seen, _)| *seen == callee) {
                let ids = removals[callee.as_str()].to_vec();
                discounted.push((callee, ids));
            }
        }
        for (body, ids) in nested {
            if (!ids.is_empty() || lifted_targets.contains(body))
                && !discounted.iter().any(|(callee, _)| callee == body)
            {
                discounted.push((body.to_string(), ids.to_vec()));
            }
        }
        discounted.sort();
        discounted
    };
    let argument_discounted: Vec<(String, Vec<VarId>)> = match &arguments_view {
        Some((_, callees)) if !removed_args.is_empty() || !removed.is_empty() => callees
            .iter()
            .map(|callee| (callee.clone(), argument_removals[callee.as_str()].to_vec()))
            .collect(),
        _ => Vec::new(),
    };
    if removed == function.removed_params
        && discounted == function.callee_removed_params
        && removed_args == function.removed_args
        && argument_discounted == function.callee_removed_args
    {
        return false;
    }
    function.removed_params = removed;
    function.callee_removed_params = discounted;
    function.removed_args = removed_args;
    function.callee_removed_args = argument_discounted;
    true
}

/// Whether the function so declared has a parameter a call of it may pass
/// on: a runtime argument, a receiver, or a compile-time parameter of its
/// own or, as a method, its struct's; or is nested in another, whose
/// parameters it may read, or is lifted and declares binders, which its
/// application binds.
fn has_parameters(declaration: &MirFunctionDeclaration, declarations: &MirDeclarations) -> bool {
    declaration.has_receiver
        || !declaration.param_names.is_empty()
        || declaration.enclosing.is_some()
        || (!declaration.param_decls.is_empty()
            && mojito_symbol::symbol::lifted_owner(&declaration.lowered_name).is_some())
        || declaration
            .param_decls
            .iter()
            .any(|decl| *decl.id().owner == *declaration.lowered_name)
        || !method_owner_params(&declaration.lowered_name, declarations).is_empty()
}

/// The callee of each call, resolved method call, subscript call,
/// resolved operator, and iteration step in `blocks` and their regions.
fn call_targets<'a>(blocks: &'a [MirBlock], targets: &mut BTreeSet<&'a str>) {
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        match instruction {
            MirInstr::Call { func, .. } => {
                targets.insert(&func.0);
            }
            MirInstr::MethodCall {
                resolved: Some(target),
                ..
            }
            | MirInstr::BinOp {
                resolved: Some(target),
                ..
            } => {
                targets.insert(target);
            }
            MirInstr::GetIter { prepare, .. } => {
                targets.extend(prepare.iter().map(String::as_str));
            }
            MirInstr::TryNext { call, .. } => {
                targets.insert(&call.target);
            }
            _ => {
                if let Some(call) = instruction_subscript_call(instruction) {
                    targets.insert(&call.target);
                }
            }
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

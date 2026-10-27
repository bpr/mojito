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
//! those of the `comptime for` regions it is declared in included, beside
//! its own, as upstream lifts a closure to one: the pass removes
//! those it never reads, the elaborator keys its instances on the rest, and
//! the enclosing function reads through a nested body it names only the
//! binders that body kept. Those removals join the snapshot the verifier
//! reads. A nested body loses its unused arguments, captures included, like
//! any other: a call of it by name is a direct call, which names the reduced
//! generator as any direct call does, and only a body a closure or function
//! value names keeps its full generator too, whose every binder the
//! enclosing function then reads; such a body joins the snapshot with no
//! removals.
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
//! ([`Callees::unit_body`]). A self-recursive call is the one cycle it
//! reads through, as upstream's walk does (`isRecursive`): an argument
//! every such call passes back at its own position, and that the body
//! uses nowhere else, is unused, and a parameter every such call passes as
//! itself (spelled, inferred, or through the call's receiver), and that the
//! body reads nowhere else, is removed, each the greatest such set found by
//! assuming the call removed it until the assumption holds; a forward
//! around a longer cycle, or to another position, stays a use. Each run
//! records the body's unread parameters afresh, and the pipeline's last
//! round runs it after every cleanup pass, so a read they removed keeps no
//! parameter.

use super::pre_elaboration::{CallGraphPass, Callees, Cycles};
use mojito_hir::hir::VarId;
use mojito_mir::mir::{
    Const, MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr, MirPlace,
    MirSubscriptArg, Reg, instruction_regions, instruction_subscript_call, lifecycle_callees,
    method_owner_params, named_lifted_bodies, operator_method, removed_argument_operands,
    supplied_parameter_arguments, unread_parameters_of, unused_arguments,
    without_removed_argument_forwards, without_removed_arguments, without_removed_forwards,
};
use mojito_types::ct::CtValue;
use mojito_types::param_expr::{ParamId, ParamKind};
use mojito_types::types::{Ty, TyArg, TyKind};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub const REMOVE_UNUSED_PARAMS: CallGraphPass = CallGraphPass {
    name: "remove-unused-params",
    follows: has_parameters,
    cycles: Cycles::Refused,
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
        .iter()
        .filter_map(|&target| {
            callees
                .unit_body(target)
                .filter(|body| !body.removed_args.is_empty())
                .map(|body| (target, body.removed_args.as_slice()))
        })
        .collect();
    // Assume each self-recursive call removed the arguments it passes as
    // themselves, narrowing to those found unused until none drops out.
    let mut recursive_args = self_forwarded_arguments(name, function, declarations);
    let (mut arguments_view, removed_args) = loop {
        let removals = |callee: &str| {
            if callee == name {
                (!recursive_args.is_empty()).then_some(recursive_args.as_slice())
            } else {
                argument_removals.get(callee).copied()
            }
        };
        let view = without_removed_argument_forwards(function, declarations, &removals);
        let source = view.as_ref().map_or(&*function, |(view, _)| view);
        let removed_args = unused_arguments(name, source, declarations);
        let kept = recursive_args.len();
        recursive_args.retain(|slot| removed_args.contains(slot));
        if recursive_args.len() == kept {
            break (view, removed_args);
        }
    };
    // The rewritten self-recursive call passes none of the removed
    // arguments, as every direct call of the reduced generator does.
    if forwarded.contains(name) && recursive_args != removed_args {
        arguments_view = without_removed_argument_forwards(function, declarations, &|callee| {
            if callee == name {
                Some(removed_args.as_slice())
            } else {
                argument_removals.get(callee).copied()
            }
        });
    }
    let source = arguments_view.as_ref().map_or(&*function, |(view, _)| view);
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
    let mut called = BTreeSet::new();
    let mut valued = BTreeSet::new();
    nested_bodies(&body.blocks, declarations, &mut called, &mut valued);
    let lifted_targets = named_lifted_bodies(name, body, declarations);
    // A nested body a closure or function value names keeps its full
    // generator beside the reduced one its direct calls name, which reads
    // every binder.
    let mut full = BTreeSet::new();
    let nested: BTreeMap<&str, &[ParamId]> = called
        .into_iter()
        .chain(valued.iter().copied())
        .chain(lifted_targets.iter().map(String::as_str))
        .filter_map(|target| {
            callees.unit_body(target).map(|body| {
                if !body.removed_args.is_empty() && valued.contains(target) {
                    full.insert(target);
                    (target, &[][..])
                } else {
                    (target, body.removed_params.as_slice())
                }
            })
        })
        .collect();
    // Assume each self-recursive call removed the parameters it forwards
    // to themselves, narrowing to those found unread until none drops out.
    let mut recursive = self_forwarded_parameters(name, body, declarations);
    let (removed, mut discounted) = loop {
        let (unread, discounted) = {
            let mut removals: BTreeMap<&str, &[ParamId]> = targets
                .iter()
                .filter_map(|&target| {
                    callees
                        .unit_body(target)
                        .map(|body| (target, body.removed_params.as_slice()))
                })
                .collect();
            if !recursive.is_empty() {
                removals.insert(name, recursive.as_slice());
            }
            let callee_removals = |callee: &str| removals.get(callee).copied();
            let nested_removals = |body: &str| nested.get(body).copied();
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
            let view = without_removed_forwards(name, body, declarations, &callee_removals);
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
            for callee in &unread.member_callees {
                if !discounted.iter().any(|(seen, _)| seen == callee) {
                    let ids = removals[callee.as_str()].to_vec();
                    discounted.push((callee.clone(), ids));
                }
            }
            (unread.params, discounted)
        };
        let kept = recursive.len();
        recursive.retain(|id| unread.contains(id));
        if recursive.len() == kept {
            break (unread, discounted);
        }
    };
    let discounted = if removed.is_empty() {
        Vec::new()
    } else {
        for (body, ids) in nested {
            if (!ids.is_empty() || lifted_targets.contains(body) || full.contains(body))
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
            .map(|callee| {
                let slots = if callee == name {
                    removed_args.as_slice()
                } else {
                    argument_removals[callee.as_str()]
                };
                (callee.clone(), slots.to_vec())
            })
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

/// The compile-time parameters of `name` that every call of `name` in the
/// body `function`, direct, a resolved method call, a resolved operator, or
/// a subscript read, supplies as the parameter itself, a method's struct's
/// included when the call's receiver, a value's or a static call's spelled
/// one, spells them so (`Self`, `Grid[n]`, `S[Self.n]`); none when `name`
/// never calls itself.
fn self_forwarded_parameters(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
) -> Vec<ParamId> {
    let mut forwarded = None;
    self_forwards(
        name,
        function,
        &function.blocks,
        declarations,
        &mut forwarded,
    );
    forwarded.unwrap_or_default()
}

/// The parameter slots of `name`, the body `function`, that every call of
/// `name` in it ([`SelfCall::of`]) passes as themselves at their own
/// position: the operand bound to the slot is a whole read of it, or its
/// place. None when `name` never calls itself, or calls itself
/// through a spread, which binds as many slots as its pack holds.
fn self_forwarded_arguments(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
) -> Vec<VarId> {
    let Some(declaration) = declarations.function(name) else {
        return Vec::new();
    };
    let mut reads = HashMap::new();
    whole_slot_reads(&function.blocks, &mut reads);
    let mut forwarded = None;
    self_argument_forwards(
        name,
        declaration,
        function.n_params,
        &function.blocks,
        &reads,
        &mut forwarded,
    );
    forwarded.unwrap_or_default()
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

/// Each nested function `blocks` and their regions name: `called` by a
/// direct call, `valued` by a closure or a function value.
fn nested_bodies<'a>(
    blocks: &'a [MirBlock],
    declarations: &MirDeclarations,
    called: &mut BTreeSet<&'a str>,
    valued: &mut BTreeSet<&'a str>,
) {
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        let (body, by_call) = match instruction {
            MirInstr::Call { func, .. } => (Some(func.0.as_str()), true),
            MirInstr::MakeClosure { function, .. }
            | MirInstr::Const {
                k: Const::Function(function) | Const::FunctionInstance { function, .. },
                ..
            } => (Some(function.as_str()), false),
            _ => (None, false),
        };
        if let Some(body) = body
            && declarations
                .function(body)
                .is_some_and(|declaration| declaration.enclosing.is_some())
        {
            let bodies: &mut BTreeSet<&'a str> = if by_call { called } else { valued };
            bodies.insert(body);
        }
        for region in instruction_regions(instruction) {
            nested_bodies(region, declarations, called, valued);
        }
    }
}

/// Narrow `forwarded` to the parameters each call of `name`, the body
/// `function`, in `blocks` and their regions supplies as themselves
/// ([`self_forwarded_parameters`]); the first call sets it. A call
/// supplies a parameter by an explicit argument or, inferred, by its
/// `instantiated_args` entry when those align with the callee's binders.
fn self_forwards(
    name: &str,
    function: &MirFunction,
    blocks: &[MirBlock],
    declarations: &MirDeclarations,
    forwarded: &mut Option<Vec<ParamId>>,
) {
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        let call = match instruction {
            MirInstr::Call {
                func,
                param_arg_regs,
                instantiated_args,
                receiver,
                ..
            } if func.0 == name => declarations.function(name).map(|callee| {
                (
                    callee.param_decls.as_slice(),
                    param_arg_regs.as_slice(),
                    instantiated_args.as_slice(),
                    receiver.as_ref(),
                )
            }),
            MirInstr::MethodCall {
                recv,
                recv_place,
                resolved: Some(target),
                param_arg_regs,
                param_decls,
                instantiated_args,
                ..
            } if target == name => {
                let receiver = recv_place
                    .as_deref()
                    .and_then(|place| place.storage_ty(&function.var_tys))
                    .or_else(|| function.reg_types.get(&recv.0));
                Some((
                    param_decls.as_slice(),
                    param_arg_regs.as_slice(),
                    instantiated_args.as_slice(),
                    receiver,
                ))
            }
            MirInstr::BinOp {
                op,
                a,
                resolved: Some(target),
                ..
            } if target == name && operator_method(*op).is_some() => {
                Some((&[][..], &[][..], &[][..], function.reg_types.get(&a.0)))
            }
            MirInstr::Index {
                base: object,
                base_place: object_place,
                call: Some(call),
                ..
            }
            | MirInstr::MultiIndex {
                object,
                object_place,
                call: Some(call),
                ..
            } if call.target == name => {
                let receiver = object_place
                    .as_deref()
                    .and_then(|place| place.storage_ty(&function.var_tys))
                    .or_else(|| function.reg_types.get(&object.0));
                Some((
                    call.param_decls.as_slice(),
                    call.param_arg_regs.as_slice(),
                    &[][..],
                    receiver,
                ))
            }
            _ => None,
        };
        if let Some((decls, arguments, instantiated, receiver)) = call {
            let mut itself: Vec<ParamId> = supplied_parameter_arguments(decls, arguments)
                .into_iter()
                .filter(|(decl, argument)| {
                    argument.expr.as_ref().is_some_and(|expr| {
                        matches!(expr.kind(), ParamKind::DeclRef(reference) if reference.id == *decl.id())
                    })
                })
                .map(|(decl, _)| decl.id().clone())
                .collect();
            if instantiated.len() == decls.len() {
                itself.extend(
                    decls
                        .iter()
                        .zip(instantiated)
                        .filter(|(decl, argument)| names_parameter(argument, decl.id()))
                        .map(|(decl, _)| decl.id().clone()),
                );
            }
            itself.extend(receiver.map_or_else(Vec::new, |receiver| {
                receiver_forwarded_owner_params(name, receiver, declarations)
            }));
            // A parameter both spelled and inferred is supplied once.
            let mut seen = BTreeSet::new();
            itself.retain(|id| seen.insert(id.clone()));
            match forwarded {
                Some(ids) => ids.retain(|id| itself.contains(id)),
                None => *forwarded = Some(itself),
            }
        }
        for region in instruction_regions(instruction) {
            self_forwards(name, function, region, declarations, forwarded);
        }
    }
}

/// The parameters of the struct whose method `name` is that a call of
/// `name` on a `receiver` of that type passes as themselves: every one
/// when the receiver is the bare `Self`, else each the receiver's
/// argument at it names.
fn receiver_forwarded_owner_params(
    name: &str,
    receiver: &Ty,
    declarations: &MirDeclarations,
) -> Vec<ParamId> {
    let receiver = match receiver.kind() {
        TyKind::Ref(reference) => &reference.referent,
        _ => receiver,
    };
    let TyKind::Struct(struct_name, arguments) = receiver.kind() else {
        return Vec::new();
    };
    if mojito_symbol::symbol::split_method_symbol(name)
        .is_none_or(|(owner, _)| owner != *struct_name)
    {
        return Vec::new();
    }
    let decls = method_owner_params(name, declarations);
    if arguments.is_empty() {
        return decls.iter().map(|decl| decl.id().clone()).collect();
    }
    if arguments.len() != decls.len() {
        return Vec::new();
    }
    decls
        .iter()
        .zip(arguments)
        .filter(|(decl, argument)| names_parameter(argument, decl.id()))
        .map(|(decl, _)| decl.id().clone())
        .collect()
}

/// Whether the type or value argument `argument` is the parameter `id`
/// itself.
fn names_parameter(argument: &TyArg, id: &ParamId) -> bool {
    match argument {
        TyArg::Val(CtValue::Expr(expr)) => {
            matches!(expr.kind(), ParamKind::DeclRef(reference) if reference.id == *id)
        }
        TyArg::Ty(ty) => matches!(ty.kind(), TyKind::Param { binder, .. } if binder.id == *id),
        TyArg::Val(_) | TyArg::Origin(_) => false,
    }
}

/// The slot each register of `blocks` and their regions reads whole, by a
/// slot use or a load of its unprojected place.
fn whole_slot_reads(blocks: &[MirBlock], reads: &mut HashMap<u32, VarId>) {
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        match instruction {
            MirInstr::UseVar { dest, var, .. } => {
                reads.insert(dest.0, *var);
            }
            MirInstr::LoadPlace { dest, place } if place.proj.is_empty() => {
                reads.insert(dest.0, place.root);
            }
            _ => {}
        }
        for region in instruction_regions(instruction) {
            whole_slot_reads(region, reads);
        }
    }
}

/// Narrow `forwarded` to the slots of `name`, declared with `n_params`
/// slots, each call of `name` in `blocks` and their regions passes as
/// themselves ([`self_forwarded_arguments`]); the first call sets it.
fn self_argument_forwards(
    name: &str,
    declaration: &MirFunctionDeclaration,
    n_params: usize,
    blocks: &[MirBlock],
    reads: &HashMap<u32, VarId>,
    forwarded: &mut Option<Vec<VarId>>,
) {
    let itself = |slot: VarId, (reg, place): Operand<'_>| {
        reg.is_some_and(|reg| reads.get(&reg.0) == Some(&slot))
            || place.is_some_and(|place| place.root == slot && place.proj.is_empty())
    };
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        if let Some(call) = SelfCall::of(name, instruction) {
            let keywords: Vec<&str> = call.kwargs.iter().map(|(name, _)| *name).collect();
            let passed: Vec<VarId> = (0..n_params as VarId)
                .filter(|&slot| {
                    if call.spread {
                        return false;
                    }
                    if declaration.has_receiver && slot == 0 {
                        return call.receiver.is_some_and(|(recv, recv_place)| {
                            itself(slot, (Some(recv), recv_place))
                        });
                    }
                    let Ok((positional, keyword)) =
                        removed_argument_operands(declaration, &[slot], call.args.len(), &keywords)
                    else {
                        return false;
                    };
                    match (positional.len(), keyword.len()) {
                        (1, 0) => positional
                            .iter()
                            .all(|&index| itself(slot, call.args[index])),
                        (0, 1) => keyword
                            .iter()
                            .all(|&index| itself(slot, call.kwargs[index].1)),
                        _ => false,
                    }
                })
                .collect();
            match forwarded {
                Some(slots) => slots.retain(|slot| passed.contains(slot)),
                None => *forwarded = Some(passed),
            }
        }
        for region in instruction_regions(instruction) {
            self_argument_forwards(name, declaration, n_params, region, reads, forwarded);
        }
    }
}

/// The operands a call of a function passes it.
struct SelfCall<'a> {
    receiver: Option<(Reg, Option<&'a MirPlace>)>,
    args: Vec<Operand<'a>>,
    kwargs: Vec<(&'a str, Operand<'a>)>,
    /// Whether the call spreads a pack, which binds as many slots as it
    /// holds.
    spread: bool,
}

impl<'a> SelfCall<'a> {
    /// The operands `instruction` passes `name`, if it calls it: a direct
    /// call, a resolved method call, a resolved operator, whose left
    /// operand is the receiver, or a subscript read of its accessor.
    fn of(name: &str, instruction: &'a MirInstr) -> Option<Self> {
        let operands = |regs: &'a [Reg], places: &'a [Option<MirPlace>]| {
            regs.iter()
                .enumerate()
                .map(|(index, reg)| (Some(*reg), places.get(index).and_then(Option::as_ref)))
                .collect()
        };
        let keyword_operands = |kwargs: &'a [(String, Reg)], places: &'a [Option<MirPlace>]| {
            kwargs
                .iter()
                .enumerate()
                .map(|(index, (name, reg))| {
                    (
                        name.as_str(),
                        (Some(*reg), places.get(index).and_then(Option::as_ref)),
                    )
                })
                .collect()
        };
        let subscript = |argument: &MirSubscriptArg| match argument {
            MirSubscriptArg::Index(reg) => Some(*reg),
        };
        match instruction {
            MirInstr::Call {
                func,
                args,
                kwargs,
                arg_places,
                kwarg_places,
                spread,
                ..
            } if func.0 == name => Some(Self {
                receiver: None,
                args: operands(args, arg_places),
                kwargs: keyword_operands(kwargs, kwarg_places),
                spread: spread.is_some(),
            }),
            MirInstr::MethodCall {
                recv,
                recv_place,
                resolved: Some(target),
                args,
                kwargs,
                arg_places,
                kwarg_places,
                spread,
                ..
            } if target == name => Some(Self {
                receiver: Some((*recv, recv_place.as_deref())),
                args: operands(args, arg_places),
                kwargs: keyword_operands(kwargs, kwarg_places),
                spread: spread.is_some(),
            }),
            MirInstr::BinOp {
                op,
                a,
                b,
                resolved: Some(target),
                ..
            } if target == name && operator_method(*op).is_some() => Some(Self {
                receiver: Some((*a, None)),
                args: vec![(Some(*b), None)],
                kwargs: Vec::new(),
                spread: false,
            }),
            MirInstr::Index {
                base,
                index,
                base_place,
                index_place,
                call: Some(call),
                ..
            } if call.target == name => Some(Self {
                receiver: Some((*base, base_place.as_deref())),
                args: vec![(Some(*index), index_place.as_deref())],
                kwargs: Vec::new(),
                spread: false,
            }),
            MirInstr::MultiIndex {
                object,
                object_place,
                args,
                arg_places,
                kwargs,
                kwarg_places,
                call: Some(call),
                ..
            } if call.target == name => Some(Self {
                receiver: Some((*object, object_place.as_deref())),
                args: args
                    .iter()
                    .enumerate()
                    .map(|(index, argument)| {
                        (
                            subscript(argument),
                            arg_places.get(index).and_then(Option::as_ref),
                        )
                    })
                    .collect(),
                kwargs: kwargs
                    .iter()
                    .enumerate()
                    .map(|(index, (name, argument))| {
                        (
                            name.as_str(),
                            (
                                subscript(argument),
                                kwarg_places.get(index).and_then(Option::as_ref),
                            ),
                        )
                    })
                    .collect(),
                spread: false,
            }),
            _ => None,
        }
    }
}

/// One operand of a call, its register and its place.
type Operand<'a> = (Option<Reg>, Option<&'a MirPlace>);

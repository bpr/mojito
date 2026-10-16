//! Parameter uses: which of a generic function's compile-time parameters,
//! its own and, for a method, its struct's, its body and signature read,
//! the analysis upstream's `RemoveUnusedParams` runs before it drops a
//! parameter from a generator.
//!
//! MIR names a binder by its identity wherever it refers to one: a type
//! parameter, a dependent type, a parameter expression, a call's parameter
//! arguments. So the walk is structural, after MLIR's walk over every
//! attribute and type an operation holds: it visits the value's serialized
//! form, and a `(owner, slot)` pair spelling one of the function's binders
//! is a read of it. A pair that only happens to spell one counts too, which
//! keeps a parameter and never drops one. A method's struct spelled with
//! no arguments is its `Self` (a receiver, `out self`, a `Self` result),
//! which reads the struct's parameters as the struct spelled with them
//! does.
//!
//! Upstream's walk runs over the structural types `LowerLIT` left, so a
//! struct type in a signature, a slot, a register, or a place reads only
//! the arguments its lowered layout reads ([`StructReads`]). A struct type
//! is read whole where upstream's IR keeps it nominal: a type value (a
//! call's parameter arguments, a compile-time constant or condition), and
//! the lifecycle member a copy, a move, or a drop runs: upstream's
//! destructor and copy/move insertion leaves a call of it, so a declared
//! member reads the value's arguments at the struct parameters it kept,
//! when its removals are at hand, and all of them otherwise, while a
//! synthesized one reads what its fields' members read. A call of a member of the struct (a method call, a
//! subscript, a resolved operator, an iteration step) passes the member's
//! parameters as values, so it reads the receiver's arguments at the
//! struct parameters the member kept, when the member's removals are at
//! hand; otherwise, and for an undeclared callee that may run a member (a
//! constructor, a builtin such as `len`), every type the instruction names
//! is read whole (of a method call's place, only the type it designates,
//! the root and projections addressing it reading their layout).
//!
//! Two further reads are not identities: a value parameter's runtime reads
//! go through the slot its name binds, and a body nested in the function
//! (named `f$…`) reads the binders of its enclosing scope its own analysis
//! kept, as upstream's `RemoveUnusedParams` reads a lifted closure's
//! generator after removing its unused parameters. A body lifted from the
//! function (named `$comptime$f$…`, `$default$f$…`) reads the binders it
//! declares that its own analysis kept, as upstream's lifted function is an
//! ordinary generator. A nested or lifted body whose analysis is not at hand
//! reads every parameter.

use crate::mir::struct_reads::{LayoutReads, StructReads};
use crate::mir::verify::{instruction_operand_regs, instruction_places, instruction_result_regs};
use crate::mir::{
    MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr, MirParamArg,
    MirPlace, MirSubscriptArg, MirTerm, Proj, Reg, TyArg, UseMode, count_register_reads,
    destroyed_structs, evaluated_at_call_site, instruction_regions, instruction_regions_mut,
    instruction_slots, instruction_subscript_call, matched_parameter_arguments,
    supplied_parameter_arguments,
};
use mojito_ast::ast::InfixOp;
use mojito_hir::hir::VarId;
use mojito_types::param_expr::ParamId;
use mojito_types::types::{ParamDecl, Ty, TyKind};
use serde::Serialize;
use serde::ser;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;

/// The parameters upstream's `RemoveUnusedParams` removes from `name`.
///
/// For a top-level function they are its own compile-time parameters and,
/// for a method, its struct's, as upstream's method generator lists them
/// before its own; for a nested one the parameters of the declarations it
/// is nested in; for a lifted one (`$comptime$f$…`, `$default$f$…`) every
/// binder it declares; each that nothing in its body `function` or its
/// declaration's signature reads, sorted by owner and slot. A nested body `function` names reads
/// the candidates its own removals `nested` keep, and a lifted body the
/// binders it declares that they keep; one `nested` does not answer reads
/// them all.
///
/// A default of another parameter is not a read, since a default is the
/// call site's, nor is a runtime default every call omitting it evaluates
/// ([`crate::mir::evaluated_at_call_site`]). A function inside a `comptime for`, `__toplevel__`, and a
/// function without a declaration have no candidates, nor has a nested
/// function's own parameter. A callable value parameter is a candidate like
/// any other: a call through it reads it, and the elaborator binds no
/// runtime closure or environment for one it removed.
///
/// A call of a member whose removals `callees` answers reads only the
/// receiver's arguments at the struct parameters that member kept, as a
/// drop, copy, or move running a declared lifecycle member it answers reads
/// only the value's arguments at the parameters that member kept.
pub fn unread_parameters<'a>(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
    nested: &Removals<'a>,
    callees: &Removals<'a>,
) -> UnreadParameters {
    declarations
        .function(name)
        .map(|declaration| {
            unread_parameters_of(name, function, declaration, declarations, nested, callees)
        })
        .unwrap_or_default()
}

/// [`unread_parameters`] of `name` so declared.
///
/// The signature read is `declaration`'s, as the generator with its unused
/// arguments removed declares it ([`crate::mir::without_removed_arguments`]).
pub fn unread_parameters_of<'a>(
    name: &str,
    function: &MirFunction,
    declaration: &MirFunctionDeclaration,
    declarations: &MirDeclarations,
    nested: &Removals<'a>,
    callees: &Removals<'a>,
) -> UnreadParameters {
    if name == "__toplevel__" || !declaration.region_binders.is_empty() {
        return UnreadParameters::default();
    }
    let candidates = candidate_parameters(name, declaration, declarations);
    if candidates.is_empty() {
        return UnreadParameters::default();
    }
    let structure = method_struct(name, declarations);
    let mut owners: Vec<&str> = Vec::new();
    for decl in &candidates {
        if !owners.contains(&&*decl.id().owner) {
            owners.push(&decl.id().owner);
        }
    }
    let mut bodies: Vec<&str> = owners
        .iter()
        .copied()
        .filter(|owner| structure.is_none_or(|(spelled, _)| spelled != *owner))
        .collect();
    if !bodies.contains(&name) {
        bodies.push(name);
    }
    let structs = StructReads::with_members(declarations, callees);
    let mut walker = Walker::new(owners, nested);
    walker.bodies = bodies;
    walker.declarations = Some(declarations);
    walker.structs = Some(&structs);
    walker.layout = true;
    walker.callees = Some(callees);
    if let Some((spelled, decls)) = structure
        && let Some(owner) = walker.owners.iter().position(|owner| *owner == spelled)
    {
        walker.self_spelling = Some((
            spelled,
            decls.iter().map(|decl| (owner, decl.id().slot)).collect(),
        ));
        walker.self_type = Some(
            TyKind::Struct(
                spelled.to_string(),
                decls.iter().map(ParamDecl::own_argument).collect(),
            )
            .into(),
        );
    }
    walker.signature(declaration);
    walker.body(function);
    walker.nominal_reads(function, declarations);
    if walker.reads_all {
        return UnreadParameters::default();
    }
    let read_by_slot = slot_reads(function, &candidates);
    let read_from_receiver = structure
        .map(|(spelled, decls)| receiver_parameter_reads(function, spelled, decls, declarations))
        .unwrap_or_default();
    let read = |decl: &ParamDecl| {
        let id = decl.id();
        walker
            .owners
            .iter()
            .position(|owner| **owner == *id.owner)
            .is_some_and(|owner| walker.slots.contains(&(owner, id.slot)))
            || read_by_slot.contains(decl.name().trim_start_matches('*'))
            || read_from_receiver.contains(id)
            || walker
                .nested_removals
                .iter()
                .any(|removed| !removed.contains(id))
    };
    let mut unread: Vec<ParamId> = candidates
        .into_iter()
        .filter(|decl| !read(decl))
        .map(|decl| decl.id().clone())
        .collect();
    unread.sort();
    let mut member_callees = walker.member_callees;
    member_callees.extend(structs.consulted());
    UnreadParameters {
        params: unread,
        member_callees,
    }
}

/// The parameters [`unread_parameters`] found unread, and the member
/// callees, called or run by a drop, copy, or move, whose removals the walk
/// relied on.
#[derive(Debug, Default)]
pub struct UnreadParameters {
    pub params: Vec<ParamId>,
    pub member_callees: BTreeSet<String>,
}

/// The removals of a body the function being analysed names, by its name:
/// `None` when its analysis is not at hand.
pub type Removals<'a> = dyn Fn(&str) -> Option<&'a [ParamId]> + 'a;

/// The declared lifecycle members `function`'s drops, copies, and moves
/// run.
///
/// Upstream's destructor and copy/move insertion leaves a call of each: the
/// destructor a drop names, the one a drop naming none runs and, through a
/// synthesized one, its fields' ([`crate::mir::destroyed_structs`]), the
/// constructor a copy or move names, and the one a site naming none runs
/// ([`StructReads::member_symbols`]).
pub fn lifecycle_callees(
    function: &MirFunction,
    declarations: &MirDeclarations,
) -> BTreeSet<String> {
    fn sites<'f>(
        function: &'f MirFunction,
        blocks: &'f [MirBlock],
        out: &mut Vec<LifecycleSite<'f>>,
    ) {
        for block in blocks {
            for instruction in &block.instrs {
                out.extend(lifecycle_site(function, instruction));
                for region in instruction_regions(instruction) {
                    sites(function, region, out);
                }
            }
        }
    }
    let structs = StructReads::new(declarations);
    let destroyed = destroyed_structs(function, &|name| {
        declarations
            .structure(name)
            .map(|structure| structure.fields.as_slice())
    });
    let mut found: Vec<LifecycleSite<'_>> = destroyed
        .structs
        .iter()
        .map(|ty| (ty, "__deinit__", None))
        .collect();
    sites(function, &function.blocks, &mut found);
    found
        .into_iter()
        .flat_map(|(ty, member, named)| match named {
            Some(named) => BTreeSet::from([named.to_owned()]),
            None => structs.member_symbols(ty, member),
        })
        .chain(
            destroyed
                .named
                .into_iter()
                .map(|(_, destructor)| destructor),
        )
        .collect()
}

/// The lifted bodies `function`, the body of `name`, or its signature names.
///
/// They are `$comptime$f$…` and `$default$f$…` bodies, of which
/// [`unread_parameters`] reads the binders each kept.
pub fn named_lifted_bodies(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
) -> BTreeSet<String> {
    let mut walker = Walker::new(Vec::new(), &|_| None);
    if let Some(declaration) = declarations.function(name) {
        walker.signature(declaration);
    }
    walker.body(function);
    walker.lifted
}

/// The compile-time parameters of the struct whose method `name`, or the
/// method `name` is nested in, is: upstream's method generator lists them
/// before its own. Empty for a function that is no method.
pub fn method_owner_params<'a>(
    name: &'a str,
    declarations: &'a MirDeclarations,
) -> &'a [ParamDecl] {
    method_struct(name, declarations).map_or(&[], |(_, decls)| decls)
}

/// A compiler-known scalar, whose drop runs no code.
pub fn has_no_destructor(ty: &Ty) -> bool {
    matches!(
        ty.kind(),
        TyKind::Int
            | TyKind::UInt
            | TyKind::Bool
            | TyKind::Float64
            | TyKind::StringLiteral
            | TyKind::None
            | TyKind::IntLiteral
            | TyKind::FloatLiteral
            | TyKind::Dtype
            | TyKind::Simd { .. }
    )
}

/// `function`, the body of `name`, with every call dropping the arguments of the callee's
/// removed parameters, `removed(callee)`; `None` when no call drops one.
///
/// It is the body upstream's `RemoveUnusedParams` reads once it has
/// rewritten each call of a callee it processed before. Returns the view and the callees whose calls dropped an argument. The
/// view drops each such argument's register and expression, its entry in
/// the call's `instantiated_args` (when they align with the callee's
/// declarations), and then the instructions that only computed a dropped
/// argument: an unread copy of a slot whose type has no destructor, a
/// built-in operator that cannot trap or that only fed dropped arguments,
/// and anything trivially dead. A removed parameter that the default of a kept parameter the call
/// omits reads keeps its argument, which that default takes its value from.
pub fn without_removed_forwards<'a>(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
    removed: &dyn Fn(&str) -> Option<&'a [ParamId]>,
) -> Option<(MirFunction, Vec<String>)> {
    removed_forwards_view(name, function, declarations, removed, Spellings::Drop)
}

/// The body of `name` an instance runs, without the operands its calls
/// pass for their callees' removed parameters.
///
/// `function` with every call passing no operand for the callee's removed
/// parameters, `removed(callee)`, and the instructions that only computed
/// one erased, as [`without_removed_forwards`] views it; `None` when no
/// call drops one. It is the body an instance runs once upstream's `RemoveUnusedParams`
/// has rewritten each call of a generator it removed a parameter from: no
/// operation computes the argument, so none can trap (`10 // n` at
/// `n == 0`). A call's `instantiated_args` and a static receiver's
/// arguments keep their spellings, which the elaborator still binds the
/// callee's removed parameters from where they have a value.
pub fn with_removed_forwards_erased<'a>(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
    removed: &dyn Fn(&str) -> Option<&'a [ParamId]>,
) -> Option<MirFunction> {
    removed_forwards_view(name, function, declarations, removed, Spellings::Keep)
        .map(|(view, _)| view)
}

/// Erase from `function` every instruction that only computes a value
/// nothing reads and that may go without a trace: one
/// [`MirInstr::would_be_trivially_dead`] allows, a copy of a slot whose
/// type has no destructor or of a `callables` slot, or a built-in operator
/// over such operands that cannot trap (integer arithmetic wraps). A
/// built-in operator that can trap goes too when every read of it the body
/// held before the view dropped arguments (`original`) was a dropped
/// argument's or an erased instruction's that only fed one: a call's
/// parameter argument is a compile-time attribute on the pin, which
/// `RemoveUnusedParams` drops with no operation left to trap. Repeats until
/// nothing changes, drops the erased registers' types, and returns them.
///
/// A move of a parameter slot whose drop runs no code
/// ([`MirDeclarations::drop_runs_no_code`]) goes when every read of it
/// was a dropped argument's: the argument it handed a call is no longer
/// passed, and nothing is left to destroy. A copy of any other slot stays,
/// since the copy is a call of its own.
pub fn erase_forward_chains(
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    callables: &HashSet<VarId>,
    original: &[u32],
) -> HashSet<u32> {
    let mut erased = HashSet::new();
    let mut forwarded: Vec<u32> = original
        .iter()
        .zip(register_reads(function))
        .map(|(before, after)| before.saturating_sub(after))
        .collect();
    loop {
        let reads = register_reads(function);
        let before = erased.len();
        let MirFunction {
            blocks,
            var_tys,
            reg_types,
            n_params,
            ..
        } = &mut *function;
        let trivial = |reg: &crate::mir::Reg| reg_types.get(&reg.0).is_some_and(has_no_destructor);
        erase_unread(
            blocks,
            &mut |instruction| {
                let mut results = Vec::new();
                instruction_result_regs(instruction, &mut results);
                let forward_only = results.iter().all(|reg| {
                    let index = reg.0 as usize;
                    original.get(index).is_some_and(|reads| *reads > 0)
                        && forwarded.get(index) == original.get(index)
                });
                let erasable = match instruction {
                    MirInstr::UseVar { var, mode, .. } => {
                        callables.contains(var)
                            || var_tys.get(var).is_some_and(has_no_destructor)
                            || (forward_only
                                && matches!(mode, UseMode::Forward | UseMode::Move)
                                && (*var as usize) < *n_params
                                && var_tys
                                    .get(var)
                                    .is_some_and(|ty| declarations.drop_runs_no_code(ty)))
                    }
                    MirInstr::UnOp {
                        a, raises: None, ..
                    } => trivial(a),
                    MirInstr::BinOp {
                        op,
                        a,
                        b,
                        resolved: None,
                        raises: None,
                        ..
                    } => {
                        (forward_only
                            || matches!(
                                op,
                                InfixOp::Add
                                    | InfixOp::Sub
                                    | InfixOp::Mul
                                    | InfixOp::BitAnd
                                    | InfixOp::BitOr
                                    | InfixOp::BitXor
                                    | InfixOp::Eq
                                    | InfixOp::Ne
                                    | InfixOp::Lt
                                    | InfixOp::Gt
                                    | InfixOp::Le
                                    | InfixOp::Ge
                            ))
                            && trivial(a)
                            && trivial(b)
                    }
                    _ => instruction.would_be_trivially_dead(),
                };
                if erasable && forward_only {
                    let mut operands = Vec::new();
                    instruction_operand_regs(instruction, &mut operands);
                    for reg in operands {
                        if let Some(count) = forwarded.get_mut(reg.0 as usize) {
                            *count += 1;
                        }
                    }
                }
                erasable
            },
            &reads,
            &mut erased,
        );
        if erased.len() == before {
            break;
        }
    }
    function.reg_types.retain(|reg, _| !erased.contains(reg));
    erased
}

/// How many times `function`'s body reads each of its registers.
pub fn register_reads(function: &mut MirFunction) -> Vec<u32> {
    let mut reads = vec![0_u32; function.n_regs as usize];
    count_register_reads(&mut function.blocks, &mut reads);
    reads
}

/// The parameters `unread_parameters` may remove from `name`: its own
/// compile-time parameters and, for a method, its struct's, or a nested
/// function's enclosing declarations' parameters, each once. A lifted
/// function binds every binder it declares by its own arguments, whoever
/// owns it, as upstream's lifted function is an ordinary generator.
fn candidate_parameters<'d>(
    name: &'d str,
    declaration: &'d MirFunctionDeclaration,
    declarations: &'d MirDeclarations,
) -> Vec<&'d ParamDecl> {
    if mojito_symbol::symbol::lifted_owner(name).is_some() {
        return declaration.param_decls.iter().collect();
    }
    let mut candidates: Vec<&ParamDecl> = Vec::new();
    if declaration.enclosing.is_none() {
        candidates.extend(
            declaration
                .param_decls
                .iter()
                .filter(|decl| &*decl.id().owner == name),
        );
        for decl in method_owner_params(name, declarations) {
            if !candidates.iter().any(|seen| seen.id() == decl.id()) {
                candidates.push(decl);
            }
        }
    }
    let mut link = declaration
        .enclosing
        .as_deref()
        .and_then(|enclosing| declarations.function(enclosing));
    while let Some(enclosing) = link {
        for decl in &enclosing.param_decls {
            if !candidates.iter().any(|seen| seen.id() == decl.id()) {
                candidates.push(decl);
            }
        }
        link = enclosing
            .enclosing
            .as_deref()
            .and_then(|enclosing| declarations.function(enclosing));
    }
    candidates
}

/// The struct whose method `name`, or the outermost declaration `name` is
/// nested in, is, by its spelling, with its compile-time parameters.
fn method_struct<'a>(
    name: &'a str,
    declarations: &'a MirDeclarations,
) -> Option<(&'a str, &'a [ParamDecl])> {
    let mut root = name;
    while let Some(enclosing) = declarations
        .function(root)
        .and_then(|declaration| declaration.enclosing.as_deref())
    {
        root = enclosing;
    }
    let (owner, _) = mojito_symbol::symbol::split_method_symbol(root)?;
    declarations
        .structure(owner)
        .map(|structure| (owner, structure.param_decls.as_slice()))
}

/// The slots of `function` that a callable value parameter among `decls`
/// binds. Its value is a compile-time one, so copying or dropping the slot
/// runs no code.
fn callable_parameter_slots<'d>(
    function: &MirFunction,
    decls: impl IntoIterator<Item = &'d ParamDecl>,
) -> HashSet<VarId> {
    let names: HashSet<&str> = decls
        .into_iter()
        .filter(|decl| {
            matches!(
                decl,
                ParamDecl::Value { ty, callable_default, .. }
                    if callable_default.is_some()
                        || matches!(ty.kind(), TyKind::Func(_) | TyKind::GenericFunc(_))
            )
        })
        .map(ParamDecl::name)
        .collect();
    function
        .var_names
        .iter()
        .enumerate()
        .filter(|(_, name)| names.contains(name.as_str()))
        .map(|(slot, _)| slot as VarId)
        .collect()
}

/// Whether a view of a body drops a removed parameter's argument from a
/// call's `instantiated_args` and its static receiver, as
/// [`without_removed_forwards`] does, or keeps them, as
/// [`with_removed_forwards_erased`] does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Spellings {
    Drop,
    Keep,
}

/// The view of `function` that [`without_removed_forwards`] or, by
/// `spellings`, [`with_removed_forwards_erased`] describes, beside the
/// callees whose calls dropped an argument.
fn removed_forwards_view<'a>(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
    removed: &dyn Fn(&str) -> Option<&'a [ParamId]>,
    spellings: Spellings,
) -> Option<(MirFunction, Vec<String>)> {
    if !calls_with_removed(&function.blocks, removed) {
        return None;
    }
    let mut view = function.clone();
    let original = register_reads(&mut view);
    let mut discounted = BTreeSet::new();
    drop_removed_arguments(
        &mut view.blocks,
        declarations,
        removed,
        spellings,
        &mut discounted,
    );
    if discounted.is_empty() {
        return None;
    }
    let callables = declarations
        .function(name)
        .map(|declaration| callable_parameter_slots(function, &declaration.param_decls))
        .unwrap_or_default();
    erase_forward_chains(&mut view, declarations, &callables, &original);
    Some((view, discounted.into_iter().collect()))
}

/// Whether `blocks` or their regions call a callee `removed` names
/// parameters of, by a call, a resolved method call, or a subscript call.
fn calls_with_removed<'a>(
    blocks: &[MirBlock],
    removed: &dyn Fn(&str) -> Option<&'a [ParamId]>,
) -> bool {
    blocks
        .iter()
        .flat_map(|block| &block.instrs)
        .any(|instruction| {
            let target = match instruction {
                MirInstr::Call { func, .. } => Some(func.0.as_str()),
                MirInstr::MethodCall { resolved, .. } => resolved.as_deref(),
                _ => instruction_subscript_call(instruction).map(|call| call.target.as_str()),
            };
            target.and_then(removed).is_some_and(|ids| !ids.is_empty())
                || instruction_regions(instruction)
                    .into_iter()
                    .any(|region| calls_with_removed(region, removed))
        })
}

/// Drop from each call in `blocks` and their regions the arguments of the
/// callee's removed parameters, their `instantiated_args` entries and
/// static receiver arguments as `spellings` says, adding each callee that
/// lost one to `discounted`.
fn drop_removed_arguments<'a>(
    blocks: &mut [MirBlock],
    declarations: &MirDeclarations,
    removed: &dyn Fn(&str) -> Option<&'a [ParamId]>,
    spellings: Spellings,
    discounted: &mut BTreeSet<String>,
) {
    for instruction in blocks.iter_mut().flat_map(|block| &mut block.instrs) {
        for region in instruction_regions_mut(instruction) {
            drop_removed_arguments(region, declarations, removed, spellings, discounted);
        }
        if let Some(target) = drop_subscript_arguments(instruction, removed) {
            discounted.insert(target);
            continue;
        }
        let (target, decls, param_arg_regs, instantiated_args) = match instruction {
            MirInstr::Call {
                func,
                param_arg_regs,
                instantiated_args,
                receiver,
                ..
            } => {
                let Some(callee) = declarations.function(&func.0) else {
                    continue;
                };
                if let Some(ids) = removed(&func.0)
                    && spellings == Spellings::Drop
                    && drop_receiver_arguments(receiver, declarations, ids)
                {
                    discounted.insert(func.0.clone());
                }
                (
                    &func.0,
                    callee.param_decls.as_slice(),
                    param_arg_regs,
                    (spellings == Spellings::Drop).then_some(instantiated_args),
                )
            }
            MirInstr::MethodCall {
                resolved: Some(target),
                param_arg_regs,
                param_decls,
                instantiated_args,
                ..
            } => {
                let Some(ids) = removed(target) else {
                    continue;
                };
                let dropped = drop_arguments(
                    target,
                    param_decls,
                    ids,
                    param_arg_regs,
                    (spellings == Spellings::Drop).then_some(instantiated_args),
                );
                if let Some(dropped) = dropped {
                    // The rewritten call declares no removed parameter
                    // either: a self-recursive call's own declarations
                    // name the caller's binders.
                    if spellings == Spellings::Drop {
                        for index in dropped.into_iter().rev() {
                            param_decls.remove(index);
                        }
                    }
                    discounted.insert(target.clone());
                }
                continue;
            }
            _ => continue,
        };
        if let Some(ids) = removed(target)
            && drop_arguments(target, decls, ids, param_arg_regs, instantiated_args).is_some()
        {
            discounted.insert(target.clone());
        }
    }
}

/// Drop from a static call's spelled receiver `S[…]` the arguments of the
/// struct's parameters among the callee's removed `ids`, and the receiver
/// itself when it keeps none; returns whether it dropped one. A receiver
/// that does not spell every parameter of its struct is left whole.
fn drop_receiver_arguments(
    receiver: &mut Option<Ty>,
    declarations: &MirDeclarations,
    ids: &[ParamId],
) -> bool {
    let Some(TyKind::Struct(name, arguments)) = receiver.as_ref().map(Ty::kind) else {
        return false;
    };
    let Some(decls) = declarations
        .structure(name)
        .map(|structure| structure.param_decls.as_slice())
        .filter(|decls| arguments.len() >= decls.len())
    else {
        return false;
    };
    let kept: Vec<bool> = decls.iter().map(|decl| !ids.contains(decl.id())).collect();
    if kept.iter().all(|kept| *kept) {
        return false;
    }
    *receiver = kept.contains(&true).then(|| {
        let arguments = arguments
            .iter()
            .enumerate()
            .filter(|(index, _)| kept.get(*index).is_none_or(|kept| *kept))
            .map(|(_, argument)| argument.clone())
            .collect();
        TyKind::Struct(name.clone(), arguments).into()
    });
    true
}

/// Drop from a subscript call in `instruction` the arguments of its
/// accessor's removed parameters; returns the accessor when it dropped one.
///
/// An accessor that takes no runtime index (`__getitem_param__[i: Int]`)
/// gets its index only as the parameter argument, which the pin spells as
/// an attribute with no operand; so an index operand that held a dropped
/// argument names the receiver in the view, which the instruction reads
/// anyway, and the argument's register loses its last reader.
fn drop_subscript_arguments<'a>(
    instruction: &mut MirInstr,
    removed: &dyn Fn(&str) -> Option<&'a [ParamId]>,
) -> Option<String> {
    let (receiver, indexes, call) = match instruction {
        MirInstr::Index {
            base,
            index,
            call: Some(call),
            ..
        } => (*base, vec![index], &mut **call),
        MirInstr::Slice {
            object,
            call: Some(call),
            ..
        } => (*object, Vec::new(), &mut **call),
        MirInstr::MultiIndex {
            object,
            args,
            call: Some(call),
            ..
        } => (*object, index_operands(args), &mut **call),
        MirInstr::MultiSet {
            receiver,
            args,
            call,
            ..
        } => (*receiver, index_operands(args), &mut **call),
        _ => return None,
    };
    let ids = removed(&call.target)?;
    let carriers: Vec<Reg> = call
        .param_arg_regs
        .iter()
        .filter_map(|argument| argument.value)
        .collect();
    drop_arguments(
        &call.target,
        &call.param_decls,
        ids,
        &mut call.param_arg_regs,
        None,
    )?;
    if call.arguments.is_empty() {
        let kept: Vec<Reg> = call
            .param_arg_regs
            .iter()
            .filter_map(|argument| argument.value)
            .collect();
        for index in indexes {
            if carriers.contains(index) && !kept.contains(index) {
                *index = receiver;
            }
        }
    }
    Some(call.target.clone())
}

/// The index operands among a subscript's bracket arguments, mutably.
fn index_operands(args: &mut [MirSubscriptArg]) -> Vec<&mut Reg> {
    args.iter_mut()
        .filter_map(|argument| match argument {
            MirSubscriptArg::Index(register) => Some(register),
            MirSubscriptArg::Slice { .. } => None,
        })
        .collect()
}

/// Drop from one call of `target`, whose compile-time parameters are
/// `decls`, the arguments of its removed parameters `ids`, and their
/// entries in its `instantiated_args` when it has them; returns the indices
/// in `decls` of the parameters it dropped, when it dropped one. A removed
/// parameter's argument stays when the call omits a kept parameter whose
/// default reads it.
fn drop_arguments(
    target: &str,
    decls: &[ParamDecl],
    ids: &[ParamId],
    param_arg_regs: &mut [MirParamArg],
    instantiated_args: Option<&mut Vec<TyArg>>,
) -> Option<Vec<usize>> {
    let supplied: Vec<&ParamId> = supplied_parameter_arguments(decls, param_arg_regs)
        .into_iter()
        .map(|(decl, _)| decl.id())
        .collect();
    let mut defaulted = Walker::new(vec![target], &|_| None);
    for decl in decls
        .iter()
        .filter(|decl| !ids.contains(decl.id()) && !supplied.contains(&decl.id()))
    {
        match decl {
            ParamDecl::Type { default, .. } => defaulted.visit(default),
            ParamDecl::Value { default, .. } => defaulted.visit(default),
        }
    }
    let dropped: Vec<usize> = decls
        .iter()
        .enumerate()
        .filter(|(_, decl)| {
            ids.contains(decl.id())
                && !defaulted.reads_all
                && !defaulted.slots.contains(&(0, decl.id().slot))
        })
        .map(|(index, _)| index)
        .collect();
    let arguments: Vec<usize> = matched_parameter_arguments(decls, param_arg_regs)
        .into_iter()
        .filter(|(decl, _, _)| dropped.iter().any(|index| decls[*index].id() == decl.id()))
        .filter_map(|(_, _, argument)| {
            param_arg_regs
                .iter()
                .position(|candidate| std::ptr::eq(candidate, argument))
        })
        .collect();
    let mut changed = false;
    for index in arguments {
        let argument = &mut param_arg_regs[index];
        changed |= argument.value.take().is_some() | argument.expr.take().is_some();
    }
    if let Some(instantiated_args) = instantiated_args
        && instantiated_args.len() == decls.len()
    {
        for &index in dropped.iter().rev() {
            instantiated_args.remove(index);
            changed = true;
        }
    }
    changed.then_some(dropped)
}

/// Remove each instruction of `blocks` and their regions that `erasable`
/// accepts and that defines only unread results, adding them to `erased`.
fn erase_unread(
    blocks: &mut [MirBlock],
    erasable: &mut dyn FnMut(&MirInstr) -> bool,
    reads: &[u32],
    erased: &mut HashSet<u32>,
) {
    for block in blocks {
        block.instrs.retain_mut(|instruction| {
            for region in instruction_regions_mut(instruction) {
                erase_unread(region, erasable, reads, erased);
            }
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            let dead = !results.is_empty()
                && results
                    .iter()
                    .all(|reg| reads.get(reg.0 as usize) == Some(&0))
                && erasable(instruction);
            if dead {
                erased.extend(results.iter().map(|reg| reg.0));
            }
            !dead
        });
    }
}

/// The names of the value parameters among `candidates` whose slot the body
/// reads: any instruction naming the slot, but a drop or a declaration of a
/// slot whose type has no destructor to run or that a callable parameter
/// binds.
fn slot_reads(function: &MirFunction, candidates: &[&ParamDecl]) -> HashSet<String> {
    let names: HashSet<&str> = candidates
        .iter()
        .filter(|decl| matches!(decl, ParamDecl::Value { .. }))
        .map(|decl| decl.name().trim_start_matches('*'))
        .collect();
    let slots: HashMap<VarId, &str> = function
        .var_names
        .iter()
        .enumerate()
        .filter_map(|(slot, name)| names.get(name.as_str()).map(|name| (slot as VarId, *name)))
        .collect();
    let mut reads = HashSet::new();
    if !slots.is_empty() {
        let mut cx = SlotReads {
            function,
            slots: &slots,
            callables: callable_parameter_slots(function, candidates.iter().copied()),
            reads: &mut reads,
        };
        cx.blocks(&function.blocks);
    }
    reads
}

/// The parameters of the struct `spelled` (`decls`) that `function` reads
/// through a projection of a value of it: `Self.n` is the receiver's
/// reified parameter, `self.n`, which reads `n` as the binder does.
fn receiver_parameter_reads<'d>(
    function: &MirFunction,
    spelled: &str,
    decls: &'d [ParamDecl],
    declarations: &MirDeclarations,
) -> HashSet<&'d ParamId> {
    fn walk<'f>(blocks: &'f [MirBlock], places: &mut Vec<&'f MirPlace>) {
        for block in blocks {
            for instruction in &block.instrs {
                places.extend(instruction_places(instruction));
                for region in instruction_regions(instruction) {
                    walk(region, places);
                }
            }
        }
    }
    let fields = declarations
        .structure(spelled)
        .map_or(&[][..], |structure| structure.fields.as_slice());
    let mut places = Vec::new();
    walk(&function.blocks, &mut places);
    let mut reads = HashSet::new();
    for place in places {
        let bases = place.root_ty.iter().chain(&place.projection_tys);
        for (projection, base) in place.proj.iter().zip(bases) {
            if let Proj::Field(name) = projection
                && let TyKind::Struct(base, _) = match base.kind() {
                    TyKind::Ref(reference) => reference.referent.kind(),
                    kind => kind,
                }
                && base == spelled
                && !fields.iter().any(|(field, _)| field == name)
            {
                reads.extend(
                    decls
                        .iter()
                        .filter(|decl| decl.name() == name)
                        .map(ParamDecl::id),
                );
            }
        }
    }
    reads
}

/// A copy or move site: its value's type, the constructor it runs, and the
/// one it names, if it names one.
type LifecycleSite<'f> = (&'f Ty, &'static str, Option<&'f str>);

/// The type a copy or move `instruction` relocates, the lifecycle member
/// that runs on it, and the one it names: a copying or moving slot use, a
/// register copy, or a place move.
fn lifecycle_site<'f>(
    function: &'f MirFunction,
    instruction: &'f MirInstr,
) -> Option<LifecycleSite<'f>> {
    match instruction {
        MirInstr::UseVar {
            var,
            mode,
            constructor,
            ..
        } => function
            .var_tys
            .get(var)
            .zip(crate::mir::use_constructor_member(*mode))
            .map(|(ty, member)| (ty, member, constructor.as_deref())),
        MirInstr::CopyValue {
            dest, constructor, ..
        } => function
            .reg_types
            .get(&dest.0)
            .map(|ty| (ty, "__copyinit__", constructor.as_deref())),
        MirInstr::MovePlace { place, .. } => {
            place_ty(function, place).map(|ty| (ty, "__moveinit__", None))
        }
        _ => None,
    }
}

/// The type of the storage `place` designates.
fn place_ty<'f>(function: &'f MirFunction, place: &'f MirPlace) -> Option<&'f Ty> {
    place
        .ty
        .as_ref()
        .or_else(|| place.projection_tys.last())
        .or_else(|| place.root_ty.as_ref().filter(|_| place.proj.is_empty()))
        .or_else(|| {
            place
                .proj
                .is_empty()
                .then(|| function.var_tys.get(&place.root))
                .flatten()
        })
}

struct SlotReads<'a> {
    function: &'a MirFunction,
    slots: &'a HashMap<VarId, &'a str>,
    callables: HashSet<VarId>,
    reads: &'a mut HashSet<String>,
}

impl SlotReads<'_> {
    fn blocks(&mut self, blocks: &[MirBlock]) {
        for block in blocks {
            for instruction in &block.instrs {
                self.instruction(instruction);
                for region in instruction_regions(instruction) {
                    self.blocks(region);
                }
            }
            if let MirTerm::ReturnWithCleanup { cleanup, .. }
            | MirTerm::EscapeJump { cleanup, .. } = &block.term
            {
                self.drops(cleanup);
            }
        }
    }

    fn instruction(&mut self, instruction: &MirInstr) {
        match instruction {
            MirInstr::DropVar { var, .. }
            | MirInstr::ConsumeVar { var }
            | MirInstr::DeclareVar { var } => {
                self.drops(std::slice::from_ref(var));
            }
            MirInstr::Try {
                handler, cleanup, ..
            } => {
                if let Some((Some(binding), _)) = handler {
                    self.read(*binding);
                }
                self.drops(cleanup);
            }
            _ => {
                for slot in instruction_slots(instruction) {
                    self.read(slot);
                }
            }
        }
    }

    /// Dropping a slot reads its value only when a destructor runs on it.
    fn drops(&mut self, slots: &[VarId]) {
        for &slot in slots {
            if !self.callables.contains(&slot)
                && !self
                    .function
                    .var_tys
                    .get(&slot)
                    .is_some_and(has_no_destructor)
            {
                self.read(slot);
            }
        }
    }

    fn read(&mut self, slot: VarId) {
        if let Some(name) = self.slots.get(&slot) {
            self.reads.insert((*name).to_string());
        }
    }
}

/// The serializer [`unread_parameters`] walks a value with, looking for the
/// identities of the binders of `owners`.
struct Walker<'o, 'a, 'n> {
    owners: Vec<&'o str>,
    /// The functions whose nested (`f$…`) and lifted (`$comptime$f$…`)
    /// bodies a walked value may name.
    bodies: Vec<&'o str>,
    /// The struct whose method the function is, and its binders as `slots`
    /// entries: the struct spelled with no arguments is the method's `Self`,
    /// which reads the ones its layout reads, and under a nominal read all
    /// of them.
    self_spelling: Option<(&'o str, Vec<(usize, usize)>)>,
    /// The struct whose method the function is, spelled at its own
    /// parameters: the lifecycle members its `Self` runs read them.
    self_type: Option<Ty>,
    /// Each tuple variant the walk is inside: for a struct type, how many of
    /// its fields were walked, whether its name is `self_spelling`'s, and,
    /// under `layout`, what its layout reads of its arguments.
    variants: Vec<Option<StructFrame>>,
    /// Whether the next sequence is the arguments of a struct type spelling
    /// `self_spelling`'s name.
    self_arguments: bool,
    /// The struct reads a `layout` walk consults.
    structs: Option<&'n StructReads<'n, 'a>>,
    /// Whether a struct type reads only the arguments its lowered layout
    /// reads, as upstream's walk over `LowerLIT`'s structural types does.
    layout: bool,
    /// What the struct whose arguments the next sequence holds reads of
    /// them, under `layout`.
    struct_arguments: Option<LayoutReads>,
    /// Each sequence the walk is inside: for a struct's arguments under
    /// `layout`, what its layout reads and how many were walked.
    sequences: Vec<Option<(LayoutReads, usize)>>,
    /// How many struct arguments the walk is inside that their struct's
    /// layout does not read: a binder named there is no read.
    unread: usize,
    /// How many values the walk is inside that name their binders whole
    /// whatever a layout reads: a value argument, a dependent type, a
    /// projection.
    nominal: usize,
    /// Each struct variant the walk is inside, and whether it is nominal.
    struct_variants: Vec<bool>,
    /// The removals of the nested bodies a walked value names.
    nested: &'n Removals<'a>,
    /// The removals of the members a call the walk reads names.
    callees: Option<&'n Removals<'a>>,
    /// The member callees whose removals a call read.
    member_callees: BTreeSet<String>,
    /// The binders a walked value names, as the index of their owner in
    /// `owners` and their slot.
    slots: BTreeSet<(usize, usize)>,
    /// The removals of each body nested in an owner that a walked value
    /// names: it reads every candidate they do not hold.
    nested_removals: Vec<&'a [ParamId]>,
    /// Whether a walked value names a body lifted from an owner or nested
    /// in one with no removals at hand, or holds a leaf no walk can see
    /// into, so that every parameter counts as read.
    reads_all: bool,
    /// The declarations a lifted body a walked value names is read from.
    declarations: Option<&'n MirDeclarations>,
    /// The lifted bodies a walked value names.
    lifted: BTreeSet<String>,
    /// The last leaf a walk visited.
    leaf: Leaf,
    /// Each tuple the walk is inside: its length, how many of its elements
    /// were walked, and the owner its first named.
    tuples: Vec<(usize, usize, Option<usize>)>,
    /// The reads each type walked so far makes: a type is a uniqued handle
    /// that many registers and slots share.
    types: HashMap<(Ty, bool), TypeReads<'a>>,
}

/// The binders, nested bodies' removals, opacity, and lifted bodies of one
/// walked type.
type TypeReads<'a> = (
    BTreeSet<(usize, usize)>,
    Vec<&'a [ParamId]>,
    bool,
    BTreeSet<String>,
);

/// A struct type the walk is inside: how many of its fields were walked,
/// whether its name is `self_spelling`'s, and what its layout reads.
struct StructFrame {
    field: usize,
    self_named: bool,
    reads: Option<LayoutReads>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Leaf {
    Other,
    Owner(usize),
    Slot(u64),
}

impl<'o, 'a, 'n> Walker<'o, 'a, 'n> {
    fn new(owners: Vec<&'o str>, nested: &'n Removals<'a>) -> Self {
        Self {
            bodies: owners.clone(),
            owners,
            self_spelling: None,
            self_type: None,
            variants: Vec::new(),
            self_arguments: false,
            structs: None,
            layout: false,
            struct_arguments: None,
            sequences: Vec::new(),
            unread: 0,
            nominal: 0,
            struct_variants: Vec::new(),
            nested,
            callees: None,
            member_callees: BTreeSet::new(),
            slots: BTreeSet::new(),
            nested_removals: Vec::new(),
            reads_all: false,
            declarations: None,
            lifted: BTreeSet::new(),
            leaf: Leaf::Other,
            tuples: Vec::new(),
            types: HashMap::new(),
        }
    }

    /// The declaration's signature: its runtime parameters, collectors,
    /// result, and error, as the walk's mode reads them, and whole its
    /// runtime defaults, `where` clauses, and each compile-time parameter's
    /// type and constraints. Not a compile-time parameter's default, nor a
    /// runtime one every call omitting it evaluates
    /// ([`crate::mir::evaluated_at_call_site`]), which the call site does.
    fn signature(&mut self, declaration: &MirFunctionDeclaration) {
        for ty in declaration
            .param_types
            .iter()
            .chain(&declaration.variadic)
            .chain(&declaration.kw_variadic)
            .chain(std::iter::once(&declaration.ret_ty))
            .chain(&declaration.error_ty)
        {
            self.ty(ty);
        }
        let layout = std::mem::replace(&mut self.layout, false);
        for (slot, default) in declaration.defaults.iter().enumerate() {
            if !self.declarations.is_some_and(|declarations| {
                evaluated_at_call_site(&declaration.lowered_name, declaration, slot, declarations)
            }) {
                self.visit(default);
            }
        }
        self.visit(&declaration.availability);
        for decl in &declaration.param_decls {
            match decl {
                ParamDecl::Type {
                    callable_bound,
                    constraints,
                    ..
                } => {
                    if let Some(bound) = callable_bound {
                        self.ty(bound);
                    }
                    self.visit(constraints);
                }
                ParamDecl::Value {
                    ty, constraints, ..
                } => {
                    self.ty(ty);
                    self.visit(constraints);
                }
            }
        }
        self.layout = layout;
    }

    /// The body: its blocks, and the types of its signature, slots, and
    /// registers.
    fn body(&mut self, function: &MirFunction) {
        self.visit(&function.blocks);
        for ty in function
            .param_types
            .iter()
            .chain(function.var_tys.values())
            .chain(&function.ret_ty)
            .chain(&function.error_ty)
            .chain(function.reg_types.values())
        {
            self.ty(ty);
        }
    }

    /// The reads the layout walk leaves out of `function`: each type value
    /// and compile-time condition its instructions hold, every type an
    /// instruction that runs a member reads or names, and the parts of a
    /// copied, moved, or dropped value's type its lifecycle member reads.
    fn nominal_reads(&mut self, function: &MirFunction, declarations: &MirDeclarations) {
        if self.structs.is_none() {
            return;
        }
        let layout = std::mem::replace(&mut self.layout, false);
        self.nominal_blocks(function, declarations, &function.blocks);
        let destroyed = destroyed_structs(function, &|name| {
            declarations
                .structure(name)
                .map(|structure| structure.fields.as_slice())
        });
        let named = destroyed.named.iter().map(|(ty, _)| ty);
        for ty in destroyed.structs.iter().chain(named) {
            if let Some(reads) = self.member_reads(ty, "__deinit__") {
                self.visit(&reads);
            }
        }
        self.layout = layout;
    }

    /// [`StructReads::member_reads`] of `ty`, where the method's `Self`
    /// (its struct spelled with no arguments) runs the member over the
    /// struct at its own parameters.
    fn member_reads(&self, ty: &Ty, member: &'static str) -> Option<Vec<TyArg>> {
        let ty = self.spelled_self(ty);
        self.structs.map(|structs| structs.member_reads(ty, member))
    }

    /// `ty`, but the method's `Self` (its struct spelled with no
    /// arguments) spelled at the struct's own parameters.
    fn spelled_self<'t>(&'t self, ty: &'t Ty) -> &'t Ty {
        match (ty.kind(), &self.self_type) {
            (TyKind::Struct(name, arguments), Some(self_type))
                if arguments.is_empty()
                    && matches!(self_type.kind(), TyKind::Struct(spelled, _) if spelled == name) =>
            {
                self_type
            }
            _ => ty,
        }
    }

    fn nominal_blocks(
        &mut self,
        function: &MirFunction,
        declarations: &MirDeclarations,
        blocks: &[MirBlock],
    ) {
        for block in blocks {
            for instruction in &block.instrs {
                self.nominal_instruction(function, declarations, instruction);
                for region in instruction_regions(instruction) {
                    self.nominal_blocks(function, declarations, region);
                }
            }
            if matches!(
                block.term,
                MirTerm::ComptimeBranch { .. } | MirTerm::ComptimeFor { .. }
            ) {
                self.visit(&block.term);
            }
        }
    }

    /// What `instruction` reads whole: a declared callee's parameter
    /// arguments, a constant, a compile-time condition or sequence, the
    /// lifecycle member a copy or a move runs, and, for an instruction that
    /// may run a member of a struct it names (a builtin such as `len`
    /// dispatches to its argument's `__len__`), the instruction and every
    /// type of its registers and slots. A method call's places only address
    /// the values it passes, so of each place only the type it designates
    /// (the projected receiver's, whose member runs) is read whole; its
    /// root and projections read their layout with the body. Drops are
    /// [`Self::nominal_reads`]'s.
    fn nominal_instruction(
        &mut self,
        function: &MirFunction,
        declarations: &MirDeclarations,
        instruction: &MirInstr,
    ) {
        if self.member_instruction(function, declarations, instruction) {
            return;
        }
        match instruction {
            MirInstr::Call {
                func,
                raises,
                receiver,
                param_arg_regs,
                instantiated_args,
                ..
            } if declarations.function(&func.0).is_some() => {
                self.visit(raises);
                self.visit(receiver);
                self.visit(param_arg_regs);
                self.visit(instantiated_args);
            }
            MirInstr::Const { k, .. } => self.visit(k),
            MirInstr::ComptimeIf { conds, .. } => self.visit(conds),
            MirInstr::ComptimeFor { binder, source, .. } => {
                self.visit(binder);
                self.visit(source);
            }
            MirInstr::UseVar { .. } | MirInstr::CopyValue { .. } | MirInstr::MovePlace { .. } => {
                if let Some(reads) = lifecycle_site(function, instruction)
                    .and_then(|(ty, member, _)| self.member_reads(ty, member))
                {
                    self.visit(&reads);
                }
            }
            MirInstr::UnOp { raises: None, .. }
            | MirInstr::BinOp {
                resolved: None,
                raises: None,
                ..
            }
            | MirInstr::EstablishLoans { .. }
            | MirInstr::InvalidateInteriors { .. }
            | MirInstr::MakeRef { .. }
            | MirInstr::ReadRef { .. }
            | MirInstr::Rebind { .. }
            | MirInstr::WriteRef { .. }
            | MirInstr::KeepAlive { .. }
            | MirInstr::DeclareVar { .. }
            | MirInstr::DefVar { .. }
            | MirInstr::GetField { .. }
            | MirInstr::Store { .. }
            | MirInstr::StoreRef { .. }
            | MirInstr::LoadPlace { .. }
            | MirInstr::MakeTuple { .. }
            | MirInstr::MarkInitialized { .. }
            | MirInstr::MarkDestroyed { .. }
            | MirInstr::DropVar { .. }
            | MirInstr::DropPlace { .. }
            | MirInstr::ConsumeVar { .. }
            | MirInstr::ConsumePlace { .. }
            | MirInstr::If { .. }
            | MirInstr::Loop { .. }
            | MirInstr::Try { .. } => {}
            MirInstr::MethodCall {
                recv_place,
                arg_places,
                kwarg_places,
                ..
            } => {
                let mut dispatched = instruction.clone();
                if let MirInstr::MethodCall {
                    recv_place,
                    arg_places,
                    kwarg_places,
                    ..
                } = &mut dispatched
                {
                    *recv_place = None;
                    for place in arg_places.iter_mut().chain(kwarg_places.iter_mut()) {
                        *place = None;
                    }
                }
                self.dispatching_instruction(function, &dispatched);
                for place in recv_place
                    .as_deref()
                    .into_iter()
                    .chain(arg_places.iter().chain(kwarg_places).flatten())
                {
                    match &place.ty {
                        Some(ty) => self.ty(ty),
                        None => self.visit(place),
                    }
                }
            }
            _ => self.dispatching_instruction(function, instruction),
        }
    }

    /// Whether `instruction` calls a member it names as upstream's call does
    /// ([`Self::member_call`]), which it then has read: a resolved method
    /// call, a subscript call, a resolved operator, or an iteration step.
    fn member_instruction(
        &mut self,
        function: &MirFunction,
        declarations: &MirDeclarations,
        instruction: &MirInstr,
    ) -> bool {
        let register = |reg: &Reg| function.reg_types.get(&reg.0);
        match instruction {
            MirInstr::MethodCall {
                resolved: Some(callee),
                recv,
                recv_place,
                raises,
                param_arg_regs,
                instantiated_args,
                ..
            } => self.member_call(
                declarations,
                callee,
                recv_place
                    .as_deref()
                    .and_then(|place| place_ty(function, place))
                    .or_else(|| register(recv)),
                &(raises, param_arg_regs, instantiated_args),
                |own| own.is_empty() || instantiated_args.len() >= own.len(),
            ),
            MirInstr::BinOp {
                resolved: Some(callee),
                a,
                raises,
                ..
            } => self.member_call(
                declarations,
                callee,
                register(a),
                raises,
                |own: &[&ParamDecl]| own.is_empty(),
            ),
            MirInstr::GetIter {
                source,
                prepare,
                mode,
                ..
            } => match prepare.as_slice() {
                [callee] => self.member_call(
                    declarations,
                    callee,
                    function.var_tys.get(source),
                    mode,
                    |own: &[&ParamDecl]| own.is_empty(),
                ),
                _ => false,
            },
            MirInstr::TryNext {
                iter,
                call,
                exhaustion,
                ..
            } => self.member_call(
                declarations,
                &call.target,
                function.var_tys.get(iter),
                &(&call.raises, exhaustion),
                |own: &[&ParamDecl]| own.is_empty(),
            ),
            _ => {
                let Some(call) = instruction_subscript_call(instruction) else {
                    return false;
                };
                let receiver = match instruction {
                    MirInstr::Index {
                        base, base_place, ..
                    } => base_place
                        .as_deref()
                        .and_then(|place| place_ty(function, place))
                        .or_else(|| register(base)),
                    MirInstr::Slice {
                        object,
                        object_place,
                        ..
                    }
                    | MirInstr::MultiIndex {
                        object,
                        object_place,
                        ..
                    } => object_place
                        .as_deref()
                        .and_then(|place| place_ty(function, place))
                        .or_else(|| register(object)),
                    MirInstr::MultiSet {
                        receiver,
                        receiver_place,
                        ..
                    } => receiver_place
                        .as_deref()
                        .and_then(|place| place_ty(function, place))
                        .or_else(|| register(receiver)),
                    _ => None,
                };
                let supplied: Vec<&ParamId> =
                    supplied_parameter_arguments(&call.param_decls, &call.param_arg_regs)
                        .into_iter()
                        .map(|(decl, _)| decl.id())
                        .collect();
                self.member_call(
                    declarations,
                    &call.target,
                    receiver,
                    &(&call.raises, &call.param_arg_regs),
                    |own| own.iter().all(|decl| supplied.contains(&decl.id())),
                )
            }
        }
    }

    /// Read a call of the member `callee` on a value of type `receiver` as
    /// upstream's call, which passes the member's parameters as values,
    /// reads: the receiver's arguments at the struct parameters the member
    /// kept, and the call's own `parts`, each whole. Its operands and
    /// result read their layout with the body. `own_recorded` judges
    /// whether the call records the member's kept own parameters.
    ///
    /// Returns `false`, reading nothing, when the member's removals are not
    /// at hand, the receiver does not spell every parameter of the member's
    /// struct (a trait dispatch, a variadic struct), or the call does not
    /// record the member's own parameters.
    fn member_call<T: Serialize + ?Sized>(
        &mut self,
        declarations: &MirDeclarations,
        callee: &str,
        receiver: Option<&Ty>,
        parts: &T,
        own_recorded: impl FnOnce(&[&ParamDecl]) -> bool,
    ) -> bool {
        let Some(removed) = self.callees.and_then(|callees| callees(callee)) else {
            return false;
        };
        let Some(receiver) = receiver.map(|receiver| match receiver.kind() {
            TyKind::Ref(reference) => &reference.referent,
            _ => receiver,
        }) else {
            return false;
        };
        let receiver = self.spelled_self(receiver).clone();
        let TyKind::Struct(name, arguments) = receiver.kind() else {
            return false;
        };
        let Some(decls) = mojito_symbol::symbol::split_method_symbol(callee)
            .filter(|(owner, _)| owner == name)
            .and_then(|_| declarations.structure(name))
            .map(|structure| structure.param_decls.as_slice())
            .filter(|decls| decls.len() == arguments.len())
        else {
            return false;
        };
        let own: Vec<&ParamDecl> = declarations
            .function(callee)
            .map(|declaration| {
                declaration
                    .param_decls
                    .iter()
                    .filter(|decl| *decl.id().owner == *callee && !removed.contains(decl.id()))
                    .collect()
            })
            .unwrap_or_default();
        if !own_recorded(&own) {
            return false;
        }
        for (decl, argument) in decls.iter().zip(arguments) {
            if !removed.contains(decl.id()) {
                self.visit(argument);
            }
        }
        self.visit(parts);
        self.member_callees.insert(callee.to_string());
        true
    }

    /// The reads of an instruction that may run a member of a struct it
    /// names: the instruction and every type of its registers and slots.
    fn dispatching_instruction(&mut self, function: &MirFunction, instruction: &MirInstr) {
        self.visit(instruction);
        let mut registers = Vec::new();
        instruction_operand_regs(instruction, &mut registers);
        instruction_result_regs(instruction, &mut registers);
        for register in registers {
            if let Some(ty) = function.reg_types.get(&register.0) {
                self.ty(ty);
            }
        }
        for slot in instruction_slots(instruction) {
            if let Some(ty) = function.var_tys.get(&slot) {
                self.ty(ty);
            }
        }
    }

    fn ty(&mut self, ty: &Ty) {
        let key = (ty.clone(), self.layout);
        if !self.types.contains_key(&key) {
            let mut walker = Walker::new(self.owners.clone(), self.nested);
            walker.bodies.clone_from(&self.bodies);
            walker.self_spelling.clone_from(&self.self_spelling);
            walker.structs = self.structs;
            walker.declarations = self.declarations;
            walker.layout = self.layout;
            walker.visit(ty);
            self.types.insert(
                key.clone(),
                (
                    walker.slots,
                    walker.nested_removals,
                    walker.reads_all,
                    walker.lifted,
                ),
            );
        }
        let (slots, nested_removals, reads_all, lifted) = &self.types[&key];
        self.slots.extend(slots);
        self.nested_removals.extend(nested_removals);
        self.reads_all |= reads_all;
        self.lifted.extend(lifted.iter().cloned());
    }

    /// Walk `value`. A leaf that encodes only in its compilation's context
    /// (a binding owner, a source location) names no binder, and its
    /// siblings are still walked; a value serde cannot encode at all (an
    /// unbound parameter hole) may name anything.
    fn visit<T: Serialize + ?Sized>(&mut self, value: &T) {
        let depth = self.tuples.len();
        let variants = self.variants.len();
        let sequences = self.sequences.len();
        let struct_variants = self.struct_variants.len();
        let (unread, nominal) = (self.unread, self.nominal);
        if let Err(Unwalkable(opaque)) = value.serialize(&mut *self) {
            self.reads_all |= opaque;
        }
        self.tuples.truncate(depth);
        self.variants.truncate(variants);
        self.sequences.truncate(sequences);
        self.struct_variants.truncate(struct_variants);
        (self.unread, self.nominal) = (unread, nominal);
    }

    fn string(&mut self, value: &str) {
        let layout = (self.layout && self.nominal == 0 && self.unread == 0)
            .then_some(self.structs)
            .flatten();
        if let Some(Some(frame)) = self.variants.last_mut()
            && frame.field == 0
        {
            frame.self_named = self
                .self_spelling
                .as_ref()
                .is_some_and(|(spelled, _)| *spelled == value);
            frame.reads = layout.map(|structs| structs.layout(value));
        }
        let owned = |name: &str| {
            self.bodies.iter().any(|owner| {
                name.strip_prefix(owner)
                    .is_some_and(|rest| rest.starts_with('$'))
            })
        };
        match value
            .strip_prefix("$comptime$")
            .or_else(|| value.strip_prefix("$default$"))
        {
            Some(lifted) => {
                if owned(lifted) {
                    self.lifted_reads(value);
                }
                self.lifted.insert(value.to_string());
            }
            None if owned(value) => match (self.nested)(value) {
                Some(removed) => self.nested_removals.push(removed),
                None => self.reads_all = true,
            },
            None => {}
        }
        self.leaf = self
            .owners
            .iter()
            .position(|owner| *owner == value)
            .map_or(Leaf::Other, Leaf::Owner);
    }

    /// The binders of an owner the lifted body `name` declares and its
    /// removals keep; every one when they are not at hand.
    fn lifted_reads(&mut self, name: &str) {
        let Some((removed, declaration)) = (self.nested)(name).zip(
            self.declarations
                .and_then(|declarations| declarations.function(name)),
        ) else {
            self.reads_all = true;
            return;
        };
        for id in declaration
            .param_decls
            .iter()
            .map(ParamDecl::id)
            .filter(|id| !removed.contains(id))
        {
            if let Some(owner) = self.owners.iter().position(|owner| **owner == *id.owner) {
                self.slots.insert((owner, id.slot));
            }
        }
    }

    fn element<T: Serialize + ?Sized>(&mut self, value: &T) {
        self.leaf = Leaf::Other;
        self.visit(value);
        if let Some((length, index, owner)) = self.tuples.last_mut() {
            if *length == 2 {
                match (*index, self.leaf) {
                    (0, Leaf::Owner(named)) => *owner = Some(named),
                    (1, Leaf::Slot(slot))
                        if let Some(named) = *owner
                            && self.unread == 0 =>
                    {
                        self.slots.insert((named, slot as usize));
                    }
                    _ => {}
                }
            }
            *index += 1;
        }
    }

    const fn compound(&mut self) -> &mut Self {
        self.leaf = Leaf::Other;
        self
    }
}

/// A value the walk could not encode: `true` when no context would encode
/// it, `false` for a leaf only its compilation's context encodes.
#[derive(Debug)]
struct Unwalkable(bool);

impl fmt::Display for Unwalkable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a value the parameter-use walk cannot encode")
    }
}

impl std::error::Error for Unwalkable {}

impl ser::Error for Unwalkable {
    fn custom<T: fmt::Display>(message: T) -> Self {
        // serde's own refusal of a skipped variant, as against a leaf's
        // refusal outside its context.
        Self(message.to_string().contains("cannot be serialized"))
    }
}

impl ser::Serializer for &mut Walker<'_, '_, '_> {
    type Ok = ();
    type Error = Unwalkable;
    type SerializeSeq = Self;
    type SerializeTuple = Self;
    type SerializeTupleStruct = Self;
    type SerializeTupleVariant = Self;
    type SerializeMap = Self;
    type SerializeStruct = Self;
    type SerializeStructVariant = Self;

    fn serialize_str(self, value: &str) -> Result<(), Unwalkable> {
        self.string(value);
        Ok(())
    }

    fn serialize_u64(self, value: u64) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Slot(value);
        Ok(())
    }

    fn serialize_bool(self, _: bool) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_i8(self, _: i8) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_i16(self, _: i16) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_i32(self, _: i32) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_i64(self, _: i64) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_i128(self, _: i128) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_u8(self, _: u8) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_u16(self, _: u16) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_u32(self, _: u32) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_u128(self, _: u128) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_f32(self, _: f32) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_f64(self, _: f64) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_char(self, _: char) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_bytes(self, _: &[u8]) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_none(self) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), Unwalkable> {
        self.visit(value);
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_unit(self) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_unit_struct(self, _: &'static str) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
    ) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Unwalkable> {
        self.visit(value);
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<(), Unwalkable> {
        // A value argument and a dependent type name their binders whole.
        let nominal = usize::from(matches!(variant, "Val" | "Dependent"));
        self.nominal += nominal;
        self.visit(value);
        self.nominal -= nominal;
        self.leaf = Leaf::Other;
        Ok(())
    }

    fn serialize_seq(self, length: Option<usize>) -> Result<Self, Unwalkable> {
        let arguments = self.struct_arguments.take();
        if std::mem::take(&mut self.self_arguments)
            && length == Some(0)
            && self.unread == 0
            && let Some((_, slots)) = &self.self_spelling
        {
            let reads = arguments.as_ref().unwrap_or(&LayoutReads::All);
            self.slots.extend(
                slots
                    .iter()
                    .enumerate()
                    .filter(|(position, _)| reads.reads(*position))
                    .map(|(_, slot)| *slot),
            );
        }
        self.sequences
            .push(arguments.map(|arguments| (arguments, 0)));
        Ok(self.compound())
    }

    fn serialize_tuple(self, length: usize) -> Result<Self, Unwalkable> {
        self.tuples.push((length, 0, None));
        Ok(self.compound())
    }

    fn serialize_tuple_struct(self, _: &'static str, _: usize) -> Result<Self, Unwalkable> {
        Ok(self.compound())
    }

    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<Self, Unwalkable> {
        self.variants
            .push((variant == "Struct").then_some(StructFrame {
                field: 0,
                self_named: false,
                reads: None,
            }));
        Ok(self.compound())
    }

    fn serialize_map(self, _: Option<usize>) -> Result<Self, Unwalkable> {
        Ok(self.compound())
    }

    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self, Unwalkable> {
        Ok(self.compound())
    }

    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<Self, Unwalkable> {
        // A projection off a type names its binders whole.
        let nominal = variant == "Assoc";
        self.nominal += usize::from(nominal);
        self.struct_variants.push(nominal);
        Ok(self.compound())
    }
}

impl ser::SerializeSeq for &mut Walker<'_, '_, '_> {
    type Ok = ();
    type Error = Unwalkable;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unwalkable> {
        let unread = match self.sequences.last_mut() {
            Some(Some((reads, position))) => {
                *position += 1;
                usize::from(!reads.reads(*position - 1))
            }
            _ => 0,
        };
        self.unread += unread;
        self.visit(value);
        self.unread -= unread;
        Ok(())
    }

    fn end(self) -> Result<(), Unwalkable> {
        self.sequences.pop();
        self.leaf = Leaf::Other;
        Ok(())
    }
}

impl ser::SerializeTuple for &mut Walker<'_, '_, '_> {
    type Ok = ();
    type Error = Unwalkable;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unwalkable> {
        self.element(value);
        Ok(())
    }

    fn end(self) -> Result<(), Unwalkable> {
        self.tuples.pop();
        self.leaf = Leaf::Other;
        Ok(())
    }
}

impl ser::SerializeTupleStruct for &mut Walker<'_, '_, '_> {
    type Ok = ();
    type Error = Unwalkable;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unwalkable> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }
}

impl ser::SerializeTupleVariant for &mut Walker<'_, '_, '_> {
    type Ok = ();
    type Error = Unwalkable;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unwalkable> {
        if let Some(Some(frame)) = self.variants.last_mut()
            && frame.field == 1
        {
            self.self_arguments = frame.self_named;
            self.struct_arguments = frame.reads.take();
        }
        self.visit(value);
        self.self_arguments = false;
        self.struct_arguments = None;
        if let Some(Some(frame)) = self.variants.last_mut() {
            frame.field += 1;
        }
        Ok(())
    }

    fn end(self) -> Result<(), Unwalkable> {
        self.variants.pop();
        self.leaf = Leaf::Other;
        Ok(())
    }
}

impl ser::SerializeMap for &mut Walker<'_, '_, '_> {
    type Ok = ();
    type Error = Unwalkable;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Unwalkable> {
        self.visit(key);
        Ok(())
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Unwalkable> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }
}

impl ser::SerializeStruct for &mut Walker<'_, '_, '_> {
    type Ok = ();
    type Error = Unwalkable;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Unwalkable> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Unwalkable> {
        self.leaf = Leaf::Other;
        Ok(())
    }
}

impl ser::SerializeStructVariant for &mut Walker<'_, '_, '_> {
    type Ok = ();
    type Error = Unwalkable;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _: &'static str,
        value: &T,
    ) -> Result<(), Unwalkable> {
        self.visit(value);
        Ok(())
    }

    fn end(self) -> Result<(), Unwalkable> {
        if self.struct_variants.pop() == Some(true) {
            self.nominal -= 1;
        }
        self.leaf = Leaf::Other;
        Ok(())
    }
}

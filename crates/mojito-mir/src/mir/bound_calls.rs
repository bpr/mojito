//! Calls through a `def(...)` bound: upstream reads the bound as a trait
//! whose one requirement is `__call__`, so `f(x)` on a bound value is the
//! requirement call `f.__call__(x)`, a [`MirInstr::MethodCall`] on a read
//! receiver. A generic `def[...](...)` bound's requirement is itself
//! generic: the call carries the bound's compile-time parameter
//! declarations and the arguments it applies, as a generic method's call
//! does. Where the bound closes, the call folds as a witness lookup does: a
//! struct's to the member its `def(...)` conformance records, a function's
//! inflated type to its callee's direct call, and a function value, which
//! has no witness table, to its own [`MirInstr::CallIndirect`].

use super::legacy_places::{LegacyCursor, LegacyOperandTables};
use super::{MirFunction, MirInstr};
use mojito_symbol::symbol::{
    callable_contract_target, callable_requirement_target, is_callable_contract_dispatch,
};
use mojito_types::origin::{Mutability, RefTy};
use mojito_types::types::{FuncTy, Ty, TyKind};
use std::collections::HashMap;
use std::hash::BuildHasher;

/// The signature a call of `target` on a receiver of type `receiver`
/// binds when it calls a `def(...)` bound's `__call__` requirement, over
/// a generic bound's own binders; `None` for any other call.
pub fn callable_bound_requirement<'t>(receiver: &'t Ty, target: &str) -> Option<&'t FuncTy> {
    if !is_callable_contract_dispatch(target) {
        return None;
    }
    match peel_refs(receiver).kind() {
        TyKind::Param {
            callable_bound: Some(bound),
            ..
        } => match bound.kind() {
            TyKind::Func(function) => Some(function),
            TyKind::GenericFunc(generic) => Some(&generic.func),
            _ => None,
        },
        _ => None,
    }
}

/// The bound's signature when `instruction` calls a `def(...)` bound's
/// `__call__` requirement on a receiver still a type parameter.
pub fn bound_call_contract<'t, S: BuildHasher>(
    instruction: &MirInstr,
    reg_types: &'t HashMap<u32, Ty, S>,
) -> Option<&'t FuncTy> {
    let MirInstr::MethodCall {
        recv,
        method,
        resolved: Some(target),
        ..
    } = instruction
    else {
        return None;
    };
    (method == "__call__")
        .then(|| callable_bound_requirement(reg_types.get(&recv.0)?, target))
        .flatten()
}

/// Rewrite a `__call__` requirement call through a `def(...)` bound whose
/// receiver substitution made a function value into that value's own
/// indirect call; returns the loads to splice ahead of the call.
///
/// A function value has no witness table, so upstream's call of the
/// requirement on it is the call of the value. The requirement call passed
/// the callee's address; a function value is register-passable, so the
/// rewritten call reads it through that reference (a `ref.read` among the
/// returned loads), as upstream's `LowerArgConventions` loads a
/// register-passable `read` operand. The rewritten call names
/// the receiver's own contract and keeps every argument, argument place,
/// and capture access; its callee is the operand alone, naming no place. A generic bound's call keeps the bound's compile-time parameter
/// declarations, which its parameter arguments name, so the bound's
/// binders and defaults, not the value's own, bind them.
pub fn indirect_from_bound_call(
    instruction: &mut MirInstr,
    function: &mut MirFunction,
) -> Vec<MirInstr> {
    let MirInstr::MethodCall {
        dest,
        recv,
        method,
        resolved: Some(target),
        raises,
        args,
        kwargs,
        capture_accesses,
        param_arg_regs,
        param_decls,
        instantiated_args,
        spread,
        ..
    } = instruction
    else {
        return Vec::new();
    };
    let Some(receiver) = function.reg_types.get(&recv.0).map(peel_refs).cloned() else {
        return Vec::new();
    };
    if method != "__call__"
        || !is_callable_contract_dispatch(target)
        || !matches!(receiver.kind(), TyKind::Func(_) | TyKind::GenericFunc(_))
    {
        return Vec::new();
    }
    let mut callee = *recv;
    let loads = super::read_addressed_operands(function, [&mut callee]);
    *instruction = MirInstr::CallIndirect {
        dest: *dest,
        callee,
        resolved: callable_contract_target(&receiver),
        raises: raises.take(),
        args: std::mem::take(args),
        kwargs: std::mem::take(kwargs),
        capture_accesses: std::mem::take(capture_accesses),
        param_arg_regs: std::mem::take(param_arg_regs),
        param_decls: std::mem::take(param_decls),
        // A generic value applied to solved arguments names the signature
        // they instantiate, as the call of the value by its own type does.
        instantiated_contract: (!instantiated_args.is_empty())
            .then(|| {
                super::verify::instantiate_generic_callable_contract(&receiver, instantiated_args)
                    .ok()
            })
            .flatten(),
        instantiated_args: std::mem::take(instantiated_args),
        spread: spread.take(),
    };
    loads
}

/// Rewrite a `__call__` requirement call on an inflated function type
/// into the direct call of its callee.
///
/// Upstream's `#__CALL__#` is the inflated type's `__call__` witness: the
/// receiver, which has no field, is dropped, and the callee's own
/// compile-time arguments lead the call's.
///
/// Returns the bound's compile-time parameter declarations, which the
/// rewritten call's `param_arg_regs` still name, for the caller to rename
/// to the callee's ([`super::witness_arguments`]); `None`, leaving
/// `instruction` as it was, for any other call.
pub fn direct_from_inflated_bound_call(
    instruction: &mut MirInstr,
    function: &MirFunction,
) -> Option<Vec<mojito_types::types::ParamDecl>> {
    let MirInstr::MethodCall {
        dest,
        recv,
        method,
        resolved: Some(target),
        raises,
        args,
        kwargs,
        conventions,
        capture_accesses,
        param_arg_regs,
        param_decls,
        instantiated_args,
        spread,
        ..
    } = instruction
    else {
        return None;
    };
    if method != "__call__" || !is_callable_contract_dispatch(target) {
        return None;
    }
    let callee = mojito_symbol::symbol::inflated_callee(function.reg_types.get(&recv.0)?)?;
    let bound = std::mem::take(param_decls);
    *instruction = MirInstr::Call {
        dest: *dest,
        func: super::FuncRef(callee.symbol.clone()),
        raises: raises.take(),
        args: std::mem::take(args),
        kwargs: std::mem::take(kwargs),
        conventions: std::mem::take(conventions),
        capture_accesses: std::mem::take(capture_accesses),
        param_arg_regs: std::mem::take(param_arg_regs),
        receiver: None,
        instantiated_args: callee
            .arguments
            .iter()
            .cloned()
            .chain(std::mem::take(instantiated_args))
            .collect(),
        spread: spread.take(),
    };
    Some(bound)
}

/// Upgrade the `def(...)` bound calls of `functions`, read from an
/// artifact written before such a call passed its callee's address.
///
/// Each requirement call whose receiver is still the callee's value and
/// whose receiver place, as the reader kept it in `legacy`, is the
/// callee's typed place passes an immutable `ref.make` of that place (the
/// reference a place holding one stores), as lowering forms it: the read
/// defining the receiver in the call's block becomes the `ref.make`, or
/// one is emitted before the call.
pub fn upgrade_bound_receivers(
    functions: &mut [(String, MirFunction)],
    legacy: &LegacyOperandTables,
) {
    for (index, (_, function)) in functions.iter_mut().enumerate() {
        let mut body = std::mem::take(&mut function.body);
        upgrade_bound_receivers_in(&mut body, function, &mut legacy.cursor(index));
        function.body = body;
    }
}

/// Whether `ty`, through any references, is a type parameter with a
/// `def(...)` bound.
pub(super) fn is_callable_bound(ty: &Ty) -> bool {
    matches!(
        peel_refs(ty).kind(),
        TyKind::Param {
            callable_bound: Some(_),
            ..
        }
    )
}

/// The requirement call a lowered indirect call through a `def(...)`
/// bound is, `instruction` itself for any other call: the bound's
/// `__call__` on its read receiver, `callee`, the callee's address
/// (`Flatten::address_bound_callee`), keeping a generic bound's compile-time parameters and the arguments the
/// call applies. `reference_result` is the call's checked reference result.
pub(super) fn bound_call_from_indirect(
    instruction: MirInstr,
    callee_ty: Option<&Ty>,
    reference_result: Option<RefTy>,
) -> MirInstr {
    let requirement = callee_ty
        .map(peel_refs)
        .filter(|ty| is_callable_bound(ty))
        .and_then(callable_requirement_target)
        .filter(|requirement| {
            matches!(
                &instruction,
                MirInstr::CallIndirect { resolved, .. }
                    if resolved.as_ref().is_none_or(|target| target == requirement)
            )
        });
    match instruction {
        MirInstr::CallIndirect {
            dest,
            callee,
            raises,
            args,
            kwargs,
            capture_accesses,
            param_arg_regs,
            param_decls,
            instantiated_args,
            spread,
            ..
        } if requirement.is_some() => MirInstr::MethodCall {
            conventions: crate::mir::MirArgConventions::default(),
            dest,
            recv: callee,
            method: "__call__".to_string(),
            resolved: requirement,
            raises,
            reference_result: reference_result.map(Box::new),
            result_adapter: None,
            args,
            kwargs,
            recv_writes: false,
            receiver_convention: None,
            capture_accesses,
            param_arg_regs,
            param_decls,
            instantiated_args,
            spread,
            refined_bound: None,
        },
        instruction => instruction,
    }
}

fn peel_refs(ty: &Ty) -> &Ty {
    match ty.kind() {
        TyKind::Ref(reference) => peel_refs(&reference.referent),
        _ => ty,
    }
}

fn upgrade_bound_receivers_in(
    block: &mut super::MirBlock,
    function: &mut MirFunction,
    cursor: &mut LegacyCursor<'_>,
) {
    let mut index = 0;
    while index < block.instrs.len() {
        for region in super::instruction_regions_mut(&mut block.instrs[index]) {
            upgrade_bound_receivers_in(region, function, cursor);
        }
        let place = cursor
            .visit(&block.instrs[index])
            .and_then(|places| places.receiver.as_ref());
        let receiver = bound_call_contract(&block.instrs[index], &function.reg_types)
            .is_some()
            .then_some(&block.instrs[index])
            .zip(place)
            .and_then(|(instruction, place)| match instruction {
                MirInstr::MethodCall { recv, .. }
                    if place.is_typed()
                        && !matches!(
                            function.reg_types.get(&recv.0).map(Ty::kind),
                            Some(TyKind::Ref(_))
                        ) =>
                {
                    Some((*recv, place.clone()))
                }
                _ => None,
            });
        let Some((value, place)) = receiver else {
            index += 1;
            continue;
        };
        let (place, handle) = super::call_addresses::stored_handle(&place, Mutability::Immutable)
            .unwrap_or_else(|| {
                let handle = super::mir_place_handle_ty(&place, Some(Mutability::Immutable))
                    .unwrap_or_else(Ty::error);
                (place, handle)
            });
        let defined = block.instrs[..index]
            .iter()
            .rposition(|instruction| match instruction {
                MirInstr::UseVar { dest, var, .. } => {
                    *dest == value && place.is_whole_root() && place.root == *var
                }
                MirInstr::LoadPlace { dest, place: read } => {
                    *dest == value && super::call_addresses::same_storage(read, &place)
                }
                _ => false,
            });
        let address = if let Some(at) = defined {
            function.reg_types.insert(value.0, handle);
            block.instrs[at] = MirInstr::MakeRef { dest: value, place };
            value
        } else {
            let span = function.spans.0.get(&value.0).map(|(span, _)| span.clone());
            let address = super::fresh_register(function, handle, span.as_ref());
            block.instrs.insert(
                index,
                MirInstr::MakeRef {
                    dest: address,
                    place,
                },
            );
            index += 1;
            address
        };
        if let MirInstr::MethodCall { recv, .. } = &mut block.instrs[index] {
            *recv = address;
        }
        index += 1;
    }
}

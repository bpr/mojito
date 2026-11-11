//! Calls through a `def(...)` bound: upstream reads the bound as a trait
//! whose one requirement is `__call__`, so `f(x)` on a bound value is the
//! requirement call `f.__call__(x)`, a [`MirInstr::MethodCall`] on a read
//! receiver. A generic `def[...](...)` bound's requirement is itself
//! generic: the call carries the bound's compile-time parameter
//! declarations and the arguments it applies, as a generic method's call
//! does. Where the bound closes, the call folds as a witness lookup does: a
//! struct's to the member its `def(...)` conformance records, and a
//! function value, which has no witness table, to its own
//! [`MirInstr::CallIndirect`].

use super::{MirInstr, generic_callable_param_decls};
use mojito_symbol::symbol::{
    callable_contract_target, callable_requirement_target, is_callable_contract_dispatch,
};
use mojito_types::origin::RefTy;
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
/// indirect call; returns whether it rewrote `instruction`.
///
/// A function value has no witness table, so upstream's call of the
/// requirement on it is the call of the value. The rewritten call names
/// the receiver's own contract and keeps every operand, place, and capture
/// access.
pub fn indirect_from_bound_call<S: BuildHasher>(
    instruction: &mut MirInstr,
    reg_types: &HashMap<u32, Ty, S>,
) -> bool {
    let MirInstr::MethodCall {
        dest,
        recv,
        method,
        resolved: Some(target),
        raises,
        args,
        kwargs,
        recv_place,
        arg_places,
        kwarg_places,
        capture_accesses,
        param_arg_regs,
        instantiated_args,
        spread,
        ..
    } = instruction
    else {
        return false;
    };
    let Some(receiver) = reg_types.get(&recv.0).map(peel_refs) else {
        return false;
    };
    if method != "__call__"
        || !is_callable_contract_dispatch(target)
        || !matches!(receiver.kind(), TyKind::Func(_) | TyKind::GenericFunc(_))
    {
        return false;
    }
    *instruction = MirInstr::CallIndirect {
        dest: *dest,
        callee: *recv,
        resolved: callable_contract_target(receiver),
        raises: raises.take(),
        args: std::mem::take(args),
        kwargs: std::mem::take(kwargs),
        callee_place: recv_place.take(),
        arg_places: std::mem::take(arg_places),
        kwarg_places: std::mem::take(kwarg_places),
        capture_accesses: std::mem::take(capture_accesses),
        param_arg_regs: std::mem::take(param_arg_regs),
        param_decls: generic_callable_param_decls(receiver),
        // A generic value applied to solved arguments names the signature
        // they instantiate, as the call of the value by its own type does.
        instantiated_contract: (!instantiated_args.is_empty())
            .then(|| {
                super::verify::instantiate_generic_callable_contract(receiver, instantiated_args)
                    .ok()
            })
            .flatten(),
        instantiated_args: std::mem::take(instantiated_args),
        spread: spread.take(),
    };
    true
}

/// The requirement call a lowered indirect call through a `def(...)`
/// bound is, `instruction` itself for any other call: the bound's
/// `__call__` on its read receiver, `callee`, whose place is the callee's,
/// keeping a generic bound's compile-time parameters and the arguments the
/// call applies. `reference_result` is the call's checked reference result.
pub(super) fn bound_call_from_indirect(
    instruction: MirInstr,
    callee_ty: Option<&Ty>,
    reference_result: Option<RefTy>,
) -> MirInstr {
    let requirement = callee_ty
        .map(peel_refs)
        .filter(|ty| {
            matches!(
                ty.kind(),
                TyKind::Param {
                    callable_bound: Some(_),
                    ..
                }
            )
        })
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
            callee_place,
            arg_places,
            kwarg_places,
            capture_accesses,
            param_arg_regs,
            param_decls,
            instantiated_args,
            spread,
            ..
        } if requirement.is_some() => MirInstr::MethodCall {
            dest,
            recv: callee,
            method: "__call__".to_string(),
            resolved: requirement,
            raises,
            reference_result: reference_result.map(Box::new),
            result_adapter: None,
            args,
            kwargs,
            recv_place: callee_place,
            recv_writes: false,
            receiver_convention: None,
            arg_places,
            kwarg_places,
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

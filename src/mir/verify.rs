//! Standalone semantic verification for typed MIR.
//!
//! The verifier consumes a lowered [`MirProgram`] plus its checked declaration
//! metadata — never source AST — and reports every violation as a message in
//! the program's `invariant_errors` style. Production compilation and the VM
//! reject programs with any finding; ownership dataflow remains owned by
//! `crate::analysis` and is composed with this verifier by the pipeline.
//!
//! Check classes:
//! - typed-place completeness and projection consistency;
//! - register bounds and register-type completeness;
//! - instruction and call type consistency (via the checker's coercion
//!   predicate; calls are compared against `MirFunctionDeclaration` facts when
//!   the callee is declared — builtin callees have no declaration and only
//!   participate in register checks);
//! - CFG edges: jump-target bounds per region, `FallOff`/`EscapeJump` only
//!   inside `try` sub-regions;
//! - effects: a raising site (a `Raise`, or a call carrying a checked error
//!   type) inside a nonraising function must be protected by a handler;
//! - reference invariants: `StoreRef` initializes reference storage, and a
//!   declared write-back parameter receives a caller place.

use super::{
    FuncRef, MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr,
    MirIntrinsicSubscript, MirPlace, MirProgram, MirTerm, Proj, Reg,
};
use crate::ct::CtValue;
use crate::origin::{Mutability, PointerOrigin};
use crate::types::{DependentType, ParamDecl, Ty, TyArg, tuple_elements};
use std::collections::{HashMap, HashSet};

pub fn verify(program: &MirProgram) -> Vec<String> {
    let mut errors = Vec::new();
    verify_runtime_pack_abi(&program.declarations, &mut errors);
    for (name, function) in &program.functions {
        verify_function(name, function, &program.declarations, &mut errors);
    }
    errors
}

/// The result registers an instruction defines (call/operation destinations
/// and loan/consumption markers).
pub(crate) fn instruction_result_regs(instruction: &MirInstr, out: &mut Vec<Reg>) {
    match instruction {
        MirInstr::MakeRef { dest, .. }
        | MirInstr::ReadRef { dest, .. }
        | MirInstr::CopyValue { dest, .. }
        | MirInstr::Const { dest, .. }
        | MirInstr::MaterializeLiteral { dest, .. }
        | MirInstr::UseVar { dest, .. }
        | MirInstr::MovePlace { dest, .. }
        | MirInstr::UnOp { dest, .. }
        | MirInstr::BinOp { dest, .. }
        | MirInstr::Call { dest, .. }
        | MirInstr::CallIndirect { dest, .. }
        | MirInstr::MethodCall { dest, .. }
        | MirInstr::PointerStorageTake { dest, .. }
        | MirInstr::PointerStorageDestroy { dest, .. }
        | MirInstr::GetField { dest, .. }
        | MirInstr::Index { dest, .. }
        | MirInstr::Slice { dest, .. }
        | MirInstr::MultiIndex { dest, .. }
        | MirInstr::LoadPlace { dest, .. }
        | MirInstr::MakeTuple { dest, .. }
        | MirInstr::MakeVariant { dest, .. }
        | MirInstr::MakeSimd { dest, .. }
        | MirInstr::SimdCast { dest, .. }
        | MirInstr::SimdShuffle { dest, .. }
        | MirInstr::MakeClosure { dest, .. }
        | MirInstr::VariantIs { dest, .. }
        | MirInstr::VariantGet { dest, .. }
        | MirInstr::VariantSet { dest, .. }
        | MirInstr::VariantTake { dest, .. }
        | MirInstr::VariantReplace { dest, .. }
        | MirInstr::HasNext { dest, .. }
        | MirInstr::Next { dest, .. } => out.push(*dest),
        MirInstr::TryNext { dest, yielded, .. } => {
            out.push(*dest);
            out.push(*yielded);
        }
        MirInstr::EstablishLoans { marker, .. }
        | MirInstr::InvalidateInteriors { marker, .. }
        | MirInstr::ConsumePlace { marker, .. } => out.push(*marker),
        _ => {}
    }
}

/// The registers an instruction reads (operands, arguments, stored values, and
/// place index registers). `Try` sub-regions are walked separately.
pub(crate) fn instruction_operand_regs(instruction: &MirInstr, out: &mut Vec<Reg>) {
    let place = |p: &MirPlace, out: &mut Vec<Reg>| {
        for projection in &p.proj {
            if let Proj::Index(register) = projection {
                out.push(*register);
            }
        }
    };
    match instruction {
        MirInstr::EstablishLoans { loans, .. } => {
            for loan in loans {
                place(&loan.place, out);
            }
        }
        MirInstr::ConsumePlace { place: p, .. }
        | MirInstr::MakeRef { place: p, .. }
        | MirInstr::MovePlace { place: p, .. }
        | MirInstr::LoadPlace { place: p, .. } => place(p, out),
        MirInstr::ReadRef { reference, .. } => out.push(*reference),
        MirInstr::CopyValue { value, .. } => out.push(*value),
        MirInstr::WriteRef { reference, value } => out.extend([*reference, *value]),
        MirInstr::MaterializeLiteral { value, .. } => out.push(*value),
        MirInstr::UnOp { a, .. } => out.push(*a),
        MirInstr::BinOp { a, b, .. } => out.extend([*a, *b]),
        MirInstr::Store { place: p, src } => {
            place(p, out);
            out.push(*src);
        }
        MirInstr::StoreRef {
            place: p,
            reference,
        } => {
            place(p, out);
            out.push(*reference);
        }
        MirInstr::MultiSet {
            receiver,
            receiver_place,
            args,
            arg_places,
            value,
            value_place,
            call,
            ..
        } => {
            out.push(*receiver);
            if let Some(receiver_place) = receiver_place {
                place(receiver_place, out);
            }
            for argument in args {
                subscript_arg_regs(argument, out);
            }
            for argument_place in arg_places.iter().flatten() {
                place(argument_place, out);
            }
            out.push(*value);
            if let Some(value_place) = value_place {
                place(value_place, out);
            }
            out.extend(
                call.param_arg_regs
                    .iter()
                    .filter_map(|argument| argument.value),
            );
        }
        MirInstr::Call {
            args,
            kwargs,
            arg_places,
            kwarg_places,
            param_arg_regs,
            ..
        } => {
            out.extend(args.iter().copied());
            out.extend(kwargs.iter().map(|(_, register)| *register));
            for p in arg_places.iter().flatten() {
                place(p, out);
            }
            for p in kwarg_places.iter().flatten() {
                place(p, out);
            }
            out.extend(param_arg_regs.iter().filter_map(|argument| argument.value));
        }
        MirInstr::CallIndirect {
            callee,
            args,
            kwargs,
            param_arg_regs,
            callee_place,
            arg_places,
            kwarg_places,
            ..
        } => {
            out.push(*callee);
            out.extend(args.iter().copied());
            out.extend(kwargs.iter().map(|(_, register)| *register));
            out.extend(param_arg_regs.iter().filter_map(|argument| argument.value));
            for p in callee_place
                .iter()
                .chain(arg_places.iter().flatten())
                .chain(kwarg_places.iter().flatten())
            {
                place(p, out);
            }
        }
        MirInstr::MethodCall {
            recv,
            args,
            kwargs,
            param_arg_regs,
            recv_place,
            arg_places,
            kwarg_places,
            ..
        } => {
            out.push(*recv);
            out.extend(args.iter().copied());
            out.extend(kwargs.iter().map(|(_, register)| *register));
            out.extend(param_arg_regs.iter().filter_map(|argument| argument.value));
            for p in recv_place
                .iter()
                .chain(arg_places.iter().flatten())
                .chain(kwarg_places.iter().flatten())
            {
                place(p, out);
            }
        }
        MirInstr::PointerStorageTake { pointer, index, .. }
        | MirInstr::PointerStorageDestroy { pointer, index, .. } => {
            out.extend([*pointer, *index]);
        }
        MirInstr::GetField { base, .. } => out.push(*base),
        MirInstr::Index {
            base,
            index,
            base_place,
            index_place,
            call,
            ..
        } => {
            out.extend([*base, *index]);
            if let Some(base_place) = base_place {
                place(base_place, out);
            }
            if let Some(index_place) = index_place {
                place(index_place, out);
            }
            if let Some(call) = call {
                out.extend(
                    call.param_arg_regs
                        .iter()
                        .filter_map(|argument| argument.value),
                );
            }
        }
        MirInstr::Slice {
            object,
            lower,
            upper,
            step,
            object_place,
            arg_places,
            call,
            ..
        } => {
            out.push(*object);
            out.extend([lower, upper, step].into_iter().flatten().copied());
            if let Some(object_place) = object_place {
                place(object_place, out);
            }
            for argument_place in arg_places.iter().flatten() {
                place(argument_place, out);
            }
            if let Some(call) = call {
                out.extend(
                    call.param_arg_regs
                        .iter()
                        .filter_map(|argument| argument.value),
                );
            }
        }
        MirInstr::MultiIndex {
            object,
            args,
            object_place,
            arg_places,
            call,
            ..
        } => {
            out.push(*object);
            for argument in args {
                subscript_arg_regs(argument, out);
            }
            if let Some(object_place) = object_place {
                place(object_place, out);
            }
            for argument_place in arg_places.iter().flatten() {
                place(argument_place, out);
            }
            if let Some(call) = call {
                out.extend(
                    call.param_arg_regs
                        .iter()
                        .filter_map(|argument| argument.value),
                );
            }
        }
        MirInstr::MakeTuple { elems, .. } | MirInstr::MakeSimd { elems, .. } => {
            out.extend(elems.iter().copied())
        }
        MirInstr::SimdCast { value, .. } | MirInstr::SimdShuffle { value, .. } => out.push(*value),
        MirInstr::MakeVariant { value, .. } => out.push(*value),
        MirInstr::MakeClosure { captures, .. } => {
            for capture in captures {
                place(&capture.place, out);
            }
        }
        MirInstr::VariantIs { variant, .. } | MirInstr::VariantGet { variant, .. } => {
            out.push(*variant)
        }
        MirInstr::VariantTake { variant, .. } => out.push(*variant),
        MirInstr::VariantSet {
            place: p, value, ..
        } => {
            place(p, out);
            out.push(*value);
        }
        MirInstr::VariantReplace {
            place: p, value, ..
        } => {
            place(p, out);
            out.push(*value);
        }
        MirInstr::Raise { src } => out.push(*src),
        MirInstr::Drop { reg } => out.push(*reg),
        MirInstr::DefVar { src, .. } => out.push(*src),
        MirInstr::InvalidateInteriors { .. }
        | MirInstr::Const { .. }
        | MirInstr::UseVar { .. }
        | MirInstr::KeepAlive { .. }
        | MirInstr::DropVar { .. }
        | MirInstr::ConsumeVar { .. }
        | MirInstr::GetIter { .. }
        | MirInstr::HasNext { .. }
        | MirInstr::Next { .. }
        | MirInstr::TryNext { .. }
        | MirInstr::Unsupported(_)
        | MirInstr::Try { .. } => {}
    }
}

/// Compatibility for verification purposes: either direction of the checker's
/// coercion predicate. Lowering emits checker-approved conversions before
/// values flow, so remaining differences are representational (literal
/// materialization, generic instantiation), not errors to re-litigate. A type
/// mentioning an unsubstituted parameter is not compared — instantiation is
/// the checker's domain and the verifier never re-derives it.
fn types_compatible(found: &Ty, expected: &Ty) -> bool {
    fn callable_environment(ty: &Ty) -> Option<&crate::origin::CallableEnvironment> {
        match ty {
            Ty::Func { environment, .. } | Ty::GenericFunc { environment, .. } => Some(environment),
            _ => None,
        }
    }
    // Some semantic-only sum types (notably unresolved overload sets) are not
    // ordinary value-coercion sources or destinations. Identity is still the
    // strongest possible compatibility proof and must precede those structural
    // special cases.
    if found == expected {
        return true;
    }
    if let (Some(found), Some(expected)) =
        (callable_environment(found), callable_environment(expected))
        && !crate::checker::callable_environment_coerces(found, expected)
    {
        // Environment differences are semantic, not a representational detail
        // that lowering may erase. In particular, an inference/default contract
        // is not a general MIR-level wildcard for a concrete capture set.
        return false;
    }
    if contains_type_param(found) || contains_type_param(expected) {
        return true;
    }
    // A bare `Struct(name, [])` is the established erased spelling for a
    // receiver or synthesized construction of any instantiation of `name`.
    if let (Ty::Struct(found_name, found_args), Ty::Struct(expected_name, expected_args)) =
        (found, expected)
        && found_name == expected_name
        && (found_args.is_empty() || expected_args.is_empty())
    {
        return true;
    }
    // A contextual selection narrows an overload set to one member.
    if let Ty::Overload(members) = found {
        return members
            .iter()
            .any(|member| types_compatible(member, expected));
    }
    // A struct may nominally conform to a `def(...)` callable trait; the
    // conformance is checker-verified and not yet recorded in MIR
    // declarations, so the verifier does not re-check it here.
    if matches!(found, Ty::Struct(..)) && matches!(expected, Ty::Func { .. }) {
        return true;
    }
    crate::checker::value_coerces(found, expected) || crate::checker::value_coerces(expected, found)
}

fn declared<'a>(
    declarations: &'a MirDeclarations,
    callee: &str,
) -> Option<&'a MirFunctionDeclaration> {
    declarations
        .functions
        .iter()
        .find(|declaration| declaration.lowered_name == callee)
}

fn iterator_result_matches_declaration(
    call: &crate::checked::CheckedIteratorCall,
    declaration: &MirFunctionDeclaration,
) -> bool {
    if call.result_adapter.is_some() {
        return false;
    }
    match (&call.reference_result, &call.result_ty) {
        (Some(reference), Ty::Ref(result_reference)) => {
            declaration.returns_reference
                && reference == result_reference
                && types_compatible(&reference.referent, &declaration.ret_ty)
        }
        (None, result) => {
            !declaration.returns_reference && types_compatible(result, &declaration.ret_ty)
        }
        _ => false,
    }
}

fn verify_iterator_result_adapter(
    prefix: &str,
    call: &crate::checked::CheckedIteratorCall,
    errors: &mut Vec<String>,
) -> bool {
    let abstract_dispatch = call.target == "__iterator_dispatch.__next__";
    match call.result_adapter {
        Some(crate::checked::CheckedResultAdapter::CopyIteratorReference) => {
            if !abstract_dispatch {
                errors.push(format!(
                    "{prefix}: iterator copy-reference adapter is attached to concrete target '{}'",
                    call.target
                ));
            }
            if call.reference_result.is_some() {
                errors.push(format!(
                    "{prefix}: adapted abstract iterator result also carries a concrete reference ABI"
                ));
            }
        }
        None if abstract_dispatch && call.reference_result.is_none() => errors.push(format!(
            "{prefix}: abstract value-returning iterator dispatch lacks its copy-reference adapter"
        )),
        None => {}
    }
    abstract_dispatch
}

fn contains_runtime_pack(ty: &Ty) -> bool {
    match ty {
        Ty::RuntimePack(_) => true,
        Ty::ComptimeList(inner) | Ty::Pointer { element: inner, .. } => {
            contains_runtime_pack(inner)
        }
        Ty::Tuple(elements) | Ty::Variant(elements) | Ty::Overload(elements) => {
            elements.iter().any(contains_runtime_pack)
        }
        Ty::Ref(reference) => contains_runtime_pack(&reference.referent),
        Ty::Struct(_, arguments) => arguments.iter().any(|argument| match argument {
            crate::types::TyArg::Ty(inner) => contains_runtime_pack(inner),
            crate::types::TyArg::Val(_) | crate::types::TyArg::Origin(_) => false,
        }),
        Ty::Assoc { base, .. } => contains_runtime_pack(base),
        Ty::Func {
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        }
        | Ty::GenericFunc {
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        } => {
            params.iter().any(contains_runtime_pack)
                || contains_runtime_pack(ret)
                || variadic.as_deref().is_some_and(contains_runtime_pack)
                || kw_variadic.as_deref().is_some_and(contains_runtime_pack)
                || error.as_deref().is_some_and(contains_runtime_pack)
        }
        _ => false,
    }
}

fn instantiate_checked_type(
    ty: &Ty,
    type_arguments: &HashMap<String, Ty>,
    value_arguments: &HashMap<String, CtValue>,
    bound_values: &HashSet<String>,
) -> Result<Ty, String> {
    Ok(match ty {
        Ty::Param {
            name,
            bounds,
            callable_bound,
        } => type_arguments
            .get(name)
            .or_else(|| type_arguments.get(name.trim_start_matches('*')))
            .cloned()
            .unwrap_or_else(|| Ty::Param {
                name: name.clone(),
                bounds: bounds.clone(),
                callable_bound: callable_bound.clone(),
            }),
        Ty::Dependent(DependentType::Indexed { elements, index }) => {
            let elements = elements
                .iter()
                .map(|element| {
                    instantiate_checked_type(element, type_arguments, value_arguments, bound_values)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let Some(value) = index.evaluate(value_arguments) else {
                let mut referenced = HashSet::new();
                index.referenced_parameters(&mut referenced);
                if !referenced.is_empty()
                    && referenced.iter().all(|name| bound_values.contains(name))
                {
                    return Ok(Ty::Dependent(DependentType::Indexed {
                        elements,
                        index: index.clone(),
                    }));
                }
                let mut unbound: Vec<_> = referenced.difference(bound_values).cloned().collect();
                unbound.sort();
                return Err(if unbound.is_empty() {
                    "dependent index did not evaluate to a compile-time value".to_string()
                } else {
                    format!(
                        "dependent index references unsubstituted parameter(s): {}",
                        unbound.join(", ")
                    )
                });
            };
            let index_value = match value {
                CtValue::Int(value) => Some(value),
                CtValue::UInt(value) => i64::try_from(value).ok(),
                CtValue::IntLiteral(value) => value.to_i64(),
                _ => None,
            }
            .ok_or_else(|| "dependent index is not an Int".to_string())?;
            let position = usize::try_from(index_value)
                .map_err(|_| format!("dependent index {index_value} is negative"))?;
            elements.get(position).cloned().ok_or_else(|| {
                format!(
                    "dependent index {index_value} is out of range for {} element(s)",
                    elements.len()
                )
            })?
        }
        Ty::Struct(name, arguments) => Ty::Struct(
            name.clone(),
            arguments
                .iter()
                .map(|argument| match argument {
                    TyArg::Ty(ty) => {
                        instantiate_checked_type(ty, type_arguments, value_arguments, bound_values)
                            .map(TyArg::Ty)
                    }
                    TyArg::Val(value) => Ok(TyArg::Val(value.clone())),
                    TyArg::Origin(origin) => Ok(TyArg::Origin(origin.clone())),
                })
                .collect::<Result<Vec<_>, String>>()?,
        ),
        Ty::ComptimeList(element) => Ty::ComptimeList(Box::new(instantiate_checked_type(
            element,
            type_arguments,
            value_arguments,
            bound_values,
        )?)),
        Ty::Tuple(elements) => Ty::Tuple(
            elements
                .iter()
                .map(|ty| {
                    instantiate_checked_type(ty, type_arguments, value_arguments, bound_values)
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Ty::RuntimePack(elements) => Ty::RuntimePack(
            elements
                .iter()
                .map(|ty| {
                    instantiate_checked_type(ty, type_arguments, value_arguments, bound_values)
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Ty::VariadicPack(element) => Ty::VariadicPack(Box::new(instantiate_checked_type(
            element,
            type_arguments,
            value_arguments,
            bound_values,
        )?)),
        Ty::Variant(elements) => Ty::Variant(
            elements
                .iter()
                .map(|ty| {
                    instantiate_checked_type(ty, type_arguments, value_arguments, bound_values)
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Ty::Pointer { element, origin } => Ty::Pointer {
            element: Box::new(instantiate_checked_type(
                element,
                type_arguments,
                value_arguments,
                bound_values,
            )?),
            origin: origin.clone(),
        },
        Ty::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = Box::new(instantiate_checked_type(
                &reference.referent,
                type_arguments,
                value_arguments,
                bound_values,
            )?);
            Ty::Ref(reference)
        }
        Ty::Assoc { base, name, args } => Ty::Assoc {
            base: Box::new(instantiate_checked_type(
                base,
                type_arguments,
                value_arguments,
                bound_values,
            )?),
            name: name.clone(),
            args: args
                .iter()
                .map(|argument| match argument {
                    TyArg::Ty(ty) => Ok(TyArg::Ty(instantiate_checked_type(
                        ty,
                        type_arguments,
                        value_arguments,
                        bound_values,
                    )?)),
                    TyArg::Val(value) => Ok(TyArg::Val(value.clone())),
                    TyArg::Origin(origin) => Ok(TyArg::Origin(origin.clone())),
                })
                .collect::<Result<Vec<_>, String>>()?,
        },
        Ty::Overload(candidates) => Ty::Overload(
            candidates
                .iter()
                .map(|ty| {
                    instantiate_checked_type(ty, type_arguments, value_arguments, bound_values)
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Ty::Func {
            environment,
            params,
            names,
            ret,
            required,
            variadic,
            kw_variadic,
            positional_only,
            keyword_only,
            raises,
            error,
            conventions,
            ref_params,
            ref_return,
            transfers,
        } => Ty::Func {
            environment: environment.clone(),
            params: params
                .iter()
                .map(|ty| {
                    instantiate_checked_type(ty, type_arguments, value_arguments, bound_values)
                })
                .collect::<Result<Vec<_>, _>>()?,
            names: names.clone(),
            ret: Box::new(instantiate_checked_type(
                ret,
                type_arguments,
                value_arguments,
                bound_values,
            )?),
            required: required.clone(),
            variadic: variadic
                .as_ref()
                .map(|ty| {
                    instantiate_checked_type(ty, type_arguments, value_arguments, bound_values)
                        .map(Box::new)
                })
                .transpose()?,
            kw_variadic: kw_variadic
                .as_ref()
                .map(|ty| {
                    instantiate_checked_type(ty, type_arguments, value_arguments, bound_values)
                        .map(Box::new)
                })
                .transpose()?,
            positional_only: *positional_only,
            keyword_only: *keyword_only,
            raises: *raises,
            error: error
                .as_ref()
                .map(|ty| {
                    instantiate_checked_type(ty, type_arguments, value_arguments, bound_values)
                        .map(Box::new)
                })
                .transpose()?,
            conventions: conventions.clone(),
            ref_params: ref_params.clone(),
            ref_return: ref_return.clone(),
            transfers: transfers.clone(),
        },
        // Nested generic callable contracts own their own binder scope. The
        // outer verifier validates that scope recursively; retaining it is
        // sound and avoids capturing same-spelled outer substitution names.
        Ty::GenericFunc { .. } => ty.clone(),
        _ => ty.clone(),
    })
}

fn contains_type_param(ty: &Ty) -> bool {
    match ty {
        Ty::Param { .. } | Ty::Assoc { .. } | Ty::Dependent(_) => true,
        Ty::ComptimeList(inner) | Ty::Pointer { element: inner, .. } => contains_type_param(inner),
        Ty::Tuple(elements) | Ty::RuntimePack(elements) | Ty::Variant(elements) => {
            elements.iter().any(contains_type_param)
        }
        Ty::Ref(reference) => contains_type_param(&reference.referent),
        Ty::Struct(_, arguments) => arguments.iter().any(|argument| match argument {
            crate::types::TyArg::Ty(inner) => contains_type_param(inner),
            crate::types::TyArg::Val(_) | crate::types::TyArg::Origin(_) => false,
        }),
        Ty::Func {
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        }
        | Ty::GenericFunc {
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        } => {
            params.iter().any(contains_type_param)
                || contains_type_param(ret)
                || variadic.as_deref().is_some_and(contains_type_param)
                || kw_variadic.as_deref().is_some_and(contains_type_param)
                || error.as_deref().is_some_and(contains_type_param)
        }
        _ => false,
    }
}

#[derive(Clone, Copy)]
enum ReferencePermission {
    Immutable,
    Mutable,
    Param(crate::origin::OriginParamId),
}

impl ReferencePermission {
    fn from_mutability(mutability: Mutability) -> Self {
        match mutability {
            Mutability::Immutable => Self::Immutable,
            Mutability::Mutable => Self::Mutable,
            Mutability::Param(parameter) => Self::Param(parameter),
        }
    }

    fn allows_write(self) -> bool {
        !matches!(self, Self::Immutable)
    }

    /// Whether a capability with this permission can initialize or be viewed
    /// as one requiring `target`. A symbolic permission has already been
    /// constrained by the checker; retaining it here avoids guessing that an
    /// executable generic body is either mutable or immutable.
    fn satisfies(self, target: Self) -> bool {
        match target {
            Self::Immutable => true,
            Self::Mutable => self.allows_write(),
            Self::Param(target) => match self {
                Self::Mutable => true,
                Self::Param(found) => found == target,
                Self::Immutable => false,
            },
        }
    }
}

/// Where a run of blocks sits: the function's top level, or one `try`
/// sub-region (with its handler-protection status).
struct RegionContext {
    /// Number of blocks in this region — the bound for region-local jumps.
    region_len: usize,
    /// Number of blocks in the enclosing function — the bound for
    /// `EscapeJump` targets.
    function_len: usize,
    /// Whether this run of blocks is a `try` sub-region (where `FallOff` and
    /// `EscapeJump` are legal terminators).
    in_try_region: bool,
    /// Whether a raise from this position reaches an `except` handler before
    /// leaving the function.
    protected: bool,
}

struct SubscriptSources<'a> {
    receiver_ty: Option<&'a Ty>,
    method: &'static str,
    receiver_place: Option<&'a MirPlace>,
    positional_places: &'a [Option<MirPlace>],
    keyword_places: &'a [Option<MirPlace>],
    positional_types: &'a [Option<Ty>],
    keyword_types: &'a [Option<Ty>],
    dest: Option<Reg>,
}

/// Runtime frame/slot capabilities are represented either by a source-level
/// `ref T` or by an origin-bearing `UnsafePointer[T, origin]`. Raw/static/
/// untracked pointer values use allocation arithmetic and are not valid
/// operands for `ReadRef`/`WriteRef`.
fn reference_capability(ty: &Ty) -> Option<ReferenceCapability<'_>> {
    match ty {
        Ty::Ref(reference) => Some(ReferenceCapability {
            target: &reference.referent,
            permission: ReferencePermission::from_mutability(reference.mutability),
        }),
        Ty::Pointer { element, origin } => {
            let permission = match origin {
                PointerOrigin::Place { mutable, .. } => {
                    if *mutable {
                        ReferencePermission::Mutable
                    } else {
                        ReferencePermission::Immutable
                    }
                }
                PointerOrigin::Param { mutability, .. } => {
                    ReferencePermission::from_mutability(*mutability)
                }
                PointerOrigin::Legacy
                | PointerOrigin::Static
                | PointerOrigin::Untracked { .. }
                | PointerOrigin::UnsafeAny { .. } => return None,
            };
            Some(ReferenceCapability {
                target: element,
                permission,
            })
        }
        _ => None,
    }
}

fn verify_subscript_receiver_place(
    prefix: &str,
    receiver_ty: Option<&Ty>,
    receiver_place: Option<&MirPlace>,
    errors: &mut Vec<String>,
) {
    let (Some(found), Some(storage)) = (
        receiver_ty,
        receiver_place.and_then(|place| place.ty.as_ref()),
    ) else {
        return;
    };
    let receiver = match storage {
        Ty::Ref(reference) => reference.referent.as_ref(),
        other => other,
    };
    // The loaded base register may legitimately stay reference-typed one
    // level above its place (the VM's LoadPlace second dereference resolves
    // it at runtime); peel exactly one level, symmetric with the storage
    // peel above, so genuine mismatches still fail.
    let found = match found {
        Ty::Ref(reference) => reference.referent.as_ref(),
        other => other,
    };
    if !types_compatible(found, receiver) {
        errors.push(format!(
            "{prefix}: subscript receiver place type {receiver} does not match base value type {found}"
        ));
    }
}

fn verify_subscript_call(
    prefix: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
    call: &crate::mir::MirSubscriptCall,
    sources: SubscriptSources<'_>,
    errors: &mut Vec<String>,
) {
    let SubscriptSources {
        receiver_ty,
        method,
        receiver_place,
        positional_places,
        keyword_places,
        positional_types,
        keyword_types,
        dest,
    } = sources;
    verify_capture_accesses(prefix, function, &call.capture_accesses, errors);
    let abstract_trait_dispatch = call.target.starts_with("__trait_dispatch.");
    let target_family = call.target.rsplit_once('.').is_some_and(|(_, symbol)| {
        symbol == method
            || symbol
                .strip_prefix(method)
                .is_some_and(|suffix| suffix.starts_with('$'))
            || method == "__getitem__"
                && (symbol == "__getitem_param__"
                    || symbol.starts_with("__getitem_param__$")
                    || symbol.starts_with("__getitem_param_value__$"))
    });
    if !target_family {
        errors.push(format!(
            "{prefix}: selected subscript target '{}' is not in the {method} method family",
            call.target
        ));
    }
    let concrete_receiver = match receiver_ty {
        Some(Ty::Struct(name, _)) => Some(name.as_str()),
        Some(Ty::Ref(reference)) => match reference.referent.as_ref() {
            Ty::Struct(name, _) => Some(name.as_str()),
            _ => None,
        },
        _ => None,
    };
    if !abstract_trait_dispatch
        && let Some(receiver) = concrete_receiver
        && call
            .target
            .rsplit_once('.')
            .is_some_and(|(owner, _)| owner != receiver)
    {
        errors.push(format!(
            "{prefix}: selected subscript target '{}' does not belong to receiver type {receiver}",
            call.target
        ));
    }
    if call.receiver_requires_place && receiver_place.is_none() {
        errors.push(format!(
            "{prefix}: selected subscript reference receiver has no retained caller place"
        ));
    }
    if positional_places.len() != positional_types.len()
        || keyword_places.len() != keyword_types.len()
    {
        errors.push(format!(
            "{prefix}: subscript argument place/type metadata is not aligned"
        ));
    }
    let mut positional_uses = vec![0usize; positional_places.len()];
    let mut keyword_uses = vec![0usize; keyword_places.len()];
    for argument in &call.arguments {
        let (retained, actual_ty) = match argument.source {
            crate::checked::CheckedCallArgumentSource::Positional(index) => {
                if let Some(uses) = positional_uses.get_mut(index) {
                    *uses += 1;
                } else {
                    errors.push(format!(
                        "{prefix}: selected subscript positional source {index} is out of range"
                    ));
                }
                (
                    positional_places.get(index).and_then(Option::as_ref),
                    positional_types.get(index).and_then(Option::as_ref),
                )
            }
            crate::checked::CheckedCallArgumentSource::Keyword(index) => {
                if let Some(uses) = keyword_uses.get_mut(index) {
                    *uses += 1;
                } else {
                    errors.push(format!(
                        "{prefix}: selected subscript keyword source {index} is out of range"
                    ));
                }
                (
                    keyword_places.get(index).and_then(Option::as_ref),
                    keyword_types.get(index).and_then(Option::as_ref),
                )
            }
            crate::checked::CheckedCallArgumentSource::Default => (None, None),
        };
        if argument.requires_place && retained.is_none() {
            errors.push(format!(
                "{prefix}: selected subscript mut/ref argument has no retained caller place"
            ));
        }
        if let Some(actual_ty) = actual_ty
            && !types_compatible(actual_ty, &argument.parameter_ty)
        {
            errors.push(format!(
                "{prefix}: subscript source has type {actual_ty}, selected parameter expects {}",
                argument.parameter_ty
            ));
        }
        if matches!(
            argument.convention,
            Some(crate::ast::ArgConvention::Mut | crate::ast::ArgConvention::Ref)
        ) && !argument.requires_place
        {
            errors.push(format!(
                "{prefix}: selected subscript mut/ref convention does not require a caller place"
            ));
        }
    }
    for (index, uses) in positional_uses.iter().enumerate() {
        if *uses != 1 {
            errors.push(format!(
                "{prefix}: subscript positional source {index} is represented {uses} times"
            ));
        }
    }
    for (index, uses) in keyword_uses.iter().enumerate() {
        if *uses != 1 {
            errors.push(format!(
                "{prefix}: subscript keyword source {index} is represented {uses} times"
            ));
        }
    }
    let retained_reference_ty = call
        .reference_result
        .as_ref()
        .map(|reference| Ty::Ref(reference.clone()));
    match &retained_reference_ty {
        Some(expected) if &call.result_ty != expected => errors.push(format!(
            "{prefix}: subscript reference-result metadata {expected} does not match selected result type {}",
            call.result_ty
        )),
        None if matches!(call.result_ty, Ty::Ref(_)) => errors.push(format!(
            "{prefix}: subscript selected result type {} lacks reference-result metadata",
            call.result_ty
        )),
        _ => {}
    }
    if let Some(dest) = dest
        && let Some(found) = function.reg_types.get(&dest.0)
    {
        let exact_reference_mismatch = matches!(
            (found, &call.result_ty),
            (Ty::Ref(found), Ty::Ref(expected)) if found != expected
        );
        if exact_reference_mismatch || !types_compatible(found, &call.result_ty) {
            errors.push(format!(
                "{prefix}: subscript result has type {found}, selected contract returns {}",
                call.result_ty
            ));
        }
    }
    verify_param_arguments(
        prefix,
        function,
        &call.param_decls,
        &call.param_arg_regs,
        errors,
    );
    match declared(declarations, &call.target) {
        // Trait-bound dispatch is intentionally abstract in checked MIR. Its
        // complete selected argument/result contract is still verified here;
        // the VM retargets the symbol to the concrete receiver declaration.
        None if abstract_trait_dispatch => {}
        None => errors.push(format!(
            "{prefix}: subscript refers to undeclared method '{}'",
            call.target
        )),
        Some(declaration) => {
            if !declaration.has_receiver {
                errors.push(format!(
                    "{prefix}: selected subscript target '{}' has no declared receiver",
                    call.target
                ));
            }
            if !effective_call_convention_matches(
                declaration.receiver_convention,
                call.receiver_convention,
            ) {
                errors.push(format!(
                    "{prefix}: subscript receiver convention {:?} does not match '{}' declaration {:?}",
                    call.receiver_convention, call.target, declaration.receiver_convention
                ));
            }
            let declared_receiver_place = matches!(
                declaration.receiver_convention,
                Some(crate::ast::ArgConvention::Mut | crate::ast::ArgConvention::Ref)
            );
            if call.receiver_requires_place != declared_receiver_place {
                errors.push(format!(
                    "{prefix}: subscript receiver place requirement does not match '{}' declaration",
                    call.target
                ));
            }
            if declaration.param_decls != call.param_decls {
                errors.push(format!(
                    "{prefix}: subscript generic declaration metadata does not match '{}'",
                    call.target
                ));
            }
            if declaration.raises != call.raises.is_some() {
                errors.push(format!(
                    "{prefix}: subscript raising metadata does not match '{}'",
                    call.target
                ));
            } else if let (Some(found), Some(expected)) =
                (&call.raises, declaration.error_ty.as_ref())
                && !types_compatible(found, expected)
            {
                errors.push(format!(
                    "{prefix}: subscript error type {found} does not match '{}' contract {expected}",
                    call.target
                ));
            }
            if declaration.returns_reference != call.reference_result.is_some() {
                errors.push(format!(
                    "{prefix}: subscript reference-result ABI does not match '{}' declaration",
                    call.target
                ));
            }
            if let Some(reference) = &call.reference_result
                && !types_compatible(reference.referent.as_ref(), &declaration.ret_ty)
            {
                errors.push(format!(
                    "{prefix}: subscript reference-result referent {} does not match '{}' declaration {}",
                    reference.referent, call.target, declaration.ret_ty
                ));
            }
            if call.reference_result.is_none()
                && !types_compatible(&call.result_ty, &declaration.ret_ty)
            {
                errors.push(format!(
                    "{prefix}: subscript selected result type {} does not match '{}' declaration {}",
                    call.result_ty, call.target, declaration.ret_ty
                ));
            }
            if call.arguments.len() < declaration.param_types.len() {
                errors.push(format!(
                    "{prefix}: selected subscript has {} argument contract(s), but '{}' declares {} fixed parameter(s)",
                    call.arguments.len(),
                    call.target,
                    declaration.param_types.len()
                ));
            }
            for (index, expected) in declaration.param_types.iter().enumerate() {
                let Some(argument) = call.arguments.get(index) else {
                    continue;
                };
                let declared_convention =
                    declaration.param_conventions.get(index).copied().flatten();
                if !effective_call_convention_matches(declared_convention, argument.convention) {
                    errors.push(format!(
                        "{prefix}: selected subscript parameter {index} convention {:?} does not match '{}' declaration {:?}",
                        argument.convention, call.target, declared_convention
                    ));
                }
                if !types_compatible(&argument.parameter_ty, expected) {
                    errors.push(format!(
                        "{prefix}: selected subscript parameter type {} does not match '{}' declaration {expected}",
                        argument.parameter_ty, call.target
                    ));
                }
                let requires_place = declaration.ref_params.get(index).copied().unwrap_or(false);
                if argument.requires_place != requires_place {
                    errors.push(format!(
                        "{prefix}: selected subscript parameter {index} place requirement does not match '{}' declaration",
                        call.target
                    ));
                }
                if matches!(
                    argument.source,
                    crate::checked::CheckedCallArgumentSource::Default
                ) && declaration.required.get(index).copied().unwrap_or(true)
                {
                    errors.push(format!(
                        "{prefix}: required subscript parameter {index} of '{}' is bound to a default",
                        call.target
                    ));
                }
            }
            let overflow = call
                .arguments
                .get(declaration.param_types.len()..)
                .unwrap_or_default();
            let positional_overflow = overflow
                .iter()
                .filter(|argument| {
                    matches!(
                        argument.source,
                        crate::checked::CheckedCallArgumentSource::Positional(_)
                    )
                })
                .collect::<Vec<_>>();
            let expected_positional = match declaration.variadic.as_ref() {
                Some(Ty::RuntimePack(elements)) => {
                    if elements.len() != positional_overflow.len() {
                        errors.push(format!(
                            "{prefix}: selected subscript has {} positional overflow argument(s), but '{}' requires RuntimePack arity {}",
                            positional_overflow.len(),
                            call.target,
                            elements.len()
                        ));
                    }
                    Some(elements.as_slice())
                }
                _ => None,
            };
            let mut positional_index = 0;
            for argument in overflow {
                let expected = match argument.source {
                    crate::checked::CheckedCallArgumentSource::Positional(_) => {
                        let expected = match (&declaration.variadic, expected_positional) {
                            (Some(Ty::RuntimePack(_)), Some(elements)) => {
                                elements.get(positional_index)
                            }
                            (Some(element), _) => Some(element),
                            (None, _) => None,
                        };
                        positional_index += 1;
                        expected
                    }
                    crate::checked::CheckedCallArgumentSource::Keyword(_) => {
                        declaration.kw_variadic.as_ref()
                    }
                    crate::checked::CheckedCallArgumentSource::Default => {
                        errors.push(format!(
                            "{prefix}: selected subscript overflow argument cannot use a default"
                        ));
                        None
                    }
                };
                let Some(expected) = expected else {
                    if !matches!(
                        argument.source,
                        crate::checked::CheckedCallArgumentSource::Default
                    ) {
                        errors.push(format!(
                            "{prefix}: selected subscript overflow argument has no matching variadic collector in '{}'",
                            call.target
                        ));
                    }
                    continue;
                };
                if !types_compatible(&argument.parameter_ty, expected) {
                    errors.push(format!(
                        "{prefix}: selected subscript overflow type {} does not match '{}' collector {expected}",
                        argument.parameter_ty, call.target
                    ));
                }
            }
        }
    }
}

/// Check the source-order MIR representation against a declaration-order
/// generic ABI. Keyword names select their declaration directly; positional
/// entries skip infer-only declarations. A type argument occupies a slot but
/// has no register, while every supplied value argument must carry a register
/// compatible with its declared checked type.
fn verify_param_arguments(
    prefix: &str,
    function: &MirFunction,
    declarations: &[crate::types::ParamDecl],
    arguments: &[crate::mir::MirParamArg],
    errors: &mut Vec<String>,
) {
    if declarations.is_empty() {
        // A specialized nominal constructor/method may retain erased source
        // type applications even though its selected MIR declaration is
        // monomorphic. They deliberately carry no register. A value argument,
        // however, would imply a runtime compile-time slot absent from the ABI.
        if arguments.iter().any(|argument| argument.value.is_some()) {
            errors.push(format!(
                "{prefix}: nongeneric call carries a compile-time value argument"
            ));
        }
        return;
    }
    let mut occupied = vec![false; declarations.len()];
    let mut next_positional = 0;
    for argument in arguments {
        let index = if let Some(name) = &argument.name {
            declarations
                .iter()
                .position(|declaration| declaration.name().trim_start_matches('*') == name.as_str())
        } else {
            while declarations
                .get(next_positional)
                .is_some_and(|declaration| {
                    occupied[next_positional]
                        || match declaration {
                            crate::types::ParamDecl::Type { infer_only, .. }
                            | crate::types::ParamDecl::Value { infer_only, .. } => *infer_only,
                        }
                })
            {
                next_positional += 1;
            }
            let index = (next_positional < declarations.len()).then_some(next_positional);
            next_positional += usize::from(index.is_some());
            index
        };
        let Some(index) = index else {
            errors.push(format!(
                "{prefix}: compile-time argument does not match a parameter declaration"
            ));
            continue;
        };
        if occupied[index] {
            errors.push(format!(
                "{prefix}: compile-time parameter '{}' is supplied more than once",
                declarations[index].name()
            ));
            continue;
        }
        occupied[index] = true;
        match (&declarations[index], argument.value) {
            (crate::types::ParamDecl::Type { name, .. }, Some(_)) => errors.push(format!(
                "{prefix}: type parameter '{name}' unexpectedly carries a runtime register"
            )),
            (crate::types::ParamDecl::Value { name, .. }, None) => errors.push(format!(
                "{prefix}: value parameter '{name}' has no runtime register"
            )),
            (crate::types::ParamDecl::Value { name, ty, .. }, Some(register)) => {
                if let Some(found) = function.reg_types.get(&register.0)
                    && !(matches!(ty.as_ref(), Ty::Func { .. } | Ty::GenericFunc { .. })
                        && crate::checker::callable_bound_accepts(found, ty))
                    && !types_compatible(found, ty)
                {
                    errors.push(format!(
                        "{prefix}: compile-time value parameter '{name}' has register type {found}, declared {ty}"
                    ));
                }
            }
            (crate::types::ParamDecl::Type { .. }, None) => {}
        }
    }
    for (index, declaration) in declarations.iter().enumerate() {
        if occupied[index] {
            continue;
        }
        if let crate::types::ParamDecl::Value {
            name,
            default,
            callable_default,
            infer_only,
            variadic,
            ..
        } = declaration
            && default.is_none()
            && callable_default.is_none()
            && !infer_only
            && !variadic
        {
            errors.push(format!(
                "{prefix}: required compile-time value parameter '{name}' is missing"
            ));
        }
    }
}

fn verify_capture_accesses(
    prefix: &str,
    function: &MirFunction,
    accesses: &[crate::mir::MirCaptureAccess],
    errors: &mut Vec<String>,
) {
    for access in accesses {
        if access.root as usize >= function.var_names.len() {
            errors.push(format!(
                "{prefix}: callable capture access uses unknown owner slot {}",
                access.root
            ));
        }
    }
}

#[derive(Clone, Copy)]
struct ReferenceCapability<'a> {
    target: &'a Ty,
    permission: ReferencePermission,
}

fn verify_blocks(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
    blocks: &[MirBlock],
    context: &RegionContext,
    errors: &mut Vec<String>,
) {
    for (block_index, block) in blocks.iter().enumerate() {
        for instruction in &block.instrs {
            verify_instruction(
                name,
                function,
                declarations,
                block_index,
                instruction,
                context,
                errors,
            );
        }
        verify_terminator(name, function, block_index, &block.term, context, errors);
    }
}

/// Arity, argument-type, and write-back checks against a declaration. Only the
/// plain positional shape is compared — defaulted, keyword, and variadic calls
/// are bound by the runtime matcher, whose slotting the verifier does not
/// replicate.
fn verify_direct_call(
    prefix: &str,
    function: &MirFunction,
    declaration: &MirFunctionDeclaration,
    args: &[Reg],
    kwargs: &[(String, Reg)],
    arg_places: &[Option<MirPlace>],
    errors: &mut Vec<String>,
) {
    let plain = kwargs.is_empty()
        && declaration.variadic.is_none()
        && declaration.kw_variadic.is_none()
        && args.len() == declaration.param_types.len();
    if !plain {
        return;
    }
    for (index, (argument, expected)) in args.iter().zip(&declaration.param_types).enumerate() {
        if let Some(found) = function.reg_types.get(&argument.0)
            && !types_compatible(found, expected)
        {
            errors.push(format!(
                "{prefix}: argument {index} of '{}' has type {found}, declared {expected}",
                declaration.lowered_name
            ));
        }
        if declaration.ref_params.get(index).copied().unwrap_or(false)
            && arg_places
                .get(index)
                .map(Option::as_ref)
                .unwrap_or(None)
                .is_none()
        {
            errors.push(format!(
                "{prefix}: write-back parameter {index} of '{}' has no caller place",
                declaration.lowered_name
            ));
        }
    }
}

fn subscript_arg_regs(argument: &super::MirSubscriptArg, out: &mut Vec<Reg>) {
    match argument {
        super::MirSubscriptArg::Index(register) => out.push(*register),
        super::MirSubscriptArg::Slice {
            lower, upper, step, ..
        } => out.extend([lower, upper, step].into_iter().flatten().copied()),
    }
}

fn simd_element_type(dtype: crate::ast::Dtype) -> Ty {
    match dtype {
        crate::ast::Dtype::Int => Ty::Int,
        crate::ast::Dtype::Float64 => Ty::Float64,
        dtype => Ty::Simd { dtype, width: 1 },
    }
}

fn slice_descriptor_ty(kind: crate::types::SliceKind) -> Ty {
    Ty::Struct(kind.type_name().to_string(), Vec::new())
}

fn subscript_argument_ty(
    function: &MirFunction,
    argument: &crate::mir::MirSubscriptArg,
) -> Option<Ty> {
    match argument {
        crate::mir::MirSubscriptArg::Index(register) => {
            function.reg_types.get(&register.0).cloned()
        }
        crate::mir::MirSubscriptArg::Slice { kind, .. } => Some(slice_descriptor_ty(*kind)),
    }
}

fn effective_call_convention_matches(
    declared: Option<crate::ast::ArgConvention>,
    effective: Option<crate::ast::ArgConvention>,
) -> bool {
    declared == effective
        || matches!(
            (declared, effective),
            (
                Some(crate::ast::ArgConvention::Ref),
                Some(crate::ast::ArgConvention::Read)
            )
        )
}

fn generic_callable_decls(ty: &Ty) -> Option<&[crate::types::ParamDecl]> {
    match ty {
        Ty::GenericFunc { decls, .. } => Some(decls),
        Ty::Param {
            callable_bound: Some(bound),
            ..
        } => generic_callable_decls(bound),
        _ => None,
    }
}

struct GenericArgumentMaps {
    types: HashMap<String, Ty>,
    values: HashMap<String, CtValue>,
}

/// Ensure each symbolic dependent index is owned by an explicit enclosing
/// value-parameter binder. This permits generic MIR to remain symbolic while
/// rejecting a misspelled or escaped index name before wildcard compatibility
/// could hide it.
fn validate_dependent_bindings(ty: &Ty) -> Result<(), String> {
    fn walk(ty: &Ty, bound: &HashSet<String>) -> Result<(), String> {
        match ty {
            Ty::Dependent(DependentType::Indexed { elements, index }) => {
                let mut referenced = HashSet::new();
                index.referenced_parameters(&mut referenced);
                let mut unbound: Vec<_> = referenced.difference(bound).cloned().collect();
                unbound.sort();
                if !unbound.is_empty() {
                    return Err(format!(
                        "dependent index references unbound parameter(s): {}",
                        unbound.join(", ")
                    ));
                }
                for element in elements {
                    walk(element, bound)?;
                }
            }
            Ty::GenericFunc {
                decls,
                params,
                ret,
                variadic,
                kw_variadic,
                error,
                ..
            } => {
                let mut signature_scope = bound.clone();
                for declaration in decls {
                    match declaration {
                        ParamDecl::Type {
                            callable_bound,
                            default,
                            ..
                        } => {
                            if let Some(callable) = callable_bound {
                                walk(callable, &signature_scope)?;
                            }
                            if let Some(default) = default {
                                walk(default, &signature_scope)?;
                            }
                        }
                        ParamDecl::Value { name, ty, .. } => {
                            walk(ty, &signature_scope)?;
                            signature_scope.insert(name.trim_start_matches('*').to_string());
                        }
                    }
                }
                for parameter in params {
                    walk(parameter, &signature_scope)?;
                }
                walk(ret, &signature_scope)?;
                if let Some(parameter) = variadic {
                    walk(parameter, &signature_scope)?;
                }
                if let Some(parameter) = kw_variadic {
                    walk(parameter, &signature_scope)?;
                }
                if let Some(error) = error {
                    walk(error, &signature_scope)?;
                }
            }
            Ty::Func {
                params,
                ret,
                variadic,
                kw_variadic,
                error,
                ..
            } => {
                for parameter in params {
                    walk(parameter, bound)?;
                }
                walk(ret, bound)?;
                if let Some(parameter) = variadic {
                    walk(parameter, bound)?;
                }
                if let Some(parameter) = kw_variadic {
                    walk(parameter, bound)?;
                }
                if let Some(error) = error {
                    walk(error, bound)?;
                }
            }
            Ty::Param {
                callable_bound: Some(callable),
                ..
            } => walk(callable, bound)?,
            Ty::Struct(_, arguments) => {
                for argument in arguments {
                    if let TyArg::Ty(ty) = argument {
                        walk(ty, bound)?;
                    }
                }
            }
            Ty::ComptimeList(element) | Ty::VariadicPack(element) | Ty::Pointer { element, .. } => {
                walk(element, bound)?
            }
            Ty::Tuple(elements)
            | Ty::RuntimePack(elements)
            | Ty::Variant(elements)
            | Ty::Overload(elements) => {
                for element in elements {
                    walk(element, bound)?;
                }
            }
            Ty::Assoc { base, .. } => walk(base, bound)?,
            Ty::Ref(reference) => walk(&reference.referent, bound)?,
            _ => {}
        }
        Ok(())
    }

    walk(ty, &HashSet::new())
}

/// The storage a `MakeRef` handle designates. A capability-typed root already
/// contains a runtime frame/slot handle, so `MakeRef` forwards that handle and
/// extends its projection instead of borrowing the capability slot itself.
/// An ordinary root, including a struct field whose value happens to be a
/// reference, is borrowed as storage and therefore retains `place.ty`.
fn make_ref_target(place: &MirPlace) -> Option<(&Ty, Option<ReferencePermission>)> {
    let root_capability = place.root_ty.as_ref().and_then(reference_capability);
    match (root_capability, place.proj.is_empty()) {
        (Some(capability), true) => Some((capability.target, Some(capability.permission))),
        (Some(capability), false) => place
            .ty
            .as_ref()
            .map(|target| (target, Some(capability.permission))),
        (None, _) => place.ty.as_ref().map(|target| (target, None)),
    }
}

fn verify_function(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
    errors: &mut Vec<String>,
) {
    if function.var_names.len() != function.n_vars {
        errors.push(format!(
            "MIR function '{name}' has {} variable names for {} slots",
            function.var_names.len(),
            function.n_vars
        ));
    }
    for (index, ty) in function.param_types.iter().enumerate() {
        if contains_runtime_pack(ty) {
            errors.push(format!(
                "MIR function '{name}' parameter slot {index} retains ABI-only RuntimePack type {ty}"
            ));
        }
    }
    for (slot, ty) in &function.var_tys {
        if contains_runtime_pack(ty) {
            errors.push(format!(
                "MIR function '{name}' variable slot {slot} retains ABI-only RuntimePack type {ty}"
            ));
        }
    }
    for (register, ty) in &function.reg_types {
        if contains_runtime_pack(ty) {
            errors.push(format!(
                "MIR function '{name}' register r{register} retains ABI-only RuntimePack type {ty}"
            ));
        }
    }
    for (description, ty) in [
        ("return", function.ret_ty.as_ref()),
        ("error", function.error_ty.as_ref()),
    ] {
        if ty.is_some_and(contains_runtime_pack) {
            errors.push(format!(
                "MIR function '{name}' {description} contract retains ABI-only RuntimePack type"
            ));
        }
    }
    let context = RegionContext {
        region_len: function.blocks.len(),
        function_len: function.blocks.len(),
        in_try_region: false,
        protected: false,
    };
    verify_blocks(
        name,
        function,
        declarations,
        &function.blocks,
        &context,
        errors,
    );
}

/// `RuntimePack[T0, ...]` distinguishes a specialized heterogeneous `*args`
/// declaration from an ordinary homogeneous `*args: Tuple[...]`. It may occur
/// only as the top-level positional-variadic ABI type. Once arguments have
/// been matched, the body sees an ordinary native `Tuple[T0, ...]` slot.
fn verify_runtime_pack_abi(declarations: &MirDeclarations, errors: &mut Vec<String>) {
    for declaration in &declarations.structs {
        for (field, ty) in &declaration.fields {
            if contains_runtime_pack(ty) {
                errors.push(format!(
                    "MIR struct '{}.{}' contains ABI-only RuntimePack type {ty}",
                    declaration.name, field
                ));
            }
        }
    }
    for declaration in &declarations.functions {
        let name = &declaration.lowered_name;
        let parameter_count = declaration.param_types.len();
        for (description, found) in [
            ("names", declaration.param_names.len()),
            ("defaults", declaration.defaults.len()),
            ("required mask", declaration.required.len()),
            ("conventions", declaration.param_conventions.len()),
            ("reference mask", declaration.ref_params.len()),
        ] {
            if found != parameter_count {
                errors.push(format!(
                    "MIR declaration '{name}' has {found} parameter {description}, expected {parameter_count}"
                ));
            }
        }
        if !declaration.has_receiver && declaration.receiver_convention.is_some() {
            errors.push(format!(
                "MIR declaration '{name}' has a receiver convention but no receiver"
            ));
        }
        for (index, convention) in declaration.param_conventions.iter().enumerate() {
            let expected_reference = matches!(
                convention,
                Some(crate::ast::ArgConvention::Mut | crate::ast::ArgConvention::Ref)
            );
            if declaration.ref_params.get(index).copied() != Some(expected_reference) {
                errors.push(format!(
                    "MIR declaration '{name}' parameter {index} convention/reference mask disagree"
                ));
            }
        }
        for (index, ty) in declaration.param_types.iter().enumerate() {
            if contains_runtime_pack(ty) {
                errors.push(format!(
                    "MIR declaration '{name}' regular parameter {index} contains ABI-only RuntimePack type {ty}"
                ));
            }
        }
        if let Some(variadic) = &declaration.variadic {
            match variadic {
                Ty::RuntimePack(elements) => {
                    if elements.iter().any(contains_runtime_pack) {
                        errors.push(format!(
                            "MIR declaration '{name}' has a nested RuntimePack variadic ABI"
                        ));
                    }
                }
                other if contains_runtime_pack(other) => errors.push(format!(
                    "MIR declaration '{name}' embeds RuntimePack below its variadic ABI root"
                )),
                _ => {}
            }
        }
        for (description, ty) in [
            ("keyword variadic", declaration.kw_variadic.as_ref()),
            ("return", Some(&declaration.ret_ty)),
            ("error", declaration.error_ty.as_ref()),
        ] {
            if ty.is_some_and(contains_runtime_pack) {
                errors.push(format!(
                    "MIR declaration '{name}' {description} type contains ABI-only RuntimePack"
                ));
            }
        }
    }
}

fn verify_instruction(
    name: &str,
    function: &MirFunction,
    declarations: &MirDeclarations,
    block_index: usize,
    instruction: &MirInstr,
    context: &RegionContext,
    errors: &mut Vec<String>,
) {
    let prefix = format!("MIR function '{name}' block {block_index}");
    // Loan places are analytical origin paths and may name a nominal
    // collection element (`owner[index]`) even though executable collection
    // access retained its checked method call. Every other instruction place
    // is VM navigation and must name concrete indexed storage.
    let executable_place = !matches!(instruction, MirInstr::EstablishLoans { .. });
    for place in instruction_places(instruction) {
        verify_place(
            name,
            block_index,
            function,
            declarations,
            place,
            executable_place,
            errors,
        );
    }
    // Register bounds and type completeness.
    let mut regs = Vec::new();
    instruction_result_regs(instruction, &mut regs);
    instruction_operand_regs(instruction, &mut regs);
    for register in &regs {
        if register.0 >= function.n_regs {
            errors.push(format!("{prefix}: invalid register r{}", register.0));
        } else if !function.reg_types.contains_key(&register.0) {
            errors.push(format!("{prefix}: untyped register r{}", register.0));
        }
    }
    let reg_ty = |register: &Reg| function.reg_types.get(&register.0);
    let valid_simd_width = |width: usize| width >= 1 && (width & (width - 1)) == 0;
    match instruction {
        // Widths are validated during checked elaboration; this is the
        // phase-boundary backstop for assembled artifacts.
        MirInstr::MakeSimd { width, .. } | MirInstr::SimdCast { width, .. } => {
            if !valid_simd_width(*width) {
                errors.push(format!(
                    "{prefix}: SIMD width {width} is not a positive power of two"
                ));
            }
        }
        MirInstr::SimdShuffle { value, mask, .. } => {
            if !valid_simd_width(mask.len()) {
                errors.push(format!(
                    "{prefix}: SIMD shuffle mask length {} is not a positive power of two",
                    mask.len()
                ));
            }
            if let Some(Ty::Simd { width, .. }) = reg_ty(value)
                && let Some(bad) = mask.iter().find(|lane| **lane as i64 >= *width)
            {
                errors.push(format!(
                    "{prefix}: SIMD shuffle lane {bad} is out of range for width {width}"
                ));
            }
        }
        MirInstr::EstablishLoans {
            reference,
            loans,
            dest_interior,
            ..
        } => {
            if *reference as usize >= function.n_vars {
                errors.push(format!(
                    "{prefix}: loan generation has invalid reference slot {reference}"
                ));
            }
            if loans.is_empty() {
                errors.push(format!("{prefix}: loan generation has no owner loans"));
            }
            if let Some(domain) = dest_interior {
                if domain.root != *reference {
                    errors.push(format!(
                        "{prefix}: loan destination domain roots at slot {} instead of the \
                         generation's reference slot {reference}",
                        domain.root
                    ));
                }
                if domain.path.is_empty() {
                    errors.push(format!(
                        "{prefix}: loan destination domain has an empty interior path"
                    ));
                }
            }
            for loan in loans {
                let Some(origin) = &loan.interior else {
                    continue;
                };
                if origin.root as usize >= function.n_vars {
                    errors.push(format!(
                        "{prefix}: interior loan has invalid root slot {}",
                        origin.root
                    ));
                }
                if !origin
                    .path
                    .iter()
                    .any(|segment| matches!(segment, crate::origin::OriginSeg::Interior(_)))
                {
                    errors.push(format!(
                        "{prefix}: interior loan origin rooted at slot {} has no interior segment",
                        origin.root
                    ));
                }
            }
        }
        MirInstr::InvalidateInteriors {
            base,
            except,
            include_base_generation,
            ..
        } => {
            if base.root as usize >= function.n_vars {
                errors.push(format!(
                    "{prefix}: interior invalidation has invalid root slot {}",
                    base.root
                ));
            }
            if let Some(reference) = except
                && *reference as usize >= function.n_vars
            {
                errors.push(format!(
                    "{prefix}: interior invalidation exception has invalid reference slot {reference}"
                ));
            }
            if *include_base_generation
                && !base
                    .path
                    .iter()
                    .any(|segment| matches!(segment, crate::origin::OriginSeg::Interior(_)))
            {
                errors.push(format!(
                    "{prefix}: inclusive interior invalidation has no named interior generation"
                ));
            }
        }
        MirInstr::MakeRef { dest, place } => {
            if let Some(destination) = reg_ty(dest) {
                let Some(capability) = reference_capability(destination) else {
                    errors.push(format!(
                        "{prefix}: MakeRef destination has non-reference-capability type {destination}"
                    ));
                    return;
                };
                if let Some((target, source_permission)) = make_ref_target(place) {
                    if !types_compatible(capability.target, target) {
                        errors.push(format!(
                            "{prefix}: MakeRef destination targets {}, incompatible with place storage {target}",
                            capability.target
                        ));
                    }
                    if source_permission
                        .is_some_and(|permission| !permission.satisfies(capability.permission))
                    {
                        errors.push(format!(
                            "{prefix}: MakeRef destination recovers permission unavailable through its source capability"
                        ));
                    }
                }
            }
        }
        MirInstr::ReadRef { dest, reference } => {
            if let Some(source) = reg_ty(reference) {
                let Some(capability) = reference_capability(source) else {
                    errors.push(format!(
                        "{prefix}: ReadRef source has non-reference-capability type {source}"
                    ));
                    return;
                };
                if let Some(destination) = reg_ty(dest)
                    && !types_compatible(destination, capability.target)
                {
                    errors.push(format!(
                        "{prefix}: ReadRef result type {destination} is incompatible with referent {}",
                        capability.target
                    ));
                }
            }
        }
        MirInstr::WriteRef { reference, value } => {
            if let Some(source) = reg_ty(reference) {
                let Some(capability) = reference_capability(source) else {
                    errors.push(format!(
                        "{prefix}: WriteRef source has non-reference-capability type {source}"
                    ));
                    return;
                };
                if !capability.permission.allows_write() {
                    errors.push(format!(
                        "{prefix}: WriteRef source capability of type {source} is immutable"
                    ));
                }
                if let Some(value) = reg_ty(value)
                    && !types_compatible(value, capability.target)
                {
                    errors.push(format!(
                        "{prefix}: WriteRef value type {value} is incompatible with referent {}",
                        capability.target
                    ));
                }
            }
        }
        MirInstr::MaterializeLiteral { value, target, .. } => {
            let valid_target = matches!(target, Ty::Int | Ty::UInt | Ty::Float64)
                || matches!(target, Ty::Simd { width: 1, .. });
            if !valid_target {
                errors.push(format!(
                    "{prefix}: literal materialization has non-scalar target {target}"
                ));
            }
            if let Some(found) = reg_ty(value) {
                let valid_source = match found {
                    Ty::IntLiteral => matches!(
                        target,
                        Ty::Int | Ty::UInt | Ty::Float64 | Ty::Simd { width: 1, .. }
                    ),
                    Ty::FloatLiteral => {
                        matches!(target, Ty::Float64)
                            || matches!(target, Ty::Simd { dtype, width: 1 } if dtype.is_float())
                    }
                    _ => false,
                };
                if !valid_source {
                    errors.push(format!("{prefix}: cannot materialize {found} as {target}"));
                }
            }
        }
        MirInstr::CopyValue { dest, value } => {
            if let (Some(found), Some(expected)) = (reg_ty(value), reg_ty(dest))
                && found != expected
            {
                errors.push(format!(
                    "{prefix}: copied value has type {found}, destination has type {expected}"
                ));
            }
        }
        MirInstr::GetIter {
            source,
            dest,
            mode,
            prepare,
        } => {
            if *source as usize >= function.n_vars {
                errors.push(format!(
                    "{prefix}: GetIter uses invalid source slot {source}"
                ));
            }
            if *dest as usize >= function.n_vars {
                errors.push(format!(
                    "{prefix}: GetIter uses invalid destination slot {dest}"
                ));
            }
            let Some(first) = prepare.first() else {
                return;
            };
            let convention_matches = |convention| match mode {
                crate::checked::IterationMode::Borrowed => matches!(
                    convention,
                    None | Some(crate::ast::ArgConvention::Read)
                        | Some(crate::ast::ArgConvention::Ref)
                ),
                crate::checked::IterationMode::Owned => {
                    convention == Some(crate::ast::ArgConvention::Var)
                }
            };
            match declared(declarations, first) {
                Some(declaration) => {
                    if !declaration.has_receiver || !declaration.param_types.is_empty() {
                        errors.push(format!(
                            "{prefix}: GetIter preparation method '{first}' is not a nullary receiver operation"
                        ));
                    }
                    if !convention_matches(declaration.receiver_convention) {
                        errors.push(format!(
                            "{prefix}: GetIter {mode:?} mode does not match preparation method '{first}' receiver convention {:?}",
                            declaration.receiver_convention
                        ));
                    }
                }
                None => {
                    let matches_dispatch = match mode {
                        crate::checked::IterationMode::Borrowed => {
                            first
                                == &crate::symbol::iterator_dispatch_symbol(
                                    crate::ast::ArgConvention::Read,
                                )
                                || first
                                    == &crate::symbol::iterator_dispatch_symbol(
                                        crate::ast::ArgConvention::Ref,
                                    )
                        }
                        crate::checked::IterationMode::Owned => {
                            first
                                == &crate::symbol::iterator_dispatch_symbol(
                                    crate::ast::ArgConvention::Var,
                                )
                        }
                    };
                    if !matches_dispatch {
                        errors.push(format!(
                            "{prefix}: GetIter refers to undeclared preparation method '{first}'"
                        ));
                    }
                }
            }
        }
        MirInstr::Next { dest, iter, call } => {
            if *iter as usize >= function.n_vars {
                errors.push(format!("{prefix}: Next uses invalid iterator slot {iter}"));
            }
            let Some(call) = call else {
                return;
            };
            if reg_ty(dest) != Some(&call.result_ty) {
                errors.push(format!(
                    "{prefix}: Next result does not match its checked type {}",
                    call.result_ty
                ));
            }
            if call.raises.is_some() {
                errors.push(format!(
                    "{prefix}: bounded Next carries a raising iterator contract"
                ));
            }
            if verify_iterator_result_adapter(&prefix, call, errors) {
                return;
            }
            match declared(declarations, &call.target) {
                None => errors.push(format!(
                    "{prefix}: Next refers to undeclared iterator method '{}'",
                    call.target
                )),
                Some(declaration) => {
                    if !declaration.param_types.is_empty()
                        || declaration.receiver_convention != Some(crate::ast::ArgConvention::Mut)
                    {
                        errors.push(format!(
                            "{prefix}: Next method '{}' is not a nullary 'mut self' operation",
                            call.target
                        ));
                    }
                    if declaration.raises {
                        errors.push(format!(
                            "{prefix}: bounded Next method '{}' unexpectedly raises",
                            call.target
                        ));
                    }
                    if !iterator_result_matches_declaration(call, declaration) {
                        errors.push(format!(
                            "{prefix}: Next result contract does not match '{}'",
                            call.target
                        ));
                    }
                }
            }
        }
        MirInstr::TryNext {
            dest,
            yielded,
            iter,
            call,
            exhaustion,
        } => {
            if *iter as usize >= function.n_vars {
                errors.push(format!(
                    "{prefix}: TryNext uses invalid iterator slot {iter}"
                ));
            }
            if reg_ty(yielded).is_some_and(|ty| *ty != Ty::Bool) {
                errors.push(format!(
                    "{prefix}: TryNext yielded flag has non-Bool type {}",
                    reg_ty(yielded).expect("checked above")
                ));
            }
            let is_stop_iteration = matches!(
                exhaustion,
                Ty::Struct(name, arguments)
                    if arguments.is_empty()
                        && (name == "StopIteration" || name.ends_with("$StopIteration"))
            );
            if !is_stop_iteration {
                errors.push(format!(
                    "{prefix}: TryNext catches non-StopIteration type {exhaustion}"
                ));
            }
            if reg_ty(dest) != Some(&call.result_ty) {
                errors.push(format!(
                    "{prefix}: TryNext result does not match its checked type {}",
                    call.result_ty
                ));
            }
            if call.raises.as_ref() != Some(exhaustion) {
                errors.push(format!(
                    "{prefix}: TryNext exhaustion type {exhaustion} does not match its checked call effect"
                ));
            }
            if verify_iterator_result_adapter(&prefix, call, errors) {
                return;
            }
            match declared(declarations, &call.target) {
                None => errors.push(format!(
                    "{prefix}: TryNext refers to undeclared iterator method '{}'",
                    call.target
                )),
                Some(declaration) => {
                    if !declaration.param_types.is_empty()
                        || declaration.receiver_convention != Some(crate::ast::ArgConvention::Mut)
                    {
                        errors.push(format!(
                            "{prefix}: TryNext method '{}' is not a nullary 'mut self' operation",
                            call.target
                        ));
                    }
                    if !declaration.raises || declaration.error_ty.as_ref() != Some(exhaustion) {
                        errors.push(format!(
                            "{prefix}: TryNext exhaustion type {exhaustion} does not match '{}' raising contract",
                            call.target
                        ));
                    }
                    if !iterator_result_matches_declaration(call, declaration) {
                        errors.push(format!(
                            "{prefix}: TryNext result contract does not match '{}'",
                            call.target
                        ));
                    }
                }
            }
        }
        MirInstr::Store { place, src } => {
            if let (Some(expected), Some(found)) = (place.ty.as_ref(), reg_ty(src)) {
                let target = match expected {
                    Ty::Ref(reference) => reference.referent.as_ref(),
                    other => other,
                };
                if !types_compatible(found, target) {
                    errors.push(format!(
                        "{prefix}: store of {found} into storage of type {target}"
                    ));
                }
            }
        }
        MirInstr::StoreRef { place, reference } => {
            if let Some(storage) = place.ty.as_ref() {
                let Ty::Ref(storage_reference) = storage else {
                    errors.push(format!(
                        "{prefix}: StoreRef into non-reference storage of type {storage}"
                    ));
                    return;
                };
                if let Some(source) = reg_ty(reference) {
                    let Ty::Ref(source_reference) = source else {
                        errors.push(format!(
                            "{prefix}: StoreRef source has non-reference type {source}"
                        ));
                        return;
                    };
                    if !types_compatible(&source_reference.referent, &storage_reference.referent) {
                        errors.push(format!(
                            "{prefix}: StoreRef source referent {} is incompatible with storage referent {}",
                            source_reference.referent, storage_reference.referent
                        ));
                    }
                    let source_permission =
                        ReferencePermission::from_mutability(source_reference.mutability);
                    let storage_permission =
                        ReferencePermission::from_mutability(storage_reference.mutability);
                    if !source_permission.satisfies(storage_permission) {
                        errors.push(format!(
                            "{prefix}: StoreRef source permission cannot initialize storage of type {storage}"
                        ));
                    }
                }
            }
        }
        MirInstr::Index {
            dest,
            base,
            index,
            base_place,
            index_place,
            call,
            intrinsic,
            ..
        } => {
            verify_subscript_receiver_place(&prefix, reg_ty(base), base_place.as_ref(), errors);
            if call.is_some() == intrinsic.is_some() {
                errors.push(format!(
                    "{prefix}: index operation must carry exactly one checked-call or intrinsic dispatch"
                ));
            }
            if let Some(call) = call {
                // `__getitem_param__` reuses the syntactic index register as a
                // compile-time value argument; it has no ordinary runtime
                // positional argument. The empty checked argument list is the
                // explicit ABI discriminator used by lowering and execution.
                let positional_places = if call.arguments.is_empty() {
                    &[][..]
                } else {
                    std::slice::from_ref(index_place)
                };
                let positional_types = if call.arguments.is_empty() {
                    Vec::new()
                } else {
                    vec![reg_ty(index).cloned()]
                };
                verify_subscript_call(
                    &prefix,
                    function,
                    declarations,
                    call,
                    SubscriptSources {
                        receiver_ty: reg_ty(base),
                        method: "__getitem__",
                        receiver_place: base_place.as_ref(),
                        positional_places,
                        keyword_places: &[],
                        positional_types: &positional_types,
                        keyword_types: &[],
                        dest: Some(*dest),
                    },
                    errors,
                );
            } else if let Some(intrinsic) = intrinsic {
                verify_intrinsic_index(
                    &prefix,
                    *intrinsic,
                    reg_ty(base),
                    reg_ty(index),
                    reg_ty(dest),
                    errors,
                );
            }
        }
        MirInstr::Slice {
            dest,
            object,
            kind,
            object_place,
            arg_places,
            call,
            intrinsic,
            ..
        } => {
            verify_subscript_receiver_place(&prefix, reg_ty(object), object_place.as_ref(), errors);
            if arg_places.len() != 1 {
                errors.push(format!(
                    "{prefix}: slice place metadata is not aligned with its descriptor argument"
                ));
            }
            if call.is_some() == intrinsic.is_some() {
                errors.push(format!(
                    "{prefix}: slice operation must carry exactly one checked-call or intrinsic dispatch"
                ));
            }
            if let Some(call) = call {
                verify_subscript_call(
                    &prefix,
                    function,
                    declarations,
                    call,
                    SubscriptSources {
                        receiver_ty: reg_ty(object),
                        method: "__getitem__",
                        receiver_place: object_place.as_ref(),
                        positional_places: arg_places,
                        keyword_places: &[],
                        positional_types: &[Some(slice_descriptor_ty(*kind))],
                        keyword_types: &[],
                        dest: Some(*dest),
                    },
                    errors,
                );
            } else if let Some(intrinsic) = intrinsic {
                verify_intrinsic_slice(&prefix, *intrinsic, reg_ty(object), reg_ty(dest), errors);
            }
        }
        MirInstr::MultiIndex {
            dest,
            object,
            args,
            object_place,
            arg_places,
            kwargs,
            kwarg_places,
            call,
        } => {
            verify_subscript_receiver_place(&prefix, reg_ty(object), object_place.as_ref(), errors);
            if arg_places.len() != args.len() || kwarg_places.len() != kwargs.len() {
                errors.push(format!(
                    "{prefix}: multi-subscript place metadata is not aligned with its arguments"
                ));
            }
            let positional_types = args
                .iter()
                .map(|argument| subscript_argument_ty(function, argument))
                .collect::<Vec<_>>();
            let keyword_types = kwargs
                .iter()
                .map(|(_, register)| function.reg_types.get(&register.0).cloned())
                .collect::<Vec<_>>();
            if let Some(call) = call {
                verify_subscript_call(
                    &prefix,
                    function,
                    declarations,
                    call,
                    SubscriptSources {
                        receiver_ty: reg_ty(object),
                        method: "__getitem__",
                        receiver_place: object_place.as_ref(),
                        positional_places: arg_places,
                        keyword_places: kwarg_places,
                        positional_types: &positional_types,
                        keyword_types: &keyword_types,
                        dest: Some(*dest),
                    },
                    errors,
                );
            } else {
                errors.push(format!(
                    "{prefix}: multi-index operation lacks a checked call contract"
                ));
            }
        }
        MirInstr::MultiSet {
            receiver,
            receiver_place,
            args,
            arg_places,
            value,
            value_place,
            value_keyword,
            call,
            ..
        } => {
            verify_subscript_receiver_place(
                &prefix,
                reg_ty(receiver),
                receiver_place.as_ref(),
                errors,
            );
            if arg_places.len() != args.len() {
                errors.push(format!(
                    "{prefix}: subscript-set place metadata is not aligned with its index arguments"
                ));
            }
            let mut positional = arg_places.clone();
            let mut positional_types = args
                .iter()
                .map(|argument| subscript_argument_ty(function, argument))
                .collect::<Vec<_>>();
            let keyword = if *value_keyword {
                vec![value_place.clone()]
            } else {
                positional.push(value_place.clone());
                positional_types.push(reg_ty(value).cloned());
                Vec::new()
            };
            let keyword_types = if *value_keyword {
                vec![reg_ty(value).cloned()]
            } else {
                Vec::new()
            };
            verify_subscript_call(
                &prefix,
                function,
                declarations,
                call,
                SubscriptSources {
                    receiver_ty: reg_ty(receiver),
                    method: "__setitem__",
                    receiver_place: receiver_place.as_ref(),
                    positional_places: &positional,
                    keyword_places: &keyword,
                    positional_types: &positional_types,
                    keyword_types: &keyword_types,
                    dest: None,
                },
                errors,
            );
        }
        MirInstr::DefVar {
            src,
            binding_ty: Some(expected),
            ..
        } => {
            if let Some(found) = reg_ty(src)
                && !types_compatible(found, expected)
            {
                errors.push(format!(
                    "{prefix}: binding of {found} to a slot of type {expected}"
                ));
            }
        }
        MirInstr::MakeVariant {
            alternatives,
            index,
            value,
            ..
        } => {
            if *index >= alternatives.len() {
                errors.push(format!(
                    "{prefix}: variant construction index {index} out of {} alternatives",
                    alternatives.len()
                ));
            } else if let Some(found) = reg_ty(value)
                && !types_compatible(found, &alternatives[*index])
            {
                errors.push(format!(
                    "{prefix}: variant payload {found} does not fit alternative {}",
                    alternatives[*index]
                ));
            }
        }
        MirInstr::MakeClosure {
            dest,
            function: target,
            captures,
        } => {
            match declared(declarations, target) {
                None => errors.push(format!(
                    "{prefix}: closure refers to undeclared lifted function '{target}'"
                )),
                Some(declaration) => {
                    if captures.len() > declaration.param_types.len() {
                        errors.push(format!(
                            "{prefix}: closure has {} captures for '{}' with only {} parameters",
                            captures.len(),
                            target,
                            declaration.param_types.len()
                        ));
                    }
                    if declaration
                        .ref_params
                        .iter()
                        .take(captures.len())
                        .any(|is_reference| !is_reference)
                    {
                        errors.push(format!(
                            "{prefix}: closure environment for '{target}' is not a reference-parameter prefix"
                        ));
                    }
                }
            }
            if let Some(found) = reg_ty(dest)
                && !matches!(found, Ty::Func { .. } | Ty::GenericFunc { .. })
            {
                errors.push(format!(
                    "{prefix}: closure result has non-callable type {found}"
                ));
            }
        }
        MirInstr::PointerStorageTake {
            dest,
            pointer,
            index,
            element,
        }
        | MirInstr::PointerStorageDestroy {
            dest,
            pointer,
            index,
            element,
        } => {
            match reg_ty(pointer) {
                Some(Ty::Pointer {
                    element: actual,
                    origin: crate::origin::PointerOrigin::Legacy,
                }) if types_compatible(actual, element) => {}
                Some(found) => errors.push(format!(
                    "{prefix}: compiler-private pointer storage operation expects UnsafePointer[{element}], got {found}"
                )),
                None => {}
            }
            if let Some(found) = reg_ty(index)
                && !types_compatible(found, &Ty::Int)
            {
                errors.push(format!(
                    "{prefix}: compiler-private pointer storage index has type {found}, expected Int"
                ));
            }
            let expected = if matches!(instruction, MirInstr::PointerStorageTake { .. }) {
                element
            } else {
                &Ty::None
            };
            if let Some(found) = reg_ty(dest)
                && !types_compatible(found, expected)
            {
                errors.push(format!(
                    "{prefix}: compiler-private pointer storage result has type {found}, expected {expected}"
                ));
            }
        }
        MirInstr::Call {
            func: FuncRef(callee),
            args,
            kwargs,
            arg_places,
            kwarg_places,
            capture_accesses,
            param_arg_regs,
            ..
        } => {
            verify_capture_accesses(&prefix, function, capture_accesses, errors);
            if let Some(declaration) = declared(declarations, callee) {
                verify_direct_call(
                    &prefix,
                    function,
                    declaration,
                    args,
                    kwargs,
                    arg_places,
                    errors,
                );
                verify_param_arguments(
                    &prefix,
                    function,
                    &declaration.param_decls,
                    param_arg_regs,
                    errors,
                );
            }
            if arg_places.len() != args.len() || kwarg_places.len() != kwargs.len() {
                errors.push(format!(
                    "{prefix}: call place metadata is not aligned with its arguments"
                ));
            }
        }
        MirInstr::MethodCall {
            dest,
            method,
            resolved: Some(callee),
            reference_result,
            result_adapter,
            args,
            kwargs,
            arg_places,
            kwarg_places,
            capture_accesses,
            param_arg_regs,
            param_decls,
            ..
        } => {
            verify_capture_accesses(&prefix, function, capture_accesses, errors);
            let abstract_value_next = callee.starts_with("__trait_dispatch.")
                && method == "__next__"
                && reference_result.is_none();
            if let Some(reference) = reference_result {
                let expected = Ty::Ref(reference.clone());
                if function.reg_types.get(&dest.0) != Some(&expected) {
                    errors.push(format!(
                        "{prefix}: method-call reference ABI {expected} does not match its destination type"
                    ));
                }
            }
            match result_adapter {
                Some(crate::checked::CheckedResultAdapter::CopyIteratorReference) => {
                    if !abstract_value_next {
                        errors.push(format!(
                            "{prefix}: copy-reference result adapter is not attached to an abstract value-returning __next__ call"
                        ));
                    }
                }
                None if abstract_value_next => errors.push(format!(
                    "{prefix}: abstract value-returning __next__ call lacks its copy-reference adapter"
                )),
                None => {}
            }
            if let Some(declaration) = declared(declarations, callee) {
                verify_direct_call(
                    &prefix,
                    function,
                    declaration,
                    args,
                    kwargs,
                    arg_places,
                    errors,
                );
                let result_abi_matches = match reference_result {
                    Some(reference) => {
                        declaration.returns_reference
                            && types_compatible(&reference.referent, &declaration.ret_ty)
                    }
                    None => {
                        !declaration.returns_reference
                            && function
                                .reg_types
                                .get(&dest.0)
                                .is_some_and(|result| types_compatible(result, &declaration.ret_ty))
                    }
                };
                if !result_abi_matches {
                    errors.push(format!(
                        "{prefix}: method-call result ABI does not match '{}'",
                        declaration.lowered_name
                    ));
                }
                if &declaration.param_decls != param_decls {
                    errors.push(format!(
                        "{prefix}: method-call compile-time parameter metadata does not match '{}'",
                        declaration.lowered_name
                    ));
                }
                verify_param_arguments(
                    &prefix,
                    function,
                    &declaration.param_decls,
                    param_arg_regs,
                    errors,
                );
            }
            if arg_places.len() != args.len() || kwarg_places.len() != kwargs.len() {
                errors.push(format!(
                    "{prefix}: method-call place metadata is not aligned with its arguments"
                ));
            }
        }
        MirInstr::CallIndirect {
            dest,
            callee,
            resolved,
            raises,
            args,
            kwargs,
            arg_places,
            kwarg_places,
            capture_accesses,
            param_arg_regs,
            param_decls,
            instantiated_contract,
            instantiated_args,
            ..
        } => {
            verify_capture_accesses(&prefix, function, capture_accesses, errors);
            if arg_places.len() != args.len() || kwarg_places.len() != kwargs.len() {
                errors.push(format!(
                    "{prefix}: indirect-call place metadata is not aligned with its arguments"
                ));
            }
            let stored_contract = reg_ty(callee).and_then(crate::checker::callable_contract_ty);
            let mut verified_instantiation = None;
            let contract = match stored_contract {
                Some(symbolic @ Ty::GenericFunc { .. }) => {
                    if let Some(found) = instantiated_contract {
                        match instantiate_generic_callable_contract(symbolic, instantiated_args) {
                            Ok(expected) => {
                                if found != &expected {
                                    errors.push(format!(
                                        "{prefix}: checker-instantiated callable contract does not match its retained generic arguments"
                                    ));
                                }
                                verified_instantiation = Some(expected);
                            }
                            Err(reason) => errors.push(format!(
                                "{prefix}: invalid generic callable instantiation: {reason}"
                            )),
                        }
                        verified_instantiation.as_ref()
                    } else {
                        if !instantiated_args.is_empty() {
                            errors.push(format!(
                                "{prefix}: symbolic generic indirect call carries an orphaned instantiation witness"
                            ));
                        }
                        if let Err(reason) = validate_dependent_bindings(symbolic) {
                            errors.push(format!(
                                "{prefix}: invalid symbolic generic callable contract: {reason}"
                            ));
                        }
                        // Calls in an unspecialized generic body remain under
                        // the callable contract's own explicit binders. Their
                        // dependent references are scope-validated below; no
                        // concrete substitution witness exists at this layer.
                        Some(symbolic)
                    }
                }
                Some(contract) => {
                    if instantiated_contract.is_some() || !instantiated_args.is_empty() {
                        errors.push(format!(
                            "{prefix}: nongeneric indirect call carries generic instantiation metadata"
                        ));
                    }
                    Some(contract)
                }
                None => {
                    if instantiated_contract.is_some() || !instantiated_args.is_empty() {
                        errors.push(format!(
                            "{prefix}: non-callable indirect operand carries generic instantiation metadata"
                        ));
                    }
                    None
                }
            };
            if let Some(contract) = contract {
                verify_callable_contract_call(
                    &prefix, function, contract, raises, *dest, args, kwargs, arg_places, errors,
                );
            }
            let checked_decls = reg_ty(callee).and_then(generic_callable_decls);
            if let Some(checked_decls) = checked_decls {
                if checked_decls != param_decls {
                    errors.push(format!(
                        "{prefix}: indirect-call compile-time parameter metadata does not match its callable contract"
                    ));
                }
                verify_param_arguments(&prefix, function, checked_decls, param_arg_regs, errors);
            } else if !param_decls.is_empty() {
                errors.push(format!(
                    "{prefix}: nongeneric indirect call carries compile-time parameter metadata"
                ));
            }
            let nominal_name = match reg_ty(callee) {
                Some(Ty::Struct(name, _)) => Some(name.as_str()),
                _ => None,
            };
            if let Some(target) = resolved {
                if nominal_name.is_none()
                    && let Some(expected) =
                        reg_ty(callee).and_then(crate::checker::callable_contract_target)
                    && target != &expected
                {
                    errors.push(format!(
                        "{prefix}: indirect-call target '{target}' does not match callable contract '{expected}'"
                    ));
                }
                let is_call_target = target.rsplit_once('.').is_some_and(|(_, method)| {
                    method == "__call__" || crate::symbol::is_overload_of(method, "__call__")
                });
                if !is_call_target {
                    errors.push(format!(
                        "{prefix}: indirect-call target '{target}' is not a __call__ method"
                    ));
                }
                let concrete = nominal_name
                    .and_then(|name| crate::symbol::retarget_method_symbol(target, name))
                    .unwrap_or_else(|| target.clone());
                if let Some(declaration) = declared(declarations, &concrete) {
                    verify_direct_call(
                        &prefix,
                        function,
                        declaration,
                        args,
                        kwargs,
                        arg_places,
                        errors,
                    );
                } else if nominal_name.is_some() {
                    errors.push(format!(
                        "{prefix}: nominal indirect-call target '{concrete}' is undeclared"
                    ));
                }
            } else if let Some(name) = nominal_name {
                errors.push(format!(
                    "{prefix}: nominal callable '{name}' has no checker-selected __call__ target"
                ));
            }
        }
        MirInstr::Raise { .. } => {
            if !function.raises && !context.protected {
                errors.push(format!(
                    "{prefix}: unprotected raise in nonraising function"
                ));
            }
        }
        MirInstr::Try {
            body,
            handler,
            orelse,
            finalbody,
            ..
        } => {
            let body_context = RegionContext {
                region_len: body.len(),
                function_len: context.function_len,
                in_try_region: true,
                protected: handler.is_some() || context.protected,
            };
            verify_blocks(name, function, declarations, body, &body_context, errors);
            for region in handler
                .iter()
                .map(|(_, blocks)| blocks)
                .chain(orelse.iter())
                .chain(finalbody.iter())
            {
                let region_context = RegionContext {
                    region_len: region.len(),
                    function_len: context.function_len,
                    in_try_region: true,
                    protected: context.protected,
                };
                verify_blocks(
                    name,
                    function,
                    declarations,
                    region,
                    &region_context,
                    errors,
                );
            }
        }
        _ => {}
    }
    // A call carrying a checked error contract raises unless handled.
    if let MirInstr::Call {
        raises: Some(_), ..
    }
    | MirInstr::CallIndirect {
        raises: Some(_), ..
    }
    | MirInstr::MethodCall {
        raises: Some(_), ..
    }
    | MirInstr::Index {
        call: Some(super::MirSubscriptCall {
            raises: Some(_), ..
        }),
        ..
    }
    | MirInstr::Slice {
        call: Some(super::MirSubscriptCall {
            raises: Some(_), ..
        }),
        ..
    }
    | MirInstr::MultiIndex {
        call: Some(super::MirSubscriptCall {
            raises: Some(_), ..
        }),
        ..
    }
    | MirInstr::MultiSet {
        call: super::MirSubscriptCall {
            raises: Some(_), ..
        },
        ..
    } = instruction
        && !function.raises
        && !context.protected
    {
        errors.push(format!(
            "{prefix}: unprotected raising call in nonraising function"
        ));
    }
}

fn verify_intrinsic_index(
    prefix: &str,
    intrinsic: MirIntrinsicSubscript,
    base: Option<&Ty>,
    index: Option<&Ty>,
    dest: Option<&Ty>,
    errors: &mut Vec<String>,
) {
    if let Some(index) = index
        && !types_compatible(index, &Ty::Int)
        && !matches!(index, Ty::UInt)
    {
        errors.push(format!(
            "{prefix}: intrinsic index has non-Indexer register type {index}"
        ));
    }

    let result_candidates = match (intrinsic, base) {
        (MirIntrinsicSubscript::TupleStorage, Some(Ty::Tuple(elements)))
        | (MirIntrinsicSubscript::TupleStorage, Some(Ty::RuntimePack(elements))) => {
            Some(elements.iter().collect::<Vec<_>>())
        }
        (MirIntrinsicSubscript::TupleStorage, Some(base)) => tuple_elements(base),
        (MirIntrinsicSubscript::VariadicStorage, Some(Ty::VariadicPack(element))) => {
            Some(vec![element.as_ref()])
        }
        (MirIntrinsicSubscript::Simd, Some(Ty::Simd { dtype, .. })) => {
            let scalar = simd_element_type(*dtype);
            if let Some(dest) = dest
                && !types_compatible(dest, &scalar)
            {
                errors.push(format!(
                    "{prefix}: SIMD intrinsic result has type {dest}, expected {scalar}"
                ));
            }
            return;
        }
        (MirIntrinsicSubscript::Pointer, Some(Ty::Pointer { element, .. }))
        | (MirIntrinsicSubscript::ComptimeList, Some(Ty::ComptimeList(element))) => {
            Some(vec![element.as_ref()])
        }
        (MirIntrinsicSubscript::String, _) => {
            errors.push(format!(
                "{prefix}: String intrinsic is valid for Slice, not Index"
            ));
            return;
        }
        (_, Some(base)) => {
            errors.push(format!(
                "{prefix}: intrinsic {intrinsic:?} is incompatible with checked base type {base}"
            ));
            return;
        }
        (_, None) => return,
    };

    if let (Some(dest), Some(candidates)) = (dest, result_candidates)
        && !candidates.iter().any(|candidate| {
            types_compatible(dest, candidate)
                || matches!(candidate, Ty::Ref(reference) if types_compatible(dest, &reference.referent))
        })
    {
        errors.push(format!(
            "{prefix}: intrinsic {intrinsic:?} result type {dest} is incompatible with its checked element type"
        ));
    }
}

/// Element types a raw MIR place projection may select. Nominal collection
/// indexing is absent by construction: it must retain a checked call contract
/// instead of becoming storage navigation. Public Tuple spellings are accepted
/// only for the narrow compiler-owned storage bridge already represented by a
/// projection place.
fn indexed_place_element_types(base: &Ty) -> Option<Vec<Ty>> {
    match base {
        Ty::Tuple(elements) | Ty::RuntimePack(elements) => Some(elements.clone()),
        Ty::VariadicPack(element) | Ty::ComptimeList(element) | Ty::Pointer { element, .. } => {
            Some(vec![(**element).clone()])
        }
        Ty::Simd { dtype, .. } => Some(vec![simd_element_type(*dtype)]),
        other => tuple_elements(other).map(|elements| elements.into_iter().cloned().collect()),
    }
}

fn verify_intrinsic_slice(
    prefix: &str,
    intrinsic: MirIntrinsicSubscript,
    base: Option<&Ty>,
    dest: Option<&Ty>,
    errors: &mut Vec<String>,
) {
    if intrinsic != MirIntrinsicSubscript::String {
        errors.push(format!(
            "{prefix}: intrinsic {intrinsic:?} is not a slice dispatch"
        ));
        return;
    }
    if let Some(base) = base
        && !types_compatible(base, &Ty::StringLiteral)
    {
        errors.push(format!(
            "{prefix}: String slice intrinsic has checked base type {base}"
        ));
    }
    if let Some(dest) = dest
        && !types_compatible(dest, &Ty::StringLiteral)
    {
        errors.push(format!(
            "{prefix}: String slice intrinsic has result type {dest}"
        ));
    }
}

/// Rebuild the executable callable shape from the symbolic generic contract.
/// The full dependent/type substitution is performed here rather than by
/// inspecting parameter-materialization instructions; `arguments` is the
/// checker-retained declaration-order witness.
fn instantiate_generic_callable_contract(contract: &Ty, arguments: &[TyArg]) -> Result<Ty, String> {
    let Ty::GenericFunc {
        environment,
        decls,
        params,
        names,
        ret,
        required,
        variadic,
        kw_variadic,
        positional_only,
        keyword_only,
        raises,
        error,
        conventions,
        ref_params,
        ref_return,
        transfers,
    } = contract
    else {
        return Err("retained contract is not generic".to_string());
    };
    if decls.len() != arguments.len() {
        return Err(format!(
            "{} retained argument(s) for {} declaration(s)",
            arguments.len(),
            decls.len()
        ));
    }
    let argument_maps = generic_argument_maps(decls, arguments)?;
    let bound = HashSet::new();
    let mut instantiate =
        |ty: &Ty| instantiate_checked_type(ty, &argument_maps.types, &argument_maps.values, &bound);
    let instantiated = Ty::Func {
        environment: environment.clone(),
        params: params
            .iter()
            .map(&mut instantiate)
            .collect::<Result<Vec<_>, _>>()?,
        names: names.clone(),
        ret: Box::new(instantiate(ret)?),
        required: required.clone(),
        variadic: variadic
            .as_ref()
            .map(|ty| instantiate(ty).map(Box::new))
            .transpose()?,
        kw_variadic: kw_variadic
            .as_ref()
            .map(|ty| instantiate(ty).map(Box::new))
            .transpose()?,
        positional_only: *positional_only,
        keyword_only: *keyword_only,
        raises: *raises,
        error: error
            .as_ref()
            .map(|ty| instantiate(ty).map(Box::new))
            .transpose()?,
        conventions: conventions.clone(),
        ref_params: ref_params.clone(),
        ref_return: ref_return.clone(),
        transfers: transfers.clone(),
    };
    validate_dependent_bindings(&instantiated)?;
    Ok(instantiated)
}

fn generic_argument_maps(
    declarations: &[ParamDecl],
    arguments: &[TyArg],
) -> Result<GenericArgumentMaps, String> {
    let mut types = HashMap::new();
    let mut values = HashMap::new();
    for (declaration, argument) in declarations.iter().zip(arguments) {
        let name = declaration.name().trim_start_matches('*').to_string();
        match (declaration, argument) {
            (ParamDecl::Type { .. }, TyArg::Ty(ty)) => {
                types.insert(name, ty.clone());
            }
            (ParamDecl::Value { .. }, TyArg::Val(value))
            | (ParamDecl::Type { variadic: true, .. }, TyArg::Val(value)) => {
                values.insert(name, value.clone());
            }
            (ParamDecl::Type { .. }, TyArg::Val(_)) => {
                return Err(format!(
                    "type parameter '{}' retains a value argument",
                    declaration.name()
                ));
            }
            (ParamDecl::Value { .. }, TyArg::Ty(_)) => {
                return Err(format!(
                    "value parameter '{}' retains a type argument",
                    declaration.name()
                ));
            }
            // Origins erase from the runtime ABI, so they contribute no entry to
            // the type/value argument maps used for generic instantiation.
            (_, TyArg::Origin(_)) => {}
        }
    }
    Ok(GenericArgumentMaps { types, values })
}

/// Verify an indirect call whose callee register carries a checked anonymous
/// callable contract. Unlike nominal dispatch, there is no concrete MIR
/// declaration to consult: the `Ty::Func` retained on the bounded parameter is
/// the declaration and must be checked directly.
#[allow(clippy::too_many_arguments)]
fn verify_callable_contract_call(
    prefix: &str,
    function: &MirFunction,
    contract: &Ty,
    call_raises: &Option<Ty>,
    dest: Reg,
    args: &[Reg],
    kwargs: &[(String, Reg)],
    arg_places: &[Option<MirPlace>],
    errors: &mut Vec<String>,
) {
    let (params, ret, required, variadic, kw_variadic, conventions, ref_return, raises, error) =
        match contract {
            Ty::Func {
                params,
                ret,
                required,
                variadic,
                kw_variadic,
                conventions,
                ref_return,
                raises,
                error,
                ..
            }
            | Ty::GenericFunc {
                params,
                ret,
                required,
                variadic,
                kw_variadic,
                conventions,
                ref_return,
                raises,
                error,
                ..
            } => (
                params,
                ret,
                required,
                variadic,
                kw_variadic,
                conventions,
                ref_return,
                raises,
                error,
            ),
            _ => return,
        };
    if *raises != call_raises.is_some() {
        errors.push(format!(
            "{prefix}: indirect-call raising metadata does not match its callable contract"
        ));
    } else if let (Some(found), Some(expected)) = (call_raises, error.as_deref())
        && !types_compatible(found, expected)
    {
        errors.push(format!(
            "{prefix}: indirect-call error type {found} does not match callable contract {expected}"
        ));
    }
    if kwargs.is_empty() && variadic.is_none() && kw_variadic.is_none() {
        let omitted_required = required
            .iter()
            .skip(args.len())
            .copied()
            .any(|is_required| is_required);
        if args.len() > params.len() || omitted_required {
            errors.push(format!(
                "{prefix}: indirect call has {} positional arguments, callable contract requires {}",
                args.len(),
                required.iter().filter(|required| **required).count()
            ));
        } else {
            for (index, (argument, expected)) in args.iter().zip(params).enumerate() {
                if let Some(found) = function.reg_types.get(&argument.0)
                    && !types_compatible(found, expected)
                {
                    errors.push(format!(
                        "{prefix}: argument {index} of indirect callable has type {found}, contract declares {expected}"
                    ));
                }
                if matches!(
                    conventions.get(index).copied().flatten(),
                    Some(crate::ast::ArgConvention::Mut | crate::ast::ArgConvention::Ref)
                ) && arg_places.get(index).and_then(Option::as_ref).is_none()
                {
                    errors.push(format!(
                        "{prefix}: mutable/reference parameter {index} of indirect callable has no caller place"
                    ));
                }
            }
        }
        if required.len() != params.len() {
            errors.push(format!(
                "{prefix}: callable contract required-mask length does not match its parameters"
            ));
        }
    }
    if let Some(found) = function.reg_types.get(&dest.0) {
        let found_value = match (ref_return.is_some(), found) {
            (true, Ty::Ref(reference)) => {
                if let Some(signature) = ref_return.as_deref() {
                    let mutability_matches = match signature.mutability {
                        crate::origin::SigMutability::Immutable => {
                            reference.mutability == crate::origin::Mutability::Immutable
                        }
                        crate::origin::SigMutability::Mutable => {
                            reference.mutability == crate::origin::Mutability::Mutable
                        }
                        crate::origin::SigMutability::BoolParam(_)
                        | crate::origin::SigMutability::Infer => true,
                    };
                    if !mutability_matches {
                        errors.push(format!(
                            "{prefix}: indirect-call reference result mutability does not match its callable contract"
                        ));
                    }
                    let origin_matches = match &signature.origin {
                        crate::origin::SigOrigin::Bound(expected) => &reference.origin == expected,
                        crate::origin::SigOrigin::Static => {
                            reference.origin == crate::origin::Origin::Static
                        }
                        crate::origin::SigOrigin::Untracked { mutable } => {
                            reference.origin
                                == crate::origin::Origin::Untracked { mutable: *mutable }
                        }
                        crate::origin::SigOrigin::Self_
                        | crate::origin::SigOrigin::Param(_)
                        | crate::origin::SigOrigin::Projected(_, _)
                        | crate::origin::SigOrigin::Union(_)
                        | crate::origin::SigOrigin::Infer => true,
                    };
                    if !origin_matches {
                        errors.push(format!(
                            "{prefix}: indirect-call reference result origin does not match its callable contract"
                        ));
                    }
                }
                reference.referent.as_ref()
            }
            (true, other) => {
                errors.push(format!(
                    "{prefix}: indirect-call result has type {other}, contract returns a reference to {ret}"
                ));
                return;
            }
            (false, Ty::Ref(_)) => {
                errors.push(format!(
                    "{prefix}: indirect-call result is a reference, contract returns {ret} by value"
                ));
                return;
            }
            (false, other) => other,
        };
        if !types_compatible(found_value, ret) {
            errors.push(format!(
                "{prefix}: indirect-call result has type {found}, contract returns {ret}"
            ));
        }
    }
}

fn verify_terminator(
    name: &str,
    function: &MirFunction,
    block_index: usize,
    terminator: &MirTerm,
    context: &RegionContext,
    errors: &mut Vec<String>,
) {
    let prefix = format!("MIR function '{name}' block {block_index}");
    match terminator {
        MirTerm::Jump(target) => {
            if *target >= context.region_len {
                errors.push(format!("{prefix}: jump to invalid block {target}"));
            }
        }
        MirTerm::Branch {
            cond,
            then_b,
            else_b,
        } => {
            for target in [then_b, else_b] {
                if *target >= context.region_len {
                    errors.push(format!("{prefix}: branch to invalid block {target}"));
                }
            }
            if let Some(found) = function.reg_types.get(&cond.0)
                && *found != Ty::Bool
            {
                errors.push(format!("{prefix}: branch condition has type {found}"));
            }
        }
        MirTerm::Return(value) | MirTerm::ReturnWithCleanup { value, .. } => {
            // `Return(None)` doubles as the lowering placeholder terminator, so
            // only value-carrying returns are checked.
            if let Some(register) = value
                && !function.returns_reference
                && let (Some(found), Some(expected)) = (
                    function.reg_types.get(&register.0),
                    function.ret_ty.as_ref(),
                )
                && !types_compatible(found, expected)
            {
                errors.push(format!(
                    "{prefix}: return of {found} from a function returning {expected}"
                ));
            }
            if let MirTerm::ReturnWithCleanup { cleanup, .. } = terminator {
                for variable in cleanup {
                    if *variable as usize >= function.n_vars {
                        errors.push(format!(
                            "{prefix}: return cleanup uses invalid variable {variable}"
                        ));
                    }
                }
            }
        }
        MirTerm::FallOff => {
            if !context.in_try_region {
                errors.push(format!("{prefix}: FallOff terminator outside a try region"));
            }
        }
        MirTerm::EscapeJump { target, .. } => {
            if !context.in_try_region {
                errors.push(format!(
                    "{prefix}: EscapeJump terminator outside a try region"
                ));
            }
            if *target >= context.function_len {
                errors.push(format!("{prefix}: escape to invalid block {target}"));
            }
        }
    }
}

fn instruction_places(instruction: &MirInstr) -> Vec<&MirPlace> {
    match instruction {
        MirInstr::EstablishLoans { loans, .. } => loans.iter().map(|loan| &loan.place).collect(),
        MirInstr::MakeRef { place, .. }
        | MirInstr::MovePlace { place, .. }
        | MirInstr::Store { place, .. }
        | MirInstr::StoreRef { place, .. }
        | MirInstr::LoadPlace { place, .. }
        | MirInstr::VariantSet { place, .. }
        | MirInstr::VariantReplace { place, .. }
        | MirInstr::ConsumePlace { place, .. } => vec![place],
        MirInstr::MakeClosure { captures, .. } => {
            captures.iter().map(|capture| &capture.place).collect()
        }
        MirInstr::Call {
            arg_places,
            kwarg_places,
            ..
        } => arg_places
            .iter()
            .flatten()
            .chain(kwarg_places.iter().flatten())
            .collect(),
        MirInstr::CallIndirect {
            callee_place,
            arg_places,
            kwarg_places,
            ..
        } => callee_place
            .iter()
            .chain(arg_places.iter().flatten())
            .chain(kwarg_places.iter().flatten())
            .collect(),
        MirInstr::MethodCall {
            recv_place,
            arg_places,
            kwarg_places,
            ..
        } => recv_place
            .iter()
            .chain(arg_places.iter().flatten())
            .chain(kwarg_places.iter().flatten())
            .collect(),
        MirInstr::Index {
            base_place,
            index_place,
            ..
        } => base_place.iter().chain(index_place.iter()).collect(),
        MirInstr::Slice {
            object_place,
            arg_places,
            ..
        }
        | MirInstr::MultiIndex {
            object_place,
            arg_places,
            ..
        } => object_place
            .iter()
            .chain(arg_places.iter().flatten())
            .collect(),
        MirInstr::MultiSet {
            receiver_place,
            arg_places,
            value_place,
            ..
        } => receiver_place
            .iter()
            .chain(arg_places.iter().flatten())
            .chain(value_place.iter())
            .collect(),
        _ => Vec::new(),
    }
}

fn verify_place(
    function_name: &str,
    block: usize,
    function: &MirFunction,
    declarations: &MirDeclarations,
    place: &MirPlace,
    executable: bool,
    errors: &mut Vec<String>,
) {
    let prefix = format!("MIR function '{function_name}' block {block}");
    if place.root as usize >= function.n_vars {
        errors.push(format!(
            "{prefix} place has invalid root slot {}",
            place.root
        ));
    } else if let Some(declared) = function.var_tys.get(&place.root)
        && let Some(recorded) = &place.root_ty
        && !types_compatible(recorded, declared)
    {
        errors.push(format!(
            "{prefix} place root slot {} is typed {recorded}, declared {declared}",
            place.root
        ));
    }
    if let Some(through) = place.through {
        if through as usize >= function.n_vars {
            errors.push(format!(
                "{prefix} place has invalid through-reference slot {through}"
            ));
        } else {
            let parameter_handle = (through as usize) < function.n_params
                && function
                    .ref_params
                    .get(through as usize)
                    .copied()
                    .unwrap_or(false);
            let receiver_handle = through == 0
                && declarations.functions.iter().any(|declaration| {
                    declaration.lowered_name == function_name
                        && declaration.has_receiver
                        && matches!(
                            declaration.receiver_convention,
                            Some(crate::ast::ArgConvention::Mut | crate::ast::ArgConvention::Ref)
                        )
                });
            let local_handle = match function.var_tys.get(&through) {
                Some(Ty::Ref(_)) => true,
                Some(Ty::Pointer { origin, .. }) => origin.as_origin().is_some(),
                _ => false,
            };
            if !parameter_handle && !receiver_handle && !local_handle {
                let declared = function
                    .var_tys
                    .get(&through)
                    .map(ToString::to_string)
                    .unwrap_or_else(|| "<untyped>".to_string());
                errors.push(format!(
                    "{prefix} place through slot {through} is not a checked reference-capability binding (declared {declared})"
                ));
            }
        }
    }
    if !place.is_typed() {
        errors.push(format!(
            "{prefix} place rooted at slot {} lacks complete checked type metadata",
            place.root
        ));
        return;
    }
    if place.root_ty.as_ref().is_some_and(contains_runtime_pack)
        || place.projection_tys.iter().any(contains_runtime_pack)
        || place.ty.as_ref().is_some_and(contains_runtime_pack)
    {
        errors.push(format!(
            "{prefix} place rooted at slot {} retains ABI-only RuntimePack type metadata",
            place.root
        ));
    }
    // A materialized reference-result place is physically rooted in a hidden
    // `ref T` slot, while its projections address the referent. Ordinary
    // substituted local aliases may instead keep an owned root plus analytical
    // `through` metadata, so only an actually reference-typed root is unwrapped.
    let mut current = match &place.root_ty {
        Some(Ty::Ref(reference))
            if place.through.is_some()
                && (!place.proj.is_empty() || place.ty.as_ref() != place.root_ty.as_ref()) =>
        {
            Some((*reference.referent).clone())
        }
        other => other.clone(),
    };
    for (projection, projected) in place.proj.iter().zip(&place.projection_tys) {
        match projection {
            Proj::Index(register) => {
                if register.0 >= function.n_regs {
                    errors.push(format!(
                        "{prefix} place index uses invalid register r{}",
                        register.0
                    ));
                } else if let Some(index_ty) = function.reg_types.get(&register.0)
                    && !types_compatible(index_ty, &Ty::Int)
                    && !matches!(index_ty, Ty::UInt)
                {
                    errors.push(format!(
                        "{prefix} place index register r{} has non-Indexer type {index_ty}",
                        register.0
                    ));
                }
                if let Some(base) = &current {
                    match indexed_place_element_types(base) {
                        Some(candidates)
                            if !candidates.iter().any(|candidate| {
                                types_compatible(projected, candidate)
                                    || matches!(candidate, Ty::Ref(reference) if types_compatible(projected, &reference.referent))
                            }) =>
                        {
                            errors.push(format!(
                                "{prefix} place dynamic element typed {projected}, incompatible with checked base type {base}"
                            ));
                        }
                        None if executable => errors.push(format!(
                            "{prefix} dynamic element projection requires checked indexed storage, got {base}"
                        )),
                        None => {}
                        Some(_) => {}
                    }
                }
            }
            Proj::ConstIndex(index) => match &current {
                Some(Ty::Tuple(elements)) => match elements.get(*index) {
                    None => errors.push(format!(
                        "{prefix} place projects Tuple element {index} out of {}",
                        elements.len()
                    )),
                    Some(element) if !types_compatible(projected, element) => errors.push(
                        format!(
                            "{prefix} place Tuple element {index} typed {projected}, declared {element}"
                        ),
                    ),
                    Some(_) => {}
                },
                Some(other) => errors.push(format!(
                    "{prefix} constant element projection requires compiler-private Tuple storage, got {other}"
                )),
                None => {}
            },
            Proj::Field(field) => {
                // A concrete non-generic struct's field projection must agree
                // with its declared layout; generic layouts would need
                // substitution the verifier deliberately does not re-derive.
                if let Some(Ty::Struct(struct_name, arguments)) = &current
                    && arguments.is_empty()
                    && let Some(declaration) = declarations
                        .structs
                        .iter()
                        .find(|declaration| &declaration.name == struct_name)
                {
                    match declaration
                        .fields
                        .iter()
                        .find(|(candidate, _)| candidate == field)
                    {
                        // A value parameter reads through field syntax; its
                        // declaration lives in `param_decls`, not the layout.
                        None if declaration
                            .param_decls
                            .iter()
                            .any(|decl| {
                                matches!(decl, crate::types::ParamDecl::Value { name, .. } if name == field)
                            }) => {}
                        None => errors.push(format!(
                            "{prefix} place projects unknown field '{field}' of '{struct_name}'"
                        )),
                        Some((_, declared)) if !types_compatible(projected, declared) => errors
                            .push(format!(
                                "{prefix} place field '{field}' of '{struct_name}' typed \
                                 {projected}, declared {declared}"
                            )),
                        Some(_) => {}
                    }
                }
            }
            Proj::Variant(index) => {
                if let Some(Ty::Variant(alternatives)) = &current
                    && *index >= alternatives.len()
                {
                    errors.push(format!(
                        "{prefix} place projects variant alternative {index} out of {}",
                        alternatives.len()
                    ));
                }
            }
        }
        current = Some(projected.clone());
    }
    if let (Some(current), Some(terminal)) = (&current, &place.ty)
        && !types_compatible(terminal, current)
    {
        errors.push(format!(
            "{prefix} place terminal type {terminal} does not match projected type {current}"
        ));
    }
}

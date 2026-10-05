//! The concrete verification mode: what elaborated MIR may no longer carry.
//!
//! Parametric MIR names type and value parameters and leaves a generic call
//! to be dispatched on the erased value; elaborated MIR is one instance per
//! body, so every type is closed and every call names its target. These
//! rules reject the parametric residue; the structural rules are shared.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use crate::mir::{Const, MirParamArg, MirSubscriptCall};

/// The concreteness findings for one function body.
///
/// A finding is a symbolic type in its signature, slots, registers, places,
/// or instructions, a compile-time parameter an instruction still declares,
/// or a compile-time argument a call still forwards or computes from one.
pub fn concrete_function_findings(name: &str, function: &MirFunction) -> Vec<String> {
    let mut errors = Vec::new();
    let head = format!("MIR function '{name}'");
    let signature = function
        .param_types
        .iter()
        .map(|ty| ("parameter", ty))
        .chain(function.ret_ty.iter().map(|ty| ("return", ty)))
        .chain(function.error_ty.iter().map(|ty| ("error", ty)));
    for (role, ty) in signature {
        require_concrete(&head, &format!("{role} type"), ty, &mut errors);
    }
    let mut slots: Vec<_> = function.var_tys.iter().collect();
    slots.sort_by_key(|(slot, _)| **slot);
    for (slot, ty) in slots {
        require_concrete(&head, &format!("variable slot {slot}"), ty, &mut errors);
    }
    let mut registers: Vec<_> = function.reg_types.iter().collect();
    registers.sort_by_key(|(register, _)| **register);
    for (register, ty) in registers {
        require_concrete(&head, &format!("register r{register}"), ty, &mut errors);
    }
    concrete_blocks(name, &function.blocks, &mut errors);
    errors
}

/// The program-wide concreteness findings: every function body, and every
/// declaration the bodies are checked against. A declaration's availability
/// clauses and conformance conditions are decided when it is instantiated,
/// so none survives.
pub(super) fn verify_concrete_program(program: &MirProgram, errors: &mut Vec<String>) {
    for (name, function) in &program.functions {
        errors.extend(concrete_function_findings(name, function));
    }
    for declaration in &program.declarations.functions {
        let head = format!("MIR function '{}' declaration", declaration.lowered_name);
        require_no_parameters(&head, &declaration.param_decls, errors);
        if !declaration.availability.is_empty() {
            errors.push(format!("{head} still carries an availability clause"));
        }
        let types = declaration
            .param_types
            .iter()
            .chain(declaration.variadic.iter())
            .chain(declaration.kw_variadic.iter())
            .chain(std::iter::once(&declaration.ret_ty))
            .chain(declaration.error_ty.iter());
        for ty in types {
            require_concrete(&head, "signature", ty, errors);
        }
    }
    for declaration in &program.declarations.structs {
        let head = format!("MIR struct '{}' declaration", declaration.name);
        require_no_parameters(&head, &declaration.param_decls, errors);
        for (field, ty) in &declaration.fields {
            require_concrete(&head, &format!("field `{field}`"), ty, errors);
        }
        errors.extend(
            declaration
                .conformances
                .iter()
                .filter(|row| {
                    row.conditions.iter().any(|condition| {
                        !matches!(condition, mojito_types::types::GenericConstraint::Bool(_))
                    })
                })
                .map(|row| {
                    format!(
                        "{head} still carries a condition on its `{}` conformance",
                        row.trait_name
                    )
                }),
        );
    }
}

/// Whether `ty` is a type only parametric MIR may name: one an instantiation
/// has yet to resolve, or a generic callable, which is a compile-time carrier
/// with no runtime ABI.
fn is_parametric(ty: &Ty) -> bool {
    mojito_types::types::is_symbolic(ty)
        || mojito_types::types::mentions_open(ty, &|inner| matches!(inner, Ty::GenericFunc { .. }))
}

fn require_concrete(head: &str, role: &str, ty: &Ty, errors: &mut Vec<String>) {
    if is_parametric(ty) {
        errors.push(format!(
            "{head} {role} keeps symbolic type `{ty}` in elaborated MIR"
        ));
    }
}

fn require_no_parameters(head: &str, parameters: &[ParamDecl], errors: &mut Vec<String>) {
    if let Some(parameter) = parameters.first() {
        errors.push(format!(
            "{head} keeps compile-time parameter `{}` in elaborated MIR",
            parameter.name()
        ));
    }
}

/// A compile-time argument slot is parametric while it names the enclosing
/// binder it forwards or the expression over value binders it computes. A
/// slot holding only a register is runtime data an unresolved call reads.
fn require_no_parameter_slots(head: &str, slots: &[MirParamArg], errors: &mut Vec<String>) {
    for slot in slots {
        if let Some(binder) = &slot.binder {
            errors.push(format!(
                "{head} keeps compile-time argument forwarding `{}` in elaborated MIR",
                binder.name
            ));
        } else if slot.expr.is_some() {
            errors.push(format!(
                "{head} keeps a compile-time argument expression in elaborated MIR"
            ));
        }
    }
}

fn concrete_blocks(name: &str, blocks: &[MirBlock], errors: &mut Vec<String>) {
    for (index, block) in blocks.iter().enumerate() {
        let head = format!("MIR function '{name}' block {index}");
        for instruction in &block.instrs {
            if let MirInstr::Try {
                body,
                handler,
                orelse,
                finalbody,
                ..
            } = instruction
            {
                let regions = std::iter::once(body)
                    .chain(handler.iter().map(|(_, blocks)| blocks))
                    .chain(orelse.iter())
                    .chain(finalbody.iter());
                for region in regions {
                    concrete_blocks(name, region, errors);
                }
                continue;
            }
            concrete_instruction(&head, instruction, errors);
        }
        match &block.term {
            MirTerm::ComptimeBranch { cond, .. } => errors.push(format!(
                "{head} keeps a compile-time branch on `{cond:?}` in elaborated MIR"
            )),
            MirTerm::ComptimeFor { index, .. } => errors.push(format!(
                "{head} keeps a compile-time loop over `{index}` in elaborated MIR"
            )),
            _ => {}
        }
    }
}

fn concrete_instruction(head: &str, instruction: &MirInstr, errors: &mut Vec<String>) {
    for place in instruction_places(instruction) {
        let types = place
            .root_ty
            .iter()
            .chain(&place.projection_tys)
            .chain(place.ty.iter());
        for ty in types {
            require_concrete(head, "place", ty, errors);
        }
    }
    match instruction {
        MirInstr::Rebind { .. } => {
            errors.push(format!("{head} keeps a value rebind in elaborated MIR"));
        }
        MirInstr::ConstructTypeParam { param, .. } => errors.push(format!(
            "{head} constructs compile-time parameter `{}` in elaborated MIR",
            param.name
        )),
        MirInstr::TypeName { ty, .. } => {
            errors.push(format!("{head} names the type `{ty}` in elaborated MIR"));
        }
        MirInstr::SizeOf { ty, .. } => {
            errors.push(format!(
                "{head} keeps the layout query of `{ty}` in elaborated MIR"
            ));
        }
        MirInstr::Const {
            k: Const::Param(value),
            ..
        } => errors.push(format!(
            "{head} keeps the parameter constant `{value}` in elaborated MIR"
        )),
        MirInstr::MakeSimd { dtype, width, .. }
        | MirInstr::SimdCast { dtype, width, .. }
        | MirInstr::SimdBitcast { dtype, width, .. }
            if dtype.is_symbolic() || width.is_symbolic() =>
        {
            errors.push(format!(
                "{head} keeps the symbolic vector slots `SIMD[{dtype}, {width}]` in elaborated MIR"
            ));
        }
        MirInstr::MaterializeLiteral { target: ty, .. }
        | MirInstr::PointerStorageTake { element: ty, .. }
        | MirInstr::PointerStorageDestroy { element: ty, .. }
        | MirInstr::UninitStorageTake { element: ty, .. }
        | MirInstr::UninitStorageDestroy { element: ty, .. }
        | MirInstr::TryNext { exhaustion: ty, .. }
        | MirInstr::DefVar {
            binding_ty: Some(ty),
            ..
        } => {
            require_concrete(head, "instruction", ty, errors);
        }
        MirInstr::Call {
            raises,
            param_arg_regs,
            receiver,
            instantiated_args,
            spread,
            ..
        } => {
            for ty in raises.iter().chain(receiver.iter()) {
                require_concrete(head, "instruction", ty, errors);
            }
            for argument in instantiated_args {
                if let TyArg::Ty(ty) = argument {
                    require_concrete(head, "instantiated argument", ty, errors);
                }
            }
            require_no_parameter_slots(head, param_arg_regs, errors);
            if spread.is_some() {
                errors.push(format!(
                    "{head} keeps a whole pack spread in elaborated MIR"
                ));
            }
        }
        MirInstr::MakeTuple { element_types, .. } => {
            for ty in element_types.iter().flatten() {
                require_concrete(head, "tuple element", ty, errors);
            }
        }
        MirInstr::MakeVariant { alternatives, .. } => {
            for ty in alternatives {
                require_concrete(head, "variant alternative", ty, errors);
            }
        }
        MirInstr::CallIndirect {
            raises,
            param_arg_regs,
            param_decls,
            instantiated_contract,
            instantiated_args,
            ..
        } => {
            for ty in raises.iter().chain(instantiated_contract.iter()) {
                require_concrete(head, "callable contract", ty, errors);
            }
            for argument in instantiated_args {
                if let TyArg::Ty(ty) = argument {
                    require_concrete(head, "callable argument", ty, errors);
                }
            }
            require_no_parameters(head, param_decls, errors);
            require_no_parameter_slots(head, param_arg_regs, errors);
        }
        MirInstr::MethodCall {
            raises,
            param_arg_regs,
            param_decls,
            instantiated_args,
            ..
        } => {
            if let Some(ty) = raises {
                require_concrete(head, "error contract", ty, errors);
            }
            for argument in instantiated_args {
                if let TyArg::Ty(ty) = argument {
                    require_concrete(head, "instantiated argument", ty, errors);
                }
            }
            require_no_parameters(head, param_decls, errors);
            require_no_parameter_slots(head, param_arg_regs, errors);
        }
        MirInstr::Index {
            call: Some(call), ..
        }
        | MirInstr::Slice {
            call: Some(call), ..
        }
        | MirInstr::MultiIndex {
            call: Some(call), ..
        }
        | MirInstr::MultiSet { call, .. } => concrete_subscript(head, call, errors),
        _ => {}
    }
}

fn concrete_subscript(head: &str, call: &MirSubscriptCall, errors: &mut Vec<String>) {
    for ty in call.raises.iter().chain(std::iter::once(&call.result_ty)) {
        require_concrete(head, "subscript contract", ty, errors);
    }
    require_no_parameters(head, &call.param_decls, errors);
    require_no_parameter_slots(head, &call.param_arg_regs, errors);
}

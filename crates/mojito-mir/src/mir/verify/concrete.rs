//! The concrete verification mode: what elaborated MIR may no longer carry.
//!
//! Parametric MIR names type and value parameters and leaves a generic call
//! to be dispatched on the erased value; elaborated MIR is one instance per
//! body, so every type is closed and every call names its target. These
//! rules reject the parametric residue; the structural rules are shared.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use crate::mir::text::instruction_mnemonic;
use crate::mir::{Const, MirParamArg, MirSubscriptCall, UseMode, VarId};
use mojito_types::mlir::storage_bits;
use mojito_types::types::{TyKind, has_no_llvm_type};

/// The concreteness findings for one function body.
///
/// A finding is a symbolic type in its signature, slots, registers, places,
/// or instructions, a compile-time parameter an instruction still declares,
/// a compile-time argument slot a call still carries, a reference slot read
/// as its handle, or compile-time control flow: a `comptime if`, a `comptime
/// for`, or a terminator of their regions. Runtime structured control flow
/// is concrete, as upstream's elaborator emits `hlcf`: the VM runs it as it
/// stands, and Pliron lowers it to LLVM blocks as `LowerControlFlow` does.
/// With the program's `declarations`, an LLVM intrinsic call's signature is
/// checked too, on the result type LLVM translation sees
/// ([`MirDeclarations::translated_type`]); the elaborator, which checks it
/// as it closes the call, passes none.
pub fn concrete_function_findings(
    name: &str,
    function: &MirFunction,
    declarations: Option<&MirDeclarations>,
) -> Vec<String> {
    let mut errors = Vec::new();
    let head = format!("MIR function '{name}'");
    let signature = function
        .param_types
        .iter()
        .map(|ty| ("parameter", ty))
        .chain(function.ret_ty.iter().map(|ty| ("return", ty)))
        .chain(function.error_ty.iter().map(|ty| ("error", ty)));
    for (role, ty) in signature {
        require_concrete_value(&head, &format!("{role} type"), ty, &mut errors);
    }
    let mut slots: Vec<_> = function.var_tys.iter().collect();
    slots.sort_by_key(|(slot, _)| **slot);
    for (slot, ty) in slots {
        require_concrete_value(&head, &format!("variable slot {slot}"), ty, &mut errors);
    }
    let mut registers: Vec<_> = function.reg_types.iter().collect();
    registers.sort_by_key(|(register, _)| **register);
    for (register, ty) in registers {
        require_concrete_value(&head, &format!("register r{register}"), ty, &mut errors);
    }
    concrete_blocks(name, function, declarations, &function.blocks, &mut errors);
    errors
}

/// What LLVM translation rejects in a folded function body.
///
/// The pin's verifier rejects it once upstream's pipeline has folded the
/// instance: an LLVM intrinsic call whose `immarg` index
/// ([`LlvmIntrinsic::index_is_immediate`]) no `const` defines ("immarg
/// operand has non-immediate parameter"), or whose index
/// [`LlvmIntrinsic::index_error`] refuses. Only a body the post-elaboration
/// pipeline has folded answers it: before `sroa`, `mem-2-reg`, and `sccp`
/// the index is still a pack element.
///
/// It also rejects what has no LLVM type ([`has_no_llvm_type`]) yet
/// survived the pipeline, as upstream's KGEN-to-LLVM lowering does: a
/// signature that mentions one ("failed to convert func signature"), a
/// call whose result does ("failed to convert call result type"), and any
/// other value of one.
///
/// [`has_no_llvm_type`]: mojito_types::types::has_no_llvm_type
/// [`LlvmIntrinsic::index_is_immediate`]: mojito_types::llvm_intrinsic::LlvmIntrinsic::index_is_immediate
/// [`LlvmIntrinsic::index_error`]: mojito_types::llvm_intrinsic::LlvmIntrinsic::index_error
pub fn llvm_translation_findings(function: &MirFunction) -> Vec<String> {
    let mut constants = HashMap::new();
    integer_constants(&function.blocks, &mut constants);
    let mut errors = Vec::new();
    untranslatable_values(function, &mut errors);
    translation_blocks(function, &function.blocks, &constants, &mut errors);
    errors
}

/// The program-wide concreteness findings: every function body, and every
/// declaration the bodies are checked against. A declaration's availability
/// clauses and conformance conditions are decided when it is instantiated,
/// and an instance has no enclosing binders, so none survives. A struct's
/// member table names only lifecycle members the program defines, and its
/// witness tables, which name template members, are gone.
pub(super) fn verify_concrete_program(program: &MirProgram, errors: &mut Vec<String>) {
    for (name, function) in &program.functions {
        errors.extend(concrete_function_findings(
            name,
            function,
            Some(&program.declarations),
        ));
    }
    for declaration in &program.declarations.functions {
        let head = format!("MIR function '{}' declaration", declaration.lowered_name);
        require_no_parameters(&head, &declaration.param_decls, errors);
        if !declaration.availability.is_empty() {
            errors.push(format!("{head} still carries an availability clause"));
        }
        if let Some(enclosing) = &declaration.enclosing {
            errors.push(format!(
                "{head} is still nested in `{enclosing}`, whose binders it may name"
            ));
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
    let defined: std::collections::HashSet<&str> = program
        .functions
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    for declaration in &program.declarations.structs {
        let head = format!("MIR struct '{}' declaration", declaration.name);
        require_no_parameters(&head, &declaration.param_decls, errors);
        for (field, ty) in &declaration.fields {
            require_concrete(&head, &format!("field `{field}`"), ty, errors);
        }
        if !declaration.witness_tables.is_empty() {
            errors.push(format!(
                "{head} still carries witness tables, which elaborated MIR carries none of"
            ));
        }
        for (position, entry) in declaration.members.iter().enumerate() {
            let lifecycle = crate::mir::LIFECYCLE_MEMBERS.contains(&entry.member.as_str());
            if crate::mir::table_member(&entry.member).is_none()
                && !program
                    .declarations
                    .reached_from_type(&declaration.name, &entry.member)
            {
                errors.push(format!(
                    "{head} names `{}` as its member `{}`, which no backend reaches from its type",
                    entry.symbol, entry.member
                ));
            } else if lifecycle
                && declaration.members[..position]
                    .iter()
                    .any(|known| known.member == entry.member)
            {
                errors.push(format!(
                    "{head} names `{}` as a second `{}`",
                    entry.symbol, entry.member
                ));
            } else if !defined.contains(entry.symbol.as_str()) {
                errors.push(format!(
                    "{head} names `{}` as its `{}`, which the program does not define",
                    entry.symbol, entry.member
                ));
            }
        }
        errors.extend(
            declaration
                .conformances
                .iter()
                .chain(&declaration.declared)
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

/// [`require_concrete`] for the type of a value — a parameter, a result, a
/// variable slot, or a register — which a generic callable is not: it is a
/// compile-time carrier with no runtime ABI, though a parametric function
/// type closed over its own binders is a concrete type argument
/// (`OptionalReg[G]`, a `Pointer[G]` element, an instruction's operand).
fn require_concrete_value(head: &str, role: &str, ty: &Ty, errors: &mut Vec<String>) {
    if matches!(ty.kind(), TyKind::GenericFunc(_)) {
        errors.push(format!(
            "{head} {role} keeps symbolic type `{ty}` in elaborated MIR"
        ));
        return;
    }
    require_concrete(head, role, ty, errors);
}

/// Report `ty` where it is a type only parametric MIR may name — one an
/// instantiation has yet to resolve
/// ([`mojito_types::types::is_symbolic`]) — or a compile-time-only one.
fn require_concrete(head: &str, role: &str, ty: &Ty, errors: &mut Vec<String>) {
    if mojito_types::types::is_symbolic(ty) {
        errors.push(format!(
            "{head} {role} keeps symbolic type `{ty}` in elaborated MIR"
        ));
    }
    // A raw parameter list is compile-time data: no run-time value holds one.
    if mojito_types::types::mentions(ty, &|ty| matches!(ty.kind(), TyKind::ParamList(_))) {
        errors.push(format!(
            "{head} {role} holds the compile-time-only raw parameter list type `{ty}`"
        ));
    }
    // A trait, a type, or a generator is a compile-time value: no run-time
    // value holds one. An attribute or a KGEN string is not in that list:
    // elaborated MIR may hold one, and LLVM translation decides.
    if mojito_types::types::mentions(ty, &|ty| {
        matches!(
            ty.kind(),
            TyKind::AnyTrait(_) | TyKind::Trait(_) | TyKind::TypeType | TyKind::Generator(_)
        )
    }) {
        errors.push(format!(
            "{head} {role} holds the compile-time-only metatype `{ty}`"
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

/// A concrete call has no parameter list: its compile-time arguments are
/// attributes its instantiated arguments carry, and none has a run-time
/// register.
fn require_no_parameter_slots(head: &str, slots: &[MirParamArg], errors: &mut Vec<String>) {
    if !slots.is_empty() {
        errors.push(format!(
            "{head} keeps compile-time argument slots in elaborated MIR"
        ));
    }
}

fn concrete_blocks(
    name: &str,
    function: &MirFunction,
    declarations: Option<&MirDeclarations>,
    blocks: &[MirBlock],
    errors: &mut Vec<String>,
) {
    for (index, block) in blocks.iter().enumerate() {
        let head = format!("MIR function '{name}' block {index}");
        for instruction in &block.instrs {
            let regions = crate::mir::instruction_regions(instruction);
            if !regions.is_empty() {
                match instruction {
                    MirInstr::ComptimeIf { conds, .. } => errors.push(format!(
                        "{head} keeps a compile-time branch on `{conds:?}` in elaborated MIR"
                    )),
                    MirInstr::ComptimeFor { binder, .. } => errors.push(format!(
                        "{head} keeps a compile-time loop over `{binder}` in elaborated MIR"
                    )),
                    _ => {}
                }
                for region in regions {
                    concrete_blocks(name, function, declarations, region, errors);
                }
                continue;
            }
            concrete_instruction(&head, instruction, errors);
            reference_read(&head, function, instruction, errors);
            bitcast_width(&head, function, instruction, errors);
            concrete_llvm_intrinsic(&head, function, declarations, instruction, errors);
        }
        if matches!(
            block.term,
            MirTerm::ComptimeYield(_) | MirTerm::ComptimeBreak(_) | MirTerm::ComptimeContinue(_)
        ) {
            errors.push(format!(
                "{head} keeps the region terminator `{}` in elaborated MIR",
                crate::mir::text::terminator_mnemonic(&block.term)
            ));
        }
    }
}

fn require_known_alternative(
    head: &str,
    index: &mojito_types::types::VariantIndex,
    errors: &mut Vec<String>,
) {
    if index.is_symbolic() {
        errors.push(format!(
            "{head} keeps the symbolic Variant alternative `{index}` in elaborated MIR"
        ));
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
        for projection in &place.proj {
            match projection {
                Proj::Variant(index) => require_known_alternative(head, index, errors),
                Proj::Rebind => {
                    errors.push(format!("{head} keeps a rebound place in elaborated MIR"));
                }
                _ => {}
            }
        }
    }
    for index in instruction_variant_indices(instruction) {
        require_known_alternative(head, index, errors);
    }
    match instruction {
        MirInstr::Rebind { .. } => {
            errors.push(format!("{head} keeps a value rebind in elaborated MIR"));
        }
        MirInstr::ComptimeAssert { cond, .. } => {
            errors.push(format!(
                "{head} keeps a compile-time assertion of `{cond:?}` in elaborated MIR"
            ));
        }
        MirInstr::KeepAlive { .. }
        | MirInstr::EstablishLoans { .. }
        | MirInstr::InvalidateInteriors { .. }
        | MirInstr::MarkInitialized { .. }
        | MirInstr::MarkDestroyed { .. } => {
            errors.push(format!(
                "{head} keeps the lifetime marker `{}` in elaborated MIR",
                instruction_mnemonic(instruction)
            ));
        }
        MirInstr::DropVar {
            destructor: Some(destructor),
            ..
        }
        | MirInstr::Drop {
            destructor: Some(destructor),
            ..
        }
        | MirInstr::DropPlace {
            destructor: Some(destructor),
            ..
        } => errors.push(format!(
            "{head} keeps the drop `{}` naming `{destructor}` in elaborated MIR",
            instruction_mnemonic(instruction)
        )),
        MirInstr::ConstructTypeParam { param, .. } => errors.push(format!(
            "{head} constructs compile-time parameter `{}` in elaborated MIR",
            param.name
        )),
        MirInstr::TypeName { ty, .. } => {
            errors.push(format!("{head} names the type `{ty}` in elaborated MIR"));
        }
        MirInstr::ConstructType { ty, .. } => {
            errors.push(format!(
                "{head} constructs the type `{ty}` in elaborated MIR"
            ));
        }
        MirInstr::Const {
            k: Const::Param(value),
            ..
        }
        | MirInstr::ParamListAddress {
            values: Const::Param(value),
            ..
        } => errors.push(format!(
            "{head} keeps the parameter constant `{value}` in elaborated MIR"
        )),
        MirInstr::Const {
            k: Const::FunctionInstance { function, .. },
            ..
        } => errors.push(format!(
            "{head} keeps the unelaborated instance of `{function}` in elaborated MIR"
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
        MirInstr::ArrayGet {
            index: Const::Param(index),
            ..
        }
        | MirInstr::ArrayReplace {
            index: Const::Param(index),
            ..
        } => errors.push(format!(
            "{head} keeps the parameter array index `{index}` in elaborated MIR"
        )),
        MirInstr::SimdShuffle { mask, .. } if mask.is_symbolic() => {
            errors.push(format!(
                "{head} keeps the symbolic lane mask `{mask}` in elaborated MIR"
            ));
        }
        MirInstr::MaterializeLiteral { target: ty, .. }
        | MirInstr::PointerStorageTake { element: ty, .. }
        | MirInstr::PointerStorageDestroy { element: ty, .. }
        | MirInstr::UninitStorageTake { element: ty, .. }
        | MirInstr::UninitStorageDestroy { element: ty, .. }
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
            ..
        } => {
            for ty in raises
                .iter()
                .map(|raises| &raises.ty)
                .chain(receiver.iter())
            {
                require_concrete(head, "instruction", ty, errors);
            }
            for argument in instantiated_args {
                if let TyArg::Ty(ty) = argument {
                    require_concrete(head, "instantiated argument", ty, errors);
                }
            }
            require_no_parameter_slots(head, param_arg_regs, errors);
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
            for ty in raises
                .iter()
                .map(|raises| &raises.ty)
                .chain(instantiated_contract.iter())
            {
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
            if let Some(raises) = raises {
                require_concrete(head, "error contract", &raises.ty, errors);
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
        | MirInstr::MultiIndex {
            call: Some(call), ..
        }
        | MirInstr::MultiSet { call, .. } => concrete_subscript(head, call, errors),
        _ => {}
    }
}

fn concrete_subscript(head: &str, call: &MirSubscriptCall, errors: &mut Vec<String>) {
    for ty in call
        .raises
        .iter()
        .map(|raises| &raises.ty)
        .chain(std::iter::once(&call.result_ty))
    {
        require_concrete(head, "subscript contract", ty, errors);
    }
    require_no_parameters(head, &call.param_decls, errors);
    require_no_parameter_slots(head, &call.param_arg_regs, errors);
}

/// A reference slot is read by a whole-root `LoadPlace`, which yields its
/// referent, as lowering types it and both backends execute it. An instance
/// keeps the generic register types, so a load typed as the handle, or a
/// `UseVar` copy of the handle, is a type re-derived from the slot rather
/// than substituted. A referent that is itself a reference loads a handle
/// legitimately; a move transfers the handle and a borrow addresses the
/// slot, so neither is a read.
fn reference_read(
    head: &str,
    function: &MirFunction,
    instruction: &MirInstr,
    errors: &mut Vec<String>,
) {
    let referent_of = |slot: &VarId| {
        function.var_tys.get(slot).and_then(|ty| match ty.kind() {
            TyKind::Ref(reference) => Some(reference.referent.clone()),
            _ => None,
        })
    };
    match instruction {
        MirInstr::LoadPlace { dest, place }
            if place.proj.is_empty()
                && referent_of(&place.root)
                    .is_some_and(|referent| !matches!(referent.kind(), TyKind::Ref(_)))
                && function
                    .reg_types
                    .get(&dest.0)
                    .is_some_and(|ty| matches!(ty.kind(), TyKind::Ref(_))) =>
        {
            errors.push(format!(
                "{head} loads the handle of reference slot ${} instead of its referent",
                place.root
            ));
        }
        MirInstr::UseVar {
            var,
            mode: UseMode::Copy,
            ..
        } if referent_of(var).is_some() => errors.push(format!(
            "{head} copies reference slot ${var} by `UseVar`; a reference binding is read by `LoadPlace`"
        )),
        _ => {}
    }
}

/// `pop.bitcast`'s verifier over a closed `simd.bitcast`: the bits are
/// kept. It runs on elaborated MIR, after the instance's compile-time
/// assertions (`bitcast`'s own among them) are judged, as the pin verifies
/// the op after instantiation.
fn bitcast_width(
    head: &str,
    function: &MirFunction,
    instruction: &MirInstr,
    errors: &mut Vec<String>,
) {
    let MirInstr::SimdBitcast {
        value,
        dtype,
        width,
        ..
    } = instruction
    else {
        return;
    };
    let source = function
        .reg_types
        .get(&value.0)
        .and_then(|ty| mojito_types::types::held_simd_slots(ty.kind()));
    if let (Some(dtype), Some(width), Some((source_dtype, source_width))) =
        (dtype.known(), width.known(), source)
        && let (Some(source_dtype), Some(source_width)) =
            (source_dtype.known(), source_width.known())
        && storage_bits(source_dtype, source_width) != storage_bits(dtype, width)
    {
        errors.push(format!(
            "{head}: 'pop.bitcast' op input type `SIMD[{}, {source_width}]` and result type \
             `SIMD[{}, {width}]` are cast incompatible",
            source_dtype.name(),
            dtype.name()
        ));
    }
}

/// An LLVM intrinsic call as elaborated MIR carries it: its name and
/// side-effect flag closed, the name in the table, no pack spread, and,
/// given the program's `declarations`, its register types a signature the
/// intrinsic takes, as LLVM translation checks a reached call.
fn concrete_llvm_intrinsic(
    head: &str,
    function: &MirFunction,
    declarations: Option<&MirDeclarations>,
    instruction: &MirInstr,
    errors: &mut Vec<String>,
) {
    let MirInstr::CallLlvmIntrinsic {
        dest,
        intrinsic,
        has_side_effect,
        args,
        spread,
    } = instruction
    else {
        return;
    };
    if spread.is_some() {
        errors.push(format!(
            "{head} keeps an LLVM intrinsic call's pack spread in elaborated MIR"
        ));
    }
    if !matches!(
        has_side_effect,
        Const::Value(mojito_types::ct::CtValue::Bool(_))
    ) {
        errors.push(format!(
            "{head} keeps the side-effect flag `{has_side_effect:?}` of an LLVM intrinsic call in \
             elaborated MIR"
        ));
    }
    let Some(called) = intrinsic.llvm_intrinsic() else {
        errors.push(format!(
            "{head} calls `{intrinsic:?}`, which is no LLVM intrinsic Mojito computes"
        ));
        return;
    };
    let ty = |reg: &Reg| function.reg_types.get(&reg.0).cloned();
    if let Some(declarations) = declarations
        && let Some(result) = ty(dest)
        && let Some(operands) = args.iter().map(ty).collect::<Option<Vec<_>>>()
        && let Some(reason) =
            called.signature_error(&declarations.translated_type(&result), &operands)
    {
        errors.push(format!("{head}: {reason}"));
    }
}

/// [`llvm_translation_findings`] over `blocks` and the regions they hold.
fn untranslatable_values(function: &MirFunction, errors: &mut Vec<String>) {
    let untranslatable =
        |ty: &Ty| mojito_types::types::mentions(ty, &|ty| has_no_llvm_type(ty.kind()));
    if function
        .param_types
        .iter()
        .chain(function.ret_ty.iter())
        .any(untranslatable)
    {
        errors.push("failed to convert func signature".to_string());
    }
    let mut call_results = HashMap::new();
    for instruction in crate::mir::nested_instructions(&function.blocks) {
        match instruction {
            MirInstr::Call { dest, func, .. } => {
                call_results.insert(dest.0, format!("'{}'", func.0));
            }
            MirInstr::CallIndirect { dest, .. } => {
                call_results.insert(dest.0, "indirect call".to_string());
            }
            _ => {}
        }
    }
    let mut defined = Vec::new();
    crate::mir::defined_registers(&function.blocks, &mut defined);
    for reg in defined {
        let Some(ty) = function
            .reg_types
            .get(&reg.0)
            .filter(|ty| untranslatable(ty))
        else {
            continue;
        };
        errors.push(match call_results.get(&reg.0) {
            Some(callee) => format!("{callee}: failed to convert call result type"),
            None => format!(
                "failed to legalize operation defining %r{} of type `{ty}`",
                reg.0
            ),
        });
    }
    for (var, ty) in &function.var_tys {
        if untranslatable(ty) {
            errors.push(format!("failed to legalize slot {var} of type `{ty}`"));
        }
    }
}

fn translation_blocks(
    function: &MirFunction,
    blocks: &[MirBlock],
    constants: &HashMap<u32, i128>,
    errors: &mut Vec<String>,
) {
    for block in blocks {
        for instruction in &block.instrs {
            for region in crate::mir::instruction_regions(instruction) {
                translation_blocks(function, region, constants, errors);
            }
            let MirInstr::CallLlvmIntrinsic {
                dest,
                intrinsic,
                args,
                ..
            } = instruction
            else {
                continue;
            };
            let Some((called, position)) = intrinsic.llvm_intrinsic().and_then(|called| {
                called
                    .index_operand()
                    .map(|(position, _)| (called, position))
            }) else {
                continue;
            };
            let head = format!("'{}'", called.name());
            let Some(index) = args
                .get(position)
                .and_then(|reg| constants.get(&reg.0).copied())
            else {
                if called.index_is_immediate() {
                    errors.push(format!(
                        "{head}: immarg operand has non-immediate parameter"
                    ));
                }
                continue;
            };
            let width = |reg: &Reg| {
                function
                    .reg_types
                    .get(&reg.0)
                    .and_then(|ty| mojito_types::types::held_simd_slots(ty.kind()))
                    .and_then(|(_, width)| width.known())
                    .and_then(|width| usize::try_from(width).ok())
            };
            let widths = args
                .iter()
                .enumerate()
                .filter(|(operand, _)| *operand != position)
                .map(|(_, reg)| width(reg))
                .collect::<Option<Vec<_>>>();
            if let (Some(result), Some(widths)) = (width(dest), widths)
                && let Some(reason) = called.index_error(result, &widths, index)
            {
                errors.push(format!("{head}: {reason}"));
            }
        }
    }
}

/// Record in `constants` the integer each `const` of one integer lane in
/// `blocks` and the regions they hold defines, by register number.
fn integer_constants(blocks: &[MirBlock], constants: &mut HashMap<u32, i128>) {
    for block in blocks {
        for instruction in &block.instrs {
            for region in crate::mir::instruction_regions(instruction) {
                integer_constants(region, constants);
            }
            let MirInstr::Const { dest, k } = instruction else {
                continue;
            };
            let value = match k {
                Const::Int(value) => Some(i128::from(*value)),
                Const::Value(mojito_types::ct::CtValue::Simd { lanes, .. }) => {
                    match lanes.as_slice() {
                        [mojito_types::ct::CtLane::Int(value)] => Some(*value),
                        _ => None,
                    }
                }
                Const::Value(mojito_types::ct::CtValue::UInt(value)) => Some(i128::from(*value)),
                _ => None,
            };
            constants.extend(value.map(|value| (dest.0, value)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::{MirPlace, MirTerm, Reg};
    use mojito_types::origin::{Mutability, Origin, RefTy};

    fn ref_int() -> Ty {
        TyKind::Ref(RefTy {
            referent: Ty::int(),
            origin: Origin::Untracked { mutable: true },
            mutability: Mutability::Mutable,
        })
        .into()
    }

    fn function_with(instruction: MirInstr, dest_ty: Ty) -> MirFunction {
        MirFunction {
            blocks: vec![MirBlock {
                instrs: vec![instruction],
                term: MirTerm::TryYield(Vec::new()),
            }],
            n_regs: 1,
            n_vars: 1,
            var_names: vec!["r".to_string()],
            n_params: 0,
            param_types: Vec::new(),
            owned_params: Vec::new(),
            deinit_params: Vec::new(),
            ref_params: Vec::new(),
            returns_reference: false,
            var_tys: HashMap::from([(0, ref_int())]),
            ret_ty: Some(Ty::none()),
            raises: false,
            error_ty: None,
            error_slot: None,
            spans: crate::mir::SpanTable(HashMap::new()),
            reg_types: HashMap::from([(0, dest_ty)]),
            removed_params: Vec::new(),
            callee_removed_params: Vec::new(),
            callee_removed_args: Vec::new(),
            removed_args: Vec::new(),
            lowered_calls: Vec::new(),
        }
    }

    fn load(dest_ty: Ty) -> Vec<String> {
        let instruction = MirInstr::LoadPlace {
            dest: Reg(0),
            place: MirPlace::root(0, Some(ref_int())),
        };
        concrete_function_findings("f", &function_with(instruction, dest_ty), None)
    }

    fn use_var(mode: UseMode) -> Vec<String> {
        let instruction = MirInstr::UseVar {
            dest: Reg(0),
            var: 0,
            mode,
            constructor: None,
        };
        concrete_function_findings("f", &function_with(instruction, ref_int()), None)
    }

    #[test]
    fn whole_root_load_of_reference_slot_reads_referent() {
        assert!(load(Ty::int()).is_empty());
        let findings = load(ref_int());
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert!(findings[0].contains("loads the handle of reference slot $0"));
    }

    #[test]
    fn lifetime_markers_are_refused_in_elaborated_mir() {
        let markers = [
            MirInstr::KeepAlive { var: 0 },
            MirInstr::EstablishLoans {
                reference: 0,
                loans: Vec::new(),
                marker: Reg(0),
                dest_interior: None,
            },
            MirInstr::InvalidateInteriors {
                base: crate::mir::MirInteriorOrigin {
                    root: 0,
                    path: Vec::new(),
                },
                except: None,
                include_base_generation: false,
                marker: Reg(0),
            },
            MirInstr::MarkInitialized {
                place: crate::mir::MirPlace::root(0, None),
            },
            MirInstr::MarkDestroyed {
                place: crate::mir::MirPlace::root(0, None),
            },
        ];
        for (marker, mnemonic) in markers.into_iter().zip([
            "lifetime.keep_alive",
            "loans.establish",
            "interiors.invalidate",
            "ownership.mark_initialized",
            "ownership.mark_destroyed",
        ]) {
            let findings =
                concrete_function_findings("f", &function_with(marker, Ty::none()), None);
            assert_eq!(findings.len(), 1, "{findings:?}");
            assert!(findings[0].contains(&format!("keeps the lifetime marker `{mnemonic}`")));
        }
    }

    #[test]
    fn reference_slot_is_never_copied_by_use_var() {
        let findings = use_var(UseMode::Copy);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert!(findings[0].contains("copies reference slot $0 by `UseVar`"));
        assert!(use_var(UseMode::Move).is_empty());
    }

    #[test]
    fn runtime_structure_is_concrete() {
        let arm = |term| {
            vec![MirBlock {
                instrs: Vec::new(),
                term,
            }]
        };
        let function = function_with(
            MirInstr::If {
                results: Vec::new(),
                cond: Reg(0),
                then: arm(MirTerm::Yield(Vec::new())),
                elifs: Vec::new(),
                else_args: Vec::new(),
                orelse: arm(MirTerm::Yield(Vec::new())),
            },
            Ty::bool(),
        );
        assert!(concrete_function_findings("f", &function, None).is_empty());
    }

    fn attribute_constant(dest_ty: Ty) -> MirFunction {
        let instruction = MirInstr::Const {
            dest: Reg(0),
            k: Const::Value(CtValue::MlirAttr("#pop.fmf<fast>".to_string())),
        };
        function_with(instruction, dest_ty)
    }

    #[test]
    fn attribute_valued_function_is_concrete_but_not_translatable() {
        let mut function = attribute_constant(TyKind::KgenDeferred.into());
        function.ret_ty = Some(TyKind::KgenDeferred.into());
        let findings = concrete_function_findings("attr", &function, None);
        assert!(findings.is_empty(), "{findings:?}");
        let findings = llvm_translation_findings(&function);
        assert!(
            findings.contains(&"failed to convert func signature".to_string()),
            "{findings:?}"
        );
    }

    #[test]
    fn surviving_attribute_value_is_not_translatable() {
        let findings = llvm_translation_findings(&attribute_constant(TyKind::KgenDeferred.into()));
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert!(findings[0].contains("failed to legalize operation defining %r0"));
    }

    #[test]
    fn attribute_constant_is_read_at_an_attribute_type() {
        let declarations = MirDeclarations::default();
        let findings =
            super::super::verify_function_in("f", &attribute_constant(Ty::int()), &declarations);
        assert!(
            findings
                .iter()
                .any(|finding| finding.contains("attribute constant is read at `Int`")),
            "{findings:?}"
        );
        let findings = super::super::verify_function_in(
            "f",
            &attribute_constant(TyKind::KgenDeferred.into()),
            &declarations,
        );
        assert!(
            !findings
                .iter()
                .any(|finding| finding.contains("attribute constant")),
            "{findings:?}"
        );
    }
}

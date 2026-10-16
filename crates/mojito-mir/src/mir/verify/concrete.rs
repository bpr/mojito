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
use mojito_types::types::TyKind;

/// The concreteness findings for one function body.
///
/// A finding is a symbolic type in its signature, slots, registers, places,
/// or instructions, a compile-time parameter an instruction still declares,
/// a compile-time argument slot a call still carries, or a reference slot
/// read as its handle.
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
    concrete_blocks(name, function, &function.blocks, &mut errors);
    errors
}

/// The program-wide concreteness findings: every function body, and every
/// declaration the bodies are checked against. A declaration's availability
/// clauses and conformance conditions are decided when it is instantiated,
/// and an instance has no enclosing binders, so none survives. A struct's
/// member table names only lifecycle members the program defines.
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
        if declaration.defaults.iter().flatten().any(|default| {
            matches!(default, mojito_checked::checked::CheckedConst::Evaluate { arguments, .. } if !arguments.is_empty())
        }) {
            errors.push(format!(
                "{head} still applies an evaluated default to compile-time arguments"
            ));
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

/// Whether `ty` is a type only parametric MIR may name: one an instantiation
/// has yet to resolve, or a generic callable, which is a compile-time carrier
/// with no runtime ABI.
fn is_parametric(ty: &Ty) -> bool {
    mojito_types::types::is_symbolic(ty)
        || mojito_types::types::mentions_open(ty, &|inner| {
            matches!(inner.kind(), TyKind::GenericFunc(_))
        })
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
    blocks: &[MirBlock],
    errors: &mut Vec<String>,
) {
    for (index, block) in blocks.iter().enumerate() {
        let head = format!("MIR function '{name}' block {index}");
        for instruction in &block.instrs {
            let regions = crate::mir::instruction_regions(instruction);
            if !regions.is_empty() {
                if !matches!(instruction, MirInstr::Try { .. }) {
                    errors.push(format!(
                        "{head} keeps structured control flow `{}` in elaborated MIR",
                        crate::mir::text::instruction_mnemonic(instruction)
                    ));
                }
                for region in regions {
                    concrete_blocks(name, function, region, errors);
                }
                continue;
            }
            concrete_instruction(&head, instruction, errors);
            reference_read(&head, function, instruction, errors);
        }
        match &block.term {
            MirTerm::ComptimeBranch { cond, .. } => errors.push(format!(
                "{head} keeps a compile-time branch on `{cond:?}` in elaborated MIR"
            )),
            MirTerm::ComptimeFor { binder, .. } => errors.push(format!(
                "{head} keeps a compile-time loop over `{binder}` in elaborated MIR"
            )),
            MirTerm::Yield(_)
            | MirTerm::ElifCond(_)
            | MirTerm::Break { .. }
            | MirTerm::Continue { .. }
            | MirTerm::ComptimeYield(_)
            | MirTerm::ComptimeBreak
            | MirTerm::ComptimeContinue
            | MirTerm::Unreachable => errors.push(format!(
                "{head} keeps the region terminator `{}` in elaborated MIR",
                crate::mir::text::terminator_mnemonic(&block.term)
            )),
            _ => {}
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
        MirInstr::KeepAlive { .. } | MirInstr::InvalidateInteriors { .. } => {
            errors.push(format!(
                "{head} keeps the lifetime marker `{}` in elaborated MIR",
                instruction_mnemonic(instruction)
            ));
        }
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
        MirInstr::SizeOf { ty, .. } => {
            errors.push(format!(
                "{head} keeps the layout query of `{ty}` in elaborated MIR"
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
        MirInstr::MakeSimd { dtype, width, .. }
        | MirInstr::SimdCast { dtype, width, .. }
        | MirInstr::SimdBitcast { dtype, width, .. }
            if dtype.is_symbolic() || width.is_symbolic() =>
        {
            errors.push(format!(
                "{head} keeps the symbolic vector slots `SIMD[{dtype}, {width}]` in elaborated MIR"
            ));
        }
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
            spread,
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
            if spread.is_some() {
                errors.push(format!(
                    "{head} keeps a whole pack spread in elaborated MIR"
                ));
            }
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
                term: MirTerm::TryYield,
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
            spans: crate::mir::SpanTable(HashMap::new()),
            reg_types: HashMap::from([(0, dest_ty)]),
            removed_params: Vec::new(),
            callee_removed_params: Vec::new(),
            removed_args: Vec::new(),
        }
    }

    fn load(dest_ty: Ty) -> Vec<String> {
        let instruction = MirInstr::LoadPlace {
            dest: Reg(0),
            place: MirPlace::root(0, Some(ref_int())),
        };
        concrete_function_findings("f", &function_with(instruction, dest_ty))
    }

    fn use_var(mode: UseMode) -> Vec<String> {
        let instruction = MirInstr::UseVar {
            dest: Reg(0),
            var: 0,
            mode,
        };
        concrete_function_findings("f", &function_with(instruction, ref_int()))
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
            MirInstr::InvalidateInteriors {
                base: crate::mir::MirInteriorOrigin {
                    root: 0,
                    path: Vec::new(),
                },
                except: None,
                include_base_generation: false,
                marker: Reg(0),
            },
        ];
        for (marker, mnemonic) in markers
            .into_iter()
            .zip(["lifetime.keep_alive", "interiors.invalidate"])
        {
            let findings = concrete_function_findings("f", &function_with(marker, Ty::none()));
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
}

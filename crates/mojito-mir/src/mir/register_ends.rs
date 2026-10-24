//! Owned registers and the operands that end them.
//!
//! Upstream's `CheckLifetimes` tracks an owned SSA value beside memory
//! values in one `ValueSet`, one bit each: the operation defining it
//! initializes it, and an operand that takes ownership (`var`/`deinit`
//! argument, a store, an aggregate's element) consumes it, as does its
//! destructor call. This module answers both questions for MIR: which
//! results an instruction defines as fresh owned values
//! ([`owned_result_regs`]) and which operands it takes ownership of
//! ([`consumed_operand_regs`], [`consumed_terminator_regs`]). Arguments
//! bind to parameters through [`CallArguments::bindings`] and a callee's
//! [`ParameterContract`], the one binder the register-loan analysis reads
//! too.

use super::{
    MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr, MirPlace, MirTerm, Reg,
    TraitRequirement,
};
use mojito_ast::ast::ArgConvention;
use mojito_types::types::{FuncTy, Ty, TyKind};

/// The parameters a call binds its arguments to, as a declaration, a
/// requirement, or a function type spells them.
pub struct ParameterContract<'c> {
    pub names: &'c [String],
    pub types: &'c [Ty],
    pub conventions: &'c [Option<ArgConvention>],
    /// The first positional argument the variadic collector takes.
    pub collector: Option<usize>,
    /// The convention the variadic collector takes its arguments by.
    pub collector_convention: Option<ArgConvention>,
}

impl<'c> From<&'c MirFunctionDeclaration> for ParameterContract<'c> {
    fn from(declaration: &'c MirFunctionDeclaration) -> Self {
        Self {
            names: &declaration.param_names,
            types: &declaration.param_types,
            conventions: &declaration.param_conventions,
            collector: declaration.variadic_index,
            collector_convention: declaration.variadic_convention,
        }
    }
}

impl<'c> From<&'c TraitRequirement> for ParameterContract<'c> {
    fn from(requirement: &'c TraitRequirement) -> Self {
        Self {
            names: &requirement.param_names,
            types: &requirement.param_types,
            conventions: &requirement.param_conventions,
            collector: requirement.variadic_index,
            collector_convention: requirement.variadic_convention,
        }
    }
}

impl<'c> From<&'c FuncTy> for ParameterContract<'c> {
    fn from(function: &'c FuncTy) -> Self {
        let collector = function
            .variadic
            .as_ref()
            .map(|_| function.keyword_only.unwrap_or(function.params.len()));
        Self {
            names: &function.names,
            types: &function.params,
            conventions: &function.conventions,
            collector,
            collector_convention: collector
                .and_then(|slot| function.conventions.get(slot).copied().flatten()),
        }
    }
}

impl ParameterContract<'_> {
    /// Whether parameter `slot` takes ownership of its argument: a `var` or
    /// `deinit` convention, upstream's owned argument conventions.
    pub fn consumes(&self, slot: usize) -> bool {
        owned_convention(self.conventions.get(slot).copied().flatten())
    }

    /// Whether the argument `binding` binds is one its parameter takes
    /// ownership of.
    pub fn consumes_binding(&self, binding: ArgumentBinding) -> bool {
        match binding {
            ArgumentBinding::Slot(slot) => self.consumes(slot),
            ArgumentBinding::Collector => owned_convention(self.collector_convention),
            ArgumentBinding::Unbound => false,
        }
    }
}

/// A call's argument registers and their retained caller places.
#[derive(Default)]
pub struct CallArguments<'c> {
    pub args: &'c [Reg],
    pub kwargs: &'c [(String, Reg)],
    pub arg_places: &'c [Option<MirPlace>],
    pub kwarg_places: &'c [Option<MirPlace>],
    pub spread: Option<usize>,
}

impl CallArguments<'_> {
    /// Each argument register beside whether a retained caller place
    /// passes it and the parameter `contract` binds it to: a positional
    /// argument its slot before the collector, the collector from there
    /// and for the spread pack, and a keyword argument the slot of its
    /// name.
    pub fn bindings(&self, contract: &ParameterContract<'_>) -> Vec<BoundArgument> {
        let placed = |places: &[Option<MirPlace>], index: usize| {
            places.get(index).is_some_and(Option::is_some)
        };
        let positional = self.args.iter().enumerate().map(|(index, reg)| {
            let collected = self.spread == Some(index)
                || contract
                    .collector
                    .is_some_and(|collector| index >= collector);
            BoundArgument {
                reg: *reg,
                placed: placed(self.arg_places, index),
                binding: if collected {
                    ArgumentBinding::Collector
                } else {
                    ArgumentBinding::Slot(index)
                },
            }
        });
        let keyword = self
            .kwargs
            .iter()
            .enumerate()
            .map(|(index, (name, reg))| BoundArgument {
                reg: *reg,
                placed: placed(self.kwarg_places, index),
                binding: contract
                    .names
                    .iter()
                    .position(|parameter| parameter == name)
                    .map_or(ArgumentBinding::Unbound, ArgumentBinding::Slot),
            });
        positional.chain(keyword).collect()
    }

    /// The argument registers `contract` takes ownership of: those bound
    /// by value to an owned parameter or an owned collector.
    fn consumed(&self, contract: &ParameterContract<'_>, out: &mut Vec<Reg>) {
        out.extend(
            self.bindings(contract)
                .into_iter()
                .filter(|argument| !argument.placed && contract.consumes_binding(argument.binding))
                .map(|argument| argument.reg),
        );
    }
}

/// One argument of a call, as [`CallArguments::bindings`] binds it.
#[derive(Clone, Copy)]
pub struct BoundArgument {
    pub reg: Reg,
    /// Whether a retained caller place passes the argument by address.
    pub placed: bool,
    pub binding: ArgumentBinding,
}

/// The parameter an argument binds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArgumentBinding {
    /// The parameter at this slot.
    Slot(usize),
    /// The variadic collector.
    Collector,
    /// A keyword naming no declared parameter (a keyword collector's).
    Unbound,
}

/// Push the results of `instruction` that are fresh owned values.
///
/// Some later operand consumes each, or drop elaboration destroys it: a
/// call's or a resolved operator's value result, a copy, a slot's copying,
/// moving, or forwarding use, a moved place, a constructed aggregate,
/// variant, or closure, a payload taken out of storage, an iterator's
/// element, a subscript call's value result, a constant, a region op's
/// result, and a rebind of an owned value. A shallow read (`place.load`, `field.get`, `ref.read`) and a
/// reference are not owned; nor is anything of a type whose drop runs no
/// code, which [`owned_register`] leaves out.
pub fn owned_result_regs(instruction: &MirInstr, out: &mut Vec<Reg>) {
    match instruction {
        MirInstr::Call { dest, .. }
        | MirInstr::CallIndirect { dest, .. }
        | MirInstr::CopyValue { dest, .. }
        | MirInstr::MovePlace { dest, .. }
        | MirInstr::MakeTuple { dest, .. }
        | MirInstr::MakeArray { dest, .. }
        | MirInstr::MakeStruct { dest, .. }
        | MirInstr::MakeVariant { dest, .. }
        | MirInstr::MakeClosure { dest, .. }
        | MirInstr::PointerStorageTake { dest, .. }
        | MirInstr::UninitStorageTake { dest, .. }
        | MirInstr::VariantTake { dest, .. }
        | MirInstr::Next { dest, .. }
        | MirInstr::Const { dest, .. }
        | MirInstr::Rebind { dest, .. }
        | MirInstr::BinOp { dest, .. }
        | MirInstr::UnOp { dest, .. } => out.push(*dest),
        MirInstr::MethodCall {
            dest,
            reference_result,
            ..
        } if reference_result.is_none() => out.push(*dest),
        MirInstr::UseVar {
            dest,
            mode: super::UseMode::Copy | super::UseMode::Move | super::UseMode::Forward,
            ..
        } => out.push(*dest),
        MirInstr::Index {
            dest,
            call: Some(call),
            ..
        }
        | MirInstr::Slice {
            dest,
            call: Some(call),
            ..
        }
        | MirInstr::MultiIndex {
            dest,
            call: Some(call),
            ..
        } if call.reference_result.is_none() => out.push(*dest),
        MirInstr::If { results, .. }
        | MirInstr::Loop { results, .. }
        | MirInstr::ComptimeIf { results, .. }
        | MirInstr::ComptimeFor { results, .. }
        | MirInstr::Match { results, .. }
        | MirInstr::Try { results, .. } => out.extend(results.iter().copied()),
        _ => {}
    }
}

/// Whether a register of type `ty` holds an owned value whose end matters:
/// no reference, and a type whose drop runs code.
pub fn owned_register(ty: &Ty, declarations: &MirDeclarations) -> bool {
    !matches!(ty.kind(), TyKind::Ref(_)) && !declarations.drop_runs_no_code(ty)
}

/// Push the operands `instruction`, an instruction of `function`, takes
/// ownership of.
///
/// They are a stored value (a
/// slot's, a place's, through a reference, or into compile-time memory),
/// an argument an owned parameter or collector binds, a receiver the
/// callee takes `var` or `deinit` with no retained place, an aggregate's
/// element, a variant payload, a storage's initial value, a loop's
/// operands, a rebind's operand, and a destroyed register.
pub fn consumed_operand_regs(
    instruction: &MirInstr,
    function: &MirFunction,
    declarations: &MirDeclarations,
    out: &mut Vec<Reg>,
) {
    match instruction {
        MirInstr::DefVar { src, .. }
        | MirInstr::Store { src, .. }
        | MirInstr::StoreToMem { src, .. }
        | MirInstr::WriteRef { value: src, .. }
        | MirInstr::Rebind { value: src, .. }
        | MirInstr::MakeVariant { value: src, .. }
        | MirInstr::VariantSet { value: src, .. }
        | MirInstr::VariantReplace { value: src, .. }
        | MirInstr::Drop { reg: src, .. } => out.push(*src),
        MirInstr::UninitStorage { init, .. } => out.extend(init.iter().copied()),
        MirInstr::MakeTuple { elems, .. }
        | MirInstr::MakeArray { elems, .. }
        | MirInstr::MakeStruct { fields: elems, .. }
        | MirInstr::Loop {
            operands: elems, ..
        }
        | MirInstr::ComptimeFor {
            operands: elems, ..
        } => out.extend(elems.iter().copied()),
        MirInstr::Call {
            func,
            args,
            kwargs,
            arg_places,
            kwarg_places,
            spread,
            ..
        } => {
            if let Some(declaration) = declarations.function(&func.0) {
                CallArguments {
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                    spread: *spread,
                }
                .consumed(&declaration.into(), out);
            }
        }
        MirInstr::CallIndirect {
            callee,
            args,
            kwargs,
            arg_places,
            kwarg_places,
            spread,
            instantiated_contract,
            ..
        } => {
            let contract = instantiated_contract
                .as_ref()
                .or_else(|| function.reg_types.get(&callee.0))
                .and_then(|ty| match ty.kind() {
                    TyKind::Func(function) => Some(function),
                    _ => None,
                });
            if let Some(function) = contract {
                CallArguments {
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                    spread: *spread,
                }
                .consumed(&(&**function).into(), out);
            }
        }
        MirInstr::MethodCall {
            recv,
            resolved,
            args,
            kwargs,
            recv_place,
            deinit_receiver,
            arg_places,
            kwarg_places,
            spread,
            ..
        } => {
            let declaration = resolved
                .as_deref()
                .and_then(|name| declarations.function(name));
            if recv_place.is_none()
                && (*deinit_receiver
                    || declaration.is_some_and(|declaration| {
                        owned_convention(declaration.receiver_convention)
                    }))
            {
                out.push(*recv);
            }
            if let Some(declaration) = declaration {
                CallArguments {
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                    spread: *spread,
                }
                .consumed(&declaration.into(), out);
            }
        }
        MirInstr::BinOp {
            a,
            b,
            resolved: Some(resolved),
            ..
        } => {
            if let Some(declaration) = declarations.function(resolved) {
                if owned_convention(declaration.receiver_convention) {
                    out.push(*a);
                }
                if ParameterContract::from(declaration).consumes(0) {
                    out.push(*b);
                }
            }
        }
        MirInstr::MultiSet {
            value,
            value_place: None,
            call,
            ..
        } => {
            if let Some(declaration) = declarations.function(&call.target)
                && let Some(slot) = declaration.param_names.len().checked_sub(1)
                && ParameterContract::from(declaration).consumes(slot)
            {
                out.push(*value);
            }
        }
        _ => {}
    }
}

/// The registers `term` takes ownership of: every value it carries out of
/// its region (a result, a region argument, a returned value) and a
/// raised error; an `elif` condition is only read.
pub fn consumed_terminator_regs(term: &MirTerm) -> Vec<Reg> {
    match term {
        MirTerm::ElifCond { values, .. } => values.clone(),
        _ => super::terminator_regs(term),
    }
}

/// Whether `convention` takes ownership of its argument.
const fn owned_convention(convention: Option<ArgConvention>) -> bool {
    matches!(convention, Some(ArgConvention::Var | ArgConvention::Deinit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::{FuncRef, UseMode};

    fn declarations_with(conventions: Vec<Option<ArgConvention>>) -> MirDeclarations {
        let count = conventions.len();
        let declaration = MirFunctionDeclaration {
            lowered_name: "take".into(),
            param_names: (0..count).map(|i| format!("p{i}")).collect(),
            param_types: vec![Ty::new(TyKind::Struct("S".into(), Vec::new())); count],
            defaults: vec![None; count],
            required: vec![true; count],
            variadic: None,
            variadic_convention: None,
            variadic_index: None,
            kw_variadic: None,
            kw_variadic_convention: None,
            kw_variadic_index: None,
            positional_only: None,
            keyword_only: None,
            param_decls: Vec::new(),
            has_receiver: false,
            receiver_convention: None,
            param_conventions: conventions,
            ret_ty: Ty::none(),
            returns_reference: false,
            raises: false,
            error_ty: None,
            ref_params: vec![false; count],
            param_writes: vec![false; count],
            availability: Vec::new(),
            enclosing: None,
            region_binders: Vec::new(),
            inline: mojito_checked::declaration::InlineLevel::NoDebug,
            provenance: mojito_checked::declaration::FunctionProvenance::Synthesized,
        };
        MirDeclarations {
            functions: vec![declaration],
            ..MirDeclarations::default()
        }
    }

    fn call(args: Vec<Reg>) -> MirInstr {
        MirInstr::Call {
            dest: Reg(9),
            func: FuncRef("take".into()),
            raises: None,
            args,
            kwargs: Vec::new(),
            arg_places: Vec::new(),
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            receiver: None,
            instantiated_args: Vec::new(),
            spread: None,
        }
    }

    fn consumed(instruction: &MirInstr, declarations: &MirDeclarations) -> Vec<Reg> {
        let mut out = Vec::new();
        consumed_operand_regs(instruction, &MirFunction::default(), declarations, &mut out);
        out
    }

    #[test]
    fn an_owned_parameter_consumes_its_argument_and_a_read_one_does_not() {
        let declarations = declarations_with(vec![Some(ArgConvention::Var), None]);
        assert_eq!(
            consumed(&call(vec![Reg(1), Reg(2)]), &declarations),
            vec![Reg(1)]
        );
    }

    #[test]
    fn a_call_with_no_declaration_consumes_nothing() {
        assert!(consumed(&call(vec![Reg(1)]), &MirDeclarations::default()).is_empty());
    }

    #[test]
    fn a_store_and_an_aggregate_consume_their_values() {
        let declarations = MirDeclarations::default();
        let store = MirInstr::DefVar {
            var: 0,
            src: Reg(3),
            binding_ty: None,
            in_place: false,
        };
        assert_eq!(consumed(&store, &declarations), vec![Reg(3)]);
        let tuple = MirInstr::MakeTuple {
            dest: Reg(4),
            elems: vec![Reg(1), Reg(2)],
            element_types: None,
        };
        assert_eq!(consumed(&tuple, &declarations), vec![Reg(1), Reg(2)]);
    }

    #[test]
    fn a_copying_use_is_owned_and_a_borrow_is_not() {
        let use_var = |mode| MirInstr::UseVar {
            dest: Reg(5),
            var: 0,
            mode,
            constructor: None,
        };
        let mut out = Vec::new();
        owned_result_regs(&use_var(UseMode::Copy), &mut out);
        owned_result_regs(&use_var(UseMode::BorrowShared), &mut out);
        assert_eq!(out, vec![Reg(5)]);
    }

    #[test]
    fn a_terminator_consumes_its_values_but_not_an_elif_condition() {
        let term = MirTerm::ElifCond {
            cond: Reg(1),
            values: vec![Reg(2)],
        };
        assert_eq!(consumed_terminator_regs(&term), vec![Reg(2)]);
        assert_eq!(
            consumed_terminator_regs(&MirTerm::Return(Some(Reg(3)))),
            vec![Reg(3)]
        );
    }
}

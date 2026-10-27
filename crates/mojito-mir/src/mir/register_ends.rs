//! Owned registers and the operands that end them.
//!
//! Upstream's `CheckLifetimes` tracks an owned SSA value beside memory
//! values in one `ValueSet`, one bit each: the operation defining it
//! initializes it, and an operand that takes ownership (`var`/`deinit`
//! argument, a store, an aggregate's element) consumes it, as does its
//! destructor call. [`RegisterEnds`] answers both questions for one MIR
//! function: which registers hold an owned value, and which operands each
//! instruction and terminator take ownership of. An operand's convention
//! is its callee's, as upstream's `lit.call` carries its callee's argument
//! conventions: arguments bind to parameters through
//! [`CallArguments::bindings`] and the [`ParameterContract`] of the
//! [`Callee`] a call names, the one binder and resolver the register-loan
//! analysis reads too.

use super::{
    MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr, MirPlace,
    MirSubscriptArg, MirSubscriptCall, MirTerm, Reg, TraitRequirement, instruction_regions,
};
use mojito_ast::ast::ArgConvention;
use mojito_symbol::symbol::is_trait_dispatch_symbol;
use mojito_types::types::{FuncTy, Ty, TyKind};
use std::collections::{HashMap, HashSet};
use std::hash::BuildHasher;

/// The owned registers of one function and the operands that end them.
///
/// A register is owned when it holds a fresh owned value whose drop runs
/// code ([`owned_result_regs`], [`owned_register`]) or is a region
/// argument of such a type, is no compile-time argument a call passes as
/// a parameter (a parameter value, upstream's attribute, not an SSA
/// value), and is not a borrowed pack: a `tuple.make`,
/// `array.create`, or `struct.create` one of whose elements is a shallow
/// read of an owned-typed value (a `place.load`, a `field.get`), upstream's
/// non-owned `VariadicPack` of references. A borrowed pack takes ownership
/// of no element; its owned elements outlive it.
pub struct RegisterEnds<'f> {
    function: &'f MirFunction,
    declarations: &'f MirDeclarations,
    owned: HashSet<u32>,
    borrowed_packs: HashSet<u32>,
}

impl<'f> RegisterEnds<'f> {
    /// Classify the registers of `function`, every region's included.
    pub fn new(function: &'f MirFunction, declarations: &'f MirDeclarations) -> Self {
        let mut ends = Self {
            function,
            declarations,
            owned: HashSet::new(),
            borrowed_packs: HashSet::new(),
        };
        let mut parameters = HashSet::new();
        ends.collect(&function.blocks, &mut parameters);
        ends.owned.retain(|reg| !parameters.contains(reg));
        ends
    }

    /// Whether `reg` holds an owned value whose end matters.
    pub fn owned(&self, reg: Reg) -> bool {
        self.owned.contains(&reg.0)
    }

    /// Whether the function has no owned register.
    pub fn is_empty(&self) -> bool {
        self.owned.is_empty()
    }

    /// The owned registers in register order.
    pub fn owned_registers(&self) -> Vec<Reg> {
        let mut owned: Vec<Reg> = self.owned.iter().copied().map(Reg).collect();
        owned.sort_unstable_by_key(|reg| reg.0);
        owned
    }

    /// Push the operands `instruction` takes ownership of.
    ///
    /// They are a stored value (a slot's, a place's, through a reference,
    /// or into compile-time memory), an argument an owned parameter or
    /// collector binds, a receiver the call records as taken `var` or
    /// `deinit` with no retained place, an owned pack's element, a
    /// storage's initial value, storage or a variant a payload is taken
    /// out of, a variant's new payload, which it takes by move, as
    /// upstream's `init_pointee_move` does, a loop's operands, a rebind's
    /// operand, and a destroyed register.
    pub fn consumed(&self, instruction: &MirInstr, out: &mut Vec<Reg>) {
        match instruction {
            MirInstr::DefVar { src, .. }
            | MirInstr::Store { src, .. }
            | MirInstr::StoreToMem { src, .. }
            | MirInstr::WriteRef { value: src, .. }
            | MirInstr::Rebind { value: src, .. }
            | MirInstr::UninitStorageTake { storage: src, .. }
            | MirInstr::UninitStorageDestroy { storage: src, .. }
            | MirInstr::VariantTake { variant: src, .. }
            | MirInstr::VariantDeinitWith { variant: src, .. }
            | MirInstr::MakeVariant { value: src, .. }
            | MirInstr::VariantSet { value: src, .. }
            | MirInstr::VariantReplace { value: src, .. }
            | MirInstr::Drop { reg: src, .. } => out.push(*src),
            MirInstr::UninitStorage { init, .. } => out.extend(init.iter().copied()),
            MirInstr::MakeTuple { dest, elems, .. }
            | MirInstr::MakeArray { dest, elems }
            | MirInstr::MakeStruct {
                dest,
                fields: elems,
            } if !self.borrowed_packs.contains(&dest.0) => {
                out.extend(elems.iter().copied());
            }
            MirInstr::Loop {
                operands: elems, ..
            }
            | MirInstr::ComptimeFor {
                operands: elems, ..
            } => out.extend(elems.iter().copied()),
            _ => self.taken(instruction, true, out),
        }
    }

    /// Push the operands `instruction` hands its callee by storage: an
    /// argument an owned parameter binds with no retained place, a
    /// receiver taken `var` or `deinit` with no retained place, and an
    /// operator's owned operand, as upstream's `lit.call` passes an owned
    /// argument's memory to its callee. An owning collector's element, a
    /// stored value, and an aggregate's element move into storage of their
    /// own and are not among them.
    pub fn forwarded(&self, instruction: &MirInstr, out: &mut Vec<Reg>) {
        self.taken(instruction, false, out);
    }

    /// The registers `term` takes ownership of: every value it carries out
    /// of its region (a result, a region argument, a returned value) and a
    /// raised error; an `elif` condition is only read.
    pub fn consumed_by(&self, term: &MirTerm) -> Vec<Reg> {
        match term {
            MirTerm::ElifCond { values, .. } => values.clone(),
            _ => super::terminator_regs(term),
        }
    }

    /// Push the operands a call, method, operator, or subscript call
    /// `instruction` takes ownership of, with the arguments an owning
    /// collector binds when `collectors` holds.
    fn taken(&self, instruction: &MirInstr, collectors: bool, out: &mut Vec<Reg>) {
        match instruction {
            MirInstr::Call {
                func,
                args,
                kwargs,
                arg_places,
                kwarg_places,
                spread,
                ..
            } => {
                if let Some(callee) = Callee::direct(self.declarations, &func.0) {
                    CallArguments {
                        args,
                        kwargs,
                        arg_places,
                        kwarg_places,
                        spread: *spread,
                    }
                    .consumed(&callee.contract(), collectors, out);
                }
            }
            MirInstr::CallIndirect {
                callee,
                resolved,
                args,
                kwargs,
                callee_place,
                arg_places,
                kwarg_places,
                spread,
                instantiated_contract,
                ..
            } => {
                let arguments = CallArguments {
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                    spread: *spread,
                };
                if let Some(function) = instantiated_contract
                    .as_ref()
                    .or_else(|| self.function.reg_types.get(&callee.0))
                    .and_then(callable_contract)
                {
                    arguments.consumed(&function.into(), collectors, out);
                } else if let Some(target) = resolved.as_deref().and_then(|name| {
                    Callee::resolve(
                        self.declarations,
                        name,
                        receiver_ty(callee_place.as_deref(), *callee, &self.function.reg_types),
                        None,
                    )
                }) {
                    if callee_place.is_none() && owned_convention(target.receiver_convention()) {
                        out.push(*callee);
                    }
                    arguments.consumed(&target.contract(), collectors, out);
                }
            }
            MirInstr::MethodCall {
                recv,
                resolved,
                args,
                kwargs,
                recv_place,
                receiver_convention,
                arg_places,
                kwarg_places,
                spread,
                refined_bound,
                ..
            } => {
                if recv_place.is_none() && owned_convention(*receiver_convention) {
                    out.push(*recv);
                }
                if let Some(callee) = resolved.as_deref().and_then(|name| {
                    Callee::resolve(
                        self.declarations,
                        name,
                        receiver_ty(recv_place.as_deref(), *recv, &self.function.reg_types),
                        refined_bound
                            .as_deref()
                            .map(|bound| bound.traits.as_slice()),
                    )
                }) {
                    CallArguments {
                        args,
                        kwargs,
                        arg_places,
                        kwarg_places,
                        spread: *spread,
                    }
                    .consumed(&callee.contract(), collectors, out);
                }
            }
            MirInstr::BinOp {
                a,
                b,
                resolved: Some(resolved),
                refined_bound,
                ..
            } => {
                if let Some(callee) = Callee::resolve(
                    self.declarations,
                    resolved,
                    receiver_ty(None, *a, &self.function.reg_types),
                    refined_bound
                        .as_deref()
                        .map(|bound| bound.traits.as_slice()),
                ) {
                    if owned_convention(callee.receiver_convention()) {
                        out.push(*a);
                    }
                    if callee.contract().consumes(0) {
                        out.push(*b);
                    }
                }
            }
            MirInstr::Index {
                base,
                index,
                base_place,
                index_place,
                call: Some(call),
                ..
            } => self.subscript_consumed(
                call,
                (*base, base_place.as_deref()),
                SubscriptArguments {
                    args: &[MirSubscriptArg::Index(*index)],
                    arg_places: &[index_place.as_deref().cloned()],
                    ..SubscriptArguments::default()
                },
                collectors,
                out,
            ),
            MirInstr::MultiIndex {
                object,
                args,
                object_place,
                arg_places,
                kwargs,
                kwarg_places,
                call: Some(call),
                ..
            } => self.subscript_consumed(
                call,
                (*object, object_place.as_deref()),
                SubscriptArguments {
                    args,
                    arg_places,
                    kwargs,
                    kwarg_places,
                },
                collectors,
                out,
            ),
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
                let Some(callee) =
                    self.subscript_callee(call, *receiver, receiver_place.as_deref())
                else {
                    return;
                };
                let contract = callee.contract();
                SubscriptArguments {
                    args,
                    arg_places,
                    ..SubscriptArguments::default()
                }
                .consumed(&contract, collectors, out);
                if value_place.is_none()
                    && let Some(slot) = contract.names.len().checked_sub(1)
                    && contract.consumes(slot)
                {
                    out.push(*value);
                }
            }
            _ => {}
        }
    }

    /// Record the owned registers and borrowed packs of `blocks`, at any
    /// depth, in one pre-order walk, so a pack's elements are classified
    /// before it, and the registers passed as compile-time arguments in
    /// `parameters`.
    fn collect(&mut self, blocks: &[MirBlock], parameters: &mut HashSet<u32>) {
        for instruction in blocks.iter().flat_map(|block| &block.instrs) {
            parameters.extend(parameter_argument_regs(instruction));
            let mut defined = Vec::new();
            owned_result_regs(instruction, &mut defined);
            defined.extend(region_argument_regs(instruction));
            if let MirInstr::MakeTuple { dest, elems, .. }
            | MirInstr::MakeArray { dest, elems }
            | MirInstr::MakeStruct {
                dest,
                fields: elems,
            } = instruction
                && elems
                    .iter()
                    .any(|elem| self.owned_typed(*elem) && !self.owned(*elem))
            {
                self.borrowed_packs.insert(dest.0);
                defined.retain(|reg| reg != dest);
            }
            for reg in defined {
                if self.owned_typed(reg) {
                    self.owned.insert(reg.0);
                }
            }
            for region in instruction_regions(instruction) {
                self.collect(region, parameters);
            }
        }
    }

    /// Whether `reg`'s type holds an owned value whose end matters.
    fn owned_typed(&self, reg: Reg) -> bool {
        self.function
            .reg_types
            .get(&reg.0)
            .is_some_and(|ty| owned_register(ty, self.declarations))
    }

    /// The callee a subscript `call` over `receiver` binds.
    fn subscript_callee(
        &self,
        call: &MirSubscriptCall,
        receiver: Reg,
        place: Option<&MirPlace>,
    ) -> Option<Callee<'f>> {
        Callee::resolve(
            self.declarations,
            &call.target,
            receiver_ty(place, receiver, &self.function.reg_types),
            call.refined_bound.as_deref(),
        )
    }

    /// Push what a subscript read `call` takes ownership of: its receiver
    /// register, when it retains no place and the call takes it `var` or
    /// `deinit`, and each index its parameter, or with `collectors` its
    /// owning collector, takes ownership of.
    fn subscript_consumed(
        &self,
        call: &MirSubscriptCall,
        (receiver, place): (Reg, Option<&MirPlace>),
        arguments: SubscriptArguments<'_>,
        collectors: bool,
        out: &mut Vec<Reg>,
    ) {
        if place.is_none() && owned_convention(call.receiver_convention) {
            out.push(receiver);
        }
        if let Some(callee) = self.subscript_callee(call, receiver, place) {
            arguments.consumed(&callee.contract(), collectors, out);
        }
    }
}

/// What a call binds: a declared function, or a requirement a call through
/// a trait bound selects.
#[derive(Clone, Copy)]
pub enum Callee<'a> {
    Declared(&'a MirFunctionDeclaration),
    Required(&'a TraitRequirement),
}

impl<'a> Callee<'a> {
    /// The declaration a direct call of `callee` binds.
    pub fn direct(declarations: &'a MirDeclarations, callee: &str) -> Option<Self> {
        declarations.function(callee).map(Self::Declared)
    }

    /// The callee a method, operator, or subscript call of `target` binds
    /// over a receiver of type `receiver`: its declaration, or for a call
    /// through a bound, the requirement the bound trait or a parent
    /// declares. The bound is the one the call records it was selected
    /// through (`refined_bound`, upstream's downcast receiver), else the
    /// receiver type parameter's own.
    pub fn resolve(
        declarations: &'a MirDeclarations,
        target: &str,
        receiver: Option<&Ty>,
        refined_bound: Option<&[String]>,
    ) -> Option<Self> {
        declarations
            .function(target)
            .map(Self::Declared)
            .or_else(|| {
                let bounds = refined_bound.or_else(|| match receiver.map(Ty::kind) {
                    Some(TyKind::Param { bounds, .. }) => Some(bounds.as_slice()),
                    _ => None,
                })?;
                is_trait_dispatch_symbol(target)
                    .then(|| declarations.bound_requirement(bounds, target))
                    .flatten()
                    .map(Self::Required)
            })
    }

    /// The parameters the call binds its arguments to.
    pub fn contract(self) -> ParameterContract<'a> {
        match self {
            Self::Declared(declaration) => declaration.into(),
            Self::Required(requirement) => requirement.into(),
        }
    }

    /// The receiver convention a declaration records; a requirement records
    /// none.
    pub const fn receiver_convention(self) -> Option<ArgConvention> {
        match self {
            Self::Declared(declaration) => declaration.receiver_convention,
            Self::Required(_) => None,
        }
    }
}

/// A call receiver's type: its retained place's, else its register's.
pub fn receiver_ty<'t, S: BuildHasher>(
    place: Option<&'t MirPlace>,
    receiver: Reg,
    register_types: &'t HashMap<u32, Ty, S>,
) -> Option<&'t Ty> {
    place
        .and_then(|place| place.ty.as_ref())
        .or_else(|| register_types.get(&receiver.0))
}

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
    /// Whether a keyword collector (`var **kwargs`) takes every keyword
    /// argument naming no parameter; Mojo's is always owned.
    pub keyword_collector: bool,
}

impl<'c> From<&'c MirFunctionDeclaration> for ParameterContract<'c> {
    fn from(declaration: &'c MirFunctionDeclaration) -> Self {
        Self {
            names: &declaration.param_names,
            types: &declaration.param_types,
            conventions: &declaration.param_conventions,
            collector: declaration.variadic_index,
            collector_convention: declaration.variadic_convention,
            keyword_collector: declaration.kw_variadic.is_some(),
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
            keyword_collector: requirement.kw_variadic.is_some(),
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
            keyword_collector: function.kw_variadic.is_some(),
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
            ArgumentBinding::Unbound => self.keyword_collector,
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
    /// by value to an owned parameter, or with `collectors` to an owned
    /// collector.
    fn consumed(&self, contract: &ParameterContract<'_>, collectors: bool, out: &mut Vec<Reg>) {
        out.extend(
            self.bindings(contract)
                .into_iter()
                .filter(|argument| {
                    !argument.placed
                        && (collectors || matches!(argument.binding, ArgumentBinding::Slot(_)))
                        && contract.consumes_binding(argument.binding)
                })
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
/// variant, or closure, a payload taken out of storage or handed back by
/// a variant's replacement, an iterator's element, a subscript call's
/// value result, a constant, a region op's result, and a rebind of an
/// owned value. A shallow read (`place.load`, `field.get`, `ref.read`) and a
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
        | MirInstr::VariantReplace { dest, .. }
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

/// Whether a register of type `ty` holds an owned value whose end matters.
///
/// It is no reference, no compile-time-only value (a parameter list, a
/// trait, a type), no thin function value, which upstream passes as a
/// register-passable trivial value, and a type whose drop runs code.
pub fn owned_register(ty: &Ty, declarations: &MirDeclarations) -> bool {
    let untracked = match ty.kind() {
        TyKind::Ref(_)
        | TyKind::ParamList(_)
        | TyKind::AnyTrait(_)
        | TyKind::Trait(_)
        | TyKind::TypeType => true,
        TyKind::Func(function) => {
            matches!(
                function.environment,
                mojito_types::origin::CallableEnvironment::Thin
            )
        }
        _ => false,
    };
    !untracked && !declarations.drop_runs_no_code(ty)
}

/// Whether `convention` takes ownership of its argument.
const fn owned_convention(convention: Option<ArgConvention>) -> bool {
    matches!(convention, Some(ArgConvention::Var | ArgConvention::Deinit))
}

/// The region arguments `instruction` defines at its regions' entries.
pub fn region_argument_regs(instruction: &MirInstr) -> Vec<Reg> {
    match instruction {
        MirInstr::Loop { args, .. } | MirInstr::ComptimeFor { args, .. } => args.clone(),
        MirInstr::If { else_args, .. } => else_args.clone(),
        MirInstr::Match {
            cases, else_args, ..
        } => cases
            .iter()
            .flat_map(|case| case.args.iter())
            .chain(else_args)
            .copied()
            .collect(),
        MirInstr::Try {
            handler_args,
            else_args,
            ..
        } => handler_args.iter().chain(else_args).copied().collect(),
        _ => Vec::new(),
    }
}

/// The registers `instruction` passes as compile-time arguments: a call's
/// or a subscript call's bracket arguments, and a constructed element.
fn parameter_argument_regs(instruction: &MirInstr) -> impl Iterator<Item = u32> + '_ {
    let arguments: &[super::MirParamArg] = match instruction {
        MirInstr::ConstructTypeParam { element, .. } => element.as_slice(),
        MirInstr::Call { param_arg_regs, .. }
        | MirInstr::CallIndirect { param_arg_regs, .. }
        | MirInstr::MethodCall { param_arg_regs, .. } => param_arg_regs,
        MirInstr::Index {
            call: Some(call), ..
        }
        | MirInstr::MultiIndex {
            call: Some(call), ..
        } => &call.param_arg_regs,
        MirInstr::MultiSet { call, .. } => &call.param_arg_regs,
        _ => &[],
    };
    arguments
        .iter()
        .filter_map(|argument| argument.value.map(|reg| reg.0))
}

/// The signature a call through a value of type `ty` binds: a function
/// type, generic or not, or a type parameter's callable bound.
fn callable_contract(ty: &Ty) -> Option<&FuncTy> {
    match ty.kind() {
        TyKind::Param {
            callable_bound: Some(bound),
            ..
        } => callable_contract(bound),
        kind => kind.callable(),
    }
}

/// A subscript call's actuals beside their retained caller places.
#[derive(Default, Clone, Copy)]
struct SubscriptArguments<'c> {
    args: &'c [MirSubscriptArg],
    arg_places: &'c [Option<MirPlace>],
    kwargs: &'c [(String, MirSubscriptArg)],
    kwarg_places: &'c [Option<MirPlace>],
}

impl SubscriptArguments<'_> {
    /// Push each plain index `contract` takes ownership of with no
    /// retained place: a positional one binding its slot or, from there
    /// and with `collectors`, the collector, and a keyword one the slot of
    /// its name.
    fn consumed(&self, contract: &ParameterContract<'_>, collectors: bool, out: &mut Vec<Reg>) {
        let placed = |places: &[Option<MirPlace>], index: usize| {
            places.get(index).is_some_and(Option::is_some)
        };
        for (index, argument) in self.args.iter().enumerate() {
            let binding = if contract
                .collector
                .is_some_and(|collector| index >= collector)
            {
                ArgumentBinding::Collector
            } else {
                ArgumentBinding::Slot(index)
            };
            if let MirSubscriptArg::Index(reg) = argument
                && !placed(self.arg_places, index)
                && (collectors || binding != ArgumentBinding::Collector)
                && contract.consumes_binding(binding)
            {
                out.push(*reg);
            }
        }
        for (index, (name, argument)) in self.kwargs.iter().enumerate() {
            if let MirSubscriptArg::Index(reg) = argument
                && !placed(self.kwarg_places, index)
                && contract
                    .names
                    .iter()
                    .position(|parameter| parameter == name)
                    .is_some_and(|slot| contract.consumes(slot))
            {
                out.push(*reg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mir::{FuncRef, MirStructDeclaration, TraitDeclaration, UseMode};
    use mojito_types::param_expr::{ParamId, ParamRef};

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
        consumed_in(instruction, &MirFunction::default(), declarations)
    }

    fn consumed_in(
        instruction: &MirInstr,
        function: &MirFunction,
        declarations: &MirDeclarations,
    ) -> Vec<Reg> {
        let mut out = Vec::new();
        RegisterEnds::new(function, declarations).consumed(instruction, &mut out);
        out
    }

    fn struct_ty() -> Ty {
        Ty::new(TyKind::Struct("S".into(), Vec::new()))
    }

    /// A function of one block holding `instrs`, each register of
    /// `reg_types` typed.
    fn function_of(instrs: Vec<MirInstr>, reg_types: Vec<(u32, Ty)>) -> MirFunction {
        MirFunction {
            blocks: vec![MirBlock {
                instrs,
                term: MirTerm::Return(None),
            }],
            reg_types: reg_types.into_iter().collect(),
            ..MirFunction::default()
        }
    }

    fn method_call(
        recv: Reg,
        resolved: &str,
        args: Vec<Reg>,
        receiver_convention: Option<ArgConvention>,
    ) -> MirInstr {
        MirInstr::MethodCall {
            dest: Reg(9),
            recv,
            method: "m".into(),
            resolved: Some(resolved.into()),
            raises: None,
            reference_result: None,
            result_adapter: None,
            args,
            kwargs: Vec::new(),
            recv_place: None,
            recv_writes: false,
            receiver_convention,
            arg_places: Vec::new(),
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            param_decls: Vec::new(),
            instantiated_args: Vec::new(),
            spread: None,
            refined_bound: None,
        }
    }

    /// Declarations holding the trait `Absorber`, whose one requirement,
    /// selected by `dispatch`, takes its parameter `var`.
    fn absorber(dispatch: &str) -> MirDeclarations {
        MirDeclarations {
            traits: vec![TraitDeclaration {
                name: "Absorber".into(),
                refines: Vec::new(),
                requirements: vec![TraitRequirement {
                    name: "absorb".into(),
                    arity: 1,
                    dispatch: dispatch.into(),
                    param_names: vec!["t".into()],
                    param_types: vec![struct_ty()],
                    param_conventions: vec![Some(ArgConvention::Var)],
                    variadic_index: None,
                    required: vec![true],
                    positional_only: None,
                    keyword_only: None,
                    variadic: None,
                    kw_variadic: None,
                    variadic_convention: None,
                }],
            }],
            ..MirDeclarations::default()
        }
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
    fn an_owned_parameter_takes_its_argument_by_storage_and_a_collector_does_not() {
        let mut declarations = declarations_with(vec![Some(ArgConvention::Var), None]);
        declarations.functions[0].variadic_index = Some(1);
        declarations.functions[0].variadic_convention = Some(ArgConvention::Var);
        let call = call(vec![Reg(1), Reg(2)]);
        let function = MirFunction::default();
        let ends = RegisterEnds::new(&function, &declarations);
        let mut forwarded = Vec::new();
        ends.forwarded(&call, &mut forwarded);
        assert_eq!(forwarded, vec![Reg(1)]);
        assert_eq!(consumed(&call, &declarations), vec![Reg(1), Reg(2)]);
        let store = MirInstr::DefVar {
            var: 0,
            src: Reg(3),
            binding_ty: None,
            in_place: false,
        };
        forwarded.clear();
        ends.forwarded(&store, &mut forwarded);
        assert!(forwarded.is_empty());
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
        let function = MirFunction::default();
        let declarations = MirDeclarations::default();
        let ends = RegisterEnds::new(&function, &declarations);
        assert_eq!(ends.consumed_by(&term), vec![Reg(2)]);
        assert_eq!(
            ends.consumed_by(&MirTerm::Return(Some(Reg(3)))),
            vec![Reg(3)]
        );
    }
    #[test]
    fn an_owned_argument_through_a_trait_bound_is_consumed() {
        let dispatch = "__trait_dispatch.absorb";
        let declarations = absorber(dispatch);
        let bound = Ty::new(TyKind::Param {
            binder: ParamRef {
                id: ParamId::new("f", 0),
                name: "T".into(),
            },
            bounds: vec!["Absorber".into()],
            callable_bound: None,
        });
        let call = method_call(Reg(0), dispatch, vec![Reg(1)], None);
        let function = function_of(Vec::new(), vec![(0, bound)]);
        assert_eq!(consumed_in(&call, &function, &declarations), vec![Reg(1)]);
    }

    #[test]
    fn a_struct_name_call_binds_its_initializer() {
        let mut declarations = declarations_with(vec![Some(ArgConvention::Var)]);
        declarations.functions[0].lowered_name = "S.__init__".into();
        declarations.structs.push(MirStructDeclaration {
            name: "S".into(),
            fields: Vec::new(),
            mut_self_methods: HashSet::new(),
            fieldwise_init: false,
            param_decls: Vec::new(),
            explicit_destructors: HashMap::new(),
            explicit_destroy: None,
            conformances: Vec::new(),
            declared: Vec::new(),
            associated_types: Vec::new(),
            parameterized_associated_types: Vec::new(),
            associated_values: Vec::new(),
            parameterized_associated_values: Vec::new(),
            witness_tables: Vec::new(),
            members: Vec::new(),
        });
        let mut construct = call(vec![Reg(1)]);
        if let MirInstr::Call { func, .. } = &mut construct {
            *func = FuncRef("S".into());
        }
        assert_eq!(consumed(&construct, &declarations), vec![Reg(1)]);
    }

    #[test]
    fn a_callable_struct_call_binds_its_resolved_call_method() {
        let mut declarations = declarations_with(vec![None, Some(ArgConvention::Var)]);
        declarations.functions[0].lowered_name = "Keeper.__call__".into();
        let call = MirInstr::CallIndirect {
            dest: Reg(9),
            callee: Reg(0),
            resolved: Some("Keeper.__call__".into()),
            raises: None,
            args: vec![Reg(1), Reg(2)],
            kwargs: Vec::new(),
            callee_place: None,
            arg_places: Vec::new(),
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            param_decls: Vec::new(),
            instantiated_contract: None,
            instantiated_args: Vec::new(),
            spread: None,
        };
        let function = function_of(
            Vec::new(),
            vec![(0, Ty::new(TyKind::Struct("Keeper".into(), Vec::new())))],
        );
        assert_eq!(consumed_in(&call, &function, &declarations), vec![Reg(2)]);
    }

    #[test]
    fn a_method_call_consumes_the_receiver_it_records_as_owned() {
        let declarations = MirDeclarations::default();
        let finish = |convention| method_call(Reg(0), "H.finish", Vec::new(), convention);
        assert_eq!(
            consumed(&finish(Some(ArgConvention::Var)), &declarations),
            vec![Reg(0)]
        );
        assert!(consumed(&finish(Some(ArgConvention::Mut)), &declarations).is_empty());
        assert!(consumed(&finish(None), &declarations).is_empty());
    }

    #[test]
    fn a_pack_over_a_shallow_read_is_borrowed_and_consumes_no_element() {
        let declarations = declarations_with(vec![None]);
        let tuple = |dest, elems| MirInstr::MakeTuple {
            dest: Reg(dest),
            elems,
            element_types: None,
        };
        let load = MirInstr::LoadPlace {
            dest: Reg(1),
            place: MirPlace::root(0, Some(struct_ty())),
        };
        let mixed = tuple(3, vec![Reg(1), Reg(9)]);
        let owned = tuple(4, vec![Reg(9)]);
        let pair = Ty::new(TyKind::Tuple(vec![struct_ty(), struct_ty()]));
        let function = function_of(
            vec![load, call(Vec::new()), mixed.clone(), owned.clone()],
            vec![
                (1, struct_ty()),
                (9, struct_ty()),
                (3, pair),
                (4, Ty::new(TyKind::Tuple(vec![struct_ty()]))),
            ],
        );
        let ends = RegisterEnds::new(&function, &declarations);
        assert!(!ends.owned(Reg(1)));
        assert!(ends.owned(Reg(9)));
        assert!(!ends.owned(Reg(3)));
        assert!(ends.owned(Reg(4)));
        assert!(consumed_in(&mixed, &function, &declarations).is_empty());
        assert_eq!(consumed_in(&owned, &function, &declarations), vec![Reg(9)]);
    }

    #[test]
    fn taking_a_payload_consumes_the_variant_and_a_new_payload_moves_in() {
        let declarations = MirDeclarations::default();
        let take = MirInstr::VariantTake {
            dest: Reg(2),
            variant: Reg(1),
            index: mojito_types::types::VariantIndex::Known(0),
            checked: true,
        };
        assert_eq!(consumed(&take, &declarations), vec![Reg(1)]);
        let make = MirInstr::MakeVariant {
            dest: Reg(3),
            alternatives: vec![struct_ty()],
            index: mojito_types::types::VariantIndex::Known(0),
            value: Reg(1),
            constructor: None,
        };
        assert_eq!(consumed(&make, &declarations), vec![Reg(1)]);
    }

    #[test]
    fn a_keyword_collector_consumes_its_spread_and_stray_keywords() {
        let mut declarations = declarations_with(vec![None]);
        declarations.functions[0].kw_variadic = Some(struct_ty());
        let mut forward = call(vec![Reg(1)]);
        if let MirInstr::Call { kwargs, .. } = &mut forward {
            *kwargs = vec![("**".into(), Reg(2)), ("extra".into(), Reg(3))];
        }
        assert_eq!(consumed(&forward, &declarations), vec![Reg(2), Reg(3)]);
        declarations.functions[0].kw_variadic = None;
        assert!(consumed(&forward, &declarations).is_empty());
    }

    #[test]
    fn compile_time_values_and_thin_functions_are_not_owned() {
        let declarations = MirDeclarations::default();
        let thin = Ty::new(TyKind::Func(Box::new(FuncTy {
            environment: mojito_types::origin::CallableEnvironment::Thin,
            params: Vec::new(),
            names: Vec::new(),
            ret: Ty::none(),
            required: Vec::new(),
            variadic: None,
            kw_variadic: None,
            positional_only: None,
            keyword_only: None,
            raises: false,
            error: None,
            conventions: Vec::new(),
            ref_params: Vec::new(),
            ref_return: None,
            transfers: mojito_types::types::TransferSet::default(),
        })));
        assert!(!owned_register(&thin, &declarations));
        assert!(!owned_register(&Ty::new(TyKind::TypeType), &declarations));
        assert!(!owned_register(
            &Ty::new(TyKind::ParamList(struct_ty())),
            &declarations
        ));
        assert!(owned_register(&struct_ty(), &declarations));
    }

    #[test]
    fn a_subscript_assignment_consumes_an_owned_key() {
        let mut declarations =
            declarations_with(vec![Some(ArgConvention::Var), Some(ArgConvention::Var)]);
        declarations.functions[0].lowered_name = "Dict.__setitem__".into();
        let call = MirSubscriptCall {
            target: "Dict.__setitem__".into(),
            raises: None,
            result_ty: Ty::none(),
            receiver_requires_place: true,
            receiver_convention: Some(ArgConvention::Mut),
            arguments: Vec::new(),
            capture_accesses: Vec::new(),
            reference_result: None,
            param_arg_regs: Vec::new(),
            param_decls: Vec::new(),
            refined_bound: None,
        };
        let set = MirInstr::MultiSet {
            receiver: Reg(0),
            receiver_place: Some(Box::new(MirPlace::root(0, None))),
            args: vec![MirSubscriptArg::Index(Reg(1))],
            arg_places: vec![None],
            value: Reg(2),
            value_place: None,
            value_keyword: false,
            call: Box::new(call),
        };
        assert_eq!(consumed(&set, &declarations), vec![Reg(1), Reg(2)]);
    }
}

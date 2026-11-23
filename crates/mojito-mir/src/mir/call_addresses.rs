//! By-address call operands.
//!
//! Upstream's `lit.call` takes an argument its callee receives by address —
//! a `mut` or `ref` parameter, or a `read` one of a memory-only type — as a
//! `!lit.ref` of the caller's storage, and loads nothing. MIR lowering
//! forms that `ref.make` of the operand's retained place where it lowers
//! the call (`Flatten::address_operand`), by the check's verdict; the
//! instruction names the storage nowhere else. This module reads the
//! contract each call binds its operands by, for the phases after lowering:
//! a direct call's, a resolved `call.method`'s, and a checked subscript's
//! accessor's declaration; a method call through a trait bound's
//! requirement, as upstream's call of a requirement reads the
//! requirement's signature; a call through a function value
//! (`call.indirect`) and a `def(...)` bound's `__call__`, the value's or
//! the bound's function type, as upstream's indirect `lit.call` reads the
//! callee's `!lit.fn` signature; and a type parameter's copy construction
//! (`T(copy=x)`), its copy initializer's `read copy: Self`.
//!
//! A concrete register-passable `read` argument is passed by value, as
//! upstream's IR generation passes it under its `imm` convention. One an
//! instance's substitution makes register-passable (a `read x: T` at
//! `T = Int`) is upstream's `imm_mem`: it stays an address until
//! `LowerArgConventions`' caller half reads it through its reference once
//! elaborated ([`load_register_passable_operands`]). That half also takes a
//! `deinit` receiver an instance makes register-passable (`Optional[T]` at
//! `T = Int`) out of the storage its `ref.make` addresses, as upstream's
//! pass passes a `DeinitMem` operand as an owned value there.
//! An artifact written before lowering passed by-address operands as
//! references has its calls addressed by that contract as it loads
//! ([`upgrade_call_operands`]).

use crate::mir::bound_calls::callable_bound_requirement;
use crate::mir::legacy_places::{LegacyCallPlaces, LegacyCursor, LegacyOperandTables};
use crate::mir::{
    MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr, MirPlace,
    MirSubscriptArg, Proj, Reg, TraitRequirement, count_register_reads, instruction_regions,
    instruction_regions_mut, mir_place_handle_ty, place_holds_reference, place_value_ty,
};
use mojito_ast::ast::ArgConvention;
use mojito_ast::call::{ArgSlot, CallVariadics, match_call_slots};
use mojito_hir::hir::VarId;
use mojito_symbol::symbol::is_initializer_symbol;
use mojito_types::origin::{Mutability, OriginParamId, RefTy};
use mojito_types::types::{FuncTy, Ty, TyKind, erase_origins};
use std::collections::{BTreeSet, HashMap};

/// The regular parameter each operand of a direct call of `declaration`
/// binds.
///
/// One entry per positional operand, then one per keyword operand,
/// `None` for an operand a collector takes; the `spread` operand, the
/// positional pack the call passes, binds the positional collector. `None` overall when the call
/// does not bind, or when the callee takes a receiver the call passes
/// among its operands, whose slots the declaration's parameter lists do
/// not count: a method named by its symbol (`Pt.method(p)`) lowers to a
/// `call.method`, whose operands exclude its receiver, so no such `call`
/// reaches this question from lowering.
#[must_use]
pub fn call_operand_parameters(
    callee: &str,
    declaration: &MirFunctionDeclaration,
    positional: usize,
    spread: Option<usize>,
    kwargs: &[(String, Reg)],
) -> Option<Vec<Option<usize>>> {
    if declaration.has_receiver && !is_initializer_symbol(callee) {
        return None;
    }
    method_operand_parameters(declaration, positional, spread, kwargs)
}

/// The regular parameter each operand of a resolved `call.method` of
/// `declaration` binds, its receiver excluded.
///
/// One entry per positional operand, then one per keyword operand, `None`
/// for an operand a collector takes, the `spread` pack among them. `None`
/// overall when the operands do not bind.
#[must_use]
pub fn method_operand_parameters(
    declaration: &MirFunctionDeclaration,
    positional: usize,
    spread: Option<usize>,
    kwargs: &[(String, Reg)],
) -> Option<Vec<Option<usize>>> {
    let keywords: Vec<&str> = kwargs.iter().map(|(name, _)| name.as_str()).collect();
    operand_parameters(
        &CallShape::declared(declaration),
        positional,
        spread,
        &keywords,
    )
}

/// The regular parameter each operand of a call of function type
/// `signature` binds.
///
/// The call is through a function value or a `def(...)` bound's
/// `__call__`, as upstream's indirect `lit.call` binds the callee's
/// `!lit.fn` signature.
///
/// One entry per positional operand, then one per keyword operand, `None`
/// for an operand a collector takes, the `spread` pack among them. `None`
/// overall when the operands do not bind.
#[must_use]
pub fn signature_operand_parameters(
    signature: &FuncTy,
    positional: usize,
    spread: Option<usize>,
    kwargs: &[(String, Reg)],
) -> Option<Vec<Option<usize>>> {
    let keywords: Vec<&str> = kwargs.iter().map(|(name, _)| name.as_str()).collect();
    operand_parameters(
        &Contract::Signature(signature).shape()?,
        positional,
        spread,
        &keywords,
    )
}

/// One operand of a call as its callee's declaration binds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundOperand<'i> {
    pub register: Reg,
    /// The keyword it is passed under, `None` for a positional operand.
    pub keyword: Option<&'i str>,
    /// The regular parameter it binds, `None` for a collector's.
    pub parameter: Option<usize>,
}

/// The operands of a checked subscript (`index.get`, `index.multi`,
/// `index.multi_set`) as a call of its accessor `declaration` binds them,
/// its receiver excluded.
///
/// One entry per positional operand, then one per keyword operand (a
/// store's value under the setter's keyword). `None` overall when the instruction is no checked subscript or
/// its operands do not bind.
#[must_use]
pub fn subscript_operand_parameters<'i>(
    instruction: &'i MirInstr,
    declaration: &MirFunctionDeclaration,
) -> Option<Vec<BoundOperand<'i>>> {
    subscript_operands(instruction, &CallShape::declared(declaration))
}

/// A checked subscript's operands as a call of shape `shape` binds them.
fn subscript_operands<'i>(
    instruction: &'i MirInstr,
    shape: &CallShape<'_>,
) -> Option<Vec<BoundOperand<'i>>> {
    let subscript = SubscriptOperands::of(instruction)?;
    let names: Vec<&str> = subscript.keywords.iter().map(|(name, _)| *name).collect();
    let parameters = operand_parameters(shape, subscript.positional.len(), None, &names)?;
    let operands = subscript
        .positional
        .into_iter()
        .map(|register| (register, None))
        .chain(
            subscript
                .keywords
                .into_iter()
                .map(|(name, register)| (register, Some(name))),
        );
    Some(
        operands
            .zip(parameters)
            .map(|((register, keyword), parameter)| BoundOperand {
                register,
                keyword,
                parameter,
            })
            .collect(),
    )
}

/// How a callee receives its regular parameter `parameter` by address.
///
/// The answer is the mutability of the reference its operand is, `None`
/// when it is received by value (an owned, `deinit`, or register-passable
/// `read` one).
#[must_use]
pub fn by_address_parameter(
    declaration: &MirFunctionDeclaration,
    declarations: &MirDeclarations,
    parameter: usize,
) -> Option<Mutability> {
    by_address(
        declaration.param_conventions.get(parameter).copied()?,
        declaration.ref_params.get(parameter).copied()?,
        declaration.param_types.get(parameter)?,
        declarations,
    )
}

/// How a method receives its receiver, of type `receiver_ty` at the call,
/// by address.
///
/// A `mut` or `ref` receiver is a mutable reference, a `read` one of a
/// memory-only type an immutable one, and an owned (`var`, `deinit`) or
/// register-passable `read` receiver is `None`.
#[must_use]
pub fn by_address_receiver(
    declaration: &MirFunctionDeclaration,
    receiver_ty: &Ty,
    declarations: &MirDeclarations,
) -> Option<Mutability> {
    let convention = declaration.receiver_convention;
    let reference = matches!(convention, Some(ArgConvention::Mut | ArgConvention::Ref));
    by_address(convention, reference, receiver_ty, declarations)
}

/// How a call through a function value, or a `def(...)` bound's
/// `__call__`, of function type `signature` receives each operand by
/// address.
///
/// One entry per positional operand, then one per keyword operand, `None`
/// for one received by value or taken by a collector. `None` overall when
/// the operands do not bind.
#[must_use]
pub fn signature_by_address_operands(
    signature: &FuncTy,
    positional: usize,
    spread: Option<usize>,
    kwargs: &[(String, Reg)],
    declarations: &MirDeclarations,
) -> Option<Vec<Option<Mutability>>> {
    let contract = Contract::Signature(signature);
    let parameters = signature_operand_parameters(signature, positional, spread, kwargs)?;
    Some(
        parameters
            .into_iter()
            .map(|parameter| {
                let parameter = parameter?;
                contract.parameter(parameter, signature.params.get(parameter)?, declarations)
            })
            .collect(),
    )
}

/// The regular parameter each operand of a call through a trait bound
/// binds in `requirement`, its receiver excluded.
///
/// One entry per positional operand, then one per keyword operand, `None`
/// for an operand a collector takes. `None` overall when the operands do
/// not bind.
#[must_use]
pub fn requirement_operand_parameters(
    requirement: &TraitRequirement,
    positional: usize,
    keywords: &[&str],
) -> Option<Vec<Option<usize>>> {
    operand_parameters(
        &CallShape::required(requirement),
        positional,
        None,
        keywords,
    )
}

/// How a call through a trait bound receives a receiver of type
/// `receiver_ty` by address, under the receiver convention the call
/// records (a requirement records none).
#[must_use]
pub fn requirement_by_address_receiver(
    convention: Option<ArgConvention>,
    receiver_ty: &Ty,
    declarations: &MirDeclarations,
) -> Option<Mutability> {
    by_address(
        convention,
        is_reference(convention),
        receiver_ty,
        declarations,
    )
}

/// How a call through a trait bound receives `requirement`'s regular
/// parameter `parameter` by address.
///
/// The operand passes a value of type `operand_ty`: a requirement spells
/// its parameters over `Self`, so the operand's type is the parameter's at
/// the call.
#[must_use]
pub fn requirement_by_address_parameter(
    requirement: &TraitRequirement,
    parameter: usize,
    operand_ty: &Ty,
    declarations: &MirDeclarations,
) -> Option<Mutability> {
    let convention = requirement.param_conventions.get(parameter).copied()?;
    by_address(
        convention,
        is_reference(convention),
        operand_ty,
        declarations,
    )
}

/// The operands of a checked subscript through a trait bound as a call of
/// its accessor's `requirement` binds them, its receiver excluded
/// ([`subscript_operand_parameters`]).
#[must_use]
pub fn subscript_requirement_operand_parameters<'i>(
    instruction: &'i MirInstr,
    requirement: &TraitRequirement,
) -> Option<Vec<BoundOperand<'i>>> {
    subscript_operands(instruction, &CallShape::required(requirement))
}

/// The value a receiver or callee operand of `ty` passes: the referent of
/// the reference it is passed as ([`addressed_receiver`]).
#[must_use]
pub fn receiver_value_ty(ty: &Ty) -> &Ty {
    addressed_receiver(ty).map_or(ty, |reference| &reference.referent)
}

/// The reference an operand of type `operand` passes for a value of type
/// `value`, as upstream's `ref<P>` argument at a `P` parameter.
///
/// `LowerArgConventions` reads such an operand through or forwards it.
/// `None` when the operand is the value itself, a by-value `!lit.ref<Q>`
/// at a `!lit.ref<Q>` parameter included.
#[must_use]
pub fn addressed_operand<'t>(operand: &'t Ty, value: &Ty) -> Option<&'t RefTy> {
    match operand.kind() {
        TyKind::Ref(reference)
            if reference_depth(&reference.referent) == reference_depth(value) =>
        {
            Some(reference)
        }
        _ => None,
    }
}

/// The reference a receiver, or a bound's callee, operand of type
/// `operand` passes.
///
/// A receiver's value is always a nominal type (a struct, a type
/// parameter, a trait), since no type declares a method on a reference, so
/// `ref<Self>` against `Self` is exactly an operand of reference type.
#[must_use]
pub fn addressed_receiver(operand: &Ty) -> Option<&RefTy> {
    match operand.kind() {
        TyKind::Ref(reference) => Some(reference),
        _ => None,
    }
}

/// Read a call's by-address operand through its reference where the
/// callee takes the parameter by value.
///
/// `function` is an elaborated instance, and the read is a `ref.read`, as
/// upstream's `LowerArgConventions` loads a register-passable argument at
/// the call. A `deinit` receiver the instance makes register-passable is
/// instead taken out of the storage its `ref.make` addresses
/// ([`MirInstr::take_place`]) just before the call, as upstream's pass
/// passes a `DeinitMem` operand as an owned value there, and a `ref.make`
/// the call alone read is retired. Returns whether any operand was read.
pub fn load_register_passable_operands(
    function: &mut MirFunction,
    declarations: &MirDeclarations,
) -> bool {
    let callees = Callees::of(declarations);
    let mut next = function.n_regs;
    let mut types = Vec::new();
    let mut made = HashMap::new();
    collect_made_places(&function.body, &mut made);
    let mut reads = vec![0; function.n_regs as usize];
    count_register_reads(&mut function.body, &mut reads);
    let mut retired = Vec::new();
    let changed = load_in_blocks(
        &mut function.body,
        &function.reg_types,
        &mut LoadContext {
            declarations,
            callees: &callees,
            next: &mut next,
            types: &mut types,
            made: &made,
            reads: &reads,
            retired: &mut retired,
        },
    );
    function.n_regs = next;
    for (register, ty, from) in types {
        function.reg_types.insert(register, ty);
        if let Some(span) = function.spans.0.get(&from).cloned() {
            function.spans.0.insert(register, span);
        }
    }
    if !retired.is_empty() {
        retire_made_references(&mut function.body, &retired);
        for register in retired {
            function.reg_types.remove(&register);
            function.spans.0.remove(&register);
        }
    }
    changed
}

/// Read each operand an instance's call passes by address through its
/// reference, where a builtin answers the call and takes its operands by
/// value.
///
/// The elaborator answers a requirement call itself when an instance's
/// receiver is a compiler-modelled type with no witness (`len` of a
/// `Tuple` through `Sized`), and the template passed the receiver and
/// each memory-only `read` operand as a `ref.make` of its place, as
/// lowering forms it. Each becomes a `ref.read` of its reference, returned
/// for the caller to splice ahead of the call, as upstream's
/// `LowerArgConventions` loads a `ref<P>` argument at a by-value `P`
/// slot: the receiver when it is passed as a reference
/// ([`addressed_receiver`]), and each regular operand `operands` pairs
/// with the check's by-address verdict, which the call records
/// ([`MirArgConventions`](crate::mir::MirArgConventions)) and which
/// lowering addressed exactly.
pub fn read_addressed_operands<'r>(
    function: &mut MirFunction,
    receiver: &'r mut Reg,
    operands: impl IntoIterator<Item = (&'r mut Reg, bool)>,
) -> Vec<MirInstr> {
    let referent = |function: &MirFunction, register: &Reg| {
        function
            .reg_types
            .get(&register.0)
            .and_then(addressed_receiver)
            .map(|reference| reference.referent.clone())
    };
    let mut reads = vec![(referent(function, receiver), receiver)];
    for (register, by_address) in operands {
        let read = by_address.then(|| referent(function, register)).flatten();
        reads.push((read, register));
    }
    let mut loads = Vec::new();
    for (referent, register) in reads {
        let Some(referent) = referent else {
            continue;
        };
        let loaded = Reg(function.n_regs);
        function.n_regs += 1;
        function.reg_types.insert(loaded.0, referent);
        if let Some(span) = function.spans.0.get(&register.0).cloned() {
            function.spans.0.insert(loaded.0, span);
        }
        loads.push(MirInstr::ReadRef {
            dest: loaded,
            reference: *register,
        });
        *register = loaded;
    }
    loads
}

/// How a call receives its receiver and each regular operand by address,
/// as upstream's `lit.call` operand types encode it: a `!lit.ref<P>` at
/// each parameter the callee receives by address, `P` elsewhere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallByAddress {
    /// The receiver, with the mutability of the reference it is passed as
    /// (`None` by value).
    pub receiver: Option<(Reg, Option<Mutability>)>,
    /// Each regular operand, positional then keyword.
    pub operands: Vec<ByAddressOperand>,
}

/// One regular operand of a [`CallByAddress`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ByAddressOperand {
    pub register: Reg,
    /// The parameter it binds, `None` for a collector's.
    pub parameter: Option<usize>,
    /// The mutability of the reference it is passed as, `None` by value.
    pub by_address: Option<Mutability>,
    /// The parameter's type at the call, which a `ref<P>` operand
    /// addresses ([`addressed_operand`]); `None` for a collector's operand
    /// and the copy initializer's `copy: Self`.
    pub parameter_ty: Option<Ty>,
}

/// The receiver and regular operands `instruction`, a call of `function`,
/// passes by address.
///
/// It reads a direct `call`, a resolved or trait-bound `call.method`, a
/// checked subscript, a `call.indirect` or `def(...)` bound's `__call__`,
/// and a type parameter's copy construction, each bound by the contract
/// [`upgrade_call_operands`] reads. `None` when the instruction binds no
/// contract.
#[must_use]
pub fn call_by_address_operands(
    function: &MirFunction,
    instruction: &MirInstr,
    declarations: &MirDeclarations,
) -> Option<CallByAddress> {
    let reg_types = &function.reg_types;
    let callees = Callees::of(declarations);
    let bound = bound_call(instruction, &callees, reg_types)?;
    let receiver = bound.receiver.map(|register| {
        let by_address = reg_types
            .get(&register.0)
            .and_then(|ty| bound.contract.receiver(receiver_value_ty(ty), declarations));
        (register, by_address)
    });
    let operands = bound
        .operands
        .iter()
        .map(|&(register, parameter)| {
            let parameter_ty = parameter
                .and_then(|parameter| bound.contract.parameter_ty(parameter))
                .cloned();
            let by_address =
                parameter
                    .zip(reg_types.get(&register.0))
                    .and_then(|(parameter, operand)| {
                        let value = parameter_ty.as_ref().map_or_else(
                            || receiver_value_ty(operand),
                            |parameter_ty| {
                                addressed_operand(operand, parameter_ty)
                                    .map_or(operand, |reference| &reference.referent)
                            },
                        );
                        bound.contract.parameter(parameter, value, declarations)
                    });
            ByAddressOperand {
                register,
                parameter,
                by_address,
                parameter_ty,
            }
        })
        .collect();
    Some(CallByAddress { receiver, operands })
}

/// Whether two places name the same storage, whatever local reference
/// either is reached through.
pub fn same_storage(left: &MirPlace, right: &MirPlace) -> bool {
    left.root == right.root
        && left.root_ty == right.root_ty
        && left.ty == right.ty
        && left.proj.len() == right.proj.len()
        && left.proj.iter().zip(&right.proj).all(|pair| match pair {
            (Proj::Field(left), Proj::Field(right)) => left == right,
            (Proj::Index(left), Proj::Index(right)) => left == right,
            (Proj::ConstIndex(left), Proj::ConstIndex(right)) => left == right,
            (Proj::Variant(left), Proj::Variant(right)) => left == right,
            (Proj::UninitPayload, Proj::UninitPayload) | (Proj::Rebind, Proj::Rebind) => true,
            _ => false,
        })
}

/// The place whose `ref.make` forwards the reference `place` holds, with
/// that reference's type, for a callee wanting it as mutable as
/// `mutability`; `None` when `place` holds no reference.
///
/// Whether `place` holds one is decided from declarations
/// ([`place_holds_reference`]): `binds_root` says whether its root is a
/// `ref` binding, and `declares_ref_field` whether a struct type declares
/// a field `ref`. A `ref` field's own place forwards its stored handle,
/// lent immutably when `mutability` is; a `ref` binding's slot is reached
/// through itself, as a runtime alias's read forms it, and lends its own
/// handle.
pub fn stored_handle(
    place: &MirPlace,
    binds_root: bool,
    declares_ref_field: impl Fn(&Ty, &str) -> bool,
    mutability: Mutability,
) -> Option<(MirPlace, Ty)> {
    if !place_holds_reference(place, binds_root, declares_ref_field) {
        return None;
    }
    let TyKind::Ref(stored) = place.ty.as_ref()?.kind() else {
        return None;
    };
    if place.proj.is_empty() {
        let mut handle_place = place.clone();
        handle_place.through = place.through.or(Some(place.root));
        let handle = mir_place_handle_ty(&handle_place, binds_root, Some(mutability))?;
        return Some((handle_place, handle));
    }
    let handle = TyKind::Ref(mojito_types::origin::RefTy {
        referent: stored.referent.clone(),
        origin: stored.origin.clone(),
        mutability: if mutability == Mutability::Immutable {
            Mutability::Immutable
        } else {
            stored.mutability
        },
        address_space: stored.address_space.clone(),
    })
    .into();
    Some((place.clone(), handle))
}

/// Whether a `ref.make` of `place` whose destination is typed `made`
/// forwards the reference `place` stores rather than borrowing its slot.
///
/// It does when `place` holds a `ref` and `made` is that stored handle's
/// type, the form [`stored_handle`] makes; a destination typed as a borrow
/// of the slot (`ref (ref T)`) never forwards. Origins erase from the
/// runtime ABI, so the two referents are compared without them: a spliced
/// place's root may spell an origin argument its projections do not.
pub fn make_ref_forwards_stored_handle(place: &MirPlace, made: &Ty) -> bool {
    match (place.ty.as_ref().map(Ty::kind), made.kind()) {
        (Some(TyKind::Ref(stored)), TyKind::Ref(made)) => {
            erase_origins(&made.referent) == erase_origins(&stored.referent)
        }
        _ => false,
    }
}

/// Upgrade the calls of `functions`, read from an artifact written before
/// lowering passed by-address operands as references, whose operand
/// places the reader kept in `legacy`.
///
/// Each by-address operand its contract binds is addressed as lowering
/// addresses it ([`address_call_operands`]), then every operand
/// the call's legacy places name that no contract binds (a requirement
/// call over a pack element's receiver) and that is by address whatever
/// its convention: a read back through a `ref.make` of that same place,
/// the address an older lowering already formed, or a load of a
/// memory-only place.
pub fn upgrade_call_operands(
    functions: &mut [(String, MirFunction)],
    declarations: &MirDeclarations,
    legacy: &LegacyOperandTables,
) {
    address_call_operands(functions, declarations, legacy, Operands::All);
    for (index, (name, function)) in functions.iter_mut().enumerate() {
        let parameters = ParameterKinds::of(function, declarations.function(name));
        forward_read_addresses(function, declarations, &parameters, legacy.cursor(index));
    }
}

/// Upgrade the calls of `functions`, read from an artifact written before
/// lowering forwarded the reference a place holds, whose operand places
/// the reader kept in `legacy`.
///
/// A place holding a reference is a `ref` field or a `ref` binding's slot,
/// and an older call passed the value read through it as a by-address
/// operand: each such operand its contract binds forwards the stored
/// handle, as lowering forwards it ([`stored_handle`]).
pub fn upgrade_stored_handles(
    functions: &mut [(String, MirFunction)],
    declarations: &MirDeclarations,
    legacy: &LegacyOperandTables,
) {
    address_call_operands(functions, declarations, legacy, Operands::StoredHandles);
}

/// The declarations of the functions a program calls, by lowered name:
/// its own, then an imported package's.
struct Callees<'a> {
    declarations: &'a MirDeclarations,
    local: HashMap<&'a str, &'a MirFunctionDeclaration>,
}

impl<'a> Callees<'a> {
    fn of(declarations: &'a MirDeclarations) -> Self {
        Self {
            declarations,
            local: declarations
                .functions
                .iter()
                .map(|declaration| (declaration.lowered_name.as_str(), declaration))
                .collect(),
        }
    }

    fn get(&self, name: &str) -> Option<&'a MirFunctionDeclaration> {
        self.local.get(name).copied().or_else(|| {
            self.declarations
                .imported
                .as_ref()
                .and_then(|imported| imported.function(name))
        })
    }

    /// The requirement a method call of the trait-dispatch symbol
    /// `dispatch` binds over a receiver of type `receiver`
    /// ([`MirDeclarations::call_requirement`]).
    fn requirement(
        &self,
        dispatch: &str,
        receiver: Option<&Ty>,
        refined_bound: Option<&[String]>,
    ) -> Option<&'a TraitRequirement> {
        self.declarations
            .call_requirement(dispatch, receiver, refined_bound)
    }
}

/// The contract a resolved call binds its operands by: its callee's
/// declaration, the requirement a call through a trait bound selects,
/// beside the receiver convention the call records for it, the function
/// type a call through a function value or a `def(...)` bound reads, or
/// the copy initializer `T(copy=x)` calls.
#[derive(Clone, Copy)]
enum Contract<'a> {
    Declared(&'a MirFunctionDeclaration),
    Required {
        requirement: &'a TraitRequirement,
        receiver: Option<ArgConvention>,
    },
    Signature(&'a FuncTy),
    /// `Copyable`'s `__init__(out self, *, copy: Self)`, whose one
    /// parameter is the `read` `copy=` source.
    Copy,
}

impl<'a> Contract<'a> {
    /// The shape a call's operands bind by; `None` for the copy
    /// initializer, whose one operand binds its one parameter.
    fn shape(self) -> Option<CallShape<'a>> {
        Some(match self {
            Self::Declared(declaration) => CallShape::declared(declaration),
            Self::Copy => return None,
            Self::Required { requirement, .. } => CallShape::required(requirement),
            Self::Signature(signature) => CallShape {
                names: &signature.names,
                required: &signature.required,
                positional_only: signature.positional_only,
                keyword_only: signature.keyword_only,
                variadics: CallVariadics {
                    positional: signature.variadic.is_some(),
                    keyword: signature.kw_variadic.is_some(),
                },
            },
        })
    }

    /// How the callee receives a receiver of type `ty` by address.
    fn receiver(self, ty: &Ty, declarations: &MirDeclarations) -> Option<Mutability> {
        match self {
            Self::Declared(declaration) => by_address_receiver(declaration, ty, declarations),
            Self::Required { receiver, .. } => {
                requirement_by_address_receiver(receiver, ty, declarations)
            }
            Self::Signature(_) | Self::Copy => None,
        }
    }

    /// How the callee receives its regular parameter `parameter`, whose
    /// operand has type `operand_ty`, by address. A requirement spells its
    /// parameters over `Self`, so the operand's type is the parameter's at
    /// the call.
    fn parameter(
        self,
        parameter: usize,
        operand_ty: &Ty,
        declarations: &MirDeclarations,
    ) -> Option<Mutability> {
        match self {
            Self::Declared(declaration) => {
                by_address_parameter(declaration, declarations, parameter)
            }
            Self::Required { requirement, .. } => {
                requirement_by_address_parameter(requirement, parameter, operand_ty, declarations)
            }
            Self::Copy => by_address(None, false, operand_ty, declarations),
            Self::Signature(signature) => {
                let (convention, reference, ty) = signature_parameter(signature, parameter)?;
                by_address(convention, reference, ty, declarations)
            }
        }
    }

    /// The declared type of the regular parameter `parameter`; `None` for
    /// the copy initializer's `copy: Self`. A requirement spells it over
    /// `Self`.
    fn parameter_ty(self, parameter: usize) -> Option<&'a Ty> {
        match self {
            Self::Declared(declaration) => declaration.param_types.get(parameter),
            Self::Required { requirement, .. } => requirement.param_types.get(parameter),
            Self::Signature(signature) => signature.params.get(parameter),
            Self::Copy => None,
        }
    }

    /// The type of the regular parameter `parameter` when the callee
    /// receives it by value where its operand may be a reference: an owned
    /// one, or a `read` one an instance's substitution made
    /// register-passable. A requirement's parameters are never read here:
    /// the elaborator names the witness, whose declaration binds them, and
    /// writes the copy initializer's call itself.
    fn by_value_parameter(
        self,
        parameter: usize,
        declarations: &MirDeclarations,
    ) -> Option<&'a Ty> {
        match self {
            Self::Declared(declaration) => by_value_parameter(declaration, declarations, parameter),
            Self::Required { .. } | Self::Copy => None,
            Self::Signature(signature) => signature_parameter(signature, parameter)
                .filter(|(convention, reference, ty)| {
                    !reference && by_address(*convention, false, ty, declarations).is_none()
                })
                .map(|(_, _, ty)| ty),
        }
    }
}

/// A function type's regular parameter `parameter`: its convention,
/// whether it binds its operand as a reference, and its type.
fn signature_parameter(
    signature: &FuncTy,
    parameter: usize,
) -> Option<(Option<ArgConvention>, bool, &Ty)> {
    let ty = signature.params.get(parameter)?;
    let convention = signature.conventions.get(parameter).copied().flatten();
    let reference = is_reference(convention)
        || signature
            .ref_params
            .get(parameter)
            .is_some_and(Option::is_some);
    Some((convention, reference, ty))
}

/// The call shape a callee's operands bind by, as
/// [`match_call_slots`] reads it.
struct CallShape<'a> {
    names: &'a [String],
    required: &'a [bool],
    positional_only: Option<usize>,
    keyword_only: Option<usize>,
    variadics: CallVariadics,
}

impl<'a> CallShape<'a> {
    fn declared(declaration: &'a MirFunctionDeclaration) -> Self {
        Self {
            names: &declaration.param_names,
            required: &declaration.required,
            positional_only: declaration.positional_only,
            keyword_only: declaration.keyword_only,
            variadics: CallVariadics {
                positional: declaration.variadic.is_some(),
                keyword: declaration.kw_variadic.is_some(),
            },
        }
    }

    fn required(requirement: &'a TraitRequirement) -> Self {
        Self {
            names: &requirement.param_names,
            required: &requirement.required,
            positional_only: requirement.positional_only,
            keyword_only: requirement.keyword_only,
            variadics: CallVariadics {
                positional: requirement.variadic.is_some(),
                keyword: requirement.kw_variadic.is_some(),
            },
        }
    }
}

/// Whether a parameter of `convention` binds its operand as a reference.
const fn is_reference(convention: Option<ArgConvention>) -> bool {
    matches!(convention, Some(ArgConvention::Mut | ArgConvention::Ref))
}

/// How a parameter of `convention` and type `ty` is received by address,
/// `reference` when the callee binds it as a reference.
fn by_address(
    convention: Option<ArgConvention>,
    reference: bool,
    ty: &Ty,
    declarations: &MirDeclarations,
) -> Option<Mutability> {
    if reference {
        return Some(Mutability::Mutable);
    }
    match convention {
        None | Some(ArgConvention::Imm) => {
            (!declarations.register_passable(ty)).then_some(Mutability::Immutable)
        }
        Some(_) => None,
    }
}

/// The regular parameter each of a call's operands binds, the receiver
/// excluded ([`call_operand_parameters`]).
///
/// A `spread` operand is the positional pack the call passes whole, as
/// upstream's call passes the pack it builds: it is the last positional
/// operand, and binds the callee's positional collector.
fn operand_parameters(
    shape: &CallShape<'_>,
    positional: usize,
    spread: Option<usize>,
    keywords: &[&str],
) -> Option<Vec<Option<usize>>> {
    let Some(spread) = spread else {
        return slot_parameters(shape, positional, keywords);
    };
    if !shape.variadics.positional || spread + 1 != positional {
        return None;
    }
    let mut parameters = slot_parameters(shape, spread, keywords)?;
    parameters.insert(spread, None);
    Some(parameters)
}

/// The regular parameter each of `positional` positional operands and the
/// `keywords` operands binds by `shape`, with no pack spread.
fn slot_parameters(
    shape: &CallShape<'_>,
    positional: usize,
    keywords: &[&str],
) -> Option<Vec<Option<usize>>> {
    let matched = match_call_slots(
        shape.names,
        shape.required,
        shape.positional_only,
        shape.keyword_only,
        positional,
        keywords,
        shape.variadics,
    )
    .ok()?;
    let mut parameters = vec![None; positional + keywords.len()];
    for (parameter, slot) in matched.slots.iter().enumerate() {
        match slot {
            ArgSlot::Positional(operand) => parameters[*operand] = Some(parameter),
            ArgSlot::Keyword(operand) => parameters[positional + *operand] = Some(parameter),
            ArgSlot::Default => {}
        }
    }
    Some(parameters)
}

/// A resolved call's operands as its callee's contract binds them.
struct BoundCall<'i> {
    contract: Contract<'i>,
    /// The receiver operand, for a method call or a subscript.
    receiver: Option<Reg>,
    /// Each regular operand, positional then keyword, with the parameter
    /// it binds (`None` for a collector's).
    operands: Vec<(Reg, Option<usize>)>,
}

/// The operands of a resolved direct `call`, `call.method`, or checked
/// subscript (`index.get`, `index.multi`, `index.multi_set`), with its
/// callee's declaration: a subscript is a call of its accessor on the base,
/// as upstream's subscript emission calls `__getitem__`/`__setitem__` with
/// the same operand conventions.
///
/// A method call through a trait bound (`__trait_dispatch.f$…`) binds the
/// requirement the bound declares ([`MirDeclarations::bound_requirement`])
/// over the receiver's type parameter, or the bound the call records it
/// was selected through. A call through a function value binds the
/// value's function type, the contract the call instantiates when it
/// applies compile-time arguments, and a `def(...)` bound's `__call__` the
/// bound's ([`callable_bound_requirement`]). Neither reports its callee:
/// a function value's is passed by value, and a bound's is already its
/// place's address, formed by lowering.
fn bound_call<'i>(
    instruction: &'i MirInstr,
    callees: &Callees<'i>,
    reg_types: &'i HashMap<u32, Ty>,
) -> Option<BoundCall<'i>> {
    match instruction {
        MirInstr::Call {
            func,
            args,
            kwargs,
            spread: None,
            ..
        } => {
            let declaration = callees.get(&func.0)?;
            let parameters =
                call_operand_parameters(&func.0, declaration, args.len(), None, kwargs)?;
            Some(BoundCall {
                contract: Contract::Declared(declaration),
                receiver: None,
                operands: operand_list(args, kwargs, &parameters),
            })
        }
        MirInstr::CallIndirect {
            callee,
            args,
            kwargs,
            instantiated_contract,
            spread: None,
            ..
        } => {
            let signature = function_signature(
                instantiated_contract
                    .as_ref()
                    .or_else(|| reg_types.get(&callee.0))?,
            )?;
            let contract = Contract::Signature(signature);
            let keywords: Vec<&str> = kwargs.iter().map(|(name, _)| name.as_str()).collect();
            let parameters = operand_parameters(&contract.shape()?, args.len(), None, &keywords)?;
            Some(BoundCall {
                contract,
                receiver: None,
                operands: operand_list(args, kwargs, &parameters),
            })
        }
        MirInstr::MethodCall {
            recv,
            resolved: Some(callee),
            args,
            kwargs,
            spread: None,
            recv_writes,
            receiver_convention,
            refined_bound,
            ..
        } => {
            let receiver_ty = reg_types.get(&recv.0).map(receiver_value_ty);
            let callable_bound = receiver_ty
                .and_then(|receiver| callable_bound_requirement(receiver, callee))
                .map(Contract::Signature);
            let contract = callees
                .get(callee)
                .map(Contract::Declared)
                .or(callable_bound)
                .or_else(|| {
                    let requirement = callees.requirement(
                        callee,
                        receiver_ty,
                        refined_bound.as_ref().map(|bound| bound.traits.as_slice()),
                    )?;
                    Some(Contract::Required {
                        requirement,
                        receiver: *receiver_convention,
                    })
                })?;
            let keywords: Vec<&str> = kwargs.iter().map(|(name, _)| name.as_str()).collect();
            let parameters = operand_parameters(&contract.shape()?, args.len(), None, &keywords)?;
            // A requirement call whose receiver convention went unrecorded
            // but which writes its receiver is left to its place, and a
            // `def(...)` bound's callee is the address lowering formed.
            let unrecorded = matches!(contract, Contract::Required { receiver: None, .. })
                && *recv_writes
                || matches!(contract, Contract::Signature(_));
            Some(BoundCall {
                contract,
                receiver: (!unrecorded).then_some(*recv),
                operands: operand_list(args, kwargs, &parameters),
            })
        }
        MirInstr::ConstructTypeParam {
            kwargs,
            element: None,
            ..
        } => match kwargs.as_slice() {
            [(keyword, source)] if keyword == "copy" => Some(BoundCall {
                contract: Contract::Copy,
                receiver: None,
                operands: vec![(*source, Some(0))],
            }),
            _ => None,
        },
        _ => {
            let subscript = SubscriptOperands::of(instruction)?;
            let declaration = callees.get(subscript.callee)?;
            let operands = subscript_operand_parameters(instruction, declaration)?;
            Some(BoundCall {
                contract: Contract::Declared(declaration),
                receiver: Some(subscript.receiver),
                operands: operands
                    .into_iter()
                    .map(|operand| (operand.register, operand.parameter))
                    .collect(),
            })
        }
    }
}

/// The function type a callee of type `ty` is called by, through any
/// reference to it.
fn function_signature(ty: &Ty) -> Option<&FuncTy> {
    match ty.kind() {
        TyKind::Ref(reference) => function_signature(&reference.referent),
        TyKind::Func(function) => Some(function),
        TyKind::GenericFunc(generic) => Some(&generic.func),
        _ => None,
    }
}

/// A checked subscript's operands as a call of its accessor passes them.
struct SubscriptOperands<'i> {
    /// The accessor the checker selected.
    callee: &'i str,
    /// The base.
    receiver: Reg,
    positional: Vec<Reg>,
    /// A keyword index, or a store's value under the setter's keyword.
    keywords: Vec<(&'i str, Reg)>,
}

impl<'i> SubscriptOperands<'i> {
    /// The operands of an `index.get` with a selected accessor, an
    /// `index.multi`, or an `index.multi_set`. An accessor keyed on value
    /// parameters of its own (`__getitem_param__`) takes no runtime index.
    fn of(instruction: &'i MirInstr) -> Option<Self> {
        match instruction {
            MirInstr::Index {
                base,
                index,
                call: Some(call),
                ..
            } => Some(Self {
                callee: &call.target,
                receiver: *base,
                positional: (!call.arguments.is_empty())
                    .then_some(*index)
                    .into_iter()
                    .collect(),
                keywords: Vec::new(),
            }),
            MirInstr::MultiIndex {
                object,
                args,
                kwargs,
                call: Some(call),
                ..
            } => Some(Self {
                callee: &call.target,
                receiver: *object,
                positional: args
                    .iter()
                    .map(|MirSubscriptArg::Index(reg)| *reg)
                    .collect(),
                keywords: kwargs
                    .iter()
                    .map(|(name, MirSubscriptArg::Index(reg))| (name.as_str(), *reg))
                    .collect(),
            }),
            MirInstr::MultiSet {
                receiver,
                args,
                kwargs,
                value,
                value_keyword,
                call,
            } => Some(Self {
                callee: &call.target,
                receiver: *receiver,
                positional: args
                    .iter()
                    .map(|MirSubscriptArg::Index(reg)| *reg)
                    .collect(),
                keywords: kwargs
                    .iter()
                    .map(|(name, MirSubscriptArg::Index(reg))| (name.as_str(), *reg))
                    .chain(std::iter::once((value_keyword.as_str(), *value)))
                    .collect(),
            }),
            _ => None,
        }
    }
}

/// A call's positional then keyword operands with the parameters they
/// bind.
fn operand_list(
    args: &[Reg],
    kwargs: &[(String, Reg)],
    parameters: &[Option<usize>],
) -> Vec<(Reg, Option<usize>)> {
    args.iter()
        .chain(kwargs.iter().map(|(_, register)| register))
        .copied()
        .zip(parameters.iter().copied())
        .collect()
}

/// The registers of the operands [`bound_call`] lists for `instruction`,
/// in its order, with the receiver's.
fn operand_registers_mut(instruction: &mut MirInstr) -> (Option<&mut Reg>, Vec<&mut Reg>) {
    match instruction {
        MirInstr::Call { args, kwargs, .. } | MirInstr::CallIndirect { args, kwargs, .. } => (
            None,
            args.iter_mut()
                .chain(kwargs.iter_mut().map(|(_, register)| register))
                .collect(),
        ),
        MirInstr::ConstructTypeParam { kwargs, .. } => (
            None,
            kwargs.iter_mut().map(|(_, register)| register).collect(),
        ),
        MirInstr::MethodCall {
            recv, args, kwargs, ..
        } => (
            Some(recv),
            args.iter_mut()
                .chain(kwargs.iter_mut().map(|(_, register)| register))
                .collect(),
        ),
        MirInstr::Index {
            base,
            index,
            call: Some(call),
            ..
        } => (
            Some(base),
            if call.arguments.is_empty() {
                Vec::new()
            } else {
                vec![index]
            },
        ),
        MirInstr::MultiIndex {
            object,
            args,
            kwargs,
            ..
        } => (
            Some(object),
            args.iter_mut()
                .chain(kwargs.iter_mut().map(|(_, argument)| argument))
                .map(|MirSubscriptArg::Index(register)| register)
                .collect(),
        ),
        MirInstr::MultiSet {
            receiver,
            args,
            kwargs,
            value,
            ..
        } => (
            Some(receiver),
            args.iter_mut()
                .chain(kwargs.iter_mut().map(|(_, argument)| argument))
                .map(|MirSubscriptArg::Index(register)| register)
                .chain([value])
                .collect(),
        ),
        _ => (None, Vec::new()),
    }
}

/// What one operand's definition reads.
enum Definition {
    Load(MirPlace),
    Read(Reg),
    Address(MirPlace),
}

/// One rewrite of an operand's definition.
struct Rewrite {
    place: MirPlace,
    /// The type of the reference the rewritten definition makes.
    handle: Ty,
    /// The `ref.make` a rewritten `ref.read` read, which goes with it.
    retired: Option<Reg>,
}

/// Which by-address operands [`address_call_operands`] addresses.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Operands {
    All,
    /// Only those whose place holds a reference, which forward it.
    StoredHandles,
}

/// The place an operand addressing `place` makes its reference of, as
/// mutable as `mutability`, and that reference's type: the stored handle a
/// place holding one forwards ([`stored_handle`]), else the place's own
/// storage.
fn operand_handle(
    place: MirPlace,
    mutability: Mutability,
    parameters: &ParameterKinds,
    declarations: &MirDeclarations,
) -> (MirPlace, Ty) {
    let binds_root = parameters.ref_bindings.contains(&place.root);
    stored_handle(
        &place,
        binds_root,
        |base, field| declarations.declares_ref_field(base, field),
        mutability,
    )
    .unwrap_or_else(|| {
        let handle =
            mir_place_handle_ty(&place, binds_root, Some(mutability)).unwrap_or_else(Ty::error);
        (place, handle)
    })
}

/// Pass each by-address call operand in `functions` as a `ref.make` of its
/// retained place, rather than a whole load of it.
///
/// The calls are every direct call, call through a function value,
/// resolved method call, checked subscript, and type parameter's copy
/// construction of an artifact [`upgrade_call_operands`] loads, and each
/// operand's place is the one `legacy` keeps for it.
///
/// The operand's definition is rewritten when its one reader is the call
/// and it reads the storage the call's legacy places name: a `place.load`
/// of it, or a `ref.read` of a `ref.make` of it, which goes too when
/// nothing else reads it. Any other by-address operand, one whose read
/// another use shares or that is no whole read of the place, is passed as
/// a fresh `ref.make` of the place just before its call. A place holding a
/// reference passes the reference it holds ([`operand_handle`]).
/// `operands` picks which operands are addressed.
fn address_call_operands(
    functions: &mut [(String, MirFunction)],
    declarations: &MirDeclarations,
    legacy: &LegacyOperandTables,
    operands: Operands,
) {
    let callees = Callees::of(declarations);
    for (index, (name, function)) in functions.iter_mut().enumerate() {
        let parameters = ParameterKinds::of(function, declarations.function(name));
        let context = RemainingContext {
            declarations,
            callees: &callees,
            parameters: &parameters,
            operands,
        };
        address_function_operands(function, &context, legacy.cursor(index));
        address_remaining_operands(function, &context, legacy.cursor(index));
    }
}

fn address_function_operands(
    function: &mut MirFunction,
    context: &RemainingContext<'_, '_>,
    mut cursor: LegacyCursor<'_>,
) {
    let mut definitions = HashMap::new();
    let mut operands = Vec::new();
    collect(
        &function.body,
        &Collector {
            callees: context.callees,
            declarations: context.declarations,
            ref_bindings: &context.parameters.ref_bindings,
            reg_types: &function.reg_types,
            operands: context.operands,
        },
        &mut cursor,
        &mut definitions,
        &mut operands,
    );
    if operands.is_empty() {
        return;
    }
    let mut reads = vec![0; function.n_regs as usize];
    count_register_reads(&mut function.body, &mut reads);
    let read_once = |register: Reg| reads.get(register.0 as usize) == Some(&1);
    let mut rewrites: HashMap<u32, Rewrite> = HashMap::new();
    for (operand, place, mutability) in operands {
        if !read_once(operand) || rewrites.contains_key(&operand.0) {
            continue;
        }
        let mutability = context.parameters.place_mutability(&place, mutability);
        let (addressed, retired) = match definitions.get(&operand.0) {
            Some(Definition::Load(loaded))
                if same_storage(loaded, &place) && loaded.through == place.through =>
            {
                (place, None)
            }
            Some(Definition::Read(reference)) => match definitions.get(&reference.0) {
                Some(Definition::Address(addressed)) if same_storage(addressed, &place) => (
                    addressed.clone(),
                    read_once(*reference).then_some(*reference),
                ),
                _ => continue,
            },
            _ => continue,
        };
        let (place, handle) = operand_handle(
            addressed,
            mutability,
            context.parameters,
            context.declarations,
        );
        rewrites.insert(
            operand.0,
            Rewrite {
                place,
                handle,
                retired,
            },
        );
    }
    if rewrites.is_empty() {
        return;
    }
    for (register, rewrite) in &rewrites {
        function.reg_types.insert(*register, rewrite.handle.clone());
    }
    let retired: Vec<u32> = rewrites
        .values()
        .filter_map(|rewrite| rewrite.retired.map(|register| register.0))
        .collect();
    rewrite_blocks(&mut function.body, &rewrites, &retired);
}

/// Which of a function's slots are its parameters, and how each binds.
struct ParameterKinds {
    count: usize,
    owned: Vec<bool>,
    reference: Vec<bool>,
    /// The function's `ref` bindings ([`MirFunction::ref_bindings`]).
    ref_bindings: BTreeSet<VarId>,
    /// Whether each slot is a `ref` parameter, the receiver counted, whose
    /// mutability is its inferred binder
    /// ([`OriginParamId::reference_binder`]).
    inferred: Vec<bool>,
}

impl ParameterKinds {
    /// The parameters of `function`, whose `declaration` spells each
    /// one's convention.
    fn of(function: &MirFunction, declaration: Option<&MirFunctionDeclaration>) -> Self {
        let inferred = (0..function.n_params)
            .map(|slot| {
                function.ref_params.get(slot).copied().unwrap_or(false)
                    && declaration.is_some_and(|declaration| {
                        let convention = match (declaration.has_receiver, slot) {
                            (true, 0) => declaration.receiver_convention,
                            (true, slot) => declaration
                                .param_conventions
                                .get(slot - 1)
                                .copied()
                                .flatten(),
                            (false, slot) => {
                                declaration.param_conventions.get(slot).copied().flatten()
                            }
                        };
                        convention == Some(ArgConvention::Ref)
                    })
            })
            .collect();
        Self {
            count: function.n_params,
            owned: function.owned_params.clone(),
            reference: function.ref_params.clone(),
            ref_bindings: function.ref_bindings.clone(),
            inferred,
        }
    }

    /// The mutability of a reference to `place` a callee wants with
    /// `wanted`: a `ref` parameter's reference is as mutable as the place
    /// it addresses, as upstream's `ref` binds its operand's own
    /// mutability, so a `read` parameter's storage, or a place reached
    /// through an immutable reference, lends an immutable one, and a place
    /// rooted at this function's own `ref` parameter lends that
    /// parameter's inferred mutability, which a call binds.
    fn place_mutability(&self, place: &MirPlace, wanted: Mutability) -> Mutability {
        let root = place.root as usize;
        let read_parameter = root < self.count
            && !self.owned.get(root).copied().unwrap_or(true)
            && !self.reference.get(root).copied().unwrap_or(true);
        let immutable_reference = self.ref_bindings.contains(&place.root)
            && matches!(
                place.root_ty.as_ref().map(Ty::kind),
                Some(TyKind::Ref(reference)) if reference.mutability == Mutability::Immutable
            );
        if read_parameter || immutable_reference || wanted == Mutability::Immutable {
            return Mutability::Immutable;
        }
        match u32::try_from(root) {
            Ok(slot) if self.inferred.get(root).copied().unwrap_or(false) => {
                Mutability::Param(OriginParamId::reference_binder(slot))
            }
            _ => wanted,
        }
    }
}

/// What [`collect`] reads a function's calls by.
struct Collector<'c, 'a> {
    callees: &'c Callees<'a>,
    declarations: &'c MirDeclarations,
    ref_bindings: &'c BTreeSet<VarId>,
    reg_types: &'c HashMap<u32, Ty>,
    operands: Operands,
}

fn collect(
    block: &MirBlock,
    collector: &Collector<'_, '_>,
    cursor: &mut LegacyCursor<'_>,
    definitions: &mut HashMap<u32, Definition>,
    operands: &mut Vec<(Reg, MirPlace, Mutability)>,
) {
    for instruction in &block.instrs {
        let places = cursor.visit(instruction);
        match instruction {
            MirInstr::LoadPlace { dest, place } => {
                definitions.insert(dest.0, Definition::Load(place.clone()));
            }
            MirInstr::ReadRef { dest, reference } => {
                definitions.insert(dest.0, Definition::Read(*reference));
            }
            MirInstr::MakeRef { dest, place } => {
                definitions.insert(dest.0, Definition::Address(place.clone()));
            }
            _ => {
                if let Some(places) = places
                    && let Some(bound) =
                        bound_call(instruction, collector.callees, collector.reg_types)
                {
                    operands.extend(
                        by_address_operands(
                            &bound,
                            places,
                            instruction,
                            collector.declarations,
                            collector.ref_bindings,
                            collector.operands,
                        )
                        .into_iter()
                        .map(|operand| (operand.register, operand.place, operand.mutability)),
                    );
                }
            }
        }
        for region in instruction_regions(instruction) {
            collect(region, collector, cursor, definitions, operands);
        }
    }
}

/// The receiver and each operand of `instruction`, bound as `bound`, that
/// its callee receives by address, with the place `places` keeps for it:
/// its position among the operands [`operand_registers_mut`] lists, `None`
/// for the receiver. A place holding a reference is judged by its
/// referent, which the operand passes ([`place_value_ty`]); `operands` may
/// keep only such places.
fn by_address_operands(
    bound: &BoundCall<'_>,
    places: &LegacyCallPlaces,
    instruction: &MirInstr,
    declarations: &MirDeclarations,
    ref_bindings: &BTreeSet<VarId>,
    operands: Operands,
) -> Vec<LegacyAddressed> {
    let mut addressed = Vec::new();
    let value_ty = |place: &MirPlace| -> Option<Ty> {
        let binds_root = ref_bindings.contains(&place.root);
        let declares_ref_field =
            |base: &Ty, field: &str| declarations.declares_ref_field(base, field);
        (operands == Operands::All || place_holds_reference(place, binds_root, declares_ref_field))
            .then(|| place_value_ty(place, binds_root, declares_ref_field).cloned())
            .flatten()
    };
    if let Some(register) = bound.receiver
        && let Some(place) = &places.receiver
        && let Some(ty) = value_ty(place)
        && let Some(mutability) = bound.contract.receiver(&ty, declarations)
    {
        addressed.push(LegacyAddressed {
            position: None,
            register,
            place: place.clone(),
            mutability,
            value: ty,
        });
    }
    let operand_places = places.operands(instruction);
    for (position, ((register, parameter), place)) in
        bound.operands.iter().zip(operand_places).enumerate()
    {
        let (Some(place), Some(parameter)) = (place, parameter) else {
            continue;
        };
        let Some(ty) = value_ty(place) else {
            continue;
        };
        if let Some(mutability) = bound.contract.parameter(*parameter, &ty, declarations) {
            addressed.push(LegacyAddressed {
                position: Some(position),
                register: *register,
                place: place.clone(),
                mutability,
                value: ty,
            });
        }
    }
    addressed
}

/// An operand [`by_address_operands`] finds its callee receives by
/// address.
struct LegacyAddressed {
    /// Its position among the operands [`operand_registers_mut`] lists,
    /// `None` for the receiver.
    position: Option<usize>,
    register: Reg,
    /// The place the artifact keeps for it.
    place: MirPlace,
    mutability: Mutability,
    /// The value it passes ([`place_value_ty`]).
    value: Ty,
}

fn rewrite_blocks(block: &mut MirBlock, rewrites: &HashMap<u32, Rewrite>, retired: &[u32]) {
    block.instrs.retain(|instruction| {
        !matches!(instruction, MirInstr::MakeRef { dest, .. } if retired.contains(&dest.0))
    });
    for instruction in &mut block.instrs {
        let dest = match instruction {
            MirInstr::LoadPlace { dest, .. } | MirInstr::ReadRef { dest, .. } => Some(*dest),
            _ => None,
        };
        if let Some(dest) = dest
            && let Some(rewrite) = rewrites.get(&dest.0)
        {
            *instruction = MirInstr::MakeRef {
                dest,
                place: rewrite.place.clone(),
            };
        }
        for region in instruction_regions_mut(instruction) {
            rewrite_blocks(region, rewrites, retired);
        }
    }
}

/// Pass each by-address operand still read as a value as a fresh
/// `ref.make` of its place, emitted just before its call.
fn address_remaining_operands(
    function: &mut MirFunction,
    context: &RemainingContext<'_, '_>,
    mut cursor: LegacyCursor<'_>,
) {
    let mut next = function.n_regs;
    let mut types = Vec::new();
    address_remaining_in(
        &mut function.body,
        &function.reg_types,
        context,
        &mut cursor,
        &mut next,
        &mut types,
    );
    function.n_regs = next;
    for (register, ty, from) in types {
        function.reg_types.insert(register, ty);
        if let Some(span) = function.spans.0.get(&from).cloned() {
            function.spans.0.insert(register, span);
        }
    }
}

/// What [`address_call_operands`] reads a function's calls by.
struct RemainingContext<'c, 'a> {
    declarations: &'c MirDeclarations,
    callees: &'c Callees<'a>,
    parameters: &'c ParameterKinds,
    operands: Operands,
}

fn address_remaining_in(
    block: &mut MirBlock,
    reg_types: &HashMap<u32, Ty>,
    context: &RemainingContext<'_, '_>,
    cursor: &mut LegacyCursor<'_>,
    next: &mut u32,
    types: &mut Vec<(u32, Ty, u32)>,
) {
    let mut index = 0;
    while index < block.instrs.len() {
        for region in instruction_regions_mut(&mut block.instrs[index]) {
            address_remaining_in(region, reg_types, context, cursor, next, types);
        }
        let places = cursor.visit(&block.instrs[index]);
        let is_value = |operand: &LegacyAddressed| {
            reg_types
                .get(&operand.register.0)
                .is_none_or(|ty| addressed_operand(ty, &operand.value).is_none())
        };
        let instruction = &block.instrs[index];
        let remaining: Vec<LegacyAddressed> = places
            .zip(bound_call(instruction, context.callees, reg_types))
            .map(|(places, bound)| {
                by_address_operands(
                    &bound,
                    places,
                    instruction,
                    context.declarations,
                    &context.parameters.ref_bindings,
                    context.operands,
                )
            })
            .unwrap_or_default()
            .into_iter()
            .filter(is_value)
            .collect();
        let mut addresses = Vec::new();
        if !remaining.is_empty() {
            let (receiver, mut operands) = operand_registers_mut(&mut block.instrs[index]);
            let mut receiver = receiver;
            for LegacyAddressed {
                position,
                register,
                place,
                mutability: wanted,
                ..
            } in remaining
            {
                let slot = match position {
                    None => receiver.as_deref_mut(),
                    Some(position) => operands.get_mut(position).map(|slot| &mut **slot),
                };
                let Some(slot) = slot.filter(|slot| **slot == register) else {
                    continue;
                };
                let mutability = context.parameters.place_mutability(&place, wanted);
                let (place, handle) =
                    operand_handle(place, mutability, context.parameters, context.declarations);
                let address = Reg(*next);
                *next += 1;
                types.push((address.0, handle, register.0));
                addresses.push(MirInstr::MakeRef {
                    dest: address,
                    place,
                });
                *slot = address;
            }
        }
        let inserted = addresses.len();
        block.instrs.splice(index..index, addresses);
        index += inserted + 1;
    }
}

/// Pass each call operand the call is its one reader of, and that is
/// defined as a `ref.read` of a `ref.make` of the place the call retains
/// for it, as that reference, the read going; and each one that is a
/// `place.load` of a memory-only retained place as a `ref.make` of it.
fn forward_read_addresses(
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    parameters: &ParameterKinds,
    mut cursor: LegacyCursor<'_>,
) {
    let mut definitions = HashMap::new();
    collect_definitions(&function.body, &mut definitions);
    let mut reads = vec![0; function.n_regs as usize];
    count_register_reads(&mut function.body, &mut reads);
    let mut forwards: HashMap<u32, Reg> = HashMap::new();
    let mut rewrites: HashMap<u32, Rewrite> = HashMap::new();
    let mut pairs = Vec::new();
    retained_operands(&function.body, &mut cursor, &mut pairs);
    for (operand, place) in pairs {
        let binds_root = parameters.ref_bindings.contains(&place.root);
        let declares_ref_field =
            |base: &Ty, field: &str| declarations.declares_ref_field(base, field);
        let addressed = function
            .reg_types
            .get(&operand.0)
            .zip(place_value_ty(&place, binds_root, declares_ref_field))
            .is_some_and(|(ty, value)| addressed_operand(ty, value).is_some());
        if reads.get(operand.0 as usize) != Some(&1)
            || addressed
            || place_holds_reference(&place, binds_root, declares_ref_field)
        {
            continue;
        }
        match definitions.get(&operand.0) {
            Some(Definition::Read(reference)) => {
                if let Some(Definition::Address(addressed)) = definitions.get(&reference.0)
                    && same_storage(addressed, &place)
                {
                    forwards.insert(operand.0, *reference);
                }
            }
            Some(Definition::Load(loaded))
                if same_storage(loaded, &place)
                    && loaded.through == place.through
                    && place
                        .ty
                        .as_ref()
                        .is_some_and(|ty| !declarations.register_passable(ty)) =>
            {
                let mutability = parameters.place_mutability(&place, Mutability::Mutable);
                let (place, handle) = operand_handle(place, mutability, parameters, declarations);
                rewrites.insert(
                    operand.0,
                    Rewrite {
                        place,
                        handle,
                        retired: None,
                    },
                );
            }
            _ => {}
        }
    }
    if !forwards.is_empty() {
        forward_operands(&mut function.body, &forwards);
    }
    if !rewrites.is_empty() {
        for (register, rewrite) in &rewrites {
            function.reg_types.insert(*register, rewrite.handle.clone());
        }
        rewrite_blocks(&mut function.body, &rewrites, &[]);
    }
}

fn collect_definitions(block: &MirBlock, definitions: &mut HashMap<u32, Definition>) {
    for instruction in &block.instrs {
        match instruction {
            MirInstr::LoadPlace { dest, place } => {
                definitions.insert(dest.0, Definition::Load(place.clone()));
            }
            MirInstr::ReadRef { dest, reference } => {
                definitions.insert(dest.0, Definition::Read(*reference));
            }
            MirInstr::MakeRef { dest, place } => {
                definitions.insert(dest.0, Definition::Address(place.clone()));
            }
            _ => {}
        }
        for region in instruction_regions(instruction) {
            collect_definitions(region, definitions);
        }
    }
}

/// Each argument of a `call` or `call.method` with the place the reader
/// kept for it; a method call's receiver place may be the one an owned
/// receiver consumes, so receivers are left to their contracts.
fn retained_operands(
    block: &MirBlock,
    cursor: &mut LegacyCursor<'_>,
    pairs: &mut Vec<(Reg, MirPlace)>,
) {
    for instruction in &block.instrs {
        for region in instruction_regions(instruction) {
            retained_operands(region, cursor, pairs);
        }
        let places = cursor.visit(instruction);
        let (
            Some(places),
            MirInstr::Call {
                args,
                kwargs,
                spread,
                ..
            }
            | MirInstr::MethodCall {
                args,
                kwargs,
                spread,
                ..
            },
        ) = (places, instruction)
        else {
            continue;
        };
        let registers = args
            .iter()
            .chain(kwargs.iter().map(|(_, register)| register));
        // A spread passes its collector whole, which the elaborator expands.
        let operands = registers
            .zip(places.operands(instruction))
            .enumerate()
            .filter(|(position, _)| *spread != Some(*position));
        for (_, (register, place)) in operands {
            pairs.extend(place.map(|place| (*register, place.clone())));
        }
    }
}

/// Replace each operand `forwards` names by its reference, and remove the
/// `ref.read` that defined it.
fn forward_operands(block: &mut MirBlock, forwards: &HashMap<u32, Reg>) {
    block.instrs.retain(|instruction| {
        !matches!(instruction, MirInstr::ReadRef { dest, .. } if forwards.contains_key(&dest.0))
    });
    for instruction in &mut block.instrs {
        let operands: Vec<&mut Reg> = match instruction {
            MirInstr::Call { args, kwargs, .. } | MirInstr::MethodCall { args, kwargs, .. } => args
                .iter_mut()
                .chain(kwargs.iter_mut().map(|(_, register)| register))
                .collect(),
            _ => Vec::new(),
        };
        for operand in operands {
            if let Some(reference) = forwards.get(&operand.0) {
                *operand = *reference;
            }
        }
        for region in instruction_regions_mut(instruction) {
            forward_operands(region, forwards);
        }
    }
}

struct LoadContext<'a, 'b> {
    declarations: &'a MirDeclarations,
    callees: &'a Callees<'b>,
    next: &'a mut u32,
    types: &'a mut Vec<(u32, Ty, u32)>,
    /// The place each `ref.make` of the body addresses, by its register.
    made: &'a HashMap<u32, MirPlace>,
    /// Each register's read count over the body.
    reads: &'a [u32],
    /// The `ref.make` registers whose one reader took their place instead.
    retired: &'a mut Vec<u32>,
}

fn load_in_blocks(
    block: &mut MirBlock,
    reg_types: &HashMap<u32, Ty>,
    context: &mut LoadContext<'_, '_>,
) -> bool {
    let mut changed = false;
    let mut index = 0;
    while index < block.instrs.len() {
        for region in instruction_regions_mut(&mut block.instrs[index]) {
            changed |= load_in_blocks(region, reg_types, context);
        }
        let mut loads = Vec::new();
        let by_value =
            bound_call(&block.instrs[index], context.callees, reg_types).and_then(|bound| {
                if matches!(bound.contract, Contract::Required { .. }) {
                    return None;
                }
                let receiver = bound.receiver.is_some_and(|register| {
                    let Contract::Declared(declaration) = bound.contract else {
                        return false;
                    };
                    reg_types
                        .get(&register.0)
                        .is_some_and(|ty| by_value_receiver(declaration, ty, context.declarations))
                });
                let operands: Vec<Option<Ty>> = bound
                    .operands
                    .iter()
                    .map(|(_, parameter)| {
                        parameter.and_then(|parameter| {
                            bound
                                .contract
                                .by_value_parameter(parameter, context.declarations)
                                .cloned()
                        })
                    })
                    .collect();
                Some((receiver, operands))
            });
        take_register_passable_receiver(&mut block.instrs[index], reg_types, context, &mut loads);
        if let Some((receiver, operands)) = by_value {
            let instruction = &mut block.instrs[index];
            let (receiver_register, registers) = operand_registers_mut(instruction);
            if receiver
                && let Some(register) = receiver_register
                && let Some(reference) = reg_types.get(&register.0).and_then(addressed_receiver)
            {
                read_operand(register, reference, context, &mut loads);
            }
            for (register, parameter_ty) in registers.into_iter().zip(operands) {
                if let Some(reference) = parameter_ty.and_then(|parameter_ty| {
                    addressed_operand(reg_types.get(&register.0)?, &parameter_ty)
                }) {
                    read_operand(register, reference, context, &mut loads);
                }
            }
        }
        let inserted = loads.len();
        changed |= inserted > 0;
        block.instrs.splice(index..index, loads);
        index += inserted + 1;
    }
    changed
}

/// Take a `deinit` receiver an instance makes register-passable out of
/// the storage its `ref.make` addresses, the take going to `loads` and the
/// receiver becoming the owned value, as upstream's `LowerArgConventions`
/// passes a `DeinitMem` operand of a register-passable type by value.
///
/// The pass runs before `automatic-inline`, so a receiver is still the
/// local `ref.make` lowering formed; any other reference is left as is.
fn take_register_passable_receiver(
    instruction: &mut MirInstr,
    reg_types: &HashMap<u32, Ty>,
    context: &mut LoadContext<'_, '_>,
    loads: &mut Vec<MirInstr>,
) {
    if !instruction.deinit_receiver() {
        return;
    }
    let declared = matches!(
        bound_call(instruction, context.callees, reg_types),
        Some(BoundCall {
            contract: Contract::Declared(declaration),
            ..
        }) if declaration.receiver_convention == Some(ArgConvention::Deinit)
    );
    let MirInstr::MethodCall { recv, .. } = instruction else {
        return;
    };
    let Some(reference) = reg_types.get(&recv.0).and_then(addressed_receiver) else {
        return;
    };
    let Some(place) = context.made.get(&recv.0) else {
        return;
    };
    if !declared || !context.declarations.register_passable(&reference.referent) {
        return;
    }
    let taken = Reg(*context.next);
    *context.next += 1;
    context
        .types
        .push((taken.0, reference.referent.clone(), recv.0));
    loads.push(MirInstr::take_place(taken, place.clone()));
    if context.reads.get(recv.0 as usize) == Some(&1) {
        context.retired.push(recv.0);
    }
    *recv = taken;
}

/// The type of a callee's regular parameter `parameter` when it receives
/// it by value where its operand may be a reference: an owned one, or a
/// `read` one an instance's substitution made register-passable.
fn by_value_parameter<'d>(
    declaration: &'d MirFunctionDeclaration,
    declarations: &MirDeclarations,
    parameter: usize,
) -> Option<&'d Ty> {
    (!declaration
        .ref_params
        .get(parameter)
        .copied()
        .unwrap_or(true)
        && by_address_parameter(declaration, declarations, parameter).is_none())
    .then(|| declaration.param_types.get(parameter))
    .flatten()
}

/// Whether a method receives by value the receiver a reference of type
/// `operand_ty` addresses: a `read` receiver an instance's substitution
/// made register-passable.
fn by_value_receiver(
    declaration: &MirFunctionDeclaration,
    operand_ty: &Ty,
    declarations: &MirDeclarations,
) -> bool {
    addressed_receiver(operand_ty).is_some_and(|reference| {
        matches!(
            declaration.receiver_convention,
            None | Some(ArgConvention::Imm)
        ) && by_address_receiver(declaration, &reference.referent, declarations).is_none()
    })
}

/// Read an operand through the `reference` it passes, the `ref.read` going
/// to `loads` and the operand becoming the value it reads.
fn read_operand(
    register: &mut Reg,
    reference: &RefTy,
    context: &mut LoadContext<'_, '_>,
    loads: &mut Vec<MirInstr>,
) {
    let loaded = Reg(*context.next);
    *context.next += 1;
    context
        .types
        .push((loaded.0, reference.referent.clone(), register.0));
    loads.push(MirInstr::ReadRef {
        dest: loaded,
        reference: *register,
    });
    *register = loaded;
}

/// Record the place each `ref.make` of `block`, its regions included,
/// addresses.
fn collect_made_places(block: &MirBlock, made: &mut HashMap<u32, MirPlace>) {
    for instruction in &block.instrs {
        if let MirInstr::MakeRef { dest, place } = instruction {
            made.insert(dest.0, place.clone());
        }
        for region in instruction_regions(instruction) {
            collect_made_places(region, made);
        }
    }
}

/// Remove the `ref.make` of each register in `retired` from `block`, its
/// regions included.
fn retire_made_references(block: &mut MirBlock, retired: &[u32]) {
    block.instrs.retain(
        |instruction| !matches!(instruction, MirInstr::MakeRef { dest, .. } if retired.contains(&dest.0)),
    );
    for instruction in &mut block.instrs {
        for region in instruction_regions_mut(instruction) {
            retire_made_references(region, retired);
        }
    }
}

/// The reference layers `ty` nests: each `TyKind::Ref` and `!lit.ref`
/// (`TyKind::LitRef`) layer, so the count is the same whichever tag a
/// reference carries. A pointer is a value, not a layer.
fn reference_depth(ty: &Ty) -> usize {
    let mut depth = 0;
    let mut current = ty;
    while let TyKind::Ref(reference) | TyKind::LitRef(reference) = current.kind() {
        depth += 1;
        current = &reference.referent;
    }
    depth
}

#[cfg(test)]
mod tests {
    use super::*;
    use mojito_types::origin::{Origin, RefTy};
    use mojito_types::types::{AddressSpaceSlot, TyArg};

    fn list_of_span(origin: Origin) -> Ty {
        let span: Ty = TyKind::Struct("StringSpan".into(), vec![TyArg::Origin(origin)]).into();
        TyKind::Struct("List".into(), vec![TyArg::Ty(span)]).into()
    }

    fn reference(referent: Ty) -> Ty {
        TyKind::Ref(RefTy {
            referent,
            origin: Origin::Static,
            mutability: Mutability::Immutable,
            address_space: AddressSpaceSlot::GENERIC,
        })
        .into()
    }

    fn field_place(ty: Ty) -> MirPlace {
        MirPlace {
            root: 0,
            root_ty: None,
            proj: vec![Proj::Field("src".into())],
            projection_tys: vec![ty.clone()],
            ty: Some(ty),
            through: None,
        }
    }

    #[test]
    fn make_ref_forwards_stored_handle_across_origin_spellings() {
        let place = field_place(reference(list_of_span(Origin::Static)));
        assert!(make_ref_forwards_stored_handle(
            &place,
            &reference(list_of_span(Origin::Unbound))
        ));
    }

    #[test]
    fn make_ref_forwards_stored_handle_never_for_a_slot_borrow() {
        let stored = reference(list_of_span(Origin::Static));
        let place = field_place(stored.clone());
        assert!(!make_ref_forwards_stored_handle(&place, &reference(stored)));
        let held = field_place(list_of_span(Origin::Static));
        assert!(!make_ref_forwards_stored_handle(
            &held,
            &reference(list_of_span(Origin::Static))
        ));
    }

    fn holder() -> Ty {
        TyKind::Struct("Holder".into(), Vec::new()).into()
    }

    #[test]
    fn stored_handle_forwards_only_a_declared_ref_field() {
        let stored = reference(list_of_span(Origin::Static));
        let mut place = field_place(stored);
        place.root_ty = Some(holder());
        let declares = |base: &Ty, field: &str| *base == holder() && field == "src";
        let (forwarded, _) = stored_handle(&place, false, declares, Mutability::Mutable)
            .expect("a `ref` field forwards its stored handle");
        assert!(same_storage(&forwarded, &place));
        assert!(stored_handle(&place, false, |_, _| false, Mutability::Mutable).is_none());
    }

    #[test]
    fn stored_handle_forwards_only_a_ref_binding_slot() {
        let stored = reference(list_of_span(Origin::Static));
        let place = MirPlace::root(0, Some(stored));
        let (handle_place, _) = stored_handle(&place, true, |_, _| false, Mutability::Mutable)
            .expect("a `ref` binding's slot forwards its handle");
        assert_eq!(handle_place.through, Some(0));
        assert!(stored_handle(&place, false, |_, _| false, Mutability::Mutable).is_none());
    }

    #[test]
    fn a_non_binding_reference_root_is_ordinary_storage() {
        let stored = reference(list_of_span(Origin::Static));
        let place = MirPlace::root(0, Some(stored.clone()));
        assert_eq!(
            mir_place_handle_ty(&place, true, Some(Mutability::Mutable)),
            Some(stored.clone())
        );
        let handle = mir_place_handle_ty(&place, false, Some(Mutability::Mutable))
            .expect("a typed slot has a handle");
        assert!(matches!(handle.kind(), TyKind::Ref(outer) if outer.referent == stored));
    }
}

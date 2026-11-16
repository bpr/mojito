//! By-address call operands.
//!
//! Upstream's `lit.call` takes an argument its callee receives by address —
//! a `mut` or `ref` parameter, or a `read` one of a memory-only type — as a
//! `!lit.ref` of the caller's storage, and loads nothing. MIR lowering
//! forms that `ref.make` of the operand's retained place where it lowers
//! the call (`Flatten::address_operand`), by the check's verdict, and the
//! instruction's place table names the same storage. This module reads the
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
//! A register-passable `read` argument is passed by value, as upstream's
//! `LowerArgConventions` caller half passes it: one an instance's
//! substitution makes register-passable (a `read x: T` at `T = Int`) is read
//! through its reference once elaborated ([`load_register_passable_operands`]).
//! An artifact written before lowering passed by-address operands as
//! references has its calls addressed by that contract as it loads
//! ([`upgrade_call_operands`]).

use crate::mir::bound_calls::callable_bound_requirement;
use crate::mir::{
    MirBlock, MirDeclarations, MirFunction, MirFunctionDeclaration, MirInstr, MirPlace,
    MirSubscriptArg, Proj, Reg, TraitRequirement, count_register_reads, instruction_regions,
    instruction_regions_mut, mir_place_handle_ty,
};
use mojito_ast::ast::ArgConvention;
use mojito_ast::call::{ArgSlot, CallVariadics, match_call_slots};
use mojito_symbol::symbol::{is_initializer_symbol, is_trait_dispatch_symbol};
use mojito_types::origin::{Mutability, OriginParamId};
use mojito_types::types::{FuncTy, Ty, TyKind};
use std::collections::HashMap;

/// The regular parameter each operand of a direct call of `declaration`
/// binds.
///
/// One entry per positional operand, then one per keyword operand,
/// `None` for an operand a collector takes. `None` overall when the call
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
    kwargs: &[(String, Reg)],
) -> Option<Vec<Option<usize>>> {
    if declaration.has_receiver && !is_initializer_symbol(callee) {
        return None;
    }
    let keywords: Vec<&str> = kwargs.iter().map(|(name, _)| name.as_str()).collect();
    operand_parameters(&CallShape::declared(declaration), positional, &keywords)
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

/// The value an operand of `ty` passes: the referent of a reference.
#[must_use]
pub fn operand_value_ty(ty: &Ty) -> &Ty {
    match ty.kind() {
        TyKind::Ref(reference) => &reference.referent,
        _ => ty,
    }
}

/// Upgrade the calls of `functions`, read from an artifact written before
/// lowering passed by-address operands as references.
///
/// Each by-address operand its contract binds is addressed as lowering
/// addresses it ([`address_call_operands`]), then every operand
/// a call's place table names that no contract binds (a requirement call
/// over a pack element's receiver) and that is by address whatever its
/// convention: a read back through a `ref.make` of that same place, the
/// address an older lowering already formed, or a load of a memory-only
/// place.
pub fn upgrade_call_operands(
    functions: &mut [(String, MirFunction)],
    declarations: &MirDeclarations,
) {
    address_call_operands(functions, declarations, Operands::All);
    for (name, function) in functions.iter_mut() {
        let parameters = ParameterKinds::of(function, declarations.function(name));
        forward_read_addresses(function, declarations, &parameters);
    }
}

/// Upgrade the calls of `functions`, read from an artifact written before
/// lowering forwarded the reference a place holds.
///
/// A place holding a reference is a `ref` field or a `ref` binding's slot,
/// and an older call passed the value read through it as a by-address
/// operand: each such operand its contract binds forwards the stored
/// handle, as lowering forwards it ([`stored_handle`]).
pub fn upgrade_stored_handles(
    functions: &mut [(String, MirFunction)],
    declarations: &MirDeclarations,
) {
    address_call_operands(functions, declarations, Operands::StoredHandles);
}

/// Read a call's by-address operand through its reference where the
/// callee takes the parameter by value.
///
/// `function` is an elaborated instance, and the read is a `ref.read`, as
/// upstream's `LowerArgConventions` loads a register-passable argument at
/// the call. Returns whether any operand was read.
pub fn load_register_passable_operands(
    function: &mut MirFunction,
    declarations: &MirDeclarations,
) -> bool {
    let callees = Callees::of(declarations);
    let mut next = function.n_regs;
    let mut types = Vec::new();
    let changed = load_in_blocks(
        &mut function.body,
        &function.reg_types,
        &mut LoadContext {
            declarations,
            callees: &callees,
            next: &mut next,
            types: &mut types,
        },
    );
    function.n_regs = next;
    for (register, ty, from) in types {
        function.reg_types.insert(register, ty);
        if let Some(span) = function.spans.0.get(&from).cloned() {
            function.spans.0.insert(register, span);
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
/// lowering forms it. Each operand whose register is a reference becomes
/// a `ref.read` of it, returned for the caller to splice ahead of the
/// call, as upstream's `LowerArgConventions` loads a `!lit.ref` at a
/// by-value slot: the decision is the operand's type alone. Its
/// place-table entry is cleared, since the loaded argument names no
/// storage.
pub fn read_addressed_operands<'r>(
    function: &mut MirFunction,
    operands: impl IntoIterator<Item = (&'r mut Reg, OperandTable<'r>)>,
) -> Vec<MirInstr> {
    let mut loads = Vec::new();
    for (register, table) in operands {
        let Some(TyKind::Ref(reference)) = function.reg_types.get(&register.0).map(Ty::kind) else {
            continue;
        };
        table.clear();
        let referent = reference.referent.clone();
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

/// The place-table entry of an operand [`read_addressed_operands`] may
/// load, which the load clears.
pub enum OperandTable<'r> {
    /// An entry the load clears: a positional or keyword operand's.
    Owned(&'r mut Option<MirPlace>),
    /// A receiver's entry, which the load clears.
    OwnedBoxed(&'r mut Option<Box<MirPlace>>),
}

impl OperandTable<'_> {
    fn clear(self) {
        match self {
            Self::Owned(place) => *place = None,
            Self::OwnedBoxed(place) => *place = None,
        }
    }
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
/// A `ref` field's own place forwards its stored handle, lent immutably
/// when `mutability` is; a `ref` binding's slot is reached through itself,
/// as a runtime alias's read forms it, and lends its own handle.
pub fn stored_handle(place: &MirPlace, mutability: Mutability) -> Option<(MirPlace, Ty)> {
    let TyKind::Ref(stored) = place.ty.as_ref()?.kind() else {
        return None;
    };
    if place.proj.is_empty() {
        let mut handle_place = place.clone();
        handle_place.through = place.through.or(Some(place.root));
        let handle = mir_place_handle_ty(&handle_place, Some(mutability))?;
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
    })
    .into();
    Some((place.clone(), handle))
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
    /// `dispatch` binds over a receiver of type `receiver`: the one the
    /// bound it was selected through declares (`refined_bound`), else the
    /// receiver type parameter's own bounds.
    fn requirement(
        &self,
        dispatch: &str,
        receiver: Option<&Ty>,
        refined_bound: Option<&[String]>,
    ) -> Option<&'a TraitRequirement> {
        if !is_trait_dispatch_symbol(dispatch) {
            return None;
        }
        let bounds = refined_bound.or_else(|| match receiver.map(Ty::kind) {
            Some(TyKind::Param { bounds, .. }) => Some(bounds.as_slice()),
            _ => None,
        })?;
        self.declarations.bound_requirement(bounds, dispatch)
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
            Self::Required { requirement, .. } => CallShape {
                names: &requirement.param_names,
                required: &requirement.required,
                positional_only: requirement.positional_only,
                keyword_only: requirement.keyword_only,
                variadics: CallVariadics {
                    positional: requirement.variadic.is_some(),
                    keyword: requirement.kw_variadic.is_some(),
                },
            },
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
                by_address(receiver, is_reference(receiver), ty, declarations)
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
                let convention = requirement.param_conventions.get(parameter).copied()?;
                by_address(
                    convention,
                    is_reference(convention),
                    operand_ty,
                    declarations,
                )
            }
            Self::Copy => by_address(None, false, operand_ty, declarations),
            Self::Signature(signature) => {
                let (convention, reference, ty) = signature_parameter(signature, parameter)?;
                by_address(convention, reference, ty, declarations)
            }
        }
    }

    /// Whether the callee receives its regular parameter `parameter` by
    /// value where its operand may be a reference: an owned one, or a
    /// `read` one an instance's substitution made register-passable. A
    /// requirement's parameters are never read here: the elaborator names
    /// the witness, whose declaration binds them, and writes the copy
    /// initializer's call itself.
    fn by_value_parameter(self, parameter: usize, declarations: &MirDeclarations) -> bool {
        match self {
            Self::Declared(declaration) => by_value_parameter(declaration, declarations, parameter),
            Self::Required { .. } | Self::Copy => false,
            Self::Signature(signature) => signature_parameter(signature, parameter).is_some_and(
                |(convention, reference, ty)| {
                    !reference
                        && by_address(convention, false, ty, declarations).is_none()
                        && !matches!(ty.kind(), TyKind::Ref(_))
                },
            ),
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
fn operand_parameters(
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
    /// The receiver operand and the place it reads, for a method call or a
    /// subscript.
    receiver: Option<(Reg, Option<&'i MirPlace>)>,
    /// Each regular operand, positional then keyword, with its retained
    /// place and the parameter it binds (`None` for a collector's).
    operands: Vec<(Reg, Option<&'i MirPlace>, Option<usize>)>,
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
/// place's address, formed by lowering and read through where the bound
/// closes to a function value ([`super::indirect_from_bound_call`]).
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
            arg_places,
            kwarg_places,
            spread: None,
            ..
        } => {
            let declaration = callees.get(&func.0)?;
            let parameters = call_operand_parameters(&func.0, declaration, args.len(), kwargs)?;
            Some(BoundCall {
                contract: Contract::Declared(declaration),
                receiver: None,
                operands: operand_list(args, kwargs, arg_places, kwarg_places, &parameters),
            })
        }
        MirInstr::CallIndirect {
            callee,
            args,
            kwargs,
            arg_places,
            kwarg_places,
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
            let parameters = operand_parameters(&contract.shape()?, args.len(), &keywords)?;
            Some(BoundCall {
                contract,
                receiver: None,
                operands: operand_list(args, kwargs, arg_places, kwarg_places, &parameters),
            })
        }
        MirInstr::MethodCall {
            recv,
            recv_place,
            resolved: Some(callee),
            args,
            kwargs,
            arg_places,
            kwarg_places,
            spread: None,
            recv_writes,
            receiver_convention,
            refined_bound,
            ..
        } => {
            let receiver_ty = reg_types.get(&recv.0).map(operand_value_ty);
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
            let parameters = operand_parameters(&contract.shape()?, args.len(), &keywords)?;
            // A requirement call whose receiver convention went unrecorded
            // but which writes its receiver is left to its place, and a
            // `def(...)` bound's callee is the address lowering formed.
            let unrecorded = matches!(contract, Contract::Required { receiver: None, .. })
                && *recv_writes
                || matches!(contract, Contract::Signature(_));
            Some(BoundCall {
                contract,
                receiver: (!unrecorded).then_some((*recv, recv_place.as_deref())),
                operands: operand_list(args, kwargs, arg_places, kwarg_places, &parameters),
            })
        }
        MirInstr::ConstructTypeParam {
            kwargs,
            kwarg_places,
            element: None,
            ..
        } => match (kwargs.as_slice(), kwarg_places.as_slice()) {
            ([(keyword, source)], [place]) if keyword == "copy" => Some(BoundCall {
                contract: Contract::Copy,
                receiver: None,
                operands: vec![(*source, place.as_ref(), Some(0))],
            }),
            _ => None,
        },
        _ => {
            let subscript = SubscriptOperands::of(instruction)?;
            let declaration = callees.get(subscript.callee)?;
            let names: Vec<&str> = subscript.keywords.iter().map(|(name, _)| *name).collect();
            let parameters = operand_parameters(
                &CallShape::declared(declaration),
                subscript.positional.len(),
                &names,
            )?;
            let registers = subscript
                .positional
                .into_iter()
                .chain(subscript.keywords.into_iter().map(|(_, register)| register));
            Some(BoundCall {
                contract: Contract::Declared(declaration),
                receiver: Some(subscript.receiver),
                operands: registers
                    .zip(subscript.places)
                    .zip(parameters)
                    .map(|((register, place), parameter)| (register, place, parameter))
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
    /// The base and the place it reads.
    receiver: (Reg, Option<&'i MirPlace>),
    positional: Vec<Reg>,
    /// A keyword index, or a store's value under the setter's keyword.
    keywords: Vec<(&'i str, Reg)>,
    /// Each positional then keyword operand's retained place.
    places: Vec<Option<&'i MirPlace>>,
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
                base_place,
                index_place,
                call: Some(call),
                ..
            } => {
                let indexed = !call.arguments.is_empty();
                Some(Self {
                    callee: &call.target,
                    receiver: (*base, base_place.as_deref()),
                    positional: indexed.then_some(*index).into_iter().collect(),
                    keywords: Vec::new(),
                    places: indexed
                        .then_some(index_place.as_deref())
                        .into_iter()
                        .collect(),
                })
            }
            MirInstr::MultiIndex {
                object,
                args,
                object_place,
                arg_places,
                kwargs,
                kwarg_places,
                call: Some(call),
                ..
            } => Some(Self {
                callee: &call.target,
                receiver: (*object, object_place.as_deref()),
                positional: args
                    .iter()
                    .map(|MirSubscriptArg::Index(reg)| *reg)
                    .collect(),
                keywords: kwargs
                    .iter()
                    .map(|(name, MirSubscriptArg::Index(reg))| (name.as_str(), *reg))
                    .collect(),
                places: arg_places
                    .iter()
                    .chain(kwarg_places)
                    .map(Option::as_ref)
                    .collect(),
            }),
            MirInstr::MultiSet {
                receiver,
                receiver_place,
                args,
                arg_places,
                value,
                value_place,
                value_keyword,
                call,
            } => Some(Self {
                callee: &call.target,
                receiver: (*receiver, receiver_place.as_deref()),
                positional: args
                    .iter()
                    .map(|MirSubscriptArg::Index(reg)| *reg)
                    .collect(),
                keywords: vec![(value_keyword.as_str(), *value)],
                places: arg_places
                    .iter()
                    .map(Option::as_ref)
                    .chain([value_place.as_deref()])
                    .collect(),
            }),
            _ => None,
        }
    }
}

/// A call's positional then keyword operands with their places and the
/// parameters they bind.
fn operand_list<'i>(
    args: &[Reg],
    kwargs: &[(String, Reg)],
    arg_places: &'i [Option<MirPlace>],
    kwarg_places: &'i [Option<MirPlace>],
    parameters: &[Option<usize>],
) -> Vec<(Reg, Option<&'i MirPlace>, Option<usize>)> {
    args.iter()
        .chain(kwargs.iter().map(|(_, register)| register))
        .zip(arg_places.iter().chain(kwarg_places).map(Option::as_ref))
        .zip(parameters)
        .map(|((register, place), parameter)| (*register, place, *parameter))
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
            value,
            ..
        } => (
            Some(receiver),
            args.iter_mut()
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
fn operand_handle(place: MirPlace, mutability: Mutability) -> (MirPlace, Ty) {
    stored_handle(&place, mutability).unwrap_or_else(|| {
        let handle = mir_place_handle_ty(&place, Some(mutability)).unwrap_or_else(Ty::error);
        (place, handle)
    })
}

/// Pass each by-address call operand in `functions` as a `ref.make` of its
/// retained place, rather than a whole load of it.
///
/// The calls are every direct call, call through a function value,
/// resolved method call, checked subscript, and type parameter's copy
/// construction of an artifact [`upgrade_call_operands`] loads.
///
/// The operand's definition is rewritten when its one reader is the call
/// and it reads the storage the call's place table names: a `place.load`
/// of it, or a `ref.read` of a `ref.make` of it, which goes too when
/// nothing else reads it. Any other by-address operand, one whose read
/// another use shares or that is no whole read of the place, is passed as
/// a fresh `ref.make` of the place just before its call. A place holding a
/// reference passes the reference it holds ([`operand_handle`]).
/// `operands` picks which operands are addressed.
fn address_call_operands(
    functions: &mut [(String, MirFunction)],
    declarations: &MirDeclarations,
    operands: Operands,
) {
    let callees = Callees::of(declarations);
    for (name, function) in functions.iter_mut() {
        let parameters = ParameterKinds::of(function, declarations.function(name));
        let context = RemainingContext {
            declarations,
            callees: &callees,
            parameters: &parameters,
            operands,
        };
        address_function_operands(function, &context);
        address_remaining_operands(function, &context);
    }
}

fn address_function_operands(function: &mut MirFunction, context: &RemainingContext<'_, '_>) {
    let mut definitions = HashMap::new();
    let mut operands = Vec::new();
    collect(
        &function.body,
        &Collector {
            callees: context.callees,
            declarations: context.declarations,
            reg_types: &function.reg_types,
            operands: context.operands,
        },
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
        let (place, handle) = operand_handle(addressed, mutability);
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
        let immutable_reference = matches!(
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
    reg_types: &'c HashMap<u32, Ty>,
    operands: Operands,
}

fn collect(
    block: &MirBlock,
    collector: &Collector<'_, '_>,
    definitions: &mut HashMap<u32, Definition>,
    operands: &mut Vec<(Reg, MirPlace, Mutability)>,
) {
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
            _ => {
                if let Some(bound) = bound_call(instruction, collector.callees, collector.reg_types)
                {
                    operands.extend(
                        by_address_operands(&bound, collector.declarations, collector.operands)
                            .into_iter()
                            .map(|(_, register, place, mutability)| (register, place, mutability)),
                    );
                }
            }
        }
        for region in instruction_regions(instruction) {
            collect(region, collector, definitions, operands);
        }
    }
}

/// The receiver and each operand of a call that its callee receives by
/// address, with the place it addresses: its position among the
/// operands [`operand_registers_mut`] lists, `None` for the receiver. A
/// place holding a reference is judged by its referent, which the operand
/// passes; `operands` may keep only such places.
fn by_address_operands(
    bound: &BoundCall<'_>,
    declarations: &MirDeclarations,
    operands: Operands,
) -> Vec<(Option<usize>, Reg, MirPlace, Mutability)> {
    let mut addressed = Vec::new();
    let value_ty = |place: &MirPlace| -> Option<Ty> {
        let ty = place.ty.as_ref()?;
        let holds_reference = matches!(ty.kind(), TyKind::Ref(_));
        (operands == Operands::All || holds_reference).then(|| operand_value_ty(ty).clone())
    };
    if let Some((register, Some(place))) = bound.receiver
        && let Some(ty) = value_ty(place)
        && let Some(mutability) = bound.contract.receiver(&ty, declarations)
    {
        addressed.push((None, register, place.clone(), mutability));
    }
    for (position, (register, place, parameter)) in bound.operands.iter().enumerate() {
        let (Some(place), Some(parameter)) = (place, parameter) else {
            continue;
        };
        let Some(ty) = value_ty(place) else {
            continue;
        };
        if let Some(mutability) = bound.contract.parameter(*parameter, &ty, declarations) {
            addressed.push((Some(position), *register, (*place).clone(), mutability));
        }
    }
    addressed
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
fn address_remaining_operands(function: &mut MirFunction, context: &RemainingContext<'_, '_>) {
    let mut next = function.n_regs;
    let mut types = Vec::new();
    address_remaining_in(
        &mut function.body,
        &function.reg_types,
        context,
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
    next: &mut u32,
    types: &mut Vec<(u32, Ty, u32)>,
) {
    let mut index = 0;
    while index < block.instrs.len() {
        for region in instruction_regions_mut(&mut block.instrs[index]) {
            address_remaining_in(region, reg_types, context, next, types);
        }
        let is_value = |register: Reg| {
            !matches!(
                reg_types.get(&register.0).map(Ty::kind),
                Some(TyKind::Ref(_))
            )
        };
        let remaining: Vec<(Option<usize>, Reg, MirPlace, Mutability)> =
            bound_call(&block.instrs[index], context.callees, reg_types)
                .map(|bound| by_address_operands(&bound, context.declarations, context.operands))
                .unwrap_or_default()
                .into_iter()
                .filter(|(_, register, ..)| is_value(*register))
                .collect();
        let mut addresses = Vec::new();
        if !remaining.is_empty() {
            let (receiver, mut operands) = operand_registers_mut(&mut block.instrs[index]);
            let mut receiver = receiver;
            for (position, register, place, wanted) in remaining {
                let slot = match position {
                    None => receiver.as_deref_mut(),
                    Some(position) => operands.get_mut(position).map(|slot| &mut **slot),
                };
                let Some(slot) = slot.filter(|slot| **slot == register) else {
                    continue;
                };
                let mutability = context.parameters.place_mutability(&place, wanted);
                let (place, handle) = operand_handle(place, mutability);
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
) {
    let mut definitions = HashMap::new();
    collect_definitions(&function.body, &mut definitions);
    let mut reads = vec![0; function.n_regs as usize];
    count_register_reads(&mut function.body, &mut reads);
    let mut forwards: HashMap<u32, Reg> = HashMap::new();
    let mut rewrites: HashMap<u32, Rewrite> = HashMap::new();
    let mut pairs = Vec::new();
    retained_operands(&function.body, &mut pairs);
    let is_value = |ty: Option<&Ty>| !matches!(ty.map(Ty::kind), Some(TyKind::Ref(_)));
    for (operand, place) in pairs {
        if reads.get(operand.0 as usize) != Some(&1)
            || !is_value(function.reg_types.get(&operand.0))
            || !is_value(place.ty.as_ref())
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
                let (place, handle) = operand_handle(place, mutability);
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

/// Each argument of a `call` or `call.method` with the place its table
/// retains for it; a method call's receiver place may be the one an
/// owned receiver consumes, so receivers are left to their contracts.
fn retained_operands(block: &MirBlock, pairs: &mut Vec<(Reg, MirPlace)>) {
    for instruction in &block.instrs {
        for region in instruction_regions(instruction) {
            retained_operands(region, pairs);
        }
        let (MirInstr::Call {
            args,
            kwargs,
            arg_places,
            kwarg_places,
            spread,
            ..
        }
        | MirInstr::MethodCall {
            args,
            kwargs,
            arg_places,
            kwarg_places,
            spread,
            ..
        }) = instruction
        else {
            continue;
        };
        let registers = args
            .iter()
            .chain(kwargs.iter().map(|(_, register)| register));
        // A spread passes its collector whole, which the elaborator expands.
        let operands = registers
            .zip(arg_places.iter().chain(kwarg_places))
            .enumerate()
            .filter(|(position, _)| *spread != Some(*position));
        for (_, (register, place)) in operands {
            pairs.extend(place.clone().map(|place| (*register, place)));
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
                let receiver = bound.receiver.is_some_and(|(register, _)| {
                    let Contract::Declared(declaration) = bound.contract else {
                        return false;
                    };
                    reg_types
                        .get(&register.0)
                        .is_some_and(|ty| by_value_receiver(declaration, ty, context.declarations))
                });
                let operands: Vec<bool> = bound
                    .operands
                    .iter()
                    .map(|(_, _, parameter)| {
                        parameter.is_some_and(|parameter| {
                            bound
                                .contract
                                .by_value_parameter(parameter, context.declarations)
                        })
                    })
                    .collect();
                Some((receiver, operands))
            });
        if let Some((receiver, operands)) = by_value {
            let instruction = &mut block.instrs[index];
            let mut loaded = Vec::new();
            let (receiver_register, registers) = operand_registers_mut(instruction);
            if receiver
                && let Some(register) = receiver_register
                && read_operand(register, reg_types, context, &mut loads)
            {
                loaded.push(None);
            }
            for (position, (register, by_value)) in registers.into_iter().zip(operands).enumerate()
            {
                if by_value && read_operand(register, reg_types, context, &mut loads) {
                    loaded.push(Some(position));
                }
            }
            for position in loaded {
                clear_operand_table(instruction, position);
            }
        }
        let inserted = loads.len();
        changed |= inserted > 0;
        block.instrs.splice(index..index, loads);
        index += inserted + 1;
    }
    changed
}

/// Whether a callee receives its regular parameter `parameter` by value
/// where its operand may be a reference: an owned one, or a `read` one an
/// instance's substitution made register-passable.
fn by_value_parameter(
    declaration: &MirFunctionDeclaration,
    declarations: &MirDeclarations,
    parameter: usize,
) -> bool {
    !declaration
        .ref_params
        .get(parameter)
        .copied()
        .unwrap_or(true)
        && by_address_parameter(declaration, declarations, parameter).is_none()
        && !matches!(
            declaration.param_types.get(parameter).map(Ty::kind),
            Some(TyKind::Ref(_))
        )
}

/// Whether a method receives by value the receiver a reference of type
/// `operand_ty` addresses: a `read` receiver an instance's substitution
/// made register-passable.
fn by_value_receiver(
    declaration: &MirFunctionDeclaration,
    operand_ty: &Ty,
    declarations: &MirDeclarations,
) -> bool {
    matches!(operand_ty.kind(), TyKind::Ref(reference)
        if matches!(declaration.receiver_convention, None | Some(ArgConvention::Imm))
            && by_address_receiver(declaration, &reference.referent, declarations).is_none())
}

/// Read a reference operand through, the `ref.read` going to `loads` and
/// the operand becoming the value it reads; returns whether it was read.
fn read_operand(
    register: &mut Reg,
    reg_types: &HashMap<u32, Ty>,
    context: &mut LoadContext<'_, '_>,
    loads: &mut Vec<MirInstr>,
) -> bool {
    let Some(TyKind::Ref(reference)) = reg_types.get(&register.0).map(Ty::kind) else {
        return false;
    };
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
    true
}

/// Clear the place-table entry of the operand at `position` among those
/// [`operand_registers_mut`] lists (`None` for the receiver), once the
/// operand passes a value: upstream's call of a loaded argument names no
/// storage for it.
fn clear_operand_table(instruction: &mut MirInstr, position: Option<usize>) {
    let clear = |places: &mut Vec<Option<MirPlace>>, position: usize| {
        if let Some(place) = places.get_mut(position) {
            *place = None;
        }
    };
    match (instruction, position) {
        (
            MirInstr::MethodCall {
                recv_place: place, ..
            }
            | MirInstr::Index {
                base_place: place, ..
            }
            | MirInstr::MultiIndex {
                object_place: place,
                ..
            }
            | MirInstr::MultiSet {
                receiver_place: place,
                ..
            },
            None,
        )
        | (
            MirInstr::Index {
                index_place: place, ..
            },
            Some(_),
        ) => *place = None,
        (
            MirInstr::Call {
                args,
                arg_places,
                kwarg_places,
                ..
            }
            | MirInstr::CallIndirect {
                args,
                arg_places,
                kwarg_places,
                ..
            }
            | MirInstr::MethodCall {
                args,
                arg_places,
                kwarg_places,
                ..
            },
            Some(position),
        ) => match position.checked_sub(args.len()) {
            None => clear(arg_places, position),
            Some(keyword) => clear(kwarg_places, keyword),
        },
        (
            MirInstr::MultiIndex {
                args,
                arg_places,
                kwarg_places,
                ..
            },
            Some(position),
        ) => match position.checked_sub(args.len()) {
            None => clear(arg_places, position),
            Some(keyword) => clear(kwarg_places, keyword),
        },
        (
            MirInstr::MultiSet {
                args,
                arg_places,
                value_place,
                ..
            },
            Some(position),
        ) => {
            if position < args.len() {
                clear(arg_places, position);
            } else {
                *value_place = None;
            }
        }
        (MirInstr::ConstructTypeParam { kwarg_places, .. }, Some(position)) => {
            clear(kwarg_places, position);
        }
        _ => {}
    }
}

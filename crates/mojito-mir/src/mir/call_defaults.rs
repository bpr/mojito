//! Omitted arguments, written into the call.
//!
//! Upstream's call binding writes every argument a call leaves out into the
//! call, as the callee's default attribute evaluated in the caller: an
//! evaluated default is its compile-time application, which the elaborator
//! evaluates once per instance and each call materializes; a borrowed
//! default is the caller's temporary, destroyed once the call returns, and
//! an owned one is handed to the callee. The lowering of each
//! checked call records the callee it selected ([`LoweredCall`]), and once
//! every declaration is lowered [`fill_omitted_arguments`] binds each
//! recorded call against that declaration and fills every slot it leaves
//! out, before ownership analysis, so the filled argument is destroyed like
//! any written one and a default instance is named by the call that needs
//! it. A call of a generator without the argument passes none, so the view
//! of a body with its calls so rewritten evaluates no default for it
//! ([`erase_unpassed_defaults`]).
//!
//! The same binding builds the positional collector a callee's `*xs`
//! takes, as upstream's call builds the `VariadicPack` or `VariadicList` it
//! passes: a `tuple.make` of the call's overflow operands, or a homogeneous
//! collector's `VariadicList` over pointers to them
//! ([`crate::mir::variadics`]), passed as one operand. It
//! builds the keyword collector a callee's `**kwargs` takes as upstream's
//! call builds the dictionary: the collector's empty constructor, one
//! `_insert` per collected keyword, and the filled collector passed as the
//! call's one `**` operand.

use crate::mir::call_addresses::requirement_by_address_parameter;
use crate::mir::type_construction::storage_construction;
use crate::mir::variadics::{
    FrameTables, ListElement, build_variadic_list, build_variadic_pack, element_lit_ref_type,
};
use crate::mir::verify::instruction_result_regs;
use crate::mir::{
    CALL_RESULT_SLOT, Const, FuncRef, LoweredCall, MirArgConventions, MirBlock, MirDeclarations,
    MirFunction, MirFunctionDeclaration, MirInstr, MirParamArg, MirPlace, MirSubscriptArg, Proj,
    Reg, UseMode, by_address_parameter, callable_targets, calls::literal_materializes_to,
    fresh_register, instruction_regions, instruction_regions_mut, instruction_regs_mut,
    instruction_slots, mir_place_handle_ty, nested_instructions, terminator_regs_mut,
};
use mojito_ast::ast::{ArgConvention, FORWARDED_KWARGS_NAME};
use mojito_ast::call::{ArgSlot, CallVariadics, match_call_slots};
use mojito_checked::checked::{CheckedArgumentConvention, CheckedCallArgumentSource, CheckedConst};
use mojito_checked::declaration::TraitRequirement;
use mojito_common::token::SourceSpan;
use mojito_hir::hir::VarId;
use mojito_symbol::symbol::{LiftedOwner, lifted_owner};
use mojito_types::ct::CtValue;
use mojito_types::origin::Mutability;
use mojito_types::param_expr::{MetaTy, ParamBindings, ParamContext, ParamExpr, ParamKind};
use mojito_types::types::{
    ParamDecl, Ty, TyArg, TyKind, default_literal, mentions, names_binder,
    replace_argument_parameters, replace_parameters,
};
use std::collections::{BTreeSet, HashMap, HashSet};

/// Fill, at every call `functions` lowered from a checked call
/// ([`MirFunction::lowered_calls`]), each argument it leaves out of a
/// callee `declarations` declares, and clear the records.
///
/// Each such call also builds the positional and keyword collectors it
/// passes (the private `build_collector` and `build_keyword_collector`).
///
/// The slot takes the callee's default under the arguments the call binds
/// the callee's binders to: its own `instantiated_args`, for a method its
/// struct's from the receiver's type (`Self`'s binders stand for
/// themselves), and for a nested body each enclosing declaration's, which
/// stand for themselves inside it. A literal default is its constant,
/// materialized at the parameter type as a written literal is; a recorded
/// conversion of one runs its constructor; any other default is the
/// compile-time application of the lifted default function
/// (`$default$f$x`) to the arguments of the binders it declares, which the
/// elaborator evaluates once per instance and the call materializes. The argument goes positionally when every earlier slot is
/// filled by position, else by name; a subscript carrying one becomes a
/// `MultiIndex`. A memory-only default is bound to its call-result slot
/// ([`CALL_RESULT_SLOT`]), which a borrowed parameter reads and an owned one
/// takes.
///
/// A call through a function value the check did not name takes the
/// function its callee register statically holds ([`callable_targets`]);
/// one holding no single function keeps its slot for the callee, as does a
/// call of a callee `declarations` lacks. A default whose binders the call
/// does not bind, or that is spread, raises, or fills a reference
/// parameter, is an invariant error: upstream rejects each shape at the
/// call.
pub fn fill_omitted_arguments(
    functions: &mut [(String, MirFunction)],
    declarations: &MirDeclarations,
    invariant_errors: &mut Vec<String>,
) {
    for (name, function) in functions {
        let records = std::mem::take(&mut function.lowered_calls);
        if records.is_empty() {
            continue;
        }
        let mut pending: HashMap<u32, LoweredCall> = records
            .into_iter()
            .map(|record| (record.dest.0, record))
            .collect();
        let targets = callable_targets(function);
        let caller = Caller {
            name,
            declarations,
            targets,
            constants: register_constants(function),
        };
        let mut blocks = std::mem::take(&mut function.body);
        fill_blocks(
            &mut blocks,
            function,
            &caller,
            &mut pending,
            invariant_errors,
        );
        function.body = blocks;
    }
}

/// Erase from `function` the evaluation of each default a call no longer
/// passes, and return the registers it defined.
///
/// `dropped` are the operands a rewrite of the body's calls took for their
/// callees' removed arguments, and `original` how many times the body read
/// each register before it. Upstream's `RemoveUnusedParams` rewrites such a
/// call to pass nothing for a removed argument, so the caller evaluates no
/// default for it. An operand only the call read goes with the lifted
/// default's application (`$default$f$x`) that defines it; one loaded from
/// the read temporary that value was bound to goes with the temporary:
/// its binding, its loads, and its destruction, every instruction that
/// only served it. A temporary whose destruction runs code stays, with its
/// default, as upstream's caller still destroys the value it evaluated, and
/// so does one whose destruction reaches a terminator, a region, or a slot
/// the inliner did not introduce.
pub fn erase_unpassed_defaults(
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    dropped: &BTreeSet<u32>,
    original: &[u32],
) -> HashSet<u32> {
    let mut producers = HashMap::new();
    let mut bindings: HashMap<VarId, Vec<u32>> = HashMap::new();
    collect_producers(&function.body, function, &mut producers, &mut bindings);
    let once = |reg: u32| original.get(reg as usize) == Some(&1);
    let mut calls = HashSet::new();
    let mut slots = BTreeSet::new();
    let mut regs = HashSet::new();
    for &operand in dropped.iter().filter(|&&operand| once(operand)) {
        match producers.get(&operand) {
            Some(Producer::Default) => {
                calls.insert(operand);
            }
            Some(&Producer::Temporary(slot)) => {
                let Some(&[source]) = bindings.get(&slot).map(Vec::as_slice) else {
                    continue;
                };
                // A temporary whose destruction runs code keeps its value,
                // which the call's caller still destroys.
                let destroyed = function
                    .var_tys
                    .get(&slot)
                    .is_none_or(|ty| !declarations.drop_runs_no_code(ty));
                if destroyed
                    || !once(source)
                    || !matches!(producers.get(&source), Some(Producer::Default))
                {
                    continue;
                }
                if let Some((slice_slots, slice_regs)) = temporary_slice(function, slot, dropped) {
                    calls.insert(source);
                    slots.extend(slice_slots);
                    regs.extend(slice_regs);
                }
            }
            _ => {}
        }
    }
    let mut erased = HashSet::new();
    if calls.is_empty() {
        return erased;
    }
    erase_slice(&mut function.body, &slots, &regs, &calls, &mut erased);
    erased
}

/// `ty`, a requirement's type spelled over the abstract `Self`, with
/// `Self` the type `replacement`: its trait's `_Self` where the
/// requirement's defaults are lifted, a receiver's type at a call.
pub fn replace_abstract_self(ty: &Ty, replacement: &Ty) -> Option<Ty> {
    mojito_types::types::rewrite_ty(ty, &mut SelfAs(replacement)).ok()
}

/// The body whose calls are filled: its name, the declarations its callees
/// are found in, and the function each of its registers statically holds.
struct Caller<'a> {
    name: &'a str,
    declarations: &'a MirDeclarations,
    targets: HashMap<u32, (String, bool)>,
    /// The closed scalar each register the body defines as a constant
    /// holds ([`register_constants`]).
    constants: HashMap<u32, CtValue>,
}

/// The call being filled: its operand lists, taken from the instruction
/// and stored back once filled ([`Site::store`]).
struct Site {
    dest: Reg,
    callee: String,
    leading: usize,
    args: Vec<Reg>,
    kwargs: Vec<(String, Reg)>,
    /// The call's bindings of the callee's binders and those in scope
    /// around it.
    bound: Vec<(ParamDecl, TyArg)>,
    /// The position in `args` of the positional collector the call builds
    /// ([`build_collector`]).
    spread: Option<usize>,
    /// The receiver a call through an instance method's function value
    /// passes ahead of `args`: the declaration lists no slot for it, so it
    /// binds none of the callee's parameters.
    receiver: Option<Reg>,
    /// The receiver's value type at a call of a requirement, whose
    /// parameter types are spelled over the abstract `Self` it closes.
    self_type: Option<Ty>,
}

/// The rewrite of a requirement's abstract `Self` to another type.
struct SelfAs<'a>(&'a Ty);

impl mojito_types::types::TyRewrite for SelfAs<'_> {
    fn whole(&mut self, ty: &Ty) -> Option<Ty> {
        matches!(ty.kind(), TyKind::SelfType).then(|| self.0.clone())
    }

    fn expr(
        &mut self,
        expr: &ParamExpr,
    ) -> Result<ParamExpr, mojito_types::param_expr::ParamError> {
        Ok(expr.clone())
    }
}

/// One omitted argument, bound: the parameter it fills and the value its
/// default takes.
struct Omitted {
    slot: usize,
    name: String,
    ty: Ty,
    owned: bool,
    /// The callee's declared parameter takes the argument's storage
    /// ([`by_address_parameter`]), so the call passes the address of the
    /// temporary the default is bound to.
    by_address: bool,
    convention: Option<ArgConvention>,
    /// The callee's declared parameter writes through the argument's place
    /// ([`MirFunctionDeclaration::param_writes`]).
    writes: bool,
    default: OmittedDefault,
}

/// The value an omitted argument's default takes.
enum OmittedDefault {
    /// A compile-time application the elaborator evaluates once per
    /// instance, materialized at the call.
    Applied(ParamExpr),
    /// A literal, or a recorded conversion of one.
    Constant(CheckedConst),
}

fn fill_blocks(
    block: &mut MirBlock,
    function: &mut MirFunction,
    caller: &Caller<'_>,
    pending: &mut HashMap<u32, LoweredCall>,
    invariant_errors: &mut Vec<String>,
) {
    let instructions = std::mem::take(&mut block.instrs);
    let mut filled = Vec::with_capacity(instructions.len());
    // What follows a call goes after the in-place binding of its result,
    // which must follow the call itself.
    let mut deferred: Option<(Reg, Vec<MirInstr>)> = None;
    for mut instruction in instructions {
        if let Some((result, mut postlude)) = deferred.take() {
            if matches!(instruction, MirInstr::DefVar { src, in_place: true, .. } if src == result)
            {
                filled.push(instruction);
                filled.append(&mut postlude);
                continue;
            }
            filled.append(&mut postlude);
        }
        for region in instruction_regions_mut(&mut instruction) {
            fill_blocks(region, function, caller, pending, invariant_errors);
        }
        let mut postlude = Vec::new();
        if matches!(instruction, MirInstr::MultiSet { .. }) {
            collect_subscript_store(
                &mut instruction,
                function,
                caller,
                (&mut filled, &mut postlude),
            );
        }
        if let Some(record) = result_register(&instruction).and_then(|dest| pending.remove(&dest.0))
        {
            instruction = fill_call(
                instruction,
                &record,
                function,
                caller,
                (&mut filled, &mut postlude),
                invariant_errors,
            );
        } else {
            collect_through_contract(&mut instruction, function, caller.declarations, &mut filled);
        }
        let result = result_register(&instruction);
        filled.push(instruction);
        match result {
            Some(result) if !postlude.is_empty() => deferred = Some((result, postlude)),
            _ => filled.append(&mut postlude),
        }
    }
    if let Some((_, mut postlude)) = deferred {
        filled.append(&mut postlude);
    }
    block.instrs = filled;
}

/// The register `instruction` writes, for the instruction kinds a checked
/// call lowers to.
const fn result_register(instruction: &MirInstr) -> Option<Reg> {
    match instruction {
        MirInstr::Call { dest, .. }
        | MirInstr::MethodCall { dest, .. }
        | MirInstr::CallIndirect { dest, .. }
        | MirInstr::Index { dest, .. }
        | MirInstr::MultiIndex { dest, .. } => Some(*dest),
        _ => None,
    }
}

/// Fill `instruction`'s omitted arguments as `record` binds it, emitting
/// each default's evaluation onto the prelude of `around`, and build the
/// positional collector it passes ([`build_collector`]), what follows the
/// call onto its postlude; return the instruction, rewritten when its kind
/// cannot carry the filled operands.
fn fill_call(
    mut instruction: MirInstr,
    record: &LoweredCall,
    function: &mut MirFunction,
    caller: &Caller<'_>,
    (prelude, postlude): (&mut Vec<MirInstr>, &mut Vec<MirInstr>),
    invariant_errors: &mut Vec<String>,
) -> MirInstr {
    if let MirInstr::Index { base, call, .. } = &instruction {
        let Some((_, callee)) = record.callee.as_deref().and_then(|callee| {
            resolve_callee(
                callee,
                function.reg_types.get(&base.0),
                call.as_ref().and_then(|call| call.refined_bound.as_deref()),
                caller.declarations,
            )
        }) else {
            return instruction;
        };
        if !omits_any(callee, 1, &[]) && callee.variadic().is_none() {
            return instruction;
        }
        instruction = widen_index(instruction);
    }
    let Some((mut site, callee)) = Site::of(&instruction, record, function, caller) else {
        collect_through_contract(&mut instruction, function, caller.declarations, prelude);
        return instruction;
    };
    let declarations = caller.declarations;
    let location = function
        .spans
        .0
        .get(&site.dest.0)
        .map(|(span, _)| span.clone());
    let omitted = match omitted_arguments(&site, callee, declarations) {
        Ok(omitted) => omitted,
        Err(error) => {
            invariant_errors.push(format!(
                "{}: a call of '{}' leaves out an argument the call cannot fill: {error}",
                caller.name, site.callee
            ));
            return instruction;
        }
    };
    let leading = site.leading;
    let declared = callee.param_names().len();
    let positional_limit = callee.keyword_only().unwrap_or(declared).min(declared);
    let passes_operands = !(site.args.is_empty() && site.kwargs.is_empty());
    let mut sources = Vec::new();
    let mut conventions = Vec::new();
    for default in omitted {
        let positional = default.slot == leading + site.args.len()
            && default.slot < positional_limit
            && callee
                .variadic_index()
                .is_none_or(|collector| default.slot < collector);
        let value = evaluate_default(function, &default, location.as_ref(), declarations, prelude);
        let argument = if declarations.register_passable(&default.ty) && !default.by_address {
            value
        } else {
            bind_temporary(function, value, &default, location.as_ref(), prelude)
        };
        let source = if positional {
            CheckedCallArgumentSource::Positional(site.args.len())
        } else {
            CheckedCallArgumentSource::Keyword(site.kwargs.len())
        };
        sources.push((default.slot, source));
        conventions.push(CheckedArgumentConvention {
            source,
            convention: default.convention,
            by_address: default.by_address,
            writes: default.writes,
        });
        if positional {
            site.args.push(argument);
        } else {
            site.kwargs.push((default.name, argument));
        }
    }
    record_filled_conventions(&mut instruction, conventions, passes_operands);
    if !matches!(instruction, MirInstr::MultiIndex { .. })
        && let Err(error) = build_keyword_collector(
            &mut site,
            &callee.shape(),
            function,
            declarations,
            location.as_ref(),
            prelude,
        )
    {
        invariant_errors.push(format!(
            "{}: a call of '{}' builds no keyword collector: {error}",
            caller.name, site.callee
        ));
    }
    build_collector(
        &mut site,
        &callee.shape(),
        matches!(instruction, MirInstr::MultiIndex { .. }),
        function,
        location.as_ref(),
        prelude,
        postlude,
    );
    if let (
        Some(position),
        MirInstr::MultiIndex {
            call: Some(call), ..
        },
    ) = (site.spread, &mut instruction)
    {
        collect_contract_arguments(call, position, callee, function, site.args[position]);
    }
    site.store(&mut instruction, &sources);
    instruction
}

/// Build the positional collector of a subscript store (`g[1, 2] = v`) of
/// a `__setitem__` declaring one, as [`build_collector`] builds a call's:
/// its index operands are the overflow, its value the keyword argument
/// `value_keyword` names.
/// A store indexing by a slice keeps its operands.
fn collect_subscript_store(
    instruction: &mut MirInstr,
    function: &mut MirFunction,
    caller: &Caller<'_>,
    (prelude, postlude): (&mut Vec<MirInstr>, &mut Vec<MirInstr>),
) {
    let MirInstr::MultiSet {
        receiver,
        args,
        kwargs,
        value,
        value_keyword,
        call,
    } = instruction
    else {
        return;
    };
    let Some(callee) = caller
        .declarations
        .function(&call.target)
        .filter(|callee| callee.variadic.is_some())
    else {
        return;
    };
    let Some(registers) = args
        .iter()
        .map(|argument| match argument {
            MirSubscriptArg::Index(register) => Some(*register),
        })
        .collect::<Option<Vec<_>>>()
    else {
        return;
    };
    let receiver_ty = function.reg_types.get(&receiver.0).cloned();
    let mut site = Site {
        dest: *value,
        callee: call.target.clone(),
        leading: 0,
        args: registers,
        kwargs: kwargs
            .iter()
            .map(|(name, MirSubscriptArg::Index(register))| (name.clone(), *register))
            .chain(std::iter::once((value_keyword.clone(), *value)))
            .collect(),
        bound: bindings_of(
            &call.target,
            callee,
            (&[], &call.param_arg_regs),
            receiver_ty.as_ref(),
            caller,
        ),
        spread: None,
        receiver: None,
        self_type: None,
    };
    let location = function.spans.0.get(&value.0).map(|(span, _)| span.clone());
    build_collector(
        &mut site,
        &CallShape::from(callee),
        true,
        function,
        location.as_ref(),
        prelude,
        postlude,
    );
    let Some(position) = site.spread else {
        return;
    };
    let collector = site.args[position];
    *args = site.args.into_iter().map(MirSubscriptArg::Index).collect();
    collect_contract_arguments(
        call,
        position,
        DefaultedCallee::Declared(callee),
        function,
        collector,
    );
}

/// Name the collector a subscript builds at `position` in its contract
/// `call`, in place of the overflow operands it holds: one argument of the
/// collector's type past the declared parameters', under `callee`'s
/// collector convention.
fn collect_contract_arguments(
    call: &mut crate::mir::MirSubscriptCall,
    position: usize,
    callee: DefaultedCallee<'_>,
    function: &MirFunction,
    collector: Reg,
) {
    let Some(ty) = function.reg_types.get(&collector.0) else {
        return;
    };
    call.arguments.retain(|argument| {
        !matches!(argument.source, CheckedCallArgumentSource::Positional(index) if index >= position)
    });
    let at = callee.param_types().len().min(call.arguments.len());
    call.arguments.insert(
        at,
        mojito_checked::checked::CheckedCallArgument {
            source: CheckedCallArgumentSource::Positional(position),
            parameter_ty: ty.clone(),
            requires_place: false,
            convention: callee.variadic_convention(),
            by_address: false,
            writes: matches!(
                callee.variadic_convention(),
                Some(ArgConvention::Var | ArgConvention::Deinit)
            ),
        },
    );
}

/// What a call binds its arguments by: its callee's declared parameters
/// and collectors, or, for a call through a bound whose requirement has no
/// body, the requirement's.
struct CallShape<'a> {
    param_names: &'a [String],
    required: &'a [bool],
    positional_only: Option<usize>,
    keyword_only: Option<usize>,
    variadic: Option<&'a Ty>,
    variadic_convention: Option<ArgConvention>,
    kw_variadic: Option<&'a Ty>,
    /// The callee's own compile-time parameters a call binds by position.
    param_decls: std::borrow::Cow<'a, [ParamDecl]>,
}

impl<'a> From<&'a MirFunctionDeclaration> for CallShape<'a> {
    fn from(declaration: &'a MirFunctionDeclaration) -> Self {
        Self {
            param_names: &declaration.param_names,
            required: &declaration.required,
            positional_only: declaration.positional_only,
            keyword_only: declaration.keyword_only,
            variadic: declaration.variadic.as_ref(),
            variadic_convention: declaration.variadic_convention,
            kw_variadic: declaration.kw_variadic.as_ref(),
            param_decls: declaration.argument_view(),
        }
    }
}

/// The declaration a call binds its arguments against and takes each
/// omitted one's default from: its callee's, or, for a call through a bound
/// whose requirement has no body, the requirement's, as upstream's
/// requirement function type declares its parameters and their default
/// attributes.
#[derive(Clone, Copy)]
enum DefaultedCallee<'a> {
    Declared(&'a MirFunctionDeclaration),
    Required(&'a TraitRequirement),
}

impl<'a> DefaultedCallee<'a> {
    fn shape(self) -> CallShape<'a> {
        match self {
            Self::Declared(declaration) => CallShape::from(declaration),
            Self::Required(requirement) => CallShape {
                param_names: &requirement.param_names,
                required: &requirement.required,
                positional_only: requirement.positional_only,
                keyword_only: requirement.keyword_only,
                variadic: requirement.variadic.as_ref(),
                variadic_convention: requirement.variadic_convention,
                kw_variadic: requirement.kw_variadic.as_ref(),
                param_decls: ParamDecl::argument_view(&requirement.param_decls),
            },
        }
    }

    fn param_names(self) -> &'a [String] {
        match self {
            Self::Declared(declaration) => &declaration.param_names,
            Self::Required(requirement) => &requirement.param_names,
        }
    }

    fn required(self) -> &'a [bool] {
        match self {
            Self::Declared(declaration) => &declaration.required,
            Self::Required(requirement) => &requirement.required,
        }
    }

    fn param_types(self) -> &'a [Ty] {
        match self {
            Self::Declared(declaration) => &declaration.param_types,
            Self::Required(requirement) => &requirement.param_types,
        }
    }

    fn defaults(self) -> &'a [Option<CheckedConst>] {
        match self {
            Self::Declared(declaration) => &declaration.defaults,
            Self::Required(requirement) => &requirement.defaults,
        }
    }

    fn param_conventions(self) -> &'a [Option<ArgConvention>] {
        match self {
            Self::Declared(declaration) => &declaration.param_conventions,
            Self::Required(requirement) => &requirement.param_conventions,
        }
    }

    const fn positional_only(self) -> Option<usize> {
        match self {
            Self::Declared(declaration) => declaration.positional_only,
            Self::Required(requirement) => requirement.positional_only,
        }
    }

    const fn keyword_only(self) -> Option<usize> {
        match self {
            Self::Declared(declaration) => declaration.keyword_only,
            Self::Required(requirement) => requirement.keyword_only,
        }
    }

    const fn variadic(self) -> Option<&'a Ty> {
        match self {
            Self::Declared(declaration) => declaration.variadic.as_ref(),
            Self::Required(requirement) => requirement.variadic.as_ref(),
        }
    }

    const fn kw_variadic(self) -> Option<&'a Ty> {
        match self {
            Self::Declared(declaration) => declaration.kw_variadic.as_ref(),
            Self::Required(requirement) => requirement.kw_variadic.as_ref(),
        }
    }

    const fn variadic_index(self) -> Option<usize> {
        match self {
            Self::Declared(declaration) => declaration.variadic_index,
            Self::Required(requirement) => requirement.variadic_index,
        }
    }

    const fn variadic_convention(self) -> Option<ArgConvention> {
        match self {
            Self::Declared(declaration) => declaration.variadic_convention,
            Self::Required(requirement) => requirement.variadic_convention,
        }
    }

    /// Whether the parameter at `slot` writes through its argument's place
    /// ([`MirFunctionDeclaration::param_writes`]); a requirement's
    /// reference parameter takes no default.
    fn param_writes(self, slot: usize) -> bool {
        match self {
            Self::Declared(declaration) => {
                declaration.param_writes.get(slot).copied().unwrap_or(false)
            }
            Self::Required(_) => false,
        }
    }

    /// Whether the parameter at `slot` takes its argument's storage, the
    /// value of type `ty` at the call ([`by_address_parameter`]).
    fn by_address(self, slot: usize, ty: &Ty, declarations: &MirDeclarations) -> bool {
        match self {
            Self::Declared(declaration) => {
                by_address_parameter(declaration, declarations, slot).is_some()
            }
            Self::Required(requirement) => {
                requirement_by_address_parameter(requirement, slot, ty, declarations).is_some()
            }
        }
    }
}

/// Build the keyword collector of a call through a function value whose
/// target the body does not statically hold, or that the check named no
/// callee for, from the call shape of the value's function type, as
/// [`build_keyword_collector`] builds a declared callee's.
fn collect_through_contract(
    instruction: &mut MirInstr,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    prelude: &mut Vec<MirInstr>,
) {
    let MirInstr::CallIndirect {
        dest,
        callee,
        args,
        kwargs,
        spread: None,
        ..
    } = &*instruction
    else {
        return;
    };
    let Some(TyKind::Func(contract)) = function.reg_types.get(&callee.0).map(Ty::kind) else {
        return;
    };
    if contract.kw_variadic.is_none() {
        return;
    }
    let contract = contract.clone();
    let shape = CallShape {
        param_names: &contract.names,
        required: &contract.required,
        positional_only: contract.positional_only,
        keyword_only: contract.keyword_only,
        variadic: contract.variadic.as_ref(),
        variadic_convention: None,
        kw_variadic: contract.kw_variadic.as_ref(),
        param_decls: std::borrow::Cow::Borrowed(&[]),
    };
    let mut site = Site {
        dest: *dest,
        callee: String::new(),
        leading: 0,
        args: args.clone(),
        kwargs: kwargs.clone(),
        bound: Vec::new(),
        spread: None,
        receiver: None,
        self_type: None,
    };
    let location = function.spans.0.get(&dest.0).map(|(span, _)| span.clone());
    if build_keyword_collector(
        &mut site,
        &shape,
        function,
        declarations,
        location.as_ref(),
        prelude,
    )
    .is_ok_and(|built| built)
    {
        site.store(instruction, &[]);
    }
}

/// Pass the keyword collector a call's `shape` declares as the call's one
/// `**` operand, as upstream's call builds the dictionary its callee's
/// `**kwargs` takes: the collector's empty constructor
/// (`StringDict[V]()`, typed as [`keyword_collector_type`] closes it) bound
/// to a call-result slot, then one `_insert` per keyword no named parameter
/// claims, in call order, each passed the slot's address as its `mut self`,
/// keyed by the keyword's literal, and taking its operand, and the slot
/// forwarded to the call. Return whether it built
/// one: a call forwarding a `**kwargs^` passes that collector already, and
/// a callee without one takes none. An error says why the call's operands
/// do not bind or type the collector.
fn build_keyword_collector(
    site: &mut Site,
    shape: &CallShape<'_>,
    function: &mut MirFunction,
    declarations: &MirDeclarations,
    location: Option<&SourceSpan>,
    prelude: &mut Vec<MirInstr>,
) -> Result<bool, String> {
    let Some(element) = shape.kw_variadic else {
        return Ok(false);
    };
    let skip = site.leading;
    let keywords: Vec<&str> = site.kwargs.iter().map(|(name, _)| name.as_str()).collect();
    let matched = match_call_slots(
        shape.param_names.get(skip..).unwrap_or_default(),
        shape.required.get(skip..).unwrap_or_default(),
        shape
            .positional_only
            .and_then(|limit| limit.checked_sub(skip)),
        shape.keyword_only.and_then(|limit| limit.checked_sub(skip)),
        site.args.len(),
        &keywords,
        CallVariadics {
            positional: shape.variadic.is_some(),
            keyword: true,
        },
    )
    .map_err(|error| format!("{error:?}"))?;
    let collected = matched.keyword_overflow;
    if collected
        .iter()
        .any(|&index| site.kwargs[index].0 == FORWARDED_KWARGS_NAME)
    {
        return Ok(false);
    }
    let ty = keyword_collector_type(&shape.param_decls, element, &collected, site, function)
        .ok_or_else(|| "its element names a binder the call binds no argument for".to_string())?;
    let place = MirPlace::root(
        u32::try_from(function.n_vars).expect("variable count fits a VarId"),
        Some(ty.clone()),
    );
    let collector = fresh_register(function, ty.clone(), location);
    prelude.push(MirInstr::Call {
        conventions: crate::mir::MirArgConventions::default(),
        dest: collector,
        func: FuncRef::named(&mojito_symbol::symbol::keyword_collector_constructor_symbol()),
        raises: None,
        args: Vec::new(),
        kwargs: Vec::new(),
        capture_accesses: Vec::new(),
        param_arg_regs: Vec::new(),
        receiver: None,
        instantiated_args: constructed_arguments(
            &mojito_symbol::symbol::keyword_collector_constructor_symbol(),
            &ty,
            declarations,
        ),
        spread: None,
    });
    let variable = place.root;
    function.n_vars += 1;
    function
        .var_names
        .push(format!("{CALL_RESULT_SLOT}{}", collector.0));
    function.var_tys.insert(variable, ty.clone());
    prelude.push(MirInstr::DefVar {
        var: variable,
        src: collector,
        binding_ty: Some(ty.clone()),
        in_place: true,
    });
    // The insert's key is auto-parameterized by its text, which the key
    // register's literal type solves.
    let insert = mojito_symbol::symbol::keyword_collector_insert_symbol();
    let insert_decls = declarations
        .function(&insert)
        .map(|declaration| declaration.param_decls.clone())
        .unwrap_or_default();
    for &index in &collected {
        let (keyword, value) = site.kwargs[index].clone();
        let key_ty = Ty::string_literal_of(&keyword);
        // The key's binder is its literal type's text.
        let instantiated_args = key_ty
            .kind()
            .as_non_simd_struct()
            .map(|(_, text)| text.clone())
            .unwrap_or_default();
        let key = fresh_register(function, key_ty, location);
        prelude.push(MirInstr::Const {
            dest: key,
            k: Const::Str(keyword),
        });
        let handle = crate::mir::mir_place_handle_ty(
            &place,
            false,
            Some(mojito_types::origin::Mutability::Mutable),
        )
        .unwrap_or_else(Ty::error);
        let receiver = fresh_register(function, handle, location);
        prelude.push(MirInstr::MakeRef {
            dest: receiver,
            place: place.clone(),
        });
        prelude.push(MirInstr::MethodCall {
            conventions: crate::mir::MirArgConventions::default(),
            dest: fresh_register(function, Ty::none(), location),
            recv: receiver,
            method: "_insert".to_string(),
            resolved: Some(insert.clone()),
            raises: None,
            reference_result: None,
            result_adapter: None,
            args: vec![key, value],
            kwargs: Vec::new(),
            recv_writes: true,
            receiver_convention: Some(mojito_ast::ast::ArgConvention::Mut),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            param_decls: insert_decls.clone(),
            instantiated_args,
            spread: None,
            refined_bound: None,
        });
    }
    let passed = fresh_register(function, ty, location);
    prelude.push(MirInstr::UseVar {
        dest: passed,
        var: variable,
        mode: UseMode::Forward,
        constructor: None,
    });
    let mut index = 0;
    site.kwargs.retain(|_| {
        index += 1;
        !collected.contains(&(index - 1))
    });
    site.kwargs
        .push((FORWARDED_KWARGS_NAME.to_string(), passed));
    Ok(true)
}

/// The type of the keyword collector a call of a callee collecting
/// `element`s builds from the keywords at `collected`, in the caller's
/// binder scope: `StringDict[element]` under the call's bindings, or, when
/// they leave the element open, over the first collected operand's type,
/// its literal materialized, as the elaborator infers it.
fn keyword_collector_type(
    param_decls: &[ParamDecl],
    element: &Ty,
    collected: &[usize],
    site: &Site,
    function: &MirFunction,
) -> Option<Ty> {
    let collector = |element: Ty| {
        Ty::new(TyKind::Struct(
            mojito_symbol::symbol::KEYWORD_COLLECTOR_STRUCT.to_string(),
            vec![TyArg::Ty(element)],
        ))
    };
    let unbound = |closed: &Ty| {
        mojito_types::types::mentions(closed, &|ty| matches!(ty.kind(), TyKind::SelfType))
            || param_decls.iter().any(|decl| {
                !site.bound.iter().any(|(own, _)| own.id() == decl.id())
                    && names_binder(closed, &decl.binder())
            })
    };
    if let Some(closed) = close_parameter_type(element, &site.bound)
        && !unbound(&closed)
    {
        return Some(collector(closed));
    }
    let first = collected.first()?;
    let register = site.kwargs[*first].1;
    function
        .reg_types
        .get(&register.0)
        .map(default_literal)
        .map(collector)
}

/// Pass the positional collector a call's `shape` declares as the one
/// operand the call builds, as upstream's call builds the `VariadicPack` or
/// `VariadicList` its callee's `*xs` takes, emitted onto `prelude` and
/// passed after every regular positional operand, its position the call's
/// `spread`. A heterogeneous collector is a `tuple.make` of the overflow
/// operands, typed under the call's bindings ([`collector_type`]); a
/// homogeneous one is the `VariadicList[T, is_owned]` over pointers to
/// them ([`build_variadic_list`]), a `read` collector's element a place
/// the caller reads borrowed where it lies ([`borrowed_element`]), and what
/// keeps its elements live through the call goes onto `postlude`, after
/// it. A regular slot before the collector that a keyword fills moves to
/// its position, so the shared matcher binds the collector operand alone
/// ([`mojito_ast::call::bind_spread`]). A call that leaves such a slot out,
/// or whose collector type the call's operands do not determine, keeps its
/// operands, and so does a subscript (`keywords_stay`) a keyword of which
/// fills such a slot: its contract names each operand by its source, which
/// the move would rename.
fn build_collector(
    site: &mut Site,
    shape: &CallShape<'_>,
    keywords_stay: bool,
    function: &mut MirFunction,
    location: Option<&SourceSpan>,
    prelude: &mut Vec<MirInstr>,
    postlude: &mut Vec<MirInstr>,
) {
    // A call spreading the caller's own pack passes it as the collector.
    let (Some(element), None) = (shape.variadic, site.spread) else {
        return;
    };
    let skip = site.leading;
    let names = shape.param_names.get(skip..).unwrap_or_default();
    let keywords: Vec<&str> = site.kwargs.iter().map(|(name, _)| name.as_str()).collect();
    let Ok(matched) = match_call_slots(
        names,
        shape.required.get(skip..).unwrap_or_default(),
        shape
            .positional_only
            .and_then(|limit| limit.checked_sub(skip)),
        shape.keyword_only.and_then(|limit| limit.checked_sub(skip)),
        site.args.len(),
        &keywords,
        CallVariadics {
            positional: true,
            keyword: shape.kw_variadic.is_some(),
        },
    ) else {
        return;
    };
    let limit = shape
        .keyword_only
        .and_then(|limit| limit.checked_sub(skip))
        .unwrap_or(names.len())
        .min(names.len());
    if keywords_stay
        && matched.slots[..limit]
            .iter()
            .any(|slot| matches!(slot, ArgSlot::Keyword(_)))
    {
        return;
    }
    let mut args = Vec::with_capacity(limit + 1);
    let mut moved = HashSet::new();
    for slot in &matched.slots[..limit] {
        match *slot {
            ArgSlot::Positional(index) => {
                args.push(site.args[index]);
            }
            ArgSlot::Keyword(index) => {
                args.push(site.kwargs[index].1);
                moved.insert(index);
            }
            ArgSlot::Default => return,
        }
    }
    let elements: Vec<Reg> = matched
        .positional_overflow
        .iter()
        .map(|&index| site.args[index])
        .collect();
    let Some(ty) = collector_type(&shape.param_decls, element, &elements, site, function) else {
        return;
    };
    let homogeneous = match ty.kind() {
        TyKind::VariadicPack(element)
            if mojito_types::types::pack_spread(std::slice::from_ref(element)).is_none() =>
        {
            Some(element.clone())
        }
        TyKind::Tuple(_) | TyKind::VariadicPack(_) => None,
        _ => return,
    };
    let collector = if let Some(element) = homogeneous {
        let owned = shape.variadic_convention == Some(ArgConvention::Var);
        let elements = elements
            .into_iter()
            .map(|register| {
                (!owned)
                    .then(|| borrowed_element(prelude, register, &element, function, location))
                    .flatten()
                    .unwrap_or(ListElement::Value(register))
            })
            .collect();
        let built = build_variadic_list(
            &mut CallFrame { function, location },
            &element,
            owned,
            elements,
        );
        prelude.extend(built.before);
        prelude.extend(built.ended);
        postlude.extend(built.after);
        built.list
    } else if shape.variadic_convention != Some(ArgConvention::Var) {
        let element_types = match ty.kind() {
            TyKind::Tuple(types) => types.clone(),
            _ => return,
        };
        let mutable = shape.variadic_convention == Some(ArgConvention::Mut);
        let pack_ty = mojito_types::types::bound_variadic_pack_type(
            element,
            &element_types,
            mutable,
            false,
            mojito_types::origin::Origin::Unbound,
        );
        // A value read shallowly (a reference result's referent) belongs to
        // its owner, which the slot the pack binds it to must not destroy,
        // and which the call keeps live as upstream's reference into it
        // does.
        let shallow: Vec<Reg> = elements
            .iter()
            .copied()
            .filter(|&register| !owned_in(prelude, register))
            .collect();
        let element_owners: Vec<Vec<MirPlace>> = elements
            .iter()
            .map(|register| {
                if shallow.contains(register) {
                    element_owners(prelude, *register, &function.var_tys)
                } else {
                    Vec::new()
                }
            })
            .collect();
        let elements: Vec<ListElement> = elements
            .into_iter()
            .zip(&element_types)
            .map(|(register, element)| {
                pack_reference(prelude, register, element, mutable, function, location)
                    .unwrap_or(ListElement::Value(register))
            })
            .collect();
        // Where a borrowed element lies, and the owner a value element was
        // read out of, live through the call.
        let kept = elements
            .iter()
            .zip(element_owners)
            .flat_map(|(element, owners)| match element {
                ListElement::Borrowed { pointer, .. } => {
                    referenced_place(prelude, *pointer).into_iter().collect()
                }
                ListElement::Value(_) => owners,
            })
            .collect();
        let built = build_variadic_pack(
            &mut CallFrame { function, location },
            pack_ty,
            &element_types,
            mutable,
            elements,
            (kept, shallow),
        );
        prelude.extend(built.before);
        prelude.extend(built.ended);
        postlude.extend(built.after);
        built.list
    } else {
        // An owned collector over the caller's own pack leaves its
        // elements' types to the elaborator.
        let element_types = match ty.kind() {
            TyKind::Tuple(types) => Some(types.clone()),
            _ => None,
        };
        let storage = fresh_register(function, ty, location);
        prelude.push(MirInstr::MakeTuple {
            dest: storage,
            elems: elements,
            element_types,
        });
        storage
    };
    site.spread = Some(args.len());
    args.push(collector);
    site.args = args;
    let mut index = 0;
    site.kwargs.retain(|_| {
        index += 1;
        !moved.contains(&(index - 1))
    });
}

/// Whether `register` holds a value of its own: the last instruction of
/// `prelude` defining it gives an owned result
/// ([`crate::mir::owned_result_regs`]), or none there defines it.
fn owned_in(prelude: &[MirInstr], register: Reg) -> bool {
    prelude
        .iter()
        .rev()
        .find(|instruction| {
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            results.contains(&register)
        })
        .is_none_or(|instruction| {
            let mut owned = Vec::new();
            crate::mir::owned_result_regs(instruction, &mut owned);
            owned.contains(&register)
        })
}

/// The slots a value element `register` was read out of, which the call
/// keeps live as upstream's reference into them does: a loaded or
/// borrowed variable, or the receiver of a subscript or method call
/// returning a reference, seen through a copy, a field read, or a read
/// through a reference. A place reached through a `ref` slot names no
/// owner.
fn element_owners(
    prelude: &[MirInstr],
    register: Reg,
    var_tys: &HashMap<VarId, Ty>,
) -> Vec<MirPlace> {
    let Some(at) = prelude.iter().rposition(|instruction| {
        let mut results = Vec::new();
        instruction_result_regs(instruction, &mut results);
        results.contains(&register)
    }) else {
        return Vec::new();
    };
    // A temporary the call's setup materialized and released owns nothing
    // the call reads.
    let released = |var: VarId| {
        prelude[at + 1..]
            .iter()
            .any(|instruction| match instruction {
                MirInstr::DropVar { var: dropped, .. } | MirInstr::ConsumeVar { var: dropped } => {
                    *dropped == var
                }
                MirInstr::DropPlace { place, .. } | MirInstr::MarkDestroyed { place } => {
                    place.root == var
                }
                _ => false,
            })
    };
    let owned = |place: &MirPlace| -> Vec<MirPlace> {
        if place.through.is_some() || released(place.root) {
            return Vec::new();
        }
        vec![MirPlace::root(
            place.root,
            var_tys.get(&place.root).cloned(),
        )]
    };
    match &prelude[at] {
        MirInstr::LoadPlace { place, .. } | MirInstr::MakeRef { place, .. } => owned(place),
        MirInstr::UseVar { var, mode, .. } if *mode != UseMode::Move && !released(*var) => {
            vec![MirPlace::root(*var, var_tys.get(var).cloned())]
        }
        MirInstr::CopyValue { value, .. }
        | MirInstr::ReadRef {
            reference: value, ..
        }
        | MirInstr::GetField { base: value, .. } => element_owners(prelude, *value, var_tys),
        MirInstr::Index { base, .. } => element_owners(prelude, *base, var_tys),
        MirInstr::MethodCall {
            recv,
            reference_result: Some(_),
            ..
        } => element_owners(prelude, *recv, var_tys),
        _ => Vec::new(),
    }
}

/// The place the `ref.make` in `prelude` defining `pointer` addresses,
/// unless it goes through a `ref` slot, whose loan on its origin a later
/// read of that origin would conflict with.
fn referenced_place(prelude: &[MirInstr], pointer: Reg) -> Option<MirPlace> {
    prelude
        .iter()
        .rev()
        .find_map(|instruction| match instruction {
            MirInstr::MakeRef { dest, place } if *dest == pointer && place.through.is_none() => {
                Some(place.clone())
            }
            _ => None,
        })
}

/// The reference a `VariadicPack` holds to its element `register`, as
/// upstream's call addresses each argument where it lies: a loaded place
/// ([`borrowed_element`], or [`shared_load`] when something else reads the
/// load too), a field read out of one ([`borrowed_field`]), the reference
/// a read goes through ([`read_reference`]), or the variable or read value
/// a copy copies ([`copied_variable`], [`copied_read`]). `None` for a value
/// with no place, which the pack binds to a slot of its own.
fn pack_reference(
    prelude: &mut Vec<MirInstr>,
    register: Reg,
    element: &Ty,
    mutable: bool,
    function: &mut MirFunction,
    location: Option<&SourceSpan>,
) -> Option<ListElement> {
    borrowed_element(prelude, register, element, function, location)
        .or_else(|| borrowed_field(prelude, register, element, mutable, function, location))
        .or_else(|| read_reference(prelude, register))
        .or_else(|| copied_variable(prelude, register, element, mutable, function, location))
        .or_else(|| copied_read(prelude, register, element, mutable, function, location))
        .or_else(|| shared_load(prelude, register, element, mutable, function, location))
}

/// An element `register` reads through a reference (`ref.read`, a `ref`
/// binding's referent), held by that reference.
fn read_reference(prelude: &[MirInstr], register: Reg) -> Option<ListElement> {
    let at = prelude.iter().rposition(
        |instruction| matches!(instruction, MirInstr::ReadRef { dest, .. } if *dest == register),
    )?;
    let MirInstr::ReadRef { reference, .. } = prelude[at] else {
        return None;
    };
    let roots = prelude[..at]
        .iter()
        .rev()
        .find_map(|instruction| match instruction {
            MirInstr::MakeRef { dest, place } if *dest == reference => {
                Some(std::iter::once(place.root).chain(place.through).collect())
            }
            _ => None,
        })
        .unwrap_or_default();
    Some(ListElement::Borrowed {
        pointer: reference,
        roots,
    })
}

/// An element `register` copies out of a variable (`var.use copy`), which
/// nothing else in `prelude` reads: the variable is addressed instead,
/// as upstream's call addresses a borrowed argument rather than copying it.
fn copied_variable(
    prelude: &mut [MirInstr],
    register: Reg,
    element: &Ty,
    mutable: bool,
    function: &mut MirFunction,
    location: Option<&SourceSpan>,
) -> Option<ListElement> {
    let at = prelude.iter().rposition(|instruction| {
        matches!(instruction, MirInstr::UseVar { dest, mode: UseMode::Copy, .. } if *dest == register)
    })?;
    let MirInstr::UseVar { var, .. } = prelude[at] else {
        return None;
    };
    let ty = function
        .var_tys
        .get(&var)
        .filter(|ty| *ty == element)?
        .clone();
    let mut operands = Vec::new();
    for instruction in prelude.iter() {
        crate::mir::verify::instruction_operand_regs(instruction, &mut operands);
    }
    if operands.contains(&register) {
        return None;
    }
    let pointer = element_pointer(
        function,
        register,
        element_lit_ref_type(element, mutable),
        location,
    );
    prelude[at] = MirInstr::MakeRef {
        dest: pointer,
        place: MirPlace::root(var, Some(ty)),
    };
    orphan(function, register);
    Some(ListElement::Borrowed {
        pointer,
        roots: vec![var],
    })
}

/// An element `register` copies out of a value read where it lies (a
/// reference result's referent), which nothing else in `prelude` reads:
/// the copy goes and the read value is addressed instead, as upstream's
/// call passes the reference rather than a copy.
fn copied_read(
    prelude: &mut Vec<MirInstr>,
    register: Reg,
    element: &Ty,
    mutable: bool,
    function: &mut MirFunction,
    location: Option<&SourceSpan>,
) -> Option<ListElement> {
    let at = prelude.iter().rposition(
        |instruction| matches!(instruction, MirInstr::CopyValue { dest, .. } if *dest == register),
    )?;
    let mut operands = Vec::new();
    for instruction in prelude.iter() {
        crate::mir::verify::instruction_operand_regs(instruction, &mut operands);
    }
    let MirInstr::CopyValue { value, .. } = prelude[at] else {
        return None;
    };
    if operands.contains(&register) || owned_in(prelude, value) {
        return None;
    }
    let copy = prelude.remove(at);
    let addressed = pack_reference(prelude, value, element, mutable, function, location);
    if addressed.is_some() {
        orphan(function, register);
    } else {
        prelude.insert(at, copy);
    }
    addressed
}

/// An element `register` loads shallowly from a place something else in
/// `prelude` reads it from too: the place is addressed beside the load.
fn shared_load(
    prelude: &mut Vec<MirInstr>,
    register: Reg,
    element: &Ty,
    mutable: bool,
    function: &mut MirFunction,
    location: Option<&SourceSpan>,
) -> Option<ListElement> {
    let at = prelude.iter().rposition(|instruction| {
        matches!(instruction, MirInstr::LoadPlace { dest, place }
            if *dest == register && place.ty.as_ref() == Some(element))
    })?;
    let MirInstr::LoadPlace { place, .. } = &prelude[at] else {
        return None;
    };
    let place = place.clone();
    let roots = std::iter::once(place.root).chain(place.through).collect();
    let pointer = element_pointer(
        function,
        register,
        element_lit_ref_type(element, mutable),
        location,
    );
    prelude.insert(
        at + 1,
        MirInstr::MakeRef {
            dest: pointer,
            place,
        },
    );
    Some(ListElement::Borrowed { pointer, roots })
}

/// A field `register` reads out of a value loaded from a place (`t[0].v`
/// through a reference result), addressed where it lies instead: the
/// load becomes the `ref.make` of the field's place and the `field.get`
/// goes, as upstream's call addresses the field it passes. Each load and
/// read in the chain must be read by nothing else in `prelude`.
fn borrowed_field(
    prelude: &mut Vec<MirInstr>,
    register: Reg,
    element: &Ty,
    mutable: bool,
    function: &mut MirFunction,
    location: Option<&SourceSpan>,
) -> Option<ListElement> {
    let mut operands = Vec::new();
    for instruction in prelude.iter() {
        crate::mir::verify::instruction_operand_regs(instruction, &mut operands);
    }
    let reads = |reg: Reg| operands.iter().filter(|&&operand| operand == reg).count();
    if reads(register) != 0 {
        return None;
    }
    let mut fields = Vec::new();
    let mut extracts = Vec::new();
    let mut current = register;
    let load = loop {
        let at = prelude.iter().rposition(|instruction| {
            matches!(instruction,
                MirInstr::GetField { dest, .. } | MirInstr::LoadPlace { dest, .. }
                    if *dest == current)
        })?;
        match &prelude[at] {
            MirInstr::GetField { base, field, .. } if reads(*base) == 1 => {
                fields.push((field.clone(), function.reg_types.get(&current.0)?.clone()));
                extracts.push(at);
                current = *base;
            }
            MirInstr::LoadPlace { .. } if !fields.is_empty() => break at,
            _ => return None,
        }
    };
    let MirInstr::LoadPlace { place, .. } = &prelude[load] else {
        return None;
    };
    let mut place = place.clone();
    for (field, ty) in fields.into_iter().rev() {
        place.project(Proj::Field(field), ty);
    }
    if place.ty.as_ref() != Some(element) {
        return None;
    }
    let roots = Vec::new();
    let pointer = element_pointer(
        function,
        register,
        element_lit_ref_type(element, mutable),
        location,
    );
    let loaded = std::mem::replace(
        &mut prelude[load],
        MirInstr::MakeRef {
            dest: pointer,
            place,
        },
    );
    if let MirInstr::LoadPlace { dest, .. } = loaded {
        orphan(function, dest);
    }
    extracts.sort_unstable_by(|left, right| right.cmp(left));
    for at in extracts {
        if let MirInstr::GetField { dest, .. } = prelude.remove(at) {
            orphan(function, dest);
        }
    }
    Some(ListElement::Borrowed { pointer, roots })
}

/// The element of a `read` collector the call borrows where it lies: the
/// place `register` loads, its `place.load` in `prelude` (no other
/// instruction there reading it) becoming the `ref.make` of the element
/// pointer, as upstream's call addresses a borrowed argument rather than
/// reading it. `None` for a register no such load of an `element`-typed
/// place defines.
fn borrowed_element(
    prelude: &mut [MirInstr],
    register: Reg,
    element: &Ty,
    function: &mut MirFunction,
    location: Option<&SourceSpan>,
) -> Option<ListElement> {
    let at = prelude.iter().rposition(|instruction| {
        matches!(instruction, MirInstr::LoadPlace { dest, place }
            if *dest == register && place.ty.as_ref() == Some(element))
    })?;
    let mut operands = Vec::new();
    for instruction in prelude.iter() {
        crate::mir::verify::instruction_operand_regs(instruction, &mut operands);
    }
    if operands.contains(&register) {
        return None;
    }
    let MirInstr::LoadPlace { place, .. } = &prelude[at] else {
        return None;
    };
    let place = place.clone();
    let roots = std::iter::once(place.root).chain(place.through).collect();
    let pointer = element_pointer(
        function,
        register,
        element_lit_ref_type(element, false),
        location,
    );
    prelude[at] = MirInstr::MakeRef {
        dest: pointer,
        place,
    };
    orphan(function, register);
    Some(ListElement::Borrowed { pointer, roots })
}

/// Retype `register`, whose defining instruction a collector's
/// construction replaced, as `None`: every register keeps a type, and one
/// no instruction defines must not name a binder only an unrolled
/// `comptime for` body closes.
fn orphan(function: &mut MirFunction, register: Reg) {
    function.reg_types.insert(register.0, Ty::none());
}

/// A fresh register for the pointer that addresses the element `register`
/// held, located where the element is spelled, else at the call.
fn element_pointer(
    function: &mut MirFunction,
    register: Reg,
    ty: Ty,
    location: Option<&SourceSpan>,
) -> Reg {
    let spelled = function
        .spans
        .0
        .get(&register.0)
        .map(|(span, _)| span.clone());
    fresh_register(function, ty, spelled.as_ref().or(location))
}

/// The caller's tables, which a `VariadicList` a call builds extends, its
/// registers located at the call.
struct CallFrame<'a, 'b> {
    function: &'a mut MirFunction,
    location: Option<&'b SourceSpan>,
}

impl FrameTables for CallFrame<'_, '_> {
    fn fresh_register(&mut self, ty: Ty) -> Reg {
        fresh_register(self.function, ty, self.location)
    }

    fn fresh_slot(&mut self, name: String, ty: Ty) -> VarId {
        let slot = u32::try_from(self.function.n_vars).expect("variable count fits a VarId");
        self.function.n_vars += 1;
        self.function.var_names.push(name);
        self.function.var_tys.insert(slot, ty);
        slot
    }
}

/// The type of the collector a call of a callee collecting `element`s
/// builds from `elements`, in the caller's binder scope:
/// `VariadicPack(element)` under the call's bindings, which a bound
/// type pack turns into its elements' tuple, and whose closed element is a
/// homogeneous collector's `VariadicList`'s. A binder of the callee's own
/// `param_decls` the call binds no argument for, or one of the trait a
/// requirement is spelled over, is read off the operands instead, as the
/// elaborator infers it: a type pack is the tuple of their types, a
/// homogeneous element the first one's, each literal materialized.
fn collector_type(
    param_decls: &[ParamDecl],
    element: &Ty,
    elements: &[Reg],
    site: &Site,
    function: &MirFunction,
) -> Option<Ty> {
    let slot = Ty::new(TyKind::VariadicPack(element.clone()));
    let unbound = |closed: &Ty| {
        mojito_types::types::mentions(closed, &|ty| matches!(ty.kind(), TyKind::SelfType))
            || param_decls.iter().any(|decl| {
                !site.bound.iter().any(|(own, _)| own.id() == decl.id())
                    && names_binder(closed, &decl.binder())
            })
    };
    if let Some(closed) = close_parameter_type(&slot, &site.bound)
        && !unbound(&closed)
    {
        return Some(closed);
    }
    let types = elements
        .iter()
        .map(|register| function.reg_types.get(&register.0).map(default_literal))
        .collect::<Option<Vec<_>>>()?;
    if mojito_types::types::pack_spread(std::slice::from_ref(element)).is_some() {
        Some(Ty::new(TyKind::Tuple(types)))
    } else {
        types
            .into_iter()
            .next()
            .map(|first| Ty::new(TyKind::VariadicPack(first)))
    }
}

/// Whether a call of `callee` passing `positional` arguments and the
/// `keywords` leaves a slot out.
fn omits_any(callee: DefaultedCallee<'_>, positional: usize, keywords: &[&str]) -> bool {
    match_call_slots(
        callee.param_names(),
        callee.required(),
        callee.positional_only(),
        callee.keyword_only(),
        positional,
        keywords,
        CallVariadics {
            positional: callee.variadic().is_some(),
            keyword: callee.kw_variadic().is_some(),
        },
    )
    .is_ok_and(|matched| {
        matched
            .slots
            .iter()
            .any(|slot| matches!(slot, ArgSlot::Default))
    })
}

/// `instruction`, an `Index`, as the `MultiIndex` of its one operand, which
/// carries further operands.
fn widen_index(instruction: MirInstr) -> MirInstr {
    let MirInstr::Index {
        dest,
        base,
        index,
        call,
        ..
    } = instruction
    else {
        return instruction;
    };
    MirInstr::MultiIndex {
        dest,
        object: base,
        args: vec![MirSubscriptArg::Index(index)],
        kwargs: Vec::new(),
        call,
    }
}

impl Site {
    /// The call `instruction` as its fill binds it, and the callee it
    /// binds against.
    fn of<'a>(
        instruction: &MirInstr,
        record: &LoweredCall,
        function: &MirFunction,
        caller: &Caller<'a>,
    ) -> Option<(Self, DefaultedCallee<'a>)> {
        let declarations = caller.declarations;
        let mut leading = record.leading;
        let mut passed_receiver = None;
        let (callee, receiver, (instantiated, passed), args, kwargs, spread) = match instruction {
            MirInstr::Call {
                func,
                args,
                kwargs,
                receiver,
                instantiated_args,
                spread,
                ..
            } => (
                record.callee.clone().unwrap_or_else(|| func.0.clone()),
                receiver.clone(),
                (instantiated_args, &[][..]),
                args.clone(),
                kwargs.clone(),
                *spread,
            ),
            MirInstr::MethodCall {
                dest,
                recv,
                resolved,
                args,
                kwargs,
                instantiated_args,
                param_arg_regs,
                spread,
                refined_bound,
                ..
            } => {
                let named = record.callee.clone().or_else(|| resolved.clone())?;
                let receiver = function.reg_types.get(&recv.0)?;
                let (callee, resolved) = resolve_callee(
                    &named,
                    Some(receiver),
                    refined_bound.as_ref().map(|bound| bound.traits.as_slice()),
                    declarations,
                )?;
                if let DefaultedCallee::Required(requirement) = resolved {
                    let mut bound = passed_bindings(
                        &requirement.param_decls,
                        instantiated_args,
                        param_arg_regs,
                        &caller.constants,
                    );
                    bound.extend(requirement_self_bindings(
                        requirement,
                        receiver,
                        declarations,
                    ));
                    return Some((
                        Self {
                            dest: *dest,
                            callee,
                            leading: 0,
                            args: args.clone(),
                            kwargs: kwargs.clone(),
                            bound,
                            spread: *spread,
                            receiver: None,
                            self_type: Some(
                                crate::mir::call_addresses::receiver_value_ty(receiver).clone(),
                            ),
                        },
                        resolved,
                    ));
                }
                (
                    callee,
                    // An addressed receiver is a reference to the
                    // value whose type binds the callee's parameters.
                    Some(crate::mir::call_addresses::receiver_value_ty(receiver).clone()),
                    (instantiated_args, &param_arg_regs[..]),
                    args.clone(),
                    kwargs.clone(),
                    *spread,
                )
            }
            MirInstr::CallIndirect {
                callee,
                args,
                kwargs,
                instantiated_args,
                spread,
                ..
            } => {
                // The check names a callable contract's dispatch symbol,
                // not a declaration, for a call through a function
                // value; the function the callee register statically
                // holds is the callee then.
                let named = record
                    .callee
                    .as_deref()
                    .filter(|callee| declarations.function(callee).is_some());
                let target = if let Some(target) = named {
                    target.to_string()
                } else {
                    let (target, thin) = caller.targets.get(&callee.0)?;
                    let declaration = declarations.function(target)?;
                    // Only a thin free function's value passes no
                    // capture before the arguments.
                    if !*thin || declaration.enclosing.is_some() {
                        return None;
                    }
                    leading = 0;
                    target.clone()
                };
                // An instance method's value takes its receiver first.
                let declaration = declarations.function(&target)?;
                let mut args = args.clone();
                let mut spread = *spread;
                let mut receiver_type = None;
                if declaration.has_receiver
                    && declaration.receiver_convention != Some(ArgConvention::Out)
                {
                    if args.is_empty() || spread == Some(0) {
                        return None;
                    }
                    let register = args.remove(0);
                    receiver_type = function.reg_types.get(&register.0).cloned();
                    spread = spread.map(|position| position - 1);
                    passed_receiver = Some(register);
                }
                (
                    target,
                    receiver_type,
                    (instantiated_args, &[][..]),
                    args,
                    kwargs.clone(),
                    spread,
                )
            }
            MirInstr::MultiIndex {
                dest,
                object,
                args,
                kwargs,
                call: Some(call),
            } => {
                // Only `Index` operands bind the callee's slots; a slice
                // descriptor's call keeps its slots.
                let register = |argument: &MirSubscriptArg| match argument {
                    MirSubscriptArg::Index(register) => Some(*register),
                };
                let receiver = function.reg_types.get(&object.0)?.clone();
                let named = record.callee.clone().unwrap_or_else(|| call.target.clone());
                let (callee, resolved) = resolve_callee(
                    &named,
                    Some(&receiver),
                    call.refined_bound.as_deref(),
                    declarations,
                )?;
                let bound = match resolved {
                    DefaultedCallee::Declared(declaration) => bindings_of(
                        &callee,
                        declaration,
                        (&[], &call.param_arg_regs),
                        Some(&receiver),
                        caller,
                    ),
                    DefaultedCallee::Required(requirement) => {
                        let mut bound = passed_bindings(
                            &requirement.param_decls,
                            &[],
                            &call.param_arg_regs,
                            &caller.constants,
                        );
                        bound.extend(requirement_self_bindings(
                            requirement,
                            &receiver,
                            declarations,
                        ));
                        bound
                    }
                };
                return Some((
                    Self {
                        dest: *dest,
                        callee,
                        leading: 0,
                        args: args.iter().map(register).collect::<Option<Vec<_>>>()?,
                        kwargs: kwargs
                            .iter()
                            .map(|(name, argument)| Some((name.clone(), register(argument)?)))
                            .collect::<Option<Vec<_>>>()?,
                        bound,
                        spread: None,
                        receiver: None,
                        self_type: matches!(resolved, DefaultedCallee::Required(_))
                            .then(|| receiver.clone()),
                    },
                    resolved,
                ));
            }
            _ => return None,
        };
        let declaration = declarations.function(&callee)?;
        let bound = bindings_of(
            &callee,
            declaration,
            (instantiated, passed),
            receiver.as_ref(),
            caller,
        );
        Some((
            Self {
                dest: result_register(instruction)?,
                callee,
                leading,
                args,
                kwargs,
                bound,
                spread,
                receiver: passed_receiver,
                self_type: None,
            },
            DefaultedCallee::Declared(declaration),
        ))
    }

    /// `ty`, a parameter type of the callee, under the call's bindings
    /// ([`close_parameter_type`]), a requirement's abstract `Self` the
    /// receiver's type.
    fn close_parameter_type(&self, ty: &Ty) -> Option<Ty> {
        let ty = match &self.self_type {
            Some(receiver) => replace_abstract_self(ty, receiver)?,
            None => ty.clone(),
        };
        close_parameter_type(&ty, &self.bound)
    }

    /// Store the filled operand lists back into `instruction`; a subscript
    /// contract's argument at each filled `slot` names the operand `source`
    /// it now reads.
    fn store(self, instruction: &mut MirInstr, sources: &[(usize, CheckedCallArgumentSource)]) {
        match instruction {
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
            }
            | MirInstr::CallIndirect {
                args,
                kwargs,
                spread,
                ..
            } => {
                let leading = usize::from(self.receiver.is_some());
                *args = self.receiver.into_iter().chain(self.args).collect();
                *kwargs = self.kwargs;
                *spread = self.spread.map(|position| position + leading);
            }
            MirInstr::MultiIndex {
                args, kwargs, call, ..
            } => {
                *args = self.args.into_iter().map(MirSubscriptArg::Index).collect();
                *kwargs = self
                    .kwargs
                    .into_iter()
                    .map(|(name, register)| (name, MirSubscriptArg::Index(register)))
                    .collect();
                if let Some(call) = call {
                    for (slot, source) in sources {
                        if let Some(argument) = call.arguments.get_mut(*slot) {
                            argument.source = *source;
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// The callee a call of `name` over a receiver of type `receiver` binds
/// its omitted arguments against, and the name it fills them as: `name`'s
/// declaration; for a call through a trait bound (`__trait_dispatch.…`),
/// its trait's member when the requirement has a body
/// ([`requirement_member`]), else the requirement the bound declares
/// ([`MirDeclarations::call_requirement`]), whose defaults run whatever the
/// witness declares.
fn resolve_callee<'a>(
    name: &str,
    receiver: Option<&Ty>,
    refined_bound: Option<&[String]>,
    declarations: &'a MirDeclarations,
) -> Option<(String, DefaultedCallee<'a>)> {
    if let Some(declaration) = declarations.function(name) {
        return Some((name.to_string(), DefaultedCallee::Declared(declaration)));
    }
    if let Some(member) = requirement_member(name, declarations) {
        let declaration = declarations.function(&member)?;
        return Some((member, DefaultedCallee::Declared(declaration)));
    }
    declarations
        .call_requirement(name, receiver, refined_bound)
        .map(|requirement| (name.to_string(), DefaultedCallee::Required(requirement)))
}

/// The arguments a call binds the callee's own binders `decls` to: its
/// `instantiated` arguments when it records them, else, as a call through
/// a bound does, each compile-time argument it `passed`, as the expression
/// over the caller's binders the call records or the constant its register
/// holds (`constants`), else the binder's own default.
fn passed_bindings(
    decls: &[ParamDecl],
    instantiated: &[TyArg],
    passed: &[MirParamArg],
    constants: &HashMap<u32, CtValue>,
) -> Vec<(ParamDecl, TyArg)> {
    if !instantiated.is_empty() {
        return decls
            .iter()
            .cloned()
            .zip(instantiated.iter().cloned())
            .collect();
    }
    let mut bound: Vec<(ParamDecl, TyArg)> = Vec::new();
    for (index, decl) in decls.iter().enumerate() {
        let argument = passed
            .iter()
            .find(|argument| argument.declaration_index(decls) == Some(index))
            .and_then(|argument| {
                argument
                    .expr
                    .clone()
                    .map(CtValue::Expr)
                    .or_else(|| constants.get(&argument.value?.0).cloned())
            })
            .map(TyArg::Val)
            .or_else(|| match decl {
                ParamDecl::Value {
                    default: Some(default),
                    ..
                } => substitute_argument(&TyArg::Val(CtValue::Expr(default.clone())), &bound).ok(),
                _ => None,
            });
        if let Some(argument) = argument {
            bound.push((decl.clone(), argument));
        }
    }
    bound
}

/// The closed scalar each register `function`'s body defines as a
/// constant holds, a literal materialized or built into its storage as
/// the literal: the value a compile-time argument's register carries when
/// the call records no expression for it.
fn register_constants(function: &MirFunction) -> HashMap<u32, CtValue> {
    let mut constants = HashMap::new();
    let instructions = nested_instructions(&function.body);
    // A literal's storage construction follows its definition, so one pass
    // in order resolves each chain.
    for instruction in instructions {
        let (dest, value) = match instruction {
            MirInstr::Const { dest, k } => (
                dest,
                match k {
                    Const::Int(value) => Some(CtValue::Int(*value)),
                    Const::IntLiteral(literal) => literal.to_i64().map(CtValue::Int),
                    Const::Bool(value) => Some(CtValue::Bool(*value)),
                    Const::Str(text) => Some(CtValue::Str(text.clone())),
                    Const::Value(value) if k.is_folded_scalar() => Some(value.clone()),
                    _ => None,
                },
            ),
            MirInstr::MaterializeLiteral { dest, value, .. } => {
                (dest, constants.get(&value.0).cloned())
            }
            MirInstr::MakeStruct { dest, fields } if fields.len() == 1 => {
                (dest, constants.get(&fields[0].0).cloned())
            }
            _ => continue,
        };
        if let Some(value) = value {
            constants.insert(dest.0, value);
        }
    }
    constants
}

/// The lowered body of the trait member whose requirement dispatches as
/// `dispatch` (`__trait_dispatch.f$…`), when the member has one: the
/// trait's method `Trait.member`.
fn requirement_member(dispatch: &str, declarations: &MirDeclarations) -> Option<String> {
    declarations.trait_declarations().find_map(|declaration| {
        declaration
            .requirements
            .iter()
            .find(|requirement| requirement.dispatch == dispatch)
            .map(|requirement| format!("{}.{}", declaration.name, requirement.name))
            .filter(|member| {
                declarations
                    .function(member)
                    .is_some_and(|declaration| declaration.has_receiver)
            })
    })
}

/// The arguments the call binds `callee`'s binders to
/// ([`passed_bindings`]), then those of the scopes around it: a method's
/// struct's from `receiver`, the receiver instance's type (`Self`'s
/// binders standing for themselves), a trait member's trait's `_Self` from
/// it, and a nested body's enclosing declarations', standing for
/// themselves inside the function declaring them.
fn bindings_of(
    callee: &str,
    declaration: &MirFunctionDeclaration,
    (instantiated, passed): (&[TyArg], &[MirParamArg]),
    receiver: Option<&Ty>,
    caller: &Caller<'_>,
) -> Vec<(ParamDecl, TyArg)> {
    let declarations = caller.declarations;
    let mut bound = passed_bindings(
        &declaration.param_decls,
        instantiated,
        passed,
        &caller.constants,
    );
    if let Some(enclosing) = declaration.enclosing.as_deref()
        && declared_within(caller.name, enclosing, declarations)
    {
        bound.extend(
            scope_binders(callee, declaration, declarations)
                .into_iter()
                .map(|decl| {
                    let argument = decl.own_argument();
                    (decl, argument)
                }),
        );
    }
    if let Some((name, arguments)) = receiver.and_then(|ty| ty.kind().as_non_simd_struct())
        && let Some(owner) = declarations.structure(name)
    {
        let view = owner.argument_view();
        bound.extend(
            ParamDecl::aligned_pairs(&view, &owner.view_arguments(arguments))
                .map(|(decl, argument)| (decl.clone(), argument.clone())),
        );
    }
    // A trait member's receiver binds its trait's `_Self`, which its
    // defaults read as a method's read its struct's binders.
    if let Some(receiver) = receiver
        && let Some(owner) = mojito_symbol::symbol::split_method_symbol(callee)
            .and_then(|(owner, _)| declarations.trait_declaration(owner))
    {
        bound.extend(owner.param_decls.iter().map(|decl| {
            (
                decl.clone(),
                TyArg::Ty(crate::mir::call_addresses::receiver_value_ty(receiver).clone()),
            )
        }));
    }
    bound
}

/// The binding of the `_Self` of the trait declaring `requirement` to
/// `receiver`'s value type, which the requirement's lifted defaults read
/// as their `Self`, as a trait member's receiver binds its trait's
/// ([`bindings_of`]).
fn requirement_self_bindings(
    requirement: &TraitRequirement,
    receiver: &Ty,
    declarations: &MirDeclarations,
) -> Vec<(ParamDecl, TyArg)> {
    let receiver = crate::mir::call_addresses::receiver_value_ty(receiver);
    declarations
        .trait_declarations()
        .find(|declaration| {
            declaration
                .requirements
                .iter()
                .any(|own| std::ptr::eq(own, requirement))
        })
        .map(|declaration| {
            declaration
                .param_decls
                .iter()
                .map(|decl| (decl.clone(), TyArg::Ty(receiver.clone())))
                .collect()
        })
        .unwrap_or_default()
}

/// The binders in scope around `callee`, the function `name`, beside its
/// own: its struct's, for a method, and for a nested function each
/// enclosing function's and its struct's, innermost first.
fn scope_binders(
    name: &str,
    callee: &MirFunctionDeclaration,
    declarations: &MirDeclarations,
) -> Vec<ParamDecl> {
    let mut binders = Vec::new();
    let mut scope = Some((name, callee));
    while let Some((name, declaration)) = scope {
        if !std::ptr::eq(declaration, callee) {
            binders.extend(declaration.argument_binders().cloned());
        }
        if let Some(owner) = mojito_symbol::symbol::split_method_symbol(name)
            .and_then(|(owner, _)| declarations.structure(owner))
        {
            binders.extend(owner.argument_binders().cloned());
        }
        scope = declaration.enclosing.as_deref().and_then(|enclosing| {
            declarations
                .function(enclosing)
                .map(|declaration| (enclosing, declaration))
        });
    }
    binders
}

/// Whether `caller` is the function `enclosing` or a body nested under it,
/// where `enclosing`'s binders stand for themselves.
fn declared_within(caller: &str, enclosing: &str, declarations: &MirDeclarations) -> bool {
    let mut scope = Some(caller);
    while let Some(name) = scope {
        if name == enclosing {
            return true;
        }
        scope = declarations
            .function(name)
            .and_then(|declaration| declaration.enclosing.as_deref());
    }
    false
}

/// The arguments `site` leaves out of `callee`, in slot order, each with
/// the default it takes; an error names the first slot the call cannot
/// fill.
fn omitted_arguments(
    site: &Site,
    callee: DefaultedCallee<'_>,
    declarations: &MirDeclarations,
) -> Result<Vec<Omitted>, String> {
    let skip = site.leading;
    let keywords: Vec<&str> = site.kwargs.iter().map(|(name, _)| name.as_str()).collect();
    let matched = match_call_slots(
        callee.param_names().get(skip..).unwrap_or_default(),
        callee.required().get(skip..).unwrap_or_default(),
        callee
            .positional_only()
            .and_then(|limit| limit.checked_sub(skip)),
        callee
            .keyword_only()
            .and_then(|limit| limit.checked_sub(skip)),
        site.args.len(),
        &keywords,
        CallVariadics {
            positional: callee.variadic().is_some(),
            keyword: callee.kw_variadic().is_some(),
        },
    )
    .map_err(|error| format!("{error:?}"))?;
    matched
        .slots
        .iter()
        .enumerate()
        .filter(|(_, kind)| matches!(kind, ArgSlot::Default))
        .map(|(slot, _)| omitted_argument(site, callee, slot + skip, declarations))
        .collect()
}

/// The default `callee` declares at `slot`, bound by `site`.
fn omitted_argument(
    site: &Site,
    callee: DefaultedCallee<'_>,
    slot: usize,
    declarations: &MirDeclarations,
) -> Result<Omitted, String> {
    let name = callee
        .param_names()
        .get(slot)
        .cloned()
        .ok_or_else(|| format!("slot {slot} names no parameter"))?;
    let Some(Some(default)) = callee.defaults().get(slot) else {
        return Err(format!("parameter '{name}' declares no lowered default"));
    };
    let convention = callee.param_conventions().get(slot).copied().flatten();
    if ArgConvention::is_reference(convention) || matches!(convention, Some(ArgConvention::Out)) {
        return Err(format!(
            "parameter '{name}' is passed by reference, which no default fills"
        ));
    }
    let default = match default {
        CheckedConst::Evaluate {
            function,
            arguments,
        } => {
            let thunk = declarations
                .function(function)
                .ok_or_else(|| format!("default function '{function}' is not declared"))?;
            if thunk.raises {
                return Err(format!("the default of '{name}' raises"));
            }
            let closed = if arguments.is_empty() {
                thunk
                    .argument_binders()
                    .map(|decl| {
                        site.bound
                            .iter()
                            .find(|(own, _)| own.id() == decl.id())
                            .map(|(_, argument)| argument.clone())
                            .ok_or_else(|| {
                                format!(
                                    "the default of '{name}' reads `{}`, which the call binds no argument for",
                                    decl.name()
                                )
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                arguments
                    .iter()
                    .map(|argument| substitute_argument(argument, &site.bound))
                    .collect::<Result<Vec<_>, _>>()?
            };
            let ty = callee
                .param_types()
                .get(slot)
                .and_then(|ty| site.close_parameter_type(ty))
                .ok_or_else(|| {
                    format!("the type of '{name}' names a binder the call binds no argument for")
                })?;
            OmittedDefault::Applied(default_application(function, &closed, &ty).map_err(
                |error| format!("the default of '{name}' applies to no argument: {error}"),
            )?)
        }
        CheckedConst::Construct {
            target,
            arg,
            parameter,
        } => OmittedDefault::Constant(CheckedConst::Construct {
            target: target.clone(),
            arg: arg.clone(),
            parameter: parameter
                .as_ref()
                .map(|parameter| {
                    site.close_parameter_type(parameter).ok_or_else(|| {
                        format!(
                            "the default of '{name}' converts at a parameter naming a binder the call binds no argument for"
                        )
                    })
                })
                .transpose()?,
        }),
        default => OmittedDefault::Constant(default.clone()),
    };
    let ty = callee
        .param_types()
        .get(slot)
        .ok_or_else(|| format!("parameter '{name}' has no type"))
        .and_then(|ty| {
            site.close_parameter_type(ty).ok_or_else(|| {
                format!("the type of '{name}' names a binder the call binds no argument for")
            })
        })?;
    if mentions(&ty, &|inner| matches!(inner.kind(), TyKind::SelfType)) {
        return Err(format!("the type of '{name}' names `Self`"));
    }
    Ok(Omitted {
        slot,
        name,
        owned: matches!(convention, Some(ArgConvention::Var | ArgConvention::Deinit)),
        by_address: callee.by_address(slot, &ty, declarations),
        ty,
        convention,
        writes: callee.param_writes(slot),
        default,
    })
}

/// The bindings `bound` spells as replacement bindings.
fn param_bindings(bound: &[(ParamDecl, TyArg)]) -> (ParamContext, ParamBindings) {
    let context = ParamContext::detached();
    let mut bindings = ParamBindings::new();
    for (decl, argument) in bound {
        match argument {
            TyArg::Ty(argument) => bindings.bind_type(decl.id().clone(), argument.clone()),
            TyArg::Val(value) => {
                if let Ok(value) = context.constant(value.clone()) {
                    bindings.bind(decl.id().clone(), value);
                }
            }
            TyArg::Origin(_) => {}
        }
    }
    (context, bindings)
}

/// `argument`, an application a wrapper's declaration recorded over its own
/// binders, under the call's `bound` arguments.
fn substitute_argument(argument: &TyArg, bound: &[(ParamDecl, TyArg)]) -> Result<TyArg, String> {
    let (context, bindings) = param_bindings(bound);
    replace_argument_parameters(&context, argument, &bindings)
        .map_err(|error| format!("a default's argument does not close: {error}"))
}

/// `ty`, a parameter type of the callee, under the call's `bound`
/// arguments; `None` when it still names a binder they bind to anything but
/// itself.
fn close_parameter_type(ty: &Ty, bound: &[(ParamDecl, TyArg)]) -> Option<Ty> {
    let (context, bindings) = param_bindings(bound);
    let closed = replace_parameters(&context, ty, &bindings, 0).ok()?;
    (!bound.iter().any(|(decl, argument)| {
        *argument != decl.own_argument() && names_binder(&closed, &decl.binder())
    }))
    .then_some(closed)
}

/// Emit the value `default` takes onto `prelude`: a literal's constant
/// materialized at the parameter type, a recorded conversion's constructor
/// run on its literal, or the compile-time value of the lifted default
/// function under the arguments the call binds its binders to.
fn evaluate_default(
    function: &mut MirFunction,
    default: &Omitted,
    location: Option<&SourceSpan>,
    declarations: &MirDeclarations,
    prelude: &mut Vec<MirInstr>,
) -> Reg {
    match &default.default {
        OmittedDefault::Applied(application) => {
            let value = fresh_register(function, default.ty.clone(), location);
            prelude.push(MirInstr::Const {
                dest: value,
                k: Const::Param(application.clone()),
            });
            value
        }
        OmittedDefault::Constant(constant) => evaluate_constant(
            function,
            constant,
            &default.ty,
            location,
            declarations,
            prelude,
        ),
    }
}

/// Emit `default`, a literal or a recorded conversion of one, at the
/// parameter type `ty` onto `prelude`.
fn evaluate_constant(
    function: &mut MirFunction,
    default: &CheckedConst,
    ty: &Ty,
    location: Option<&SourceSpan>,
    declarations: &MirDeclarations,
    prelude: &mut Vec<MirInstr>,
) -> Reg {
    match default {
        CheckedConst::Construct {
            target,
            arg,
            parameter,
        } => {
            let at = parameter.clone().unwrap_or_else(|| literal_type(arg));
            let argument = evaluate_constant(function, arg, &at, location, declarations, prelude);
            let value = fresh_register(function, ty.clone(), location);
            // The constructor is owner-less: the call binds the struct's
            // parameters to the arguments of the type it builds, as a
            // written conversion's call does, a value for each declared one.
            let instantiated_args = constructed_arguments(target, ty, declarations);
            prelude.push(MirInstr::Call {
                conventions: crate::mir::MirArgConventions::default(),
                dest: value,
                func: FuncRef::named(target),
                raises: None,
                args: vec![argument],
                kwargs: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
                receiver: None,
                instantiated_args,
                spread: None,
            });
            value
        }
        literal => {
            let literal_ty = literal_type(literal);
            let value = fresh_register(function, literal_ty.clone(), location);
            prelude.push(MirInstr::Const {
                dest: value,
                k: literal_constant(literal),
            });
            materialize(function, value, &literal_ty, ty, location, prelude)
        }
    }
}

/// The compile-time application of the lifted default function `thunk` to
/// `arguments`, the closed arguments of its binders, typed at the
/// parameter type `ty`: the value the elaborator evaluates once per
/// instance and each call materializes, as upstream's call binding writes
/// the default's parameter expression into the call.
fn default_application(thunk: &str, arguments: &[TyArg], ty: &Ty) -> Result<ParamExpr, String> {
    let context = ParamContext::detached();
    let applied = arguments
        .iter()
        .map(|argument| match argument {
            TyArg::Ty(ty) => Ok(context.type_shape(ty.clone())),
            TyArg::Val(value) => context.constant(value.clone()).map_err(|e| e.to_string()),
            TyArg::Origin(_) => Err("an origin binder, which no application binds".to_string()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(context.apply(thunk, &applied, MetaTy::value(ty.clone())))
}

/// The type a literal default's constant has before conversion.
fn literal_type(literal: &CheckedConst) -> Ty {
    match literal {
        CheckedConst::Int(value) => Ty::int_literal_of(value),
        CheckedConst::Float(value) => Ty::float_literal_of(value),
        CheckedConst::Bool(_) => Ty::bool(),
        CheckedConst::String(text) => Ty::string_literal_of(text),
        CheckedConst::None => Ty::none(),
        CheckedConst::Construct { .. } | CheckedConst::Evaluate { .. } => Ty::error(),
    }
}

/// The constant a literal default's `Const` writes.
fn literal_constant(literal: &CheckedConst) -> Const {
    match literal {
        CheckedConst::Int(value) => Const::IntLiteral(value.clone()),
        CheckedConst::Float(value) => Const::FloatLiteral(value.clone()),
        CheckedConst::Bool(value) => Const::Bool(*value),
        CheckedConst::String(value) => Const::Str(value.clone()),
        CheckedConst::None | CheckedConst::Construct { .. } | CheckedConst::Evaluate { .. } => {
            Const::None
        }
    }
}

/// `value`, a literal of type `found`, materialized at the parameter type
/// `target` as a written literal argument is: a `SIMD` of any width
/// converted at its whole `!kgen.simd` storage, as `#pop.int_literal_convert`
/// is typed, and the struct created over it.
fn materialize(
    function: &mut MirFunction,
    value: Reg,
    found: &Ty,
    target: &Ty,
    location: Option<&SourceSpan>,
    prelude: &mut Vec<MirInstr>,
) -> Reg {
    if !literal_materializes_to(found, target) {
        return value;
    }
    let Some(storage_ty) = mojito_types::types::simd_storage(target) else {
        return value;
    };
    let dest = fresh_register(function, target.clone(), location);
    let storage = fresh_register(function, storage_ty.clone(), location);
    let conversion = MirInstr::MaterializeLiteral {
        dest: storage,
        value,
        target: storage_ty,
    };
    prelude.extend(storage_construction(dest, storage, conversion));
    dest
}

/// Add the convention rows of the arguments a call's fill passed to
/// `instruction`'s record, as the check records a written argument's: to
/// the record the call carries, or as the whole record of a call that
/// passed no operand of its own.
fn record_filled_conventions(
    instruction: &mut MirInstr,
    filled: Vec<CheckedArgumentConvention>,
    passes_operands: bool,
) {
    let (MirInstr::Call { conventions, .. } | MirInstr::MethodCall { conventions, .. }) =
        instruction
    else {
        return;
    };
    if filled.is_empty() || (passes_operands && conventions.iter().next().is_none()) {
        return;
    }
    *conventions = MirArgConventions::new(conventions.iter().copied().chain(filled));
}

/// Bind `value`, the default's result, to its call-result slot: a borrowed
/// parameter reads the slot, which the caller destroys once the call
/// returns, by its address when the parameter takes the argument's storage
/// (`by_address`), and an owned one takes it. Returns the operand.
fn bind_temporary(
    function: &mut MirFunction,
    value: Reg,
    default: &Omitted,
    location: Option<&SourceSpan>,
    prelude: &mut Vec<MirInstr>,
) -> Reg {
    let variable = u32::try_from(function.n_vars).expect("variable count fits a VarId");
    function.n_vars += 1;
    function
        .var_names
        .push(format!("{CALL_RESULT_SLOT}{}", value.0));
    function.var_tys.insert(variable, default.ty.clone());
    prelude.push(MirInstr::DefVar {
        var: variable,
        src: value,
        binding_ty: Some(default.ty.clone()),
        // A conversion's constructor call writes the slot in place; a
        // materialized compile-time value is bound as a written one is.
        in_place: !matches!(default.default, OmittedDefault::Applied(_)),
    });
    let place = MirPlace::root(variable, Some(default.ty.clone()));
    if default.owned {
        let read = fresh_register(function, default.ty.clone(), location);
        prelude.push(MirInstr::UseVar {
            dest: read,
            var: variable,
            mode: UseMode::Forward,
            constructor: None,
        });
        return read;
    }
    if default.by_address {
        let handle = mir_place_handle_ty(&place, false, Some(Mutability::Immutable))
            .unwrap_or_else(Ty::error);
        let address = fresh_register(function, handle, location);
        prelude.push(MirInstr::MakeRef {
            dest: address,
            place,
        });
        return address;
    }
    let read = fresh_register(function, default.ty.clone(), location);
    prelude.push(MirInstr::LoadPlace { dest: read, place });
    read
}

/// The registers `instruction` reads, as [`crate::mir::count_register_reads`]
/// counts them: every register it names but its own results.
fn register_operands(instruction: &mut MirInstr) -> Vec<u32> {
    let mut results = Vec::new();
    instruction_result_regs(instruction, &mut results);
    let mut operands = Vec::new();
    for reg in instruction_regs_mut(instruction) {
        match results.iter().position(|result| result == reg) {
            Some(at) => {
                results.swap_remove(at);
            }
            None => operands.push(reg.0),
        }
    }
    operands
}

/// What defines a register [`erase_unpassed_defaults`] may erase.
enum Producer {
    /// The compile-time application of a lifted default (`$default$f$x`).
    Default,
    /// A whole load, or the address, of a read temporary.
    Temporary(VarId),
}

/// Record in `producers` each register `block` and their regions define by
/// a [`Producer`], and in `bindings` each register a slot is bound to.
fn collect_producers(
    block: &MirBlock,
    function: &MirFunction,
    producers: &mut HashMap<u32, Producer>,
    bindings: &mut HashMap<VarId, Vec<u32>>,
) {
    for instruction in &block.instrs {
        match instruction {
            MirInstr::Const {
                dest,
                k: Const::Param(application),
            } if matches!(
                application.kind(),
                ParamKind::Apply { function, .. }
                    if matches!(lifted_owner(function), Some(LiftedOwner::Default { .. }))
            ) =>
            {
                producers.insert(dest.0, Producer::Default);
            }
            MirInstr::LoadPlace { dest, place } | MirInstr::MakeRef { dest, place }
                if place.proj.is_empty()
                    && place.through.is_none()
                    && function
                        .var_names
                        .get(place.root as usize)
                        .is_some_and(|name| name.starts_with(CALL_RESULT_SLOT)) =>
            {
                producers.insert(dest.0, Producer::Temporary(place.root));
            }
            MirInstr::DefVar { var, src, .. } => bindings.entry(*var).or_default().push(src.0),
            _ => {}
        }
        for region in instruction_regions(instruction) {
            collect_producers(region, function, producers, bindings);
        }
    }
}

/// The slots and registers that serve only the read temporary `slot`, once
/// the call took the loads of it among `dropped`: an instruction naming one
/// of the slots or reading one of the registers serves it, and its slots
/// and its results but the dropped ones join. `None` when a region's
/// instruction or a terminator would serve it, or a slot other than `slot`
/// the inliner did not introduce would join.
fn temporary_slice(
    function: &mut MirFunction,
    slot: VarId,
    dropped: &BTreeSet<u32>,
) -> Option<(BTreeSet<VarId>, HashSet<u32>)> {
    let mut slice = Slice {
        names: &function.var_names,
        dropped,
        slots: BTreeSet::from([slot]),
        regs: HashSet::new(),
    };
    loop {
        let before = (slice.slots.len(), slice.regs.len());
        slice.grow(&mut function.body)?;
        if (slice.slots.len(), slice.regs.len()) == before {
            return Some((slice.slots, slice.regs));
        }
    }
}

/// The slots and registers [`temporary_slice`] grows.
struct Slice<'a> {
    names: &'a [String],
    dropped: &'a BTreeSet<u32>,
    slots: BTreeSet<VarId>,
    regs: HashSet<u32>,
}

impl Slice<'_> {
    /// Add what each instruction of `blocks` serving the slice names and
    /// defines; `None` when the slice cannot be erased.
    fn grow(&mut self, block: &mut MirBlock) -> Option<()> {
        for instruction in &mut block.instrs {
            let named = instruction_slots(instruction);
            let serves = named.iter().any(|slot| self.slots.contains(slot))
                || register_operands(instruction)
                    .iter()
                    .any(|reg| self.regs.contains(reg));
            if !instruction_regions(instruction).is_empty() {
                if serves {
                    return None;
                }
                for region in instruction_regions_mut(instruction) {
                    self.grow(region)?;
                }
                continue;
            }
            if !serves {
                continue;
            }
            for slot in named {
                let introduced = self
                    .names
                    .get(slot as usize)
                    .is_some_and(|name| name.contains("$inline"));
                if !self.slots.contains(&slot) && !introduced {
                    return None;
                }
                self.slots.insert(slot);
            }
            let mut results = Vec::new();
            instruction_result_regs(instruction, &mut results);
            self.regs.extend(
                results
                    .into_iter()
                    .map(|reg| reg.0)
                    .filter(|reg| !self.dropped.contains(reg)),
            );
        }
        if terminator_regs_mut(&mut block.term)
            .iter()
            .any(|reg| self.regs.contains(&reg.0))
        {
            return None;
        }

        Some(())
    }
}

/// Remove from `blocks` and their regions each instruction serving the
/// slice `slots` and `regs` and each defining only `calls`, recording their
/// results in `erased`, and every cleanup entry of a slot in `slots`.
fn erase_slice(
    block: &mut MirBlock,
    slots: &BTreeSet<VarId>,
    regs: &HashSet<u32>,
    calls: &HashSet<u32>,
    erased: &mut HashSet<u32>,
) {
    block.instrs.retain_mut(|instruction| {
        if !instruction_regions(instruction).is_empty() {
            for region in instruction_regions_mut(instruction) {
                erase_slice(region, slots, regs, calls, erased);
            }
            return true;
        }
        let mut results = Vec::new();
        instruction_result_regs(instruction, &mut results);
        let serves = instruction_slots(instruction)
            .iter()
            .any(|slot| slots.contains(slot))
            || register_operands(instruction)
                .iter()
                .any(|reg| regs.contains(reg))
            || (!results.is_empty() && results.iter().all(|reg| calls.contains(&reg.0)));
        if serves {
            erased.extend(results.iter().map(|reg| reg.0));
        }
        !serves
    });
}

/// The arguments a call of `constructor` binds when it builds `ty`, an
/// instance of its struct: a value for each declared parameter
/// ([`ParamDecl::constructor_call_arguments`]).
fn constructed_arguments(constructor: &str, ty: &Ty, declarations: &MirDeclarations) -> Vec<TyArg> {
    let Some((_, arguments)) = ty.kind().as_non_simd_struct() else {
        return Vec::new();
    };
    declarations.function(constructor).map_or_else(
        || arguments.clone(),
        |declaration| ParamDecl::constructor_call_arguments(&declaration.param_decls, arguments),
    )
}

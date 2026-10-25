//! Omitted arguments, written into the call.
//!
//! Upstream's call binding writes every argument a call leaves out into the
//! call, as the callee's default attribute evaluated in the caller: a
//! borrowed default is the caller's temporary, destroyed once the call
//! returns, and an owned one is handed to the callee. The lowering of each
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
//! ([`crate::mir::variadic_list`]), passed as one operand. It
//! builds the keyword collector a callee's `**kwargs` takes as upstream's
//! call builds the dictionary: the collector's empty constructor, one
//! `_insert` per collected keyword, and the filled collector passed as the
//! call's one `**` operand.

use crate::mir::variadic_list::{
    FrameTables, ListElement, build_variadic_list, element_pointer_type,
};
use crate::mir::verify::instruction_result_regs;
use crate::mir::{
    CALL_RESULT_SLOT, Const, FuncRef, LoweredCall, MirBlock, MirDeclarations, MirFunction,
    MirFunctionDeclaration, MirInstr, MirPlace, MirSubscriptArg, Reg, UseMode, callable_targets,
    calls::literal_materializes_to, fresh_register, instruction_regions, instruction_regions_mut,
    instruction_regs_mut, instruction_slots, terminator_regs_mut,
};
use mojito_ast::ast::{ArgConvention, FORWARDED_KWARGS_NAME};
use mojito_ast::call::{ArgSlot, CallVariadics, match_call_slots};
use mojito_checked::checked::{CheckedCallArgumentSource, CheckedConst};
use mojito_common::token::SourceSpan;
use mojito_hir::hir::VarId;
use mojito_symbol::symbol::{LiftedOwner, lifted_owner};
use mojito_types::param_expr::{ParamBindings, ParamContext};
use mojito_types::types::{
    ParamDecl, Ty, TyArg, TyKind, default_literal, names_binder, replace_argument_parameters,
    replace_parameters,
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
/// struct's from the receiver's type (a bare struct's binders stand for
/// themselves), and for a nested body each enclosing declaration's, which
/// stand for themselves inside it. A literal default is its constant,
/// materialized at the parameter type as a written literal is; a recorded
/// conversion of one runs its constructor; any other default is a call of
/// the lifted default function (`$default$f$x`) under the binders it
/// declares. The argument goes positionally when every earlier slot is
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
        };
        let mut blocks = std::mem::take(&mut function.blocks);
        fill_blocks(
            &mut blocks,
            function,
            &caller,
            &mut pending,
            invariant_errors,
        );
        function.blocks = blocks;
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
/// default's call (`$default$f$x`) that defines it; one loaded from the
/// read temporary that call's result was bound to goes with the temporary:
/// its binding, its loads, and its destruction, every instruction that
/// only served it. A temporary whose destruction reaches a terminator, a
/// region, or a slot the inliner did not introduce stays, with its default.
pub fn erase_unpassed_defaults(
    function: &mut MirFunction,
    dropped: &BTreeSet<u32>,
    original: &[u32],
) -> HashSet<u32> {
    let mut producers = HashMap::new();
    let mut bindings: HashMap<VarId, Vec<u32>> = HashMap::new();
    collect_producers(&function.blocks, function, &mut producers, &mut bindings);
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
                if !once(source) || !matches!(producers.get(&source), Some(Producer::Default)) {
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
    erase_slice(&mut function.blocks, &slots, &regs, &calls, &mut erased);
    erased
}

/// The body whose calls are filled: its name, the declarations its callees
/// are found in, and the function each of its registers statically holds.
struct Caller<'a> {
    name: &'a str,
    declarations: &'a MirDeclarations,
    targets: HashMap<u32, (String, bool)>,
}

/// The call being filled: its operand lists, taken from the instruction
/// and stored back once filled ([`Site::store`]).
struct Site {
    dest: Reg,
    callee: String,
    leading: usize,
    args: Vec<Reg>,
    kwargs: Vec<(String, Reg)>,
    arg_places: Vec<Option<MirPlace>>,
    kwarg_places: Vec<Option<MirPlace>>,
    /// The call's bindings of the callee's binders and those in scope
    /// around it.
    bound: Vec<(ParamDecl, TyArg)>,
    /// The position in `args` of the positional collector the call builds
    /// ([`build_collector`]).
    spread: Option<usize>,
}

/// One omitted argument, bound: the parameter it fills and the value its
/// default takes.
struct Omitted {
    slot: usize,
    name: String,
    ty: Ty,
    owned: bool,
    default: CheckedConst,
}

fn fill_blocks(
    blocks: &mut [MirBlock],
    function: &mut MirFunction,
    caller: &Caller<'_>,
    pending: &mut HashMap<u32, LoweredCall>,
    invariant_errors: &mut Vec<String>,
) {
    for block in blocks {
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
            if let Some(record) =
                result_register(&instruction).and_then(|dest| pending.remove(&dest.0))
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
                collect_through_contract(&mut instruction, function, &mut filled);
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
    if let MirInstr::Index { .. } = &instruction {
        let Some(callee) = record
            .callee
            .as_deref()
            .and_then(|callee| caller.declarations.function(callee))
        else {
            return instruction;
        };
        if !omits_any(callee, 1, &[]) && callee.variadic.is_none() {
            return instruction;
        }
        instruction = widen_index(instruction);
    }
    let Some(mut site) = Site::of(&instruction, record, function, caller) else {
        collect_through_requirement(&mut instruction, function, caller, (prelude, postlude));
        collect_through_contract(&mut instruction, function, prelude);
        return instruction;
    };
    let declarations = caller.declarations;
    let Some(callee) = declarations.function(&site.callee) else {
        return instruction;
    };
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
    let positional_limit = callee
        .keyword_only
        .unwrap_or(callee.param_names.len())
        .min(callee.param_names.len());
    site.arg_places.resize(site.args.len(), None);
    site.kwarg_places.resize(site.kwargs.len(), None);
    let mut sources = Vec::new();
    for default in omitted {
        let positional = default.slot == leading + site.args.len()
            && default.slot < positional_limit
            && callee
                .variadic_index
                .is_none_or(|collector| default.slot < collector);
        let value = evaluate_default(function, &default, location.as_ref(), prelude);
        let argument = if declarations.register_passable(&default.ty) {
            value
        } else {
            bind_temporary(function, value, &default, location.as_ref(), prelude)
        };
        if positional {
            sources.push((
                default.slot,
                CheckedCallArgumentSource::Positional(site.args.len()),
            ));
            site.args.push(argument);
            site.arg_places.push(None);
        } else {
            sources.push((
                default.slot,
                CheckedCallArgumentSource::Keyword(site.kwargs.len()),
            ));
            site.kwargs.push((default.name, argument));
            site.kwarg_places.push(None);
        }
    }
    if !matches!(instruction, MirInstr::MultiIndex { .. })
        && let Err(error) = build_keyword_collector(
            &mut site,
            &CallShape::from(callee),
            function,
            location.as_ref(),
            prelude,
        )
    {
        invariant_errors.push(format!(
            "{}: a call of '{}' builds no keyword collector: {error}",
            caller.name, site.callee
        ));
    }
    // A subscript's contract names each operand by its source, which a
    // keyword's move to its position would rename.
    if !matches!(instruction, MirInstr::MultiIndex { .. }) || site.kwargs.is_empty() {
        build_collector(
            &mut site,
            &CallShape::from(callee),
            function,
            location.as_ref(),
            prelude,
            postlude,
        );
    }
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
/// its index operands are the overflow, its value the keyword-only `value`.
/// A store indexing by a slice keeps its operands.
fn collect_subscript_store(
    instruction: &mut MirInstr,
    function: &mut MirFunction,
    caller: &Caller<'_>,
    (prelude, postlude): (&mut Vec<MirInstr>, &mut Vec<MirInstr>),
) {
    let MirInstr::MultiSet {
        receiver,
        receiver_place,
        args,
        arg_places,
        value,
        value_place,
        value_keyword: true,
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
            MirSubscriptArg::Slice { .. } => None,
        })
        .collect::<Option<Vec<_>>>()
    else {
        return;
    };
    let receiver_ty = function
        .reg_types
        .get(&receiver.0)
        .or_else(|| receiver_place.as_ref()?.ty.as_ref())
        .cloned();
    let mut site = Site {
        dest: *value,
        callee: call.target.clone(),
        leading: 0,
        args: registers,
        kwargs: vec![("value".to_string(), *value)],
        arg_places: arg_places.clone(),
        kwarg_places: vec![value_place.as_deref().cloned()],
        bound: bindings_of(&call.target, callee, &[], receiver_ty.as_ref(), caller),
        spread: None,
    };
    let location = function.spans.0.get(&value.0).map(|(span, _)| span.clone());
    build_collector(
        &mut site,
        &CallShape::from(callee),
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
    *arg_places = site.arg_places;
    collect_contract_arguments(call, position, callee, function, collector);
}

/// Name the collector a subscript builds at `position` in its contract
/// `call`, in place of the overflow operands it holds: one argument of the
/// collector's type past the declared parameters', under `callee`'s
/// collector convention.
fn collect_contract_arguments(
    call: &mut crate::mir::MirSubscriptCall,
    position: usize,
    callee: &MirFunctionDeclaration,
    function: &MirFunction,
    collector: Reg,
) {
    let Some(ty) = function.reg_types.get(&collector.0) else {
        return;
    };
    call.arguments.retain(|argument| {
        !matches!(argument.source, CheckedCallArgumentSource::Positional(index) if index >= position)
    });
    let at = callee.param_types.len().min(call.arguments.len());
    call.arguments.insert(
        at,
        mojito_checked::checked::CheckedCallArgument {
            source: CheckedCallArgumentSource::Positional(position),
            parameter_ty: ty.clone(),
            requires_place: false,
            convention: callee.variadic_convention,
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
    /// The callee's own compile-time parameters.
    param_decls: &'a [ParamDecl],
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
            param_decls: &declaration.param_decls,
        }
    }
}

/// Build the positional and keyword collectors of a method call through a
/// bound whose requirement has no body, which no declaration describes, from
/// the requirement's call shape
/// ([`mojito_checked::declaration::TraitRequirement`]), as [`build_collector`]
/// and [`build_keyword_collector`] build a declared callee's.
fn collect_through_requirement(
    instruction: &mut MirInstr,
    function: &mut MirFunction,
    caller: &Caller<'_>,
    (prelude, postlude): (&mut Vec<MirInstr>, &mut Vec<MirInstr>),
) {
    let MirInstr::MethodCall {
        dest,
        resolved: Some(dispatch),
        args,
        kwargs,
        arg_places,
        kwarg_places,
        param_decls,
        instantiated_args,
        spread: None,
        ..
    } = &*instruction
    else {
        return;
    };
    let Some(requirement) = caller
        .declarations
        .trait_declarations()
        .flat_map(|declaration| &declaration.requirements)
        .find(|requirement| requirement.dispatch == *dispatch)
    else {
        return;
    };
    if requirement.variadic.is_none() && requirement.kw_variadic.is_none() {
        return;
    }
    let shape = CallShape {
        param_names: &requirement.param_names,
        required: &requirement.required,
        positional_only: requirement.positional_only,
        keyword_only: requirement.keyword_only,
        variadic: requirement.variadic.as_ref(),
        variadic_convention: requirement.variadic_convention,
        kw_variadic: requirement.kw_variadic.as_ref(),
        param_decls,
    };
    let mut site = Site {
        dest: *dest,
        callee: dispatch.clone(),
        leading: 0,
        args: args.clone(),
        kwargs: kwargs.clone(),
        arg_places: arg_places.clone(),
        kwarg_places: kwarg_places.clone(),
        bound: param_decls
            .iter()
            .cloned()
            .zip(instantiated_args.iter().cloned())
            .collect(),
        spread: None,
    };
    let location = function.spans.0.get(&dest.0).map(|(span, _)| span.clone());
    let collected =
        build_keyword_collector(&mut site, &shape, function, location.as_ref(), prelude)
            .is_ok_and(|built| built);
    build_collector(
        &mut site,
        &shape,
        function,
        location.as_ref(),
        prelude,
        postlude,
    );
    if collected || site.spread.is_some() {
        site.store(instruction, &[]);
    }
}

/// Build the keyword collector of a call through a function value whose
/// target the body does not statically hold, or that the check named no
/// callee for, from the call shape of the value's function type, as
/// [`build_keyword_collector`] builds a declared callee's.
fn collect_through_contract(
    instruction: &mut MirInstr,
    function: &mut MirFunction,
    prelude: &mut Vec<MirInstr>,
) {
    let MirInstr::CallIndirect {
        dest,
        callee,
        callee_place,
        args,
        kwargs,
        arg_places,
        kwarg_places,
        spread: None,
        ..
    } = &*instruction
    else {
        return;
    };
    let Some(TyKind::Func(contract)) = function
        .reg_types
        .get(&callee.0)
        .or_else(|| callee_place.as_ref()?.ty.as_ref())
        .map(Ty::kind)
    else {
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
        param_decls: &[],
    };
    let mut site = Site {
        dest: *dest,
        callee: String::new(),
        leading: 0,
        args: args.clone(),
        kwargs: kwargs.clone(),
        arg_places: arg_places.clone(),
        kwarg_places: kwarg_places.clone(),
        bound: Vec::new(),
        spread: None,
    };
    let location = function.spans.0.get(&dest.0).map(|(span, _)| span.clone());
    if build_keyword_collector(&mut site, &shape, function, location.as_ref(), prelude)
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
/// claims, in call order, each keyed by the keyword's literal and taking
/// its operand, and the slot forwarded to the call. Return whether it built
/// one: a call forwarding a `**kwargs^` passes that collector already, and
/// a callee without one takes none. An error says why the call's operands
/// do not bind or type the collector.
fn build_keyword_collector(
    site: &mut Site,
    shape: &CallShape<'_>,
    function: &mut MirFunction,
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
    let ty = keyword_collector_type(shape.param_decls, element, &collected, site, function)
        .ok_or_else(|| "its element names a binder the call binds no argument for".to_string())?;
    let place = MirPlace::root(
        u32::try_from(function.n_vars).expect("variable count fits a VarId"),
        Some(ty.clone()),
    );
    let collector = fresh_register(function, ty.clone(), location);
    prelude.push(MirInstr::Call {
        dest: collector,
        func: FuncRef::named(&mojito_symbol::symbol::keyword_collector_constructor_symbol()),
        raises: None,
        args: Vec::new(),
        kwargs: Vec::new(),
        arg_places: Vec::new(),
        kwarg_places: Vec::new(),
        capture_accesses: Vec::new(),
        param_arg_regs: Vec::new(),
        receiver: None,
        instantiated_args: match ty.kind() {
            TyKind::Struct(_, arguments) => arguments.clone(),
            _ => Vec::new(),
        },
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
    for &index in &collected {
        let (keyword, value) = site.kwargs[index].clone();
        let key = fresh_register(function, Ty::string_literal(), location);
        prelude.push(MirInstr::Const {
            dest: key,
            k: Const::Str(keyword),
        });
        let receiver = fresh_register(function, ty.clone(), location);
        prelude.push(MirInstr::LoadPlace {
            dest: receiver,
            place: place.clone(),
        });
        prelude.push(MirInstr::MethodCall {
            dest: fresh_register(function, Ty::none(), location),
            recv: receiver,
            method: "_insert".to_string(),
            resolved: Some(mojito_symbol::symbol::keyword_collector_insert_symbol()),
            raises: None,
            reference_result: None,
            result_adapter: None,
            args: vec![key, value],
            kwargs: Vec::new(),
            recv_place: Some(Box::new(place.clone())),
            recv_writes: true,
            receiver_convention: Some(mojito_ast::ast::ArgConvention::Mut),
            arg_places: vec![None, site.kwarg_places.get(index).cloned().flatten()],
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            param_decls: Vec::new(),
            instantiated_args: Vec::new(),
            spread: None,
        });
    }
    let passed = fresh_register(function, ty, location);
    prelude.push(MirInstr::UseVar {
        dest: passed,
        var: variable,
        mode: UseMode::Forward,
        constructor: None,
    });
    site.kwarg_places.resize(site.kwargs.len(), None);
    let mut index = 0;
    site.kwargs.retain(|_| {
        index += 1;
        !collected.contains(&(index - 1))
    });
    let mut index = 0;
    site.kwarg_places.retain(|_| {
        index += 1;
        !collected.contains(&(index - 1))
    });
    site.kwargs
        .push((FORWARDED_KWARGS_NAME.to_string(), passed));
    site.kwarg_places.push(None);
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
/// operands; a subscript carries no collector operand.
fn build_collector(
    site: &mut Site,
    shape: &CallShape<'_>,
    function: &mut MirFunction,
    location: Option<&SourceSpan>,
    prelude: &mut Vec<MirInstr>,
    postlude: &mut Vec<MirInstr>,
) {
    let Some(element) = shape.variadic else {
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
    let mut args = Vec::with_capacity(limit + 1);
    let mut arg_places = Vec::with_capacity(limit + 1);
    let mut moved = HashSet::new();
    for slot in &matched.slots[..limit] {
        match *slot {
            ArgSlot::Positional(index) => {
                args.push(site.args[index]);
                arg_places.push(site.arg_places.get(index).cloned().flatten());
            }
            ArgSlot::Keyword(index) => {
                args.push(site.kwargs[index].1);
                arg_places.push(site.kwarg_places.get(index).cloned().flatten());
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
    let Some(ty) = collector_type(shape.param_decls, element, &elements, site, function) else {
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
    } else {
        // A collector over the caller's own pack leaves its elements' types
        // to the elaborator.
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
    arg_places.push(None);
    site.args = args;
    site.arg_places = arg_places;
    let mut index = 0;
    site.kwargs.retain(|_| {
        index += 1;
        !moved.contains(&(index - 1))
    });
    let mut index = 0;
    site.kwarg_places.retain(|_| {
        index += 1;
        !moved.contains(&(index - 1))
    });
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
    let pointer = fresh_register(function, element_pointer_type(element, false), location);
    prelude[at] = MirInstr::MakeRef {
        dest: pointer,
        place,
    };
    Some(ListElement::Borrowed { pointer, roots })
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
fn omits_any(callee: &MirFunctionDeclaration, positional: usize, keywords: &[&str]) -> bool {
    match_call_slots(
        &callee.param_names,
        &callee.required,
        callee.positional_only,
        callee.keyword_only,
        positional,
        keywords,
        CallVariadics {
            positional: callee.variadic.is_some(),
            keyword: callee.kw_variadic.is_some(),
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
        base_place,
        index_place,
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
        object_place: base_place,
        arg_places: vec![index_place.map(|place| *place)],
        kwargs: Vec::new(),
        kwarg_places: Vec::new(),
        call,
    }
}

impl Site {
    fn of(
        instruction: &MirInstr,
        record: &LoweredCall,
        function: &MirFunction,
        caller: &Caller<'_>,
    ) -> Option<Self> {
        let declarations = caller.declarations;
        let mut leading = record.leading;
        let (callee, receiver, instantiated, args, kwargs, arg_places, kwarg_places) =
            match instruction {
                MirInstr::Call {
                    func,
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                    receiver,
                    instantiated_args,
                    spread: None,
                    ..
                } => (
                    record.callee.clone().unwrap_or_else(|| func.0.clone()),
                    receiver.clone(),
                    instantiated_args,
                    args.clone(),
                    kwargs.clone(),
                    arg_places.clone(),
                    kwarg_places.clone(),
                ),
                MirInstr::MethodCall {
                    recv,
                    recv_place,
                    resolved,
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                    instantiated_args,
                    spread: None,
                    ..
                } => {
                    let named = record.callee.clone().or_else(|| resolved.clone())?;
                    // A call through a trait bound names the requirement's
                    // dispatch symbol; its defaults are the requirement's,
                    // which the trait member's body declares, `self` first.
                    let callee = if declarations.function(&named).is_some() {
                        named
                    } else {
                        let member = requirement_member(&named, declarations)?;
                        leading = 1;
                        member
                    };
                    (
                        callee,
                        Some(
                            function
                                .reg_types
                                .get(&recv.0)
                                .or_else(|| recv_place.as_ref()?.ty.as_ref())?
                                .clone(),
                        ),
                        instantiated_args,
                        args.clone(),
                        kwargs.clone(),
                        arg_places.clone(),
                        kwarg_places.clone(),
                    )
                }
                MirInstr::CallIndirect {
                    callee,
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                    instantiated_args,
                    spread: None,
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
                    (
                        target,
                        None,
                        instantiated_args,
                        args.clone(),
                        kwargs.clone(),
                        arg_places.clone(),
                        kwarg_places.clone(),
                    )
                }
                MirInstr::MultiIndex {
                    dest,
                    object,
                    object_place,
                    args,
                    arg_places,
                    kwargs,
                    kwarg_places,
                    call: Some(call),
                } => {
                    // Only `Index` operands bind the callee's slots; a slice
                    // descriptor's call keeps its slots.
                    let register = |argument: &MirSubscriptArg| match argument {
                        MirSubscriptArg::Index(register) => Some(*register),
                        MirSubscriptArg::Slice { .. } => None,
                    };
                    let receiver = function
                        .reg_types
                        .get(&object.0)
                        .or_else(|| object_place.as_ref()?.ty.as_ref())?
                        .clone();
                    let callee = record.callee.clone().unwrap_or_else(|| call.target.clone());
                    let declaration = declarations.function(&callee)?;
                    let bound = bindings_of(&callee, declaration, &[], Some(&receiver), caller);
                    return Some(Self {
                        dest: *dest,
                        callee,
                        leading: 0,
                        args: args.iter().map(register).collect::<Option<Vec<_>>>()?,
                        kwargs: kwargs
                            .iter()
                            .map(|(name, argument)| Some((name.clone(), register(argument)?)))
                            .collect::<Option<Vec<_>>>()?,
                        arg_places: arg_places.clone(),
                        kwarg_places: kwarg_places.clone(),
                        bound,
                        spread: None,
                    });
                }
                _ => return None,
            };
        let declaration = declarations.function(&callee)?;
        let bound = bindings_of(
            &callee,
            declaration,
            instantiated,
            receiver.as_ref(),
            caller,
        );
        Some(Self {
            dest: result_register(instruction)?,
            callee,
            leading,
            args,
            kwargs,
            arg_places,
            kwarg_places,
            bound,
            spread: None,
        })
    }

    /// Store the filled operand lists back into `instruction`; a subscript
    /// contract's argument at each filled `slot` names the operand `source`
    /// it now reads.
    fn store(self, instruction: &mut MirInstr, sources: &[(usize, CheckedCallArgumentSource)]) {
        match instruction {
            MirInstr::Call {
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
            }
            | MirInstr::CallIndirect {
                args,
                kwargs,
                arg_places,
                kwarg_places,
                spread,
                ..
            } => {
                *args = self.args;
                *kwargs = self.kwargs;
                *arg_places = self.arg_places;
                *kwarg_places = self.kwarg_places;
                *spread = self.spread;
            }
            MirInstr::MultiIndex {
                args,
                arg_places,
                kwargs,
                kwarg_places,
                call,
                ..
            } => {
                *args = self.args.into_iter().map(MirSubscriptArg::Index).collect();
                *kwargs = self
                    .kwargs
                    .into_iter()
                    .map(|(name, register)| (name, MirSubscriptArg::Index(register)))
                    .collect();
                *arg_places = self.arg_places;
                *kwarg_places = self.kwarg_places;
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

/// The lowered body of the trait member whose requirement dispatches as
/// `dispatch` (`__trait_dispatch.f$…`), when the member has one: the
/// function `Trait.member`, whose first parameter is `self`.
fn requirement_member(dispatch: &str, declarations: &MirDeclarations) -> Option<String> {
    declarations.trait_declarations().find_map(|declaration| {
        declaration
            .requirements
            .iter()
            .find(|requirement| requirement.dispatch == dispatch)
            .map(|requirement| format!("{}.{}", declaration.name, requirement.name))
            .filter(|member| {
                declarations.function(member).is_some_and(|declaration| {
                    !declaration.has_receiver
                        && declaration.param_names.first().map(String::as_str) == Some("self")
                })
            })
    })
}

/// The arguments the call binds `callee`'s binders to, then those of the
/// scopes around it: a method's struct's from `receiver`, the receiver
/// instance's type (a bare struct's binders standing for themselves), and
/// a nested body's enclosing declarations', standing for themselves inside
/// the function declaring them.
fn bindings_of(
    callee: &str,
    declaration: &MirFunctionDeclaration,
    instantiated: &[TyArg],
    receiver: Option<&Ty>,
    caller: &Caller<'_>,
) -> Vec<(ParamDecl, TyArg)> {
    let declarations = caller.declarations;
    let own = declaration.param_decls.iter().cloned();
    let mut bound: Vec<(ParamDecl, TyArg)> = own.zip(instantiated.iter().cloned()).collect();
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
    if let Some(TyKind::Struct(name, arguments)) = receiver.map(Ty::kind)
        && let Some(owner) = declarations.structure(name)
    {
        if arguments.is_empty() {
            bound.extend(
                owner
                    .param_decls
                    .iter()
                    .map(|decl| (decl.clone(), decl.own_argument())),
            );
        } else {
            bound.extend(
                owner
                    .param_decls
                    .iter()
                    .cloned()
                    .zip(arguments.iter().cloned()),
            );
        }
    }
    bound
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
            binders.extend(declaration.param_decls.iter().cloned());
        }
        if let Some(owner) = mojito_symbol::symbol::split_method_symbol(name)
            .and_then(|(owner, _)| declarations.structure(owner))
        {
            binders.extend(owner.param_decls.iter().cloned());
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
    callee: &MirFunctionDeclaration,
    declarations: &MirDeclarations,
) -> Result<Vec<Omitted>, String> {
    let skip = site.leading;
    let keywords: Vec<&str> = site.kwargs.iter().map(|(name, _)| name.as_str()).collect();
    let matched = match_call_slots(
        callee.param_names.get(skip..).unwrap_or_default(),
        callee.required.get(skip..).unwrap_or_default(),
        callee
            .positional_only
            .and_then(|limit| limit.checked_sub(skip)),
        callee
            .keyword_only
            .and_then(|limit| limit.checked_sub(skip)),
        site.args.len(),
        &keywords,
        CallVariadics {
            positional: callee.variadic.is_some(),
            keyword: callee.kw_variadic.is_some(),
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
    callee: &MirFunctionDeclaration,
    slot: usize,
    declarations: &MirDeclarations,
) -> Result<Omitted, String> {
    let name = callee
        .param_names
        .get(slot)
        .cloned()
        .ok_or_else(|| format!("slot {slot} names no parameter"))?;
    let Some(Some(default)) = callee.defaults.get(slot) else {
        return Err(format!("parameter '{name}' declares no lowered default"));
    };
    let convention = callee.param_conventions.get(slot).copied().flatten();
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
            let arguments = if arguments.is_empty() {
                thunk
                    .param_decls
                    .iter()
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
            CheckedConst::Evaluate {
                function: function.clone(),
                arguments,
            }
        }
        default => default.clone(),
    };
    let ty = callee
        .param_types
        .get(slot)
        .ok_or_else(|| format!("parameter '{name}' has no type"))
        .and_then(|ty| {
            close_parameter_type(ty, &site.bound).ok_or_else(|| {
                format!("the type of '{name}' names a binder the call binds no argument for")
            })
        })?;
    Ok(Omitted {
        slot,
        name,
        ty,
        owned: matches!(convention, Some(ArgConvention::Var | ArgConvention::Deinit)),
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
/// run on its literal, or the lifted default function called under the
/// arguments the call binds its binders to.
fn evaluate_default(
    function: &mut MirFunction,
    default: &Omitted,
    location: Option<&SourceSpan>,
    prelude: &mut Vec<MirInstr>,
) -> Reg {
    evaluate_constant(function, &default.default, &default.ty, location, prelude)
}

fn evaluate_constant(
    function: &mut MirFunction,
    default: &CheckedConst,
    ty: &Ty,
    location: Option<&SourceSpan>,
    prelude: &mut Vec<MirInstr>,
) -> Reg {
    match default {
        CheckedConst::Evaluate {
            function: thunk,
            arguments,
        } => {
            let value = fresh_register(function, ty.clone(), location);
            prelude.push(MirInstr::Call {
                dest: value,
                func: FuncRef::named(thunk),
                raises: None,
                args: Vec::new(),
                kwargs: Vec::new(),
                arg_places: Vec::new(),
                kwarg_places: Vec::new(),
                capture_accesses: Vec::new(),
                param_arg_regs: Vec::new(),
                receiver: None,
                instantiated_args: arguments.clone(),
                spread: None,
            });
            value
        }
        CheckedConst::Construct { target, arg } => {
            let literal_ty = literal_type(arg);
            let argument = evaluate_constant(function, arg, &literal_ty, location, prelude);
            let value = fresh_register(function, ty.clone(), location);
            // The constructor is owner-less: the call binds the struct's
            // parameters to the arguments of the type it builds, as a
            // written conversion's call does.
            let instantiated_args = match ty.kind() {
                TyKind::Struct(_, arguments) => arguments.clone(),
                _ => Vec::new(),
            };
            prelude.push(MirInstr::Call {
                dest: value,
                func: FuncRef::named(target),
                raises: None,
                args: vec![argument],
                kwargs: Vec::new(),
                arg_places: vec![None],
                kwarg_places: Vec::new(),
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

/// The type a literal default's constant has before conversion.
fn literal_type(literal: &CheckedConst) -> Ty {
    match literal {
        CheckedConst::Int(_) => Ty::int_literal(),
        CheckedConst::Float(_) => Ty::float_literal(),
        CheckedConst::Bool(_) => Ty::bool(),
        CheckedConst::String(_) => Ty::string_literal(),
        CheckedConst::None => Ty::none(),
        CheckedConst::Dtype(_) => Ty::dtype(),
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
        CheckedConst::Dtype(value) => Const::Dtype(*value),
        CheckedConst::None | CheckedConst::Construct { .. } | CheckedConst::Evaluate { .. } => {
            Const::None
        }
    }
}

/// `value`, a literal of type `found`, materialized at the parameter type
/// `target` as a written literal argument is: a numeric scalar through
/// `MaterializeLiteral`, a multi-lane vector as the one-element splat.
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
    let dest = fresh_register(function, target.clone(), location);
    if let Some((dtype, width)) = mojito_types::types::simd_slots(target)
        && width.known().is_none_or(|width| width > 1)
    {
        prelude.push(MirInstr::MakeSimd {
            dest,
            dtype,
            width,
            elems: vec![value],
        });
    } else {
        prelude.push(MirInstr::MaterializeLiteral {
            dest,
            value,
            target: target.clone(),
        });
    }
    dest
}

/// Bind `value`, the default's result, to its call-result slot, in place
/// as the default's call writes it: a borrowed parameter reads the slot,
/// which the caller destroys once the call returns, and an owned one takes
/// it.
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
        in_place: true,
    });
    let read = fresh_register(function, default.ty.clone(), location);
    prelude.push(if default.owned {
        MirInstr::UseVar {
            dest: read,
            var: variable,
            mode: UseMode::Forward,
            constructor: None,
        }
    } else {
        MirInstr::LoadPlace {
            dest: read,
            place: MirPlace::root(variable, Some(default.ty.clone())),
        }
    });
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
    /// A call of a lifted default (`$default$f$x`).
    Default,
    /// A whole load of a read temporary.
    Temporary(VarId),
}

/// Record in `producers` each register `blocks` and their regions define by
/// a [`Producer`], and in `bindings` each register a slot is bound to.
fn collect_producers(
    blocks: &[MirBlock],
    function: &MirFunction,
    producers: &mut HashMap<u32, Producer>,
    bindings: &mut HashMap<VarId, Vec<u32>>,
) {
    for instruction in blocks.iter().flat_map(|block| &block.instrs) {
        match instruction {
            MirInstr::Call { dest, func, .. }
                if matches!(lifted_owner(&func.0), Some(LiftedOwner::Default { .. })) =>
            {
                producers.insert(dest.0, Producer::Default);
            }
            MirInstr::LoadPlace { dest, place }
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
        slice.grow(&mut function.blocks)?;
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
    fn grow(&mut self, blocks: &mut [MirBlock]) -> Option<()> {
        for block in blocks {
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
        }
        Some(())
    }
}

/// Remove from `blocks` and their regions each instruction serving the
/// slice `slots` and `regs` and each defining only `calls`, recording their
/// results in `erased`, and every cleanup entry of a slot in `slots`.
fn erase_slice(
    blocks: &mut [MirBlock],
    slots: &BTreeSet<VarId>,
    regs: &HashSet<u32>,
    calls: &HashSet<u32>,
    erased: &mut HashSet<u32>,
) {
    for block in blocks {
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
}

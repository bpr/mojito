//! Omitted evaluated defaults, filled in at the call.
//!
//! Upstream evaluates a default its call leaves out at the call site, under
//! the call's own parameter arguments, as an argument the caller owns: a
//! borrowed default is the caller's temporary, destroyed once the call
//! returns, and an owned one is handed to the callee. A direct call of a
//! free function here runs the lifted default (`$default$f$x`) the same way,
//! before ownership analysis, so the temporary is destroyed like any other
//! argument's and each default instance is named by the call that needs it.

use crate::mir::{
    FORWARDED_ARGUMENT_SLOT, FuncRef, MirBlock, MirDeclarations, MirFunction,
    MirFunctionDeclaration, MirInstr, MirPlace, READ_TEMPORARY_SLOT, Reg, UseMode,
    calls::owns_droppable_storage, instruction_regions_mut, unused_arguments,
};
use mojito_ast::ast::ArgConvention;
use mojito_ast::call::{ArgSlot, CallVariadics, match_call_slots};
use mojito_checked::checked::CheckedConst;
use mojito_common::token::SourceSpan;
use mojito_hir::hir::VarId;
use mojito_types::param_expr::{ParamBindings, ParamContext};
use mojito_types::types::{ParamDecl, Ty, TyArg, names_binder, replace_parameters};
use std::collections::HashMap;

/// Evaluate at each direct call in `functions` the evaluated defaults it
/// leaves out of a free function among them.
///
/// The call gets a call of the default under its own `instantiated_args`,
/// passed in the omitted slot: positionally when every earlier slot is,
/// else by name. A borrowed default of droppable type is bound to a
/// read-temporary slot the call reads, an owned one to a forwarding slot
/// the call takes.
///
/// A default the call cannot spell is left to the callee: one of a method,
/// a nested function, or a function outside `functions`; a raising
/// default; one reading a binder the call binds no argument for, or whose
/// type the call's arguments do not close. So is the default of an
/// argument the callee never uses ([`unused_arguments`]), which upstream's
/// `RemoveUnusedParams` removes with its evaluation.
pub fn evaluate_omitted_defaults(
    functions: &mut [(String, MirFunction)],
    declarations: &MirDeclarations,
) {
    let unused: HashMap<String, Vec<VarId>> = functions
        .iter()
        .filter(|(name, _)| {
            declarations.function(name).is_some_and(|declaration| {
                declaration
                    .defaults
                    .iter()
                    .any(|default| matches!(default, Some(CheckedConst::Evaluate { .. })))
            })
        })
        .map(|(name, function)| (name.clone(), unused_arguments(name, function, declarations)))
        .collect();
    if unused.is_empty() {
        return;
    }
    let callees = Callees {
        declarations,
        unused: &unused,
    };
    for (_, function) in functions {
        let mut blocks = std::mem::take(&mut function.blocks);
        fill_blocks(&mut blocks, function, &callees);
        function.blocks = blocks;
    }
}

/// Whether every direct call omitting `slot` of `name`, so declared,
/// evaluates its default itself ([`evaluate_omitted_defaults`]).
///
/// It is an evaluated default of a free function the program itself
/// declares, by value, that does not raise, reads only `name`'s binders,
/// and fills no positional-only slot, which an earlier unfilled one could
/// leave to the callee. Such a default is no read of `name`, as upstream's
/// callee names none.
pub fn evaluated_at_call_site(
    name: &str,
    declaration: &MirFunctionDeclaration,
    slot: usize,
    declarations: &MirDeclarations,
) -> bool {
    declarations
        .functions
        .iter()
        .any(|own| own.lowered_name == name)
        && declaration
            .positional_only
            .is_none_or(|limit| slot >= limit)
        && call_site_default(name, declaration, slot, declarations).is_some()
}

/// The free functions whose omitted evaluated defaults a call evaluates,
/// each with the argument slots its body never uses.
struct Callees<'a> {
    declarations: &'a MirDeclarations,
    unused: &'a HashMap<String, Vec<VarId>>,
}

/// One omitted default the call evaluates: the parameter it fills and the
/// default's call.
struct Omitted {
    slot: usize,
    name: String,
    ty: Ty,
    owned: bool,
    function: String,
    arguments: Vec<TyArg>,
}

fn fill_blocks(blocks: &mut [MirBlock], function: &mut MirFunction, callees: &Callees<'_>) {
    for block in blocks {
        let instructions = std::mem::take(&mut block.instrs);
        let mut filled = Vec::with_capacity(instructions.len());
        for mut instruction in instructions {
            for region in instruction_regions_mut(&mut instruction) {
                fill_blocks(region, function, callees);
            }
            fill_call(&mut instruction, function, callees, &mut filled);
            filled.push(instruction);
        }
        block.instrs = filled;
    }
}

/// Fill `instruction`'s omitted evaluated defaults, emitting each default's
/// evaluation onto `prelude`.
fn fill_call(
    instruction: &mut MirInstr,
    function: &mut MirFunction,
    callees: &Callees<'_>,
    prelude: &mut Vec<MirInstr>,
) {
    let MirInstr::Call {
        dest,
        func,
        args,
        kwargs,
        arg_places,
        kwarg_places,
        receiver: None,
        instantiated_args,
        spread: None,
        ..
    } = instruction
    else {
        return;
    };
    let (Some(unused), Some(callee)) = (
        callees.unused.get(&func.0),
        callees.declarations.function(&func.0),
    ) else {
        return;
    };
    let keywords: Vec<&str> = kwargs.iter().map(|(name, _)| name.as_str()).collect();
    let Ok(matched) = match_call_slots(
        &callee.param_names,
        &callee.required,
        callee.positional_only,
        callee.keyword_only,
        args.len(),
        &keywords,
        CallVariadics {
            positional: callee.variadic.is_some(),
            keyword: callee.kw_variadic.is_some(),
        },
    ) else {
        return;
    };
    let omitted: Vec<Omitted> = matched
        .slots
        .iter()
        .enumerate()
        .filter(|(slot, kind)| {
            matches!(kind, ArgSlot::Default)
                && VarId::try_from(*slot).is_ok_and(|slot| !unused.contains(&slot))
        })
        .filter_map(|(slot, _)| {
            omitted_default(
                &func.0,
                callee,
                slot,
                instantiated_args,
                callees.declarations,
            )
        })
        .collect();
    let location = function.spans.0.get(&dest.0).map(|(span, _)| span.clone());
    let positional_limit = callee
        .keyword_only
        .unwrap_or(callee.param_names.len())
        .min(callee.param_names.len());
    arg_places.resize(args.len(), None);
    kwarg_places.resize(kwargs.len(), None);
    for default in omitted {
        let positional = default.slot == args.len()
            && default.slot < positional_limit
            && callee
                .variadic_index
                .is_none_or(|collector| default.slot < collector);
        if !positional
            && callee
                .positional_only
                .is_some_and(|limit| default.slot < limit)
        {
            continue;
        }
        let value = fresh_reg(function, location.clone(), default.ty.clone());
        prelude.push(MirInstr::Call {
            dest: value,
            func: FuncRef::named(&default.function),
            raises: None,
            args: Vec::new(),
            kwargs: Vec::new(),
            arg_places: Vec::new(),
            kwarg_places: Vec::new(),
            capture_accesses: Vec::new(),
            param_arg_regs: Vec::new(),
            receiver: None,
            instantiated_args: default.arguments.clone(),
            spread: None,
        });
        let argument = if owns_droppable_storage(&default.ty) {
            bind_temporary(function, value, &default, location.clone(), prelude)
        } else {
            value
        };
        if positional {
            args.push(argument);
            arg_places.push(None);
        } else {
            kwargs.push((default.name, argument));
            kwarg_places.push(None);
        }
    }
}

/// The default `callee`, the free function `name`, declares at `slot`,
/// when the call can evaluate it: one [`call_site_default`] admits whose
/// binders the call's `instantiated` arguments bind and whose parameter
/// type they close.
fn omitted_default(
    name: &str,
    callee: &MirFunctionDeclaration,
    slot: usize,
    instantiated: &[TyArg],
    declarations: &MirDeclarations,
) -> Option<Omitted> {
    let (function, thunk) = call_site_default(name, callee, slot, declarations)?;
    let convention = callee.param_conventions.get(slot).copied().flatten();
    let bound = |decl: &ParamDecl| {
        callee
            .param_decls
            .iter()
            .position(|own| own.id() == decl.id())
            .and_then(|position| instantiated.get(position))
            .cloned()
    };
    let arguments = thunk
        .param_decls
        .iter()
        .map(bound)
        .collect::<Option<Vec<_>>>()?;
    let ty = close_parameter_type(callee, callee.param_types.get(slot)?, instantiated)?;
    Some(Omitted {
        slot,
        name: callee.param_names.get(slot)?.clone(),
        ty,
        owned: matches!(convention, Some(ArgConvention::Var | ArgConvention::Deinit)),
        function: function.clone(),
        arguments,
    })
}

/// The lifted function and declaration of the evaluated default `callee`,
/// the function `name`, declares at `slot`, when a call can evaluate it:
/// `callee` is a free function, the default takes no runtime argument, its
/// parameter is passed by value, and the default does not raise and binds
/// only `callee`'s binders.
fn call_site_default<'d>(
    name: &str,
    callee: &'d MirFunctionDeclaration,
    slot: usize,
    declarations: &'d MirDeclarations,
) -> Option<(&'d String, &'d MirFunctionDeclaration)> {
    if callee.has_receiver
        || callee.enclosing.is_some()
        || mojito_symbol::symbol::split_method_symbol(name).is_some()
    {
        return None;
    }
    let Some(CheckedConst::Evaluate {
        function,
        arguments,
    }) = callee.defaults.get(slot)?
    else {
        return None;
    };
    let convention = callee.param_conventions.get(slot).copied().flatten();
    if !arguments.is_empty()
        || ArgConvention::is_reference(convention)
        || matches!(convention, Some(ArgConvention::Out))
    {
        return None;
    }
    let thunk = declarations.function(function)?;
    (!thunk.raises
        && thunk
            .param_decls
            .iter()
            .all(|decl| callee.param_decls.iter().any(|own| own.id() == decl.id())))
    .then_some((function, thunk))
}

/// `ty`, a parameter type of `callee`, under the call's `instantiated`
/// arguments; `None` when it still names one of `callee`'s binders.
fn close_parameter_type(
    callee: &MirFunctionDeclaration,
    ty: &Ty,
    instantiated: &[TyArg],
) -> Option<Ty> {
    let context = ParamContext::detached();
    let mut bindings = ParamBindings::new();
    for (decl, argument) in callee.param_decls.iter().zip(instantiated) {
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
    let closed = replace_parameters(&context, ty, &bindings, 0).ok()?;
    (!callee
        .param_decls
        .iter()
        .any(|decl| names_binder(&closed, &decl.binder())))
    .then_some(closed)
}

/// Bind `value`, the default's result, to the slot the call reads it from:
/// a read temporary for a borrowed parameter, which the caller destroys once
/// the call returns, or a forwarding slot for an owned one, which the call
/// takes.
fn bind_temporary(
    function: &mut MirFunction,
    value: Reg,
    default: &Omitted,
    location: Option<SourceSpan>,
    prelude: &mut Vec<MirInstr>,
) -> Reg {
    let prefix = if default.owned {
        FORWARDED_ARGUMENT_SLOT
    } else {
        READ_TEMPORARY_SLOT
    };
    let variable = u32::try_from(function.n_vars).expect("variable count fits a VarId");
    function.n_vars += 1;
    function.var_names.push(format!("{prefix}{}", value.0));
    function.var_tys.insert(variable, default.ty.clone());
    prelude.push(MirInstr::DefVar {
        var: variable,
        src: value,
        binding_ty: Some(default.ty.clone()),
        in_place: default.owned,
    });
    let read = fresh_reg(function, location, default.ty.clone());
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

fn fresh_reg(function: &mut MirFunction, location: Option<SourceSpan>, ty: Ty) -> Reg {
    let reg = Reg(function.n_regs);
    function.n_regs += 1;
    function.reg_types.insert(reg.0, ty);
    if let Some(location) = location {
        function.spans.0.insert(reg.0, (location, None));
    }
    reg
}

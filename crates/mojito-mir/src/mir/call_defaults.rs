//! Omitted evaluated defaults, filled in at the call.
//!
//! Upstream evaluates a default its call leaves out at the call site, under
//! the call's own parameter arguments, as an argument the caller owns: a
//! borrowed default is the caller's temporary, destroyed once the call
//! returns, and an owned one is handed to the callee. A direct call, a
//! method call, a static-method call, and a construction here run the lifted
//! default (`$default$f$x`) the same way, before ownership analysis, so the
//! temporary is destroyed like any other argument's and each default
//! instance is named by the call that needs it.

use crate::mir::{
    FORWARDED_ARGUMENT_SLOT, FuncRef, MirBlock, MirDeclarations, MirFunction,
    MirFunctionDeclaration, MirInstr, MirPlace, READ_TEMPORARY_SLOT, Reg, SlotParameter, UseMode,
    calls::owns_droppable_storage, instruction_regions_mut, unused_arguments,
};
use mojito_ast::ast::ArgConvention;
use mojito_ast::call::{ArgSlot, CallVariadics, match_call_slots};
use mojito_checked::checked::CheckedConst;
use mojito_common::token::SourceSpan;
use mojito_types::param_expr::{ParamBindings, ParamContext};
use mojito_types::types::{ParamDecl, Ty, TyArg, TyKind, names_binder, replace_parameters};
use std::collections::HashMap;

/// Evaluate at each call in `functions` the evaluated defaults it leaves out
/// of a function among them.
///
/// The call gets a call of the default under the arguments it binds the
/// callee's binders and, for a method, its struct's to ([`CallSite`]),
/// passed in the omitted slot: positionally when every earlier slot is,
/// else by name. A borrowed default of droppable type is bound to a
/// read-temporary slot the call reads, an owned one to a forwarding slot
/// the call takes.
///
/// A default the call cannot spell is left to the callee: one of a
/// subscript, a nested function, or a function outside `functions`; a raising
/// default; one reading a binder the call binds no argument for, or whose
/// type the call's arguments do not close. So is the default of an
/// argument the callee never uses ([`unused_arguments`]), which upstream's
/// `RemoveUnusedParams` removes with its evaluation.
pub fn evaluate_omitted_defaults(
    functions: &mut [(String, MirFunction)],
    declarations: &MirDeclarations,
) {
    let unused: HashMap<String, Vec<SlotParameter>> = functions
        .iter()
        .filter_map(|(name, function)| {
            let declaration = declarations.function(name)?;
            declaration
                .defaults
                .iter()
                .any(|default| matches!(default, Some(CheckedConst::Evaluate { .. })))
                .then(|| {
                    let unused = unused_arguments(name, function, declarations)
                        .into_iter()
                        .map(|slot| declaration.slot_parameter(slot as usize))
                        .collect();
                    (name.clone(), unused)
                })
        })
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
/// callee names none. A method's is still a read: a subscript leaves it to
/// the callee.
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
        && !declaration.has_receiver
        && mojito_symbol::symbol::split_method_symbol(name).is_none()
        && declaration
            .positional_only
            .is_none_or(|limit| slot >= limit)
        && call_site_default(name, declaration, slot, declarations).is_some()
}

/// The functions whose omitted evaluated defaults a call evaluates, each
/// with the parameters its body never uses.
struct Callees<'a> {
    declarations: &'a MirDeclarations,
    unused: &'a HashMap<String, Vec<SlotParameter>>,
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
    let Some(site) = CallSite::of(instruction, function, callees.declarations) else {
        return;
    };
    let (Some(unused), Some(callee)) = (
        callees.unused.get(site.callee),
        callees.declarations.function(site.callee),
    ) else {
        return;
    };
    let keywords: Vec<&str> = site.kwargs.iter().map(|(name, _)| name.as_str()).collect();
    let Ok(matched) = match_call_slots(
        &callee.param_names,
        &callee.required,
        callee.positional_only,
        callee.keyword_only,
        site.args.len(),
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
            matches!(kind, ArgSlot::Default) && !unused.contains(&SlotParameter::Position(*slot))
        })
        .filter_map(|(slot, _)| {
            omitted_default(site.callee, callee, slot, &site.bound, callees.declarations)
        })
        .collect();
    let location = function
        .spans
        .0
        .get(&site.dest.0)
        .map(|(span, _)| span.clone());
    let positional_limit = callee
        .keyword_only
        .unwrap_or(callee.param_names.len())
        .min(callee.param_names.len());
    site.arg_places.resize(site.args.len(), None);
    site.kwarg_places.resize(site.kwargs.len(), None);
    for default in omitted {
        let positional = default.slot == site.args.len()
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
            site.args.push(argument);
            site.arg_places.push(None);
        } else {
            site.kwargs.push((default.name, argument));
            site.kwarg_places.push(None);
        }
    }
}

/// A call whose omitted defaults the caller may fill: the callee it names,
/// its argument lists, and the arguments it binds each of the callee's
/// binders to.
///
/// A direct call binds the callee's binders to its `instantiated_args`; a
/// static method's call and a method call bind the struct's to the
/// arguments of its receiver's type too, as a default may read both. A
/// receiver typed by the bare struct is the struct's own `Self`, whose
/// binders stand for themselves.
struct CallSite<'i> {
    dest: Reg,
    callee: &'i str,
    args: &'i mut Vec<Reg>,
    kwargs: &'i mut Vec<(String, Reg)>,
    arg_places: &'i mut Vec<Option<MirPlace>>,
    kwarg_places: &'i mut Vec<Option<MirPlace>>,
    bound: Vec<(ParamDecl, TyArg)>,
}

impl<'i> CallSite<'i> {
    fn of(
        instruction: &'i mut MirInstr,
        function: &MirFunction,
        declarations: &MirDeclarations,
    ) -> Option<Self> {
        let (dest, callee, receiver, instantiated, args, kwargs, arg_places, kwarg_places) =
            match instruction {
                MirInstr::Call {
                    dest,
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
                    *dest,
                    func.0.as_str(),
                    receiver.clone(),
                    &*instantiated_args,
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                ),
                MirInstr::MethodCall {
                    dest,
                    recv,
                    recv_place,
                    resolved: Some(resolved),
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                    instantiated_args,
                    spread: None,
                    ..
                } => (
                    *dest,
                    resolved.as_str(),
                    Some(
                        function
                            .reg_types
                            .get(&recv.0)
                            .or_else(|| recv_place.as_ref()?.ty.as_ref())?
                            .clone(),
                    ),
                    &*instantiated_args,
                    args,
                    kwargs,
                    arg_places,
                    kwarg_places,
                ),
                _ => return None,
            };
        let own = declarations.function(callee)?.param_decls.iter().cloned();
        let mut bound: Vec<(ParamDecl, TyArg)> = own.zip(instantiated.iter().cloned()).collect();
        if let Some(receiver) = receiver {
            let TyKind::Struct(name, arguments) = receiver.kind() else {
                return None;
            };
            let owner = declarations.structure(name)?;
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
        Some(Self {
            dest,
            callee,
            args,
            kwargs,
            arg_places,
            kwarg_places,
            bound,
        })
    }
}

/// The default `callee`, the function `name`, declares at `slot`, when the
/// call can evaluate it: one [`call_site_default`] admits whose binders the
/// call binds, in `bound`, and whose parameter type they close.
fn omitted_default(
    name: &str,
    callee: &MirFunctionDeclaration,
    slot: usize,
    bound: &[(ParamDecl, TyArg)],
    declarations: &MirDeclarations,
) -> Option<Omitted> {
    let (function, thunk) = call_site_default(name, callee, slot, declarations)?;
    let convention = callee.param_conventions.get(slot).copied().flatten();
    let argument = |decl: &ParamDecl| {
        bound
            .iter()
            .find(|(own, _)| own.id() == decl.id())
            .map(|(_, argument)| argument.clone())
    };
    let arguments = thunk
        .param_decls
        .iter()
        .map(argument)
        .collect::<Option<Vec<_>>>()?;
    let ty = close_parameter_type(callee.param_types.get(slot)?, bound)?;
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
/// `callee` is not nested, the default takes no runtime argument, its
/// parameter is passed by value, and the default does not raise and binds
/// only `callee`'s binders and its struct's.
fn call_site_default<'d>(
    name: &str,
    callee: &'d MirFunctionDeclaration,
    slot: usize,
    declarations: &'d MirDeclarations,
) -> Option<(&'d String, &'d MirFunctionDeclaration)> {
    if callee.enclosing.is_some() {
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
    let owner = mojito_symbol::symbol::split_method_symbol(name)
        .and_then(|(owner, _)| declarations.structure(owner))
        .map_or(&[][..], |owner| &owner.param_decls);
    let thunk = declarations.function(function)?;
    (!thunk.raises
        && thunk.param_decls.iter().all(|decl| {
            callee
                .param_decls
                .iter()
                .chain(owner)
                .any(|own| own.id() == decl.id())
        }))
    .then_some((function, thunk))
}

/// `ty`, a parameter type of the callee, under the call's `bound`
/// arguments; `None` when it still names a binder they bind to anything but
/// itself.
fn close_parameter_type(ty: &Ty, bound: &[(ParamDecl, TyArg)]) -> Option<Ty> {
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
    let closed = replace_parameters(&context, ty, &bindings, 0).ok()?;
    (!bound.iter().any(|(decl, argument)| {
        *argument != decl.own_argument() && names_binder(&closed, &decl.binder())
    }))
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

//! Copies and moves lowered to calls of their constructor.
//!
//! Upstream's copy/move insertion leaves, at each point a value is copied
//! or moved, a plain call of its type's `__copyinit__` or `__moveinit__`
//! initializing fresh storage, which the elaborator and every later pass
//! see as any call. A `var.use copy`, a `var.use move`, and a `value.copy`
//! name that constructor but stay a use, which each backend runs by a copy
//! or move path of its own. Where a use names its constructor, and the
//! constructor takes the value as its one parameter after `out self`, the
//! use is the call: a copy reads the slot (`place.load`) and passes it as
//! the constructor's borrowed argument, the shape `S(copy=x)` lowers to; a
//! move forwards the slot, which ends it and hands its storage to the
//! callee as Mojo's owned argument convention does, and passes it as the
//! constructor's `deinit` argument; a register copy passes its register.
//! The call passes the value's struct arguments as the constructor's
//! compile-time arguments, which its declaration restates, as a
//! construction does.
//! A use naming none, a symbolic type's until the elaborator names its
//! instance's, stays a use, which the backends copy or move by type. So
//! does a use of a compile-time value parameter's slot, which holds no
//! value: the elaborator materializes the parameter at each read. And so
//! does the read of a variadic collector a call spreads (`show(*args)`),
//! which the elaborator expands into the pack's element places.

use super::{
    FuncRef, MirBlock, MirFunction, MirFunctionDeclaration, MirInstr, MirPlace, Reg,
    StructDeclarations, UseMode, VarId, binder_scope, fresh_register, instruction_regions_mut,
    materialized_parameter_slots,
};
use mojito_ast::ast::ArgConvention;
use mojito_types::types::{ParamDecl, Ty, TyArg, TyKind};
use std::collections::HashSet;

/// A function's declaration, by the function's name.
pub type FunctionDeclarations<'d> = dyn Fn(&str) -> Option<&'d MirFunctionDeclaration> + 'd;

/// Lower every copy and move of the function `name`, `function`, naming
/// its constructor to a call of that constructor.
///
/// Regions are lowered too. The constructor's declaration, which
/// `declarations` answers, decides how the call passes the value: a read
/// of the source and the call, or the call alone for a register copy.
/// `structs` answers the struct a closure's body is a method of, whose
/// parameters are in its scope.
pub fn lower_copies_and_moves<'d>(
    name: &str,
    function: &mut MirFunction,
    declarations: &FunctionDeclarations<'d>,
    structs: &StructDeclarations<'d>,
) {
    let parameters =
        materialized_parameter_slots(function, &binder_scope(name, declarations, structs));
    let mut blocks = std::mem::take(&mut function.blocks);
    lower_blocks(&mut blocks, function, declarations, &parameters);
    function.blocks = blocks;
}

fn lower_blocks(
    blocks: &mut [MirBlock],
    function: &mut MirFunction,
    declarations: &FunctionDeclarations<'_>,
    parameters: &HashSet<VarId>,
) {
    for block in blocks {
        let spread: HashSet<u32> = block
            .instrs
            .iter()
            .filter_map(|instruction| match instruction {
                MirInstr::Call {
                    args,
                    spread: Some(position),
                    ..
                }
                | MirInstr::MethodCall {
                    args,
                    spread: Some(position),
                    ..
                } => args.get(*position).map(|register| register.0),
                _ => None,
            })
            .collect();
        let mut lowered = Vec::with_capacity(block.instrs.len());
        for mut instruction in std::mem::take(&mut block.instrs) {
            for region in instruction_regions_mut(&mut instruction) {
                lower_blocks(region, function, declarations, parameters);
            }
            let kept = match &instruction {
                MirInstr::UseVar { dest, var, .. } => {
                    parameters.contains(var) || spread.contains(&dest.0)
                }
                _ => false,
            };
            match (!kept)
                .then(|| call_of(&instruction, function, declarations))
                .flatten()
            {
                Some((read, call)) => {
                    lowered.extend(read);
                    lowered.push(call);
                }
                None => lowered.push(instruction),
            }
        }
        block.instrs = lowered;
    }
}

/// The read of the source, if the source is a slot, and the call of the
/// constructor `instruction` names, when it names one taking the value as
/// its one parameter.
fn call_of(
    instruction: &MirInstr,
    function: &mut MirFunction,
    declarations: &FunctionDeclarations<'_>,
) -> Option<(Option<MirInstr>, MirInstr)> {
    match instruction {
        MirInstr::UseVar {
            dest,
            var,
            mode: mode @ (UseMode::Copy | UseMode::Move),
            constructor: Some(constructor),
        } => {
            let argument =
                constructor_argument(constructor, declarations, function.var_tys.get(var)?)?;
            let ty = function.var_tys.get(var)?.clone();
            let source = source_register(function, *dest, *var);
            let read = if *mode == UseMode::Copy {
                MirInstr::LoadPlace {
                    dest: source,
                    place: MirPlace::root(*var, Some(ty)),
                }
            } else {
                MirInstr::UseVar {
                    dest: source,
                    var: *var,
                    mode: UseMode::Forward,
                    constructor: None,
                }
            };
            Some((
                Some(read),
                constructor_call(*dest, constructor, source, argument),
            ))
        }
        MirInstr::CopyValue {
            dest,
            value,
            constructor: Some(constructor),
        } => {
            let argument =
                constructor_argument(constructor, declarations, function.reg_types.get(&value.0)?)?;
            Some((None, constructor_call(*dest, constructor, *value, argument)))
        }
        _ => None,
    }
}

/// How the constructor `symbol` takes the value of type `ty` it copies or
/// moves: by the one parameter its declaration has after `out self`,
/// positional or keyword-only, beside `ty`'s arguments for the struct
/// parameters the declaration restates. `None` for a constructor whose
/// declaration is not at hand, which raises, or which takes anything else,
/// or for a `ty` that applies fewer arguments than it restates, whose use
/// stays a use.
fn constructor_argument(
    symbol: &str,
    declarations: &FunctionDeclarations<'_>,
    ty: &Ty,
) -> Option<ConstructorArgument> {
    let declaration = declarations(symbol)?;
    let initializer = declaration.has_receiver
        && declaration.receiver_convention == Some(ArgConvention::Out)
        && !declaration.raises
        && declaration.variadic.is_none()
        && declaration.kw_variadic.is_none();
    let [name] = declaration.param_names.as_slice() else {
        return None;
    };
    let struct_arguments = restated_arguments(&declaration.param_decls, ty)?;
    initializer.then(|| ConstructorArgument {
        name: name.clone(),
        keyword: declaration.keyword_only == Some(0),
        struct_arguments,
    })
}

/// The arguments of the value type `ty` for the struct parameters `decls`
/// a constructor restates, one per parameter. A type argument spreading a
/// pack (`Tuple[*Ts.reverse()]`) is a list of types, not one: a constructor
/// restating a pack takes none, its owner's type binding them, unless it
/// also restates a value parameter the call must then carry. `None` when
/// `ty` applies too few.
fn restated_arguments(decls: &[ParamDecl], ty: &Ty) -> Option<Vec<TyArg>> {
    let variadic = |decl: &ParamDecl| {
        matches!(
            decl,
            ParamDecl::Type { variadic: true, .. } | ParamDecl::Value { variadic: true, .. }
        )
    };
    if decls.iter().any(variadic) {
        return decls
            .iter()
            .all(|decl| variadic(decl) || matches!(decl, ParamDecl::Type { .. }))
            .then(Vec::new);
    }
    let arguments: Vec<TyArg> = match ty.kind() {
        TyKind::Struct(_, arguments) => arguments
            .iter()
            .take_while(|argument| !matches!(argument, TyArg::Origin(_)))
            .take(decls.len())
            .cloned()
            .collect(),
        _ => Vec::new(),
    };
    (arguments.len() == decls.len()).then_some(arguments)
}

/// A fresh register of the slot `var`'s type holding what the use `dest`
/// read of it, located where the use was and read from the slot.
fn source_register(function: &mut MirFunction, dest: Reg, var: VarId) -> Reg {
    let ty = function.var_tys[&var].clone();
    let span = function.spans.0.get(&dest.0).map(|(span, _)| span.clone());
    let source = fresh_register(function, ty, span.as_ref());
    if let Some((_, origin)) = function.spans.0.get_mut(&source.0) {
        *origin = Some(var);
    }
    source
}

/// The call of `constructor` initializing `dest` from the register
/// `source`, passed as `argument` says, as a construction `S(copy=x)` or
/// `S(move=x^)` lowers to.
fn constructor_call(
    dest: Reg,
    constructor: &str,
    source: Reg,
    argument: ConstructorArgument,
) -> MirInstr {
    let (args, kwargs) = if argument.keyword {
        (Vec::new(), vec![(argument.name, source)])
    } else {
        (vec![source], Vec::new())
    };
    let arg_places = vec![None; args.len()];
    let kwarg_places = vec![None; kwargs.len()];
    MirInstr::Call {
        dest,
        func: FuncRef::named(constructor),
        raises: None,
        args,
        kwargs,
        arg_places,
        kwarg_places,
        capture_accesses: Vec::new(),
        param_arg_regs: Vec::new(),
        receiver: None,
        instantiated_args: argument.struct_arguments,
        spread: None,
    }
}

/// The one parameter of a copy or move constructor after `out self`,
/// whether a call passes it by keyword, and the struct arguments the call
/// passes for the parameters the constructor restates.
struct ConstructorArgument {
    name: String,
    keyword: bool,
    struct_arguments: Vec<TyArg>,
}

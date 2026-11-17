//! Copies and moves lowered to calls of their constructor.
//!
//! Upstream's copy/move insertion leaves, at each point a value is copied
//! or moved, a plain call of its type's `__copyinit__` or `__moveinit__`
//! initializing fresh storage, which the elaborator and every later pass
//! see as any call. A `var.use copy`, a `var.use move`, and a `value.copy`
//! name that constructor but stay a use, which each backend runs by a copy
//! or move path of its own. Where a use names its constructor, and the
//! constructor takes the value as its one parameter after `out self`, the
//! use is the call: a copy of a slot passes a reference to it (`ref.make`)
//! and a copy of a value read through a reference passes the reference,
//! as upstream's `copy: Self` takes the source's `!lit.ref`; a register
//! copy passes its register; a move forwards the slot, which ends it and
//! hands its storage to the callee as Mojo's owned argument convention
//! does, and passes it as the constructor's `deinit` argument.
//! The call passes the value's struct arguments as the constructor's
//! compile-time arguments, which its declaration restates, as a
//! construction does.
//! A use naming none, a symbolic type's until the elaborator names its
//! instance's, stays a use, which the backends copy or move by type. So
//! does the read of a variadic collector a call spreads (`show(*args)`),
//! which the elaborator expands into the pack's element places.

use super::{
    FuncRef, MirBlock, MirFunction, MirFunctionDeclaration, MirInstr, MirPlace, Reg, UseMode,
    VarId, fresh_register, instruction_regions_mut,
};
use mojito_ast::ast::ArgConvention;
use mojito_types::types::{ParamDecl, Ty, TyArg, TyKind};
use std::collections::{HashMap, HashSet};

/// A function's declaration, by the function's name.
pub type FunctionDeclarations<'d> = dyn Fn(&str) -> Option<&'d MirFunctionDeclaration> + 'd;

/// Lower every copy and move of `function` naming its constructor to a
/// call of that constructor.
///
/// Regions are lowered too. The constructor's declaration, which
/// `declarations` answers, decides how the call passes the value: a read
/// of the source and the call, or the call alone for a register copy.
pub fn lower_copies_and_moves(function: &mut MirFunction, declarations: &FunctionDeclarations<'_>) {
    let mut blocks = std::mem::take(&mut function.body);
    lower_blocks(&mut blocks, function, declarations);
    function.body = blocks;
}

fn lower_blocks(
    block: &mut MirBlock,
    function: &mut MirFunction,
    declarations: &FunctionDeclarations<'_>,
) {
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
    let mut read_through = HashMap::new();
    for mut instruction in std::mem::take(&mut block.instrs) {
        for region in instruction_regions_mut(&mut instruction) {
            lower_blocks(region, function, declarations);
        }
        let kept = matches!(&instruction,
            MirInstr::UseVar { dest, .. } if spread.contains(&dest.0));
        let call = (!kept)
            .then(|| call_of(&instruction, function, declarations, &read_through))
            .flatten();
        match &instruction {
            MirInstr::ReadRef { dest, reference }
                if matches!(
                    function.reg_types.get(&reference.0).map(Ty::kind),
                    Some(TyKind::Ref(_))
                ) =>
            {
                read_through.insert(dest.0, *reference);
            }
            instruction if !writes_nothing(instruction) => read_through.clear(),
            _ => {}
        }
        match call {
            Some((read, call)) => {
                lowered.extend(read);
                lowered.push(call);
            }
            None => lowered.push(instruction),
        }
    }
    block.instrs = lowered;
}

/// The reference to, or the forward of, the source, if the source is a
/// slot, and the call of the constructor `instruction` names, when it
/// names one taking the value as its one parameter. A copied register `read_through` maps, read through
/// a reference with nothing written since, passes that reference.
fn call_of(
    instruction: &MirInstr,
    function: &mut MirFunction,
    declarations: &FunctionDeclarations<'_>,
    read_through: &HashMap<u32, Reg>,
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
            let (source, read) = if *mode == UseMode::Copy {
                let place = MirPlace::root(*var, Some(ty));
                let handle = crate::mir::mir_place_handle_ty(
                    &place,
                    Some(mojito_types::origin::Mutability::Immutable),
                )?;
                let source = source_register(function, *dest, *var, handle);
                (
                    source,
                    MirInstr::MakeRef {
                        dest: source,
                        place,
                    },
                )
            } else {
                let source = source_register(function, *dest, *var, ty);
                (
                    source,
                    MirInstr::UseVar {
                        dest: source,
                        var: *var,
                        mode: UseMode::Forward,
                        constructor: None,
                    },
                )
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
            let source = read_through.get(&value.0).copied().unwrap_or(*value);
            Some((None, constructor_call(*dest, constructor, source, argument)))
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
        struct_ty if let Some((_, arguments)) = struct_ty.as_non_simd_struct() => arguments
            .iter()
            .take_while(|argument| !matches!(argument, TyArg::Origin(_)))
            .take(decls.len())
            .cloned()
            .collect(),
        _ => Vec::new(),
    };
    (arguments.len() == decls.len()).then_some(arguments)
}

/// Whether `instruction` only reads, so a referent read before it still
/// holds the value read.
const fn writes_nothing(instruction: &MirInstr) -> bool {
    matches!(
        instruction,
        MirInstr::LoadPlace { .. }
            | MirInstr::ReadRef { .. }
            | MirInstr::Const { .. }
            | MirInstr::MakeRef { .. }
            | MirInstr::MaterializeLiteral { .. }
            | MirInstr::UseVar {
                mode: UseMode::Copy,
                ..
            }
    )
}

/// A fresh register of type `ty` holding the slot `var`, or a reference to
/// it, for the use `dest`, located where the use was and read from the slot.
fn source_register(function: &mut MirFunction, dest: Reg, var: VarId, ty: Ty) -> Reg {
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
    MirInstr::Call {
        conventions: crate::mir::MirArgConventions::default(),
        dest,
        func: FuncRef::named(constructor),
        raises: None,
        args,
        kwargs,
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

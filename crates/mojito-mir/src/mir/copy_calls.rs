//! Copies and moves lowered to calls of their constructor.
//!
//! Upstream's copy/move insertion leaves, at each point a value is copied
//! or moved, a plain call of its type's `__copyinit__` or `__moveinit__`
//! initializing fresh storage, which the elaborator and every later pass
//! see as any call. A `var.use copy`, a `var.use move`, and a `value.copy`
//! name that constructor but stay a use, which each backend runs by a copy
//! or move path of its own. Where a use names its constructor, and the
//! constructor takes the value as its one parameter after `out self`, the
//! use is the call: a copy of a slot passes a reference to it (`ref.make`),
//! a copy of a value read through a reference passes the reference, and a
//! copy of a value loaded from a place with nothing written since passes a
//! reference to the place, as upstream's `copy: Self` takes the source's
//! `!lit.ref` (the handle a declared `ref` field stores, for a value read
//! through one); a reference binding the place goes through then ends after
//! the call rather than at the load. Any other register copy passes its
//! register. A move forwards the slot, which ends it and hands its storage
//! to the callee as Mojo's owned argument convention does, and passes it
//! as the constructor's `deinit` argument.
//! The call passes the value's struct arguments as the constructor's
//! compile-time arguments, which its declaration restates, as a
//! construction does.
//! A use naming none, a symbolic type's until the elaborator names its
//! instance's, stays a use, which the backends copy or move by type. So
//! does the read of a variadic collector a call spreads (`show(*args)`),
//! which the elaborator expands into the pack's element places.

use super::{
    FuncRef, MirBlock, MirFunction, MirFunctionDeclaration, MirInstr, MirPlace, Reg, UseMode,
    VarId, addressed_operand, fresh_register, instruction_regions_mut,
};
use mojito_ast::ast::ArgConvention;
use mojito_types::types::{ParamDecl, Ty, TyArg};
use std::collections::{HashMap, HashSet};

/// A function's declaration, by the function's name.
pub type FunctionDeclarations<'d> = dyn Fn(&str) -> Option<&'d MirFunctionDeclaration> + 'd;

/// Whether a struct type declares a field `ref`, by the type and the
/// field's name.
pub type DeclaresRefField<'d> = dyn Fn(&Ty, &str) -> bool + 'd;

/// Lower every copy and move of `function` naming its constructor to a
/// call of that constructor.
///
/// Regions are lowered too. The constructor's declaration, which
/// `declarations` answers, decides how the call passes the value: a read
/// of the source and the call, or the call alone for a register copy.
/// `declares_ref_field` names the fields whose storage is a reference, so
/// a copy of a value loaded through one passes the stored handle.
/// `applied` says whether the constructors the uses name are generators,
/// as in parametric MIR, whose call carries the copied type's arguments
/// for the struct parameters it restates, or instances, as the elaborator
/// names them, whose call carries none: an instance is its generator
/// already applied.
pub fn lower_copies_and_moves(
    function: &mut MirFunction,
    declarations: &FunctionDeclarations<'_>,
    declares_ref_field: &DeclaresRefField<'_>,
    applied: bool,
) {
    let mut blocks = std::mem::take(&mut function.body);
    lower_blocks(
        &mut blocks,
        function,
        declarations,
        declares_ref_field,
        applied,
    );
    function.body = blocks;
}

fn lower_blocks(
    block: &mut MirBlock,
    function: &mut MirFunction,
    declarations: &FunctionDeclarations<'_>,
    declares_ref_field: &DeclaresRefField<'_>,
    applied: bool,
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
    let mut loaded = HashMap::new();
    for mut instruction in std::mem::take(&mut block.instrs) {
        for region in instruction_regions_mut(&mut instruction) {
            lower_blocks(region, function, declarations, declares_ref_field, applied);
        }
        let kept = matches!(&instruction,
            MirInstr::UseVar { dest, .. } if spread.contains(&dest.0));
        let sources = Sources {
            read_through: &read_through,
            loaded: &loaded,
            declares_ref_field,
            applied,
        };
        let call = (!kept)
            .then(|| call_of(&instruction, function, declarations, &sources))
            .flatten();
        match &instruction {
            MirInstr::ReadRef { dest, reference }
                if function
                    .reg_types
                    .get(&reference.0)
                    .zip(function.reg_types.get(&dest.0))
                    .is_some_and(|(reference, value)| {
                        addressed_operand(reference, value).is_some()
                    }) =>
            {
                read_through.insert(dest.0, *reference);
            }
            MirInstr::LoadPlace { dest, place } => {
                loaded.insert(dest.0, (place.clone(), lowered.len()));
            }
            // The end of a slot that runs no destructor keeps the places
            // loaded from it: a copy of one ends the slot after the call
            // instead ([`end_after_call`]).
            MirInstr::DropVar {
                var,
                destructor: None,
            } if !writes_nothing(&instruction, function) => {
                read_through.clear();
                loaded.retain(|_, (place, _)| place.root == *var);
            }
            instruction if !writes_nothing(instruction, function) => {
                read_through.clear();
                loaded.clear();
            }
            _ => {}
        }
        match call {
            Some(Lowered::Read(read, call)) => {
                lowered.extend(read);
                lowered.push(call);
            }
            Some(Lowered::Loaded(load, binding, reference, call)) => {
                let ended = end_after_call(&mut lowered, load, binding);
                lowered.push(reference);
                lowered.push(call);
                lowered.extend(ended);
            }
            None => lowered.push(instruction),
        }
    }
    block.instrs = lowered;
}

/// The call of the constructor `instruction` names, when it names one
/// taking the value as its one parameter, with how its source is read: a
/// slot's reference or forward, or a loaded place's reference, before the
/// call. A copied register `sources` maps, read through a reference or
/// loaded from a place with nothing written since, passes that reference
/// or the place's.
fn call_of(
    instruction: &MirInstr,
    function: &mut MirFunction,
    declarations: &FunctionDeclarations<'_>,
    sources: &Sources<'_>,
) -> Option<Lowered> {
    match instruction {
        MirInstr::UseVar {
            dest,
            var,
            mode: mode @ (UseMode::Copy | UseMode::Move),
            constructor: Some(constructor),
        } => {
            let argument = constructor_argument(
                constructor,
                declarations,
                function.var_tys.get(var)?,
                sources.applied,
            )?;
            let ty = function.var_tys.get(var)?.clone();
            let (source, read) = if *mode == UseMode::Copy {
                let place = MirPlace::root(*var, Some(ty));
                let handle = crate::mir::mir_place_handle_ty(
                    &place,
                    function.binds_reference(*var),
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
            Some(Lowered::Read(
                Some(read),
                constructor_call(*dest, constructor, source, argument),
            ))
        }
        MirInstr::CopyValue {
            dest,
            value,
            constructor: Some(constructor),
        } => {
            let argument = constructor_argument(
                constructor,
                declarations,
                function.reg_types.get(&value.0)?,
                sources.applied,
            )?;
            if let Some((place, load)) = sources.loaded.get(&value.0)
                && let Some(storage) = place.ty.clone()
            {
                let binds_root = function.binds_reference(place.root);
                let stored_handle = crate::mir::place_holds_reference(
                    place,
                    binds_root,
                    sources.declares_ref_field,
                );
                let value_ty = if stored_handle {
                    crate::mir::place_value_ty(place, binds_root, sources.declares_ref_field)
                } else {
                    Some(&storage)
                };
                if value_ty.is_some() && value_ty == function.reg_types.get(&value.0) {
                    let span = function.spans.0.get(&dest.0).map(|(span, _)| span.clone());
                    // A value read through a declared `ref` field is copied
                    // from the handle the field stores, which a load typed
                    // as the handle keeps; any other place lends its own.
                    let (source, reference) = if stored_handle {
                        let source = fresh_register(function, storage, span.as_ref());
                        let load = MirInstr::LoadPlace {
                            dest: source,
                            place: place.clone(),
                        };
                        (source, load)
                    } else {
                        let handle = crate::mir::mir_place_handle_ty(
                            place,
                            binds_root,
                            Some(mojito_types::origin::Mutability::Immutable),
                        )?;
                        let source = fresh_register(function, handle, span.as_ref());
                        let make = MirInstr::MakeRef {
                            dest: source,
                            place: place.clone(),
                        };
                        (source, make)
                    };
                    return Some(Lowered::Loaded(
                        *load,
                        place.root,
                        reference,
                        constructor_call(*dest, constructor, source, argument),
                    ));
                }
            }
            let source = sources
                .read_through
                .get(&value.0)
                .copied()
                .unwrap_or(*value);
            Some(Lowered::Read(
                None,
                constructor_call(*dest, constructor, source, argument),
            ))
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
/// stays a use. An instance constructor (`applied` false) restates none.
fn constructor_argument(
    symbol: &str,
    declarations: &FunctionDeclarations<'_>,
    ty: &Ty,
    applied: bool,
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
    let struct_arguments = if applied {
        restated_arguments(&declaration.param_decls, ty)?
    } else {
        Vec::new()
    };
    initializer.then(|| ConstructorArgument {
        name: name.clone(),
        keyword: declaration.keyword_only == Some(0),
        struct_arguments,
    })
}

/// The arguments of the value type `ty` for the struct parameters `decls`
/// a constructor restates, a value for every one, origins and their
/// `mut=` values in place, as the instance holds them. A pack struct
/// applied element by element (`Tuple[Int, Bool]`) binds its pack to the
/// list of those types and the pack's visible binder to its `TypeList`
/// ([`mojito_types::types::declared_pack_arguments`]), as its
/// construction does; one restating a value parameter besides its pack
/// takes none here. `None` when `ty` applies too few.
fn restated_arguments(decls: &[ParamDecl], ty: &Ty) -> Option<Vec<TyArg>> {
    let variadic = |decl: &ParamDecl| {
        matches!(
            decl,
            ParamDecl::Type { variadic: true, .. } | ParamDecl::Value { variadic: true, .. }
        )
    };
    let (name, arguments) = ty.kind().as_non_simd_struct()?;
    if decls.iter().any(variadic)
        && !ParamDecl::argument_binders(decls)
            .all(|decl| variadic(decl) || matches!(decl, ParamDecl::Type { .. }))
    {
        return None;
    }
    let arguments = mojito_types::types::declared_pack_arguments(name, decls, arguments);
    (arguments.len() == decls.len()).then(|| {
        arguments
            .iter()
            .map(|argument| match argument {
                // A pack bound to a list computed from packs still open
                // (`Tuple[*Ts.reverse()]`) is that list's value.
                TyArg::Ty(ty) if let Some(list) = mojito_types::types::list_spread(ty) => {
                    TyArg::Val(mojito_types::ct::CtValue::Expr(list.clone()))
                }
                argument => argument.clone(),
            })
            .collect()
    })
}

/// Whether `instruction` only reads, so a referent read or a place loaded
/// before it still holds the value read. Ending a reference binding, whose
/// drop destroys nothing, writes no storage.
fn writes_nothing(instruction: &MirInstr, function: &MirFunction) -> bool {
    if let MirInstr::DropVar {
        var,
        destructor: None,
    } = instruction
    {
        return function.binds_reference(*var);
    }
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

/// Take out of `lowered`, after the load at `load`, the end of the slot
/// `binding` (a reference binding, or a slot running no destructor), which
/// drop elaboration placed at the load as the slot's last use: the copy's
/// reference to the place in or through it is now that use, so the slot
/// ends after the call.
fn end_after_call(lowered: &mut Vec<MirInstr>, load: usize, binding: VarId) -> Vec<MirInstr> {
    let (ended, kept): (Vec<_>, Vec<_>) = lowered.split_off(load + 1).into_iter().partition(
        |instruction| matches!(instruction, MirInstr::DropVar { var, .. } if *var == binding),
    );
    lowered.extend(kept);
    ended
}

/// What a copied register's source was read from, with nothing written
/// since: the reference a `ref.read` read through, and the place a
/// `place.load` loaded with the load's index among the lowered
/// instructions.
struct Sources<'s> {
    read_through: &'s HashMap<u32, Reg>,
    loaded: &'s HashMap<u32, (MirPlace, usize)>,
    declares_ref_field: &'s DeclaresRefField<'s>,
    applied: bool,
}

/// A use lowered to its constructor's call: after the source's read, if
/// any, or after a reference to a place loaded at the index given, whose
/// root slot must stay live through the call.
enum Lowered {
    Read(Option<MirInstr>, MirInstr),
    Loaded(usize, VarId, MirInstr, MirInstr),
}

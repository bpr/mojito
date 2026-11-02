//! A type binder's construction, written as the bound type's own.
//!
//! Upstream rewrites `T()` and `T(copy=x)` when substitution binds `T`,
//! wherever substitution happens. Mojito substitutes in two places, the
//! elaborator's instance and `inline-parametric`'s binding of an inlined
//! call, and both write the construction here: a scalar's zero, a struct's
//! call of its `Defaultable` witness, a built-in's
//! value copy, or a struct's call of its copy initializer, as upstream's
//! substitution folds the `Defaultable` and `Copyable` witnesses to the
//! member symbols.

use crate::mir::{
    Const, FuncRef, MirBlock, MirDeclarations, MirInstr, MirParamArg, MirPlace, Reg,
    instruction_regions_mut,
};
use mojito_types::ct::CtValue;
use mojito_types::param_expr::ParamRef;
use mojito_types::types::{ParamDecl, Ty, TyArg, TyKind};
use std::fmt;

/// The types a construction's binder and pack element are bound to.
pub trait ConstructionTypes {
    type Error: From<TypeConstructionError>;
    /// The type `param` is bound to; `None` when it is not bound here.
    fn bound(&self, param: &ParamRef) -> Option<&Ty>;
    /// The index the pack element `element` (`Ts[i]()`) selects; `None`
    /// while it is open.
    fn element_index(&self, element: &MirParamArg) -> Option<usize>;
    /// `ty` closed under the bindings.
    ///
    /// # Errors
    ///
    /// When the bindings cannot close `ty`.
    fn close(&self, ty: &Ty) -> Result<Ty, Self::Error>;
}

/// A construction its bound type cannot build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeConstructionError {
    /// `Ts[i]()` over a pack bound to a type that is no pack.
    NonPackBinding { pack: String, bound: Ty },
    /// `Ts[i]()` past the bound pack's last element.
    ElementOutOfRange {
        pack: String,
        index: usize,
        len: usize,
    },
    /// A default construction of a type that has no default.
    NotAStruct(Ty),
}

impl fmt::Display for TypeConstructionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonPackBinding { pack, bound } => {
                write!(f, "pack `{pack}` is bound to the non-pack type `{bound}`")
            }
            Self::ElementOutOfRange { pack, index, len } => write!(
                f,
                "element {index} of pack `{pack}` is out of range for its {len} elements"
            ),
            Self::NotAStruct(ty) => write!(f, "default-constructing `{ty}`, which is no struct"),
        }
    }
}

impl std::error::Error for TypeConstructionError {}

/// Rewrite each type construction `types` decides as that type's own.
///
/// The constructions are `T()`, `Ts[i]()`, `T(copy=x)`, and a type
/// expression's (`types[i]()` over a reflected field-type list), in
/// `blocks` and their regions; the rest are left.
///
/// A nullary construction is the type's default construction: a scalar's
/// zero value, or a struct's (a `SIMD` vector's among them) call of the `__init__` its `Defaultable` witness table records
/// ([`MirDeclarations::default_initializer`]); a struct recording none, as
/// one an untaken `comptime if` arm constructs, stays the construction of
/// the decided type, which branch selection erases with its arm and the
/// concrete verifier rejects where it is reached. `T(copy=x)` copies its
/// source through the bound type's copy initializer: a built-in value by
/// the value read, a struct as [`struct_copy`] does. Each call passes the
/// struct's arguments as its `instantiated_args`, as a direct `S()` or
/// `S(copy=x)` does, and is instantiated by the elaborator's call
/// rewriting. A construction left open — a binder bound to a symbolic type, a pack
/// element whose `comptime for` index is not yet unrolled, a copy at a type
/// with no copy initializer here, an initializer other than `copy=` — stays.
///
/// # Errors
///
/// When the decided type cannot be constructed, or `types` cannot close a
/// selected pack element.
pub fn construct_type_parameters<C: ConstructionTypes>(
    blocks: &mut [MirBlock],
    types: &C,
    declarations: &MirDeclarations,
) -> Result<(), C::Error> {
    for block in blocks {
        let mut index = 0;
        while index < block.instrs.len() {
            let built = match &mut block.instrs[index] {
                MirInstr::ConstructTypeParam {
                    dest,
                    param,
                    kwargs,
                    element,
                    ..
                } if kwargs.is_empty() => constructed_type(param, element.as_ref(), types)?
                    .map(|ty| default_construction(*dest, &ty, declarations))
                    .transpose()?,
                MirInstr::ConstructType { dest, ty } => types
                    .close(ty)
                    .ok()
                    .filter(|ty| !mojito_types::types::is_symbolic(ty))
                    .map(|ty| default_construction(*dest, &ty, declarations))
                    .transpose()?,
                instruction @ MirInstr::ConstructTypeParam { .. } => {
                    copy_construction(instruction, types, declarations).map(|copy| vec![copy])
                }
                instruction => {
                    for region in instruction_regions_mut(instruction) {
                        construct_type_parameters(region, types, declarations)?;
                    }
                    None
                }
            };
            let Some(built) = built else {
                index += 1;
                continue;
            };
            let count = built.len();
            block.instrs.splice(index..=index, built);
            index += count;
        }
    }
    Ok(())
}

/// The copy construction (`S(copy=source)`) of the struct `ty` into `dest`.
///
/// It is a call of the copy initializer the struct's member table records
/// ([`MirDeclarations::copy_initializer`]) over the `copy=` keyword, which
/// passes the struct's arguments as its `instantiated_args`; a struct whose
/// copy is its trivial fieldwise copy records none, and copies by
/// [`MirInstr::CopyValue`].
pub fn struct_copy(
    dest: Reg,
    ty: &Ty,
    (source, place): (Reg, Option<MirPlace>),
    declarations: &MirDeclarations,
) -> MirInstr {
    let Some(initializer) = declarations.copy_initializer(ty) else {
        return MirInstr::CopyValue {
            dest,
            value: source,
            constructor: None,
        };
    };
    initializer_call(
        dest,
        initializer,
        struct_arguments(ty, declarations),
        vec![("copy".to_string(), source)],
        vec![place],
    )
}

/// A `SIMD` built as its initializers build it: `op`, a `pop` op writing
/// the `!kgen.simd` register `storage`, then the struct `dest` created over
/// that storage (`struct.create`).
pub fn storage_construction(dest: Reg, storage: Reg, op: MirInstr) -> [MirInstr; 2] {
    [
        op,
        MirInstr::MakeStruct {
            dest,
            fields: vec![storage],
        },
    ]
}

/// The type a nullary construction builds under `types`: the binder's own
/// binding, or for `Ts[i]()` the element its index selects from the pack's.
/// `None` while either is open.
fn constructed_type<C: ConstructionTypes>(
    param: &ParamRef,
    element: Option<&MirParamArg>,
    types: &C,
) -> Result<Option<Ty>, C::Error> {
    let Some(bound) = types.bound(param) else {
        return Ok(None);
    };
    let Some(element) = element else {
        return Ok((!mojito_types::types::is_symbolic(bound)).then(|| bound.clone()));
    };
    let TyKind::RuntimePack(elements) = bound.kind() else {
        return Err(TypeConstructionError::NonPackBinding {
            pack: param.name.to_string(),
            bound: bound.clone(),
        }
        .into());
    };
    let Some(index) = types.element_index(element) else {
        return Ok(None);
    };
    let chosen = elements
        .get(index)
        .ok_or_else(|| TypeConstructionError::ElementOutOfRange {
            pack: param.name.to_string(),
            index,
            len: elements.len(),
        })?;
    types.close(chosen).map(Some)
}

/// The default construction of the concrete `ty` into `dest`.
fn default_construction(
    dest: Reg,
    ty: &Ty,
    declarations: &MirDeclarations,
) -> Result<Vec<MirInstr>, TypeConstructionError> {
    let zero = match ty.kind() {
        // A literal's type names its one value, as upstream's empty
        // `IntLiteral[value]`, `FloatLiteral[value]`, and
        // `StringLiteral[value]` structs do.
        kind if let Some(value) = kind.int_literal_value() => {
            Some(Const::IntLiteral(value.clone()))
        }
        kind if let Some(value) = kind.float_literal_value() => {
            Some(Const::FloatLiteral(value.clone()))
        }
        kind if kind.is_int() || kind.is_uint() || matches!(kind, TyKind::IntLiteral(_)) => {
            Some(Const::Int(0))
        }
        TyKind::Bool => Some(Const::Bool(false)),
        kind if kind.is_float64() || matches!(kind, TyKind::FloatLiteral(_)) => {
            Some(Const::Float(0.0))
        }
        kind @ TyKind::StringLiteral(_) => Some(Const::Str(
            kind.string_literal_text().unwrap_or_default().to_string(),
        )),
        TyKind::None => Some(Const::None),
        _ => None,
    };
    if let Some(k) = zero {
        return Ok(vec![MirInstr::Const { dest, k }]);
    }
    match ty.kind() {
        TyKind::Struct(..) => Ok(vec![declarations.default_initializer(ty).map_or_else(
            || MirInstr::ConstructType {
                dest,
                ty: ty.clone(),
            },
            |initializer| {
                initializer_call(
                    dest,
                    initializer,
                    struct_arguments(ty, declarations),
                    Vec::new(),
                    Vec::new(),
                )
            },
        )]),
        _ => Err(TypeConstructionError::NotAStruct(ty.clone())),
    }
}

/// `T(copy=x)`'s copy through the type `types` binds `T` to; `None` for any
/// other initializer, and at a binding with no copy initializer here.
fn copy_construction<C: ConstructionTypes>(
    instruction: &MirInstr,
    types: &C,
    declarations: &MirDeclarations,
) -> Option<MirInstr> {
    let MirInstr::ConstructTypeParam {
        dest,
        param,
        kwargs,
        kwarg_places,
        ..
    } = instruction
    else {
        return None;
    };
    let [(keyword, source)] = kwargs.as_slice() else {
        return None;
    };
    if keyword != "copy" {
        return None;
    }
    let bound = types.bound(param)?;
    match bound.kind() {
        ty if mojito_types::types::builtin_copy_is_value_read(ty) => Some(MirInstr::CopyValue {
            dest: *dest,
            value: *source,
            constructor: None,
        }),
        TyKind::Struct(..) => Some(struct_copy(
            *dest,
            bound,
            (*source, kwarg_places.first().cloned().flatten()),
            declarations,
        )),
        _ => None,
    }
}

/// The compile-time arguments a call of the struct `ty`'s initializer
/// binds its parameters by: the type's arguments, one per parameter, a lone
/// pack the type spells element by element (`Tuple[Int, Bool]`) passed
/// whole as a direct call passes it.
fn struct_arguments(ty: &Ty, declarations: &MirDeclarations) -> Vec<TyArg> {
    let TyKind::Struct(name, arguments) = ty.kind() else {
        return Vec::new();
    };
    let lone_pack = declarations
        .structure(mojito_types::types::struct_template(name))
        .is_some_and(|declaration| {
            matches!(
                declaration.param_decls.as_slice(),
                [ParamDecl::Type { variadic: true, .. }]
            )
        });
    let whole = matches!(arguments.as_slice(), [TyArg::Val(_)])
        || matches!(arguments.as_slice(), [TyArg::Ty(pack)]
            if matches!(pack.kind(), TyKind::RuntimePack(_)));
    if !lone_pack || whole {
        return arguments.clone();
    }
    vec![TyArg::Val(CtValue::Tuple(
        arguments
            .iter()
            .filter_map(|argument| match argument {
                TyArg::Ty(element) => Some(CtValue::Type(element.clone())),
                TyArg::Origin(_) | TyArg::Val(_) => None,
            })
            .collect(),
    ))]
}

/// A call of the struct initializer `initializer` over the keyword
/// `kwargs`, binding the struct's parameters by `instantiated_args`.
fn initializer_call(
    dest: Reg,
    initializer: &str,
    instantiated_args: Vec<TyArg>,
    kwargs: Vec<(String, Reg)>,
    kwarg_places: Vec<Option<MirPlace>>,
) -> MirInstr {
    MirInstr::Call {
        dest,
        func: FuncRef::named(initializer),
        raises: None,
        args: Vec::new(),
        kwargs,
        arg_places: Vec::new(),
        kwarg_places,
        capture_accesses: Vec::new(),
        param_arg_regs: Vec::new(),
        receiver: None,
        instantiated_args,
        spread: None,
    }
}

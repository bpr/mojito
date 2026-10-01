//! The closed type vocabulary of `mojito`, and its correspondence with
//! checked [`Ty`]. A type outside the vocabulary is rejected, never carried
//! opaquely.

use pliron::context::Context;
use pliron::derive::{format, pliron_type};
use pliron::r#type::{TypeHandle, Typed};

use mojito_ast::ast::Dtype;
use mojito_types::origin::{
    CallableEnvironment, CaptureOrigin, CaptureOriginSet, CaptureSetParamId, Mutability, Origin,
    OriginParamId, OriginPlace, OriginSeg, OwnerId, PointerOrigin, RefSig, RefTy, SigMutability,
    SigOrigin,
};
use mojito_types::types::{SimdDtype, SimdWidth, TransferEffect, TransferSet, Ty, TyArg};

use super::A1Error;
use super::attrs::{CoreConvention, Text};
use super::params::{NodeKey, PayloadBinder, export_constant, import_constant};

#[pliron_type(name = "mojito.int", generate_get = true, format, verifier = "succ")]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct IntType;

/// The machine `Float64`.
#[pliron_type(
    name = "mojito.float64",
    generate_get = true,
    format,
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct Float64Type;

/// An exact floating literal not yet materialized into a machine float.
#[pliron_type(
    name = "mojito.float_literal",
    generate_get = true,
    format,
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct FloatLiteralType;

/// The compile-time `DType` value type.
#[pliron_type(name = "mojito.dtype", generate_get = true, format, verifier = "succ")]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct DtypeType;

#[pliron_type(name = "mojito.uint", generate_get = true, format, verifier = "succ")]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct UIntType;

#[pliron_type(name = "mojito.bool", generate_get = true, format, verifier = "succ")]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct BoolType;

#[pliron_type(name = "mojito.none", generate_get = true, format, verifier = "succ")]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct NoneType;

/// An exact, arbitrary-precision integer literal before materialization.
#[pliron_type(
    name = "mojito.int_literal",
    generate_get = true,
    format,
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct IntLiteralType;

#[pliron_type(
    name = "mojito.string_literal",
    generate_get = true,
    format,
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct StringLiteralType;

#[pliron_type(name = "mojito.error", generate_get = true, format, verifier = "succ")]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct ErrorType;

/// The sequencing token threaded through every effectful operation.
#[pliron_type(name = "mojito.effect", generate_get = true, format, verifier = "succ")]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct EffectType;

/// A pending outcome held across a `finally` body.
#[pliron_type(
    name = "mojito.outcome",
    generate_get = true,
    format,
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct OutcomeType;

/// A width-`width` vector of `dtype` lanes; width 1 is the scalar alias.
#[pliron_type(
    name = "mojito.simd",
    generate_get = true,
    format = "`<` $dtype ` x ` $width `>`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct SimdType {
    pub dtype: CoreDtype,
    pub width: u64,
}

/// A nominal struct instance with its complete argument identity.
#[pliron_type(
    name = "mojito.nominal",
    generate_get = true,
    format = "`<` $name ` [` vec($args, CharSpace(`,`)) `]>`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct NominalType {
    pub name: Text,
    pub args: Vec<CoreArg>,
}

#[pliron_type(
    name = "mojito.ref",
    generate_get = true,
    format = "`<` $referent `, ` $origin `, ` $mutability `>`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct RefType {
    pub referent: TypeHandle,
    pub origin: CoreOrigin,
    pub mutability: CoreMutability,
}

#[pliron_type(
    name = "mojito.pointer",
    generate_get = true,
    format = "`<` $element `, ` $origin `>`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct PointerType {
    pub element: TypeHandle,
    pub origin: CorePointerOrigin,
}

/// Compiler-private heterogeneous storage: a runtime tuple or pack.
#[pliron_type(
    name = "mojito.tuple",
    generate_get = true,
    format = "`<[` vec($elements, CharSpace(`,`)) `]>`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct TupleType {
    pub elements: Vec<TypeHandle>,
}

/// A sum of alternatives, tagged by position.
#[pliron_type(
    name = "mojito.variant",
    generate_get = true,
    format = "`<[` vec($alternatives, CharSpace(`,`)) `]>`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct VariantType {
    pub alternatives: Vec<TypeHandle>,
}

/// The collector storage of a specialized heterogeneous parameter pack.
#[pliron_type(
    name = "mojito.runtime_pack",
    generate_get = true,
    format = "`<[` vec($elements, CharSpace(`,`)) `]>`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct RuntimePackType {
    pub elements: Vec<TypeHandle>,
}

/// A type parameter a checked call contract names by identity, with the
/// traits that bound it.
#[pliron_type(
    name = "mojito.param",
    generate_get = true,
    format = "`<` $binder ` [` vec($bounds, CharSpace(`,`)) `]>`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct ParamType {
    pub binder: PayloadBinder,
    pub bounds: Vec<Text>,
}

/// A typed storage location: a slot, or a projection below one.
#[pliron_type(
    name = "mojito.place",
    generate_get = true,
    format = "`<` $target `>`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct PlaceType {
    pub target: TypeHandle,
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum CoreDtype {
    Int,
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Float16,
    Float32,
    Float64,
    Bool,
}

impl CoreDtype {
    pub const fn from_dtype(dtype: Dtype) -> Self {
        match dtype {
            Dtype::Int => Self::Int,
            Dtype::Int8 => Self::Int8,
            Dtype::Int16 => Self::Int16,
            Dtype::Int32 => Self::Int32,
            Dtype::Int64 => Self::Int64,
            Dtype::UInt8 => Self::UInt8,
            Dtype::UInt16 => Self::UInt16,
            Dtype::UInt32 => Self::UInt32,
            Dtype::UInt64 => Self::UInt64,
            Dtype::Float16 => Self::Float16,
            Dtype::Float32 => Self::Float32,
            Dtype::Float64 => Self::Float64,
            Dtype::Bool => Self::Bool,
        }
    }

    pub const fn dtype(self) -> Dtype {
        match self {
            Self::Int => Dtype::Int,
            Self::Int8 => Dtype::Int8,
            Self::Int16 => Dtype::Int16,
            Self::Int32 => Dtype::Int32,
            Self::Int64 => Dtype::Int64,
            Self::UInt8 => Dtype::UInt8,
            Self::UInt16 => Dtype::UInt16,
            Self::UInt32 => Dtype::UInt32,
            Self::UInt64 => Dtype::UInt64,
            Self::Float16 => Dtype::Float16,
            Self::Float32 => Dtype::Float32,
            Self::Float64 => Dtype::Float64,
            Self::Bool => Dtype::Bool,
        }
    }
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum CoreArg {
    Type(TypeHandle),
    Origin(CoreOrigin),
    /// A closed value argument, as a constant parameter node.
    Value(NodeKey),
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum CoreSeg {
    Field(Text),
    AnyIndex,
    Interior(Text),
    Subtree,
}

impl CoreSeg {
    pub fn from_seg(seg: &OriginSeg) -> Self {
        match seg {
            OriginSeg::Field(name) => Self::Field(name.into()),
            OriginSeg::AnyIndex => Self::AnyIndex,
            OriginSeg::Interior(tag) => Self::Interior(tag.into()),
            OriginSeg::Subtree => Self::Subtree,
        }
    }

    pub fn seg(&self) -> OriginSeg {
        match self {
            Self::Field(name) => OriginSeg::Field(name.0.clone()),
            Self::AnyIndex => OriginSeg::AnyIndex,
            Self::Interior(tag) => OriginSeg::Interior(tag.0.clone()),
            Self::Subtree => OriginSeg::Subtree,
        }
    }
}

/// An origin-tracked place: a stable owner and its projection path.
#[format("`owner ` $root ` [` vec($path, CharSpace(`,`)) `]`")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CorePlacePath {
    pub root: u32,
    pub path: Vec<CoreSeg>,
}

impl CorePlacePath {
    pub fn from_place(place: &OriginPlace) -> Self {
        Self {
            root: place.root.0,
            path: place.path.iter().map(CoreSeg::from_seg).collect(),
        }
    }

    pub fn place(&self) -> OriginPlace {
        OriginPlace {
            root: OwnerId(self.root),
            path: self.path.iter().map(CoreSeg::seg).collect(),
        }
    }
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum CoreOrigin {
    Param(u32),
    SelfParam,
    Place(CorePlacePath),
    Static,
    Untracked(bool),
    Unbound,
    #[format("`[` vec($0, CharSpace(`,`)) `]`")]
    Union(Vec<Self>),
}

impl CoreOrigin {
    pub fn from_origin(origin: &Origin) -> Result<Self, A1Error> {
        match origin {
            Origin::Param(id) => Ok(Self::Param(id.0)),
            Origin::SelfParam => Ok(Self::SelfParam),
            Origin::Place(place) => Ok(Self::Place(CorePlacePath::from_place(place))),
            Origin::Static => Ok(Self::Static),
            Origin::Untracked { mutable } => Ok(Self::Untracked(*mutable)),
            Origin::Unbound => Ok(Self::Unbound),
            Origin::Union(origins) => Ok(Self::Union(
                origins
                    .iter()
                    .map(Self::from_origin)
                    .collect::<Result<Vec<_>, _>>()?,
            )),
        }
    }

    pub fn origin(&self) -> Origin {
        match self {
            Self::Param(id) => Origin::Param(OriginParamId(*id)),
            Self::SelfParam => Origin::SelfParam,
            Self::Place(place) => Origin::Place(place.place()),
            Self::Static => Origin::Static,
            Self::Untracked(mutable) => Origin::Untracked { mutable: *mutable },
            Self::Unbound => Origin::Unbound,
            Self::Union(origins) => Origin::Union(origins.iter().map(Self::origin).collect()),
        }
    }
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum CoreMutability {
    Immutable,
    Mutable,
    Param(u32),
}

impl CoreMutability {
    pub const fn from_mutability(mutability: Mutability) -> Self {
        match mutability {
            Mutability::Immutable => Self::Immutable,
            Mutability::Mutable => Self::Mutable,
            Mutability::Param(id) => Self::Param(id.0),
        }
    }

    pub const fn mutability(self) -> Mutability {
        match self {
            Self::Immutable => Mutability::Immutable,
            Self::Mutable => Mutability::Mutable,
            Self::Param(id) => Mutability::Param(OriginParamId(id)),
        }
    }
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum CorePointerOrigin {
    #[format("`<` $place `, ` $mutable `>`")]
    Place {
        place: CorePlacePath,
        mutable: bool,
    },
    /// An origin binder of the enclosing declaration, which
    /// specialization leaves symbolic.
    #[format(
        "`<param ` $id `, ` $mutability `, [` vec($interior, CharSpace(`,`)) `], ` $subtree `>`"
    )]
    Param {
        id: u32,
        mutability: CoreMutability,
        interior: Vec<Text>,
        subtree: bool,
    },
    /// The receiver's own place, symbolic until a call site rebases it.
    #[format("`<self ` $mutability `, [` vec($interior, CharSpace(`,`)) `], ` $subtree `>`")]
    SelfPlace {
        mutability: CoreMutability,
        interior: Vec<Text>,
        subtree: bool,
    },
    Static,
    Untracked(bool),
    UnsafeAny(bool),
}

impl CorePointerOrigin {
    pub fn from_origin(origin: &PointerOrigin) -> Result<Self, A1Error> {
        match origin {
            PointerOrigin::Place { place, mutable } => Ok(Self::Place {
                place: CorePlacePath::from_place(place),
                mutable: *mutable,
            }),
            PointerOrigin::Static => Ok(Self::Static),
            PointerOrigin::Untracked { mutable } => Ok(Self::Untracked(*mutable)),
            PointerOrigin::UnsafeAny { mutable } => Ok(Self::UnsafeAny(*mutable)),
            PointerOrigin::Param {
                id,
                mutability,
                interior,
                subtree,
            } => Ok(Self::Param {
                id: id.0,
                mutability: CoreMutability::from_mutability(*mutability),
                interior: interior.iter().map(Text::from).collect(),
                subtree: *subtree,
            }),
            PointerOrigin::SelfPlace {
                mutability,
                interior,
                subtree,
            } => Ok(Self::SelfPlace {
                mutability: CoreMutability::from_mutability(*mutability),
                interior: interior.iter().map(Text::from).collect(),
                subtree: *subtree,
            }),
        }
    }

    pub fn origin(&self) -> PointerOrigin {
        match self {
            Self::Place { place, mutable } => PointerOrigin::Place {
                place: place.place(),
                mutable: *mutable,
            },
            Self::Param {
                id,
                mutability,
                interior,
                subtree,
            } => PointerOrigin::Param {
                id: OriginParamId(*id),
                mutability: mutability.mutability(),
                interior: interior.iter().map(|tag| tag.0.clone()).collect(),
                subtree: *subtree,
            },
            Self::SelfPlace {
                mutability,
                interior,
                subtree,
            } => PointerOrigin::SelfPlace {
                mutability: mutability.mutability(),
                interior: interior.iter().map(|tag| tag.0.clone()).collect(),
                subtree: *subtree,
            },
            Self::Static => PointerOrigin::Static,
            Self::Untracked(mutable) => PointerOrigin::Untracked { mutable: *mutable },
            Self::UnsafeAny(mutable) => PointerOrigin::UnsafeAny { mutable: *mutable },
        }
    }
}

/// The core type of a checked type, or the rejection of a type outside the
/// vocabulary.
pub fn import_type(ctx: &mut Context, ty: &Ty) -> Result<TypeHandle, A1Error> {
    Ok(match ty {
        Ty::Int => IntType::get(ctx).into(),
        Ty::UInt => UIntType::get(ctx).into(),
        Ty::Bool => BoolType::get(ctx).into(),
        Ty::None => NoneType::get(ctx).into(),
        Ty::IntLiteral => IntLiteralType::get(ctx).into(),
        Ty::StringLiteral => StringLiteralType::get(ctx).into(),
        Ty::Error => ErrorType::get(ctx).into(),
        Ty::Float64 => Float64Type::get(ctx).into(),
        Ty::FloatLiteral => FloatLiteralType::get(ctx).into(),
        Ty::Dtype => DtypeType::get(ctx).into(),
        Ty::Simd {
            dtype: SimdDtype::Known(dtype),
            width: SimdWidth::Known(width),
        } => {
            let width = u64::try_from(*width)
                .map_err(|_| A1Error::unsupported_type("a negative SIMD width"))?;
            SimdType::get(ctx, CoreDtype::from_dtype(*dtype), width).into()
        }
        Ty::Struct(name, arguments) => {
            let args = arguments
                .iter()
                .map(|argument| import_arg(ctx, argument))
                .collect::<Result<Vec<_>, _>>()?;
            NominalType::get(ctx, name.into(), args).into()
        }
        Ty::Func {
            environment,
            params,
            names,
            ret,
            required,
            variadic,
            kw_variadic,
            positional_only,
            keyword_only,
            raises,
            error,
            conventions,
            ref_params,
            ref_return,
            transfers,
        } => {
            let aligned = [
                names.len(),
                required.len(),
                conventions.len(),
                ref_params.len(),
            ]
            .iter()
            .all(|length| *length == params.len());
            if !aligned {
                return Err(A1Error::unsupported_type(
                    "a function type whose parameter tables misalign",
                ));
            }
            let params = (0..params.len())
                .map(|index| {
                    Ok(CoreFuncParam {
                        name: Text::from(&names[index]),
                        ty: import_type(ctx, &params[index])?,
                        required: required[index],
                        convention: conventions[index].map(CoreConvention::from_convention),
                        ref_sig: ref_params[index]
                            .as_ref()
                            .map(CoreRefSig::from_sig)
                            .transpose()?,
                    })
                })
                .collect::<Result<Vec<_>, A1Error>>()?;
            let environment = CoreEnvironment::from_environment(environment)?;
            let ret = import_type(ctx, ret)?;
            let variadic = variadic
                .as_deref()
                .map(|ty| import_type(ctx, ty))
                .transpose()?;
            let kw_variadic = kw_variadic
                .as_deref()
                .map(|ty| import_type(ctx, ty))
                .transpose()?;
            let error = error
                .as_deref()
                .map(|ty| import_type(ctx, ty))
                .transpose()?;
            let ref_return = ref_return
                .as_deref()
                .map(CoreRefSig::from_sig)
                .transpose()?;
            let transfers = transfers
                .0
                .iter()
                .map(CoreTransfer::from_effect)
                .collect::<Result<Vec<_>, _>>()?;
            FuncType::get(
                ctx,
                environment,
                CoreFuncSignature {
                    params,
                    ret,
                    variadic,
                    kw_variadic,
                    positional_only: positional_only.map(|index| index as u64),
                    keyword_only: keyword_only.map(|index| index as u64),
                },
                CoreFuncEffects {
                    raises: *raises,
                    error,
                    ref_return,
                    transfers,
                },
            )
            .into()
        }
        Ty::Ref(reference) => {
            let referent = import_type(ctx, &reference.referent)?;
            let origin = CoreOrigin::from_origin(&reference.origin)?;
            let mutability = CoreMutability::from_mutability(reference.mutability);
            RefType::get(ctx, referent, origin, mutability).into()
        }
        Ty::Pointer { element, origin } => {
            let element = import_type(ctx, element)?;
            let origin = CorePointerOrigin::from_origin(origin)?;
            PointerType::get(ctx, element, origin).into()
        }
        Ty::Tuple(elements) => {
            let elements = elements
                .iter()
                .map(|element| import_type(ctx, element))
                .collect::<Result<Vec<_>, _>>()?;
            TupleType::get(ctx, elements).into()
        }
        Ty::RuntimePack(elements) => {
            let elements = elements
                .iter()
                .map(|element| import_type(ctx, element))
                .collect::<Result<Vec<_>, _>>()?;
            RuntimePackType::get(ctx, elements).into()
        }
        Ty::Variant(alternatives) => {
            let alternatives = alternatives
                .iter()
                .map(|alternative| import_type(ctx, alternative))
                .collect::<Result<Vec<_>, _>>()?;
            VariantType::get(ctx, alternatives).into()
        }
        Ty::Param {
            binder,
            bounds,
            callable_bound: None,
        } => {
            let bounds = bounds.iter().map(Text::from).collect();
            ParamType::get(ctx, PayloadBinder::from_ref(binder), bounds).into()
        }
        other => return Err(A1Error::unsupported_type(format!("`{other:?}`"))),
    })
}

/// The core spelling of one type argument.
pub fn import_arg(ctx: &mut Context, argument: &TyArg) -> Result<CoreArg, A1Error> {
    match argument {
        TyArg::Ty(ty) => import_type(ctx, ty).map(CoreArg::Type),
        TyArg::Origin(origin) => CoreOrigin::from_origin(origin).map(CoreArg::Origin),
        TyArg::Val(value) => import_constant(ctx, value).map(CoreArg::Value),
    }
}

/// The checked type argument a core argument stands for.
pub fn export_arg(ctx: &Context, argument: &CoreArg) -> Result<TyArg, A1Error> {
    match argument {
        CoreArg::Type(ty) => export_type(ctx, *ty).map(TyArg::Ty),
        CoreArg::Origin(origin) => Ok(TyArg::Origin(origin.origin())),
        CoreArg::Value(value) => export_constant(ctx, *value).map(TyArg::Val),
    }
}

/// The checked type a core value type stands for. Place, effect, and
/// outcome types have none.
pub fn export_type(ctx: &Context, ty: TypeHandle) -> Result<Ty, A1Error> {
    let object = ty.deref(ctx);
    if object.is::<IntType>() {
        return Ok(Ty::Int);
    }
    if object.is::<UIntType>() {
        return Ok(Ty::UInt);
    }
    if object.is::<BoolType>() {
        return Ok(Ty::Bool);
    }
    if object.is::<NoneType>() {
        return Ok(Ty::None);
    }
    if object.is::<IntLiteralType>() {
        return Ok(Ty::IntLiteral);
    }
    if object.is::<StringLiteralType>() {
        return Ok(Ty::StringLiteral);
    }
    if object.is::<ErrorType>() {
        return Ok(Ty::Error);
    }
    if object.is::<Float64Type>() {
        return Ok(Ty::Float64);
    }
    if object.is::<FloatLiteralType>() {
        return Ok(Ty::FloatLiteral);
    }
    if object.is::<DtypeType>() {
        return Ok(Ty::Dtype);
    }
    if let Some(simd) = object.downcast_ref::<SimdType>() {
        let width = i64::try_from(simd.width)
            .map_err(|_| A1Error::unsupported_type("a SIMD width beyond Int"))?;
        return Ok(Ty::Simd {
            dtype: SimdDtype::Known(simd.dtype.dtype()),
            width: SimdWidth::Known(width),
        });
    }
    if let Some(nominal) = object.downcast_ref::<NominalType>() {
        let arguments = nominal
            .args
            .iter()
            .map(|argument| export_arg(ctx, argument))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Ty::Struct(nominal.name.0.clone(), arguments));
    }
    if let Some(func) = object.downcast_ref::<FuncType>() {
        let (signature, effects) = (&func.signature, &func.effects);
        let index = |value: Option<u64>| {
            value
                .map(|value| {
                    usize::try_from(value)
                        .map_err(|_| A1Error::unsupported_type("a parameter marker beyond usize"))
                })
                .transpose()
        };
        let boxed =
            |ty: Option<TypeHandle>| ty.map(|ty| export_type(ctx, ty).map(Box::new)).transpose();
        return Ok(Ty::Func {
            environment: func.environment.environment(),
            params: signature
                .params
                .iter()
                .map(|param| export_type(ctx, param.ty))
                .collect::<Result<Vec<_>, _>>()?,
            names: signature
                .params
                .iter()
                .map(|param| param.name.0.clone())
                .collect(),
            ret: Box::new(export_type(ctx, signature.ret)?),
            required: signature
                .params
                .iter()
                .map(|param| param.required)
                .collect(),
            variadic: boxed(signature.variadic)?,
            kw_variadic: boxed(signature.kw_variadic)?,
            positional_only: index(signature.positional_only)?,
            keyword_only: index(signature.keyword_only)?,
            raises: effects.raises,
            error: boxed(effects.error)?,
            conventions: signature
                .params
                .iter()
                .map(|param| param.convention.map(CoreConvention::convention))
                .collect(),
            ref_params: Box::new(
                signature
                    .params
                    .iter()
                    .map(|param| param.ref_sig.as_ref().map(CoreRefSig::sig).transpose())
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            ref_return: effects
                .ref_return
                .as_ref()
                .map(|sig| sig.sig().map(Box::new))
                .transpose()?,
            transfers: TransferSet(
                effects
                    .transfers
                    .iter()
                    .map(CoreTransfer::effect)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
        });
    }
    if let Some(reference) = object.downcast_ref::<RefType>() {
        return Ok(Ty::Ref(RefTy {
            referent: Box::new(export_type(ctx, reference.referent)?),
            origin: reference.origin.origin(),
            mutability: reference.mutability.mutability(),
        }));
    }
    if let Some(tuple) = object.downcast_ref::<TupleType>() {
        let elements = tuple
            .elements
            .iter()
            .map(|element| export_type(ctx, *element))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Ty::Tuple(elements));
    }
    if let Some(variant) = object.downcast_ref::<VariantType>() {
        let alternatives = variant
            .alternatives
            .iter()
            .map(|alternative| export_type(ctx, *alternative))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Ty::Variant(alternatives));
    }
    if let Some(pack) = object.downcast_ref::<RuntimePackType>() {
        let elements = pack
            .elements
            .iter()
            .map(|element| export_type(ctx, *element))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Ty::RuntimePack(elements));
    }
    if let Some(param) = object.downcast_ref::<ParamType>() {
        return Ok(Ty::Param {
            binder: param.binder.reference()?,
            bounds: param.bounds.iter().map(|bound| bound.0.clone()).collect(),
            callable_bound: None,
        });
    }
    if let Some(pointer) = object.downcast_ref::<PointerType>() {
        return Ok(Ty::Pointer {
            element: Box::new(export_type(ctx, pointer.element)?),
            origin: pointer.origin.origin(),
        });
    }
    Err(A1Error::unsupported_type(
        "a place, effect, or outcome type where a value type is required",
    ))
}

/// A function value's type.
///
/// Its environment, its signature, and the effects a call through it
/// replays. `transfers` are part of the identity, as the v1 text prints
/// them, where checked types ignore them: two checked-equal function types
/// may be two handles.
#[pliron_type(
    name = "mojito.func",
    generate_get = true,
    format = "`<` $environment ` ` $signature ` ` $effects `>`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct FuncType {
    pub environment: CoreEnvironment,
    pub signature: CoreFuncSignature,
    pub effects: CoreFuncEffects,
}

/// A function type's parameters, result, collectors, and markers.
#[format(
    "`(` vec($params, CharSpace(`,`)) `) -> ` $ret ` variadic` opt($variadic, delimiters(`(`, `)`)) ` kw_variadic` opt($kw_variadic, delimiters(`(`, `)`)) ` positional_only` opt($positional_only, delimiters(`(`, `)`)) ` keyword_only` opt($keyword_only, delimiters(`(`, `)`))"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreFuncSignature {
    pub params: Vec<CoreFuncParam>,
    pub ret: TypeHandle,
    pub variadic: Option<TypeHandle>,
    pub kw_variadic: Option<TypeHandle>,
    pub positional_only: Option<u64>,
    pub keyword_only: Option<u64>,
}

/// What a call through a function type may raise, return by reference,
/// and transfer.
#[format(
    "`raises ` $raises ` error` opt($error, delimiters(`(`, `)`)) ` ref_return` opt($ref_return, delimiters(`(`, `)`)) ` transfers [` vec($transfers, CharSpace(`,`)) `]`"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreFuncEffects {
    pub raises: bool,
    pub error: Option<TypeHandle>,
    pub ref_return: Option<CoreRefSig>,
    pub transfers: Vec<CoreTransfer>,
}

/// One regular parameter of a function type.
#[format(
    "$name ` : ` $ty ` ` $required ` convention` opt($convention, delimiters(`(`, `)`)) ` ref` opt($ref_sig, delimiters(`(`, `)`))"
)]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreFuncParam {
    pub name: Text,
    pub ty: TypeHandle,
    pub required: bool,
    pub convention: Option<CoreConvention>,
    pub ref_sig: Option<CoreRefSig>,
}

/// The environment a callable closes over: none, a thin one, or a capture
/// set, inferred, bound to a binder, or concrete.
#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum CoreEnvironment {
    Default,
    Thin,
    Capturing(CoreCaptureSet),
}

impl CoreEnvironment {
    pub fn from_environment(environment: &CallableEnvironment) -> Result<Self, A1Error> {
        Ok(match environment {
            CallableEnvironment::Default => Self::Default,
            CallableEnvironment::Thin => Self::Thin,
            CallableEnvironment::Capturing(set) => Self::Capturing(match set {
                CaptureOriginSet::Infer => CoreCaptureSet::Infer,
                CaptureOriginSet::Param(id) => CoreCaptureSet::Param(id.0),
                CaptureOriginSet::Concrete(captures) => CoreCaptureSet::Concrete(
                    captures
                        .iter()
                        .map(|capture| {
                            Ok(CoreCapture {
                                origin: CoreOrigin::from_origin(&capture.origin)?,
                                write: capture.access == mojito_types::origin::CaptureAccess::Write,
                            })
                        })
                        .collect::<Result<Vec<_>, A1Error>>()?,
                ),
            }),
        })
    }

    pub fn environment(&self) -> CallableEnvironment {
        match self {
            Self::Default => CallableEnvironment::Default,
            Self::Thin => CallableEnvironment::Thin,
            Self::Capturing(set) => CallableEnvironment::Capturing(match set {
                CoreCaptureSet::Infer => CaptureOriginSet::Infer,
                CoreCaptureSet::Param(id) => CaptureOriginSet::Param(CaptureSetParamId(*id)),
                CoreCaptureSet::Concrete(captures) => CaptureOriginSet::Concrete(
                    captures
                        .iter()
                        .map(|capture| CaptureOrigin {
                            origin: capture.origin.origin(),
                            access: if capture.write {
                                mojito_types::origin::CaptureAccess::Write
                            } else {
                                mojito_types::origin::CaptureAccess::Read
                            },
                        })
                        .collect(),
                ),
            }),
        }
    }
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum CoreCaptureSet {
    Infer,
    Param(u32),
    #[format("`[` vec($0, CharSpace(`,`)) `]`")]
    Concrete(Vec<CoreCapture>),
}

/// One concrete dependency of a capturing environment.
#[format("$origin ` ` $write")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreCapture {
    pub origin: CoreOrigin,
    pub write: bool,
}

/// An origin as a callable signature names it: relative to the receiver
/// or a parameter slot, bound to a concrete origin, static, or untracked.
#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub enum CoreSigOrigin {
    Receiver,
    Param(u64),
    Bound(CoreOrigin),
    Static,
    Untracked(bool),
    UnsafeAny(bool),
    /// A projection of one base origin, held in a one-element list as the
    /// text format has no box.
    #[format("`(` vec($0, CharSpace(`,`)) ` [` vec($1, CharSpace(`,`)) `])`")]
    Projected(Vec<Self>, Vec<CoreSeg>),
    #[format("`[` vec($0, CharSpace(`,`)) `]`")]
    Union(Vec<Self>),
    Infer,
}

impl CoreSigOrigin {
    pub fn from_origin(origin: &SigOrigin) -> Result<Self, A1Error> {
        Ok(match origin {
            SigOrigin::Self_ => Self::Receiver,
            SigOrigin::Param(index) => Self::Param(*index as u64),
            SigOrigin::Bound(origin) => Self::Bound(CoreOrigin::from_origin(origin)?),
            SigOrigin::Static => Self::Static,
            SigOrigin::Untracked { mutable } => Self::Untracked(*mutable),
            SigOrigin::UnsafeAny { mutable } => Self::UnsafeAny(*mutable),
            SigOrigin::Projected(base, path) => Self::Projected(
                vec![Self::from_origin(base)?],
                path.iter().map(CoreSeg::from_seg).collect(),
            ),
            SigOrigin::Union(origins) => Self::Union(
                origins
                    .iter()
                    .map(Self::from_origin)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            SigOrigin::Infer => Self::Infer,
        })
    }

    pub fn origin(&self) -> Result<SigOrigin, A1Error> {
        Ok(match self {
            Self::Receiver => SigOrigin::Self_,
            Self::Param(index) => SigOrigin::Param(usize::try_from(*index).map_err(|_| {
                A1Error::new(super::A1ErrorKind::Export, "a parameter index beyond usize")
            })?),
            Self::Bound(origin) => SigOrigin::Bound(origin.origin()),
            Self::Static => SigOrigin::Static,
            Self::Untracked(mutable) => SigOrigin::Untracked { mutable: *mutable },
            Self::UnsafeAny(mutable) => SigOrigin::UnsafeAny { mutable: *mutable },
            Self::Projected(base, path) => {
                let [base] = base.as_slice() else {
                    return Err(A1Error::new(
                        super::A1ErrorKind::Export,
                        "a projected signature origin has one base",
                    ));
                };
                SigOrigin::Projected(
                    Box::new(base.origin()?),
                    path.iter().map(CoreSeg::seg).collect(),
                )
            }
            Self::Union(origins) => SigOrigin::Union(
                origins
                    .iter()
                    .map(Self::origin)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Self::Infer => SigOrigin::Infer,
        })
    }
}

#[format]
#[derive(Hash, PartialEq, Eq, Debug, Clone, Copy)]
pub enum CoreSigMutability {
    Immutable,
    Mutable,
    BoolParam(u64),
    Infer,
}

impl CoreSigMutability {
    pub const fn from_mutability(mutability: &SigMutability) -> Self {
        match *mutability {
            SigMutability::Immutable => Self::Immutable,
            SigMutability::Mutable => Self::Mutable,
            SigMutability::BoolParam(index) => Self::BoolParam(index as u64),
            SigMutability::Infer => Self::Infer,
        }
    }

    pub fn mutability(self) -> Result<SigMutability, A1Error> {
        Ok(match self {
            Self::Immutable => SigMutability::Immutable,
            Self::Mutable => SigMutability::Mutable,
            Self::BoolParam(index) => {
                SigMutability::BoolParam(usize::try_from(index).map_err(|_| {
                    A1Error::new(super::A1ErrorKind::Export, "a binder index beyond usize")
                })?)
            }
            Self::Infer => SigMutability::Infer,
        })
    }
}

/// A reference contract a callable signature retains for a parameter or
/// its result.
#[format("$origin ` ` $mutability")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreRefSig {
    pub origin: CoreSigOrigin,
    pub mutability: CoreSigMutability,
}

impl CoreRefSig {
    pub fn from_sig(sig: &RefSig) -> Result<Self, A1Error> {
        Ok(Self {
            origin: CoreSigOrigin::from_origin(&sig.origin)?,
            mutability: CoreSigMutability::from_mutability(&sig.mutability),
        })
    }

    pub fn sig(&self) -> Result<RefSig, A1Error> {
        Ok(RefSig {
            origin: self.origin.origin()?,
            mutability: self.mutability.mutability()?,
        })
    }
}

/// One inferred transfer effect a call through the function replays.
#[format("$dest ` <- ` $src ` ` $src_is_place ` ` $mutable")]
#[derive(Hash, PartialEq, Eq, Debug, Clone)]
pub struct CoreTransfer {
    pub dest: CoreSigOrigin,
    pub src: CoreSigOrigin,
    pub src_is_place: bool,
    pub mutable: bool,
}

impl CoreTransfer {
    pub fn from_effect(effect: &TransferEffect) -> Result<Self, A1Error> {
        Ok(Self {
            dest: CoreSigOrigin::from_origin(&effect.dest)?,
            src: CoreSigOrigin::from_origin(&effect.src)?,
            src_is_place: effect.src_is_place,
            mutable: effect.mutable,
        })
    }

    pub fn effect(&self) -> Result<TransferEffect, A1Error> {
        Ok(TransferEffect {
            dest: self.dest.origin()?,
            src: self.src.origin()?,
            src_is_place: self.src_is_place,
            mutable: self.mutable,
        })
    }
}

/// The scalar type a one-lane vector of `dtype` is spelled as when the
/// checker names it by its alias (`Int` for `Scalar[DType.int]`), or none
/// when the lane type has no alias.
pub fn scalar_alias(ctx: &mut Context, dtype: CoreDtype) -> Option<TypeHandle> {
    Some(match dtype {
        CoreDtype::Int => IntType::get(ctx).into(),
        CoreDtype::Bool => BoolType::get(ctx).into(),
        CoreDtype::Float64 => Float64Type::get(ctx).into(),
        _ => return None,
    })
}

/// The type of `value`'s place target, when `value` is a place.
pub fn place_target(ctx: &Context, value: pliron::value::Value) -> Option<TypeHandle> {
    value
        .get_type(ctx)
        .deref(ctx)
        .downcast_ref::<PlaceType>()
        .map(|place| place.target)
}

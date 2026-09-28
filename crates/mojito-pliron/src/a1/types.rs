//! The closed type vocabulary of `mojito_core`, and its correspondence with
//! checked [`Ty`]. A type outside the vocabulary is rejected, never carried
//! opaquely.

use pliron::context::Context;
use pliron::derive::{format, pliron_type};
use pliron::r#type::{TypeHandle, Typed};

use mojito_ast::ast::Dtype;
use mojito_types::origin::{
    Mutability, Origin, OriginParamId, OriginPlace, OriginSeg, OwnerId, PointerOrigin, RefTy,
};
use mojito_types::types::{SimdDtype, SimdWidth, Ty, TyArg};

use super::A1Error;
use super::attrs::Text;
use super::params::{NodeKey, PayloadBinder, export_constant, import_constant};

#[pliron_type(
    name = "mojito_core.int",
    generate_get = true,
    format,
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct IntType;

#[pliron_type(
    name = "mojito_core.uint",
    generate_get = true,
    format,
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct UIntType;

#[pliron_type(
    name = "mojito_core.bool",
    generate_get = true,
    format,
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct BoolType;

#[pliron_type(
    name = "mojito_core.none",
    generate_get = true,
    format,
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct NoneType;

/// An exact, arbitrary-precision integer literal before materialization.
#[pliron_type(
    name = "mojito_core.int_literal",
    generate_get = true,
    format,
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct IntLiteralType;

#[pliron_type(
    name = "mojito_core.string_literal",
    generate_get = true,
    format,
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct StringLiteralType;

#[pliron_type(
    name = "mojito_core.error",
    generate_get = true,
    format,
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct ErrorType;

/// The sequencing token threaded through every effectful operation.
#[pliron_type(
    name = "mojito_core.effect",
    generate_get = true,
    format,
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct EffectType;

/// A pending outcome held across a `finally` body.
#[pliron_type(
    name = "mojito_core.outcome",
    generate_get = true,
    format,
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct OutcomeType;

/// A width-`width` vector of `dtype` lanes; width 1 is the scalar alias.
#[pliron_type(
    name = "mojito_core.simd",
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
    name = "mojito_core.nominal",
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
    name = "mojito_core.ref",
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
    name = "mojito_core.pointer",
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
    name = "mojito_core.tuple",
    generate_get = true,
    format = "`<[` vec($elements, CharSpace(`,`)) `]>`",
    verifier = "succ"
)]
#[derive(Hash, PartialEq, Eq, Debug)]
pub struct TupleType {
    pub elements: Vec<TypeHandle>,
}

/// The collector storage of a specialized heterogeneous parameter pack.
#[pliron_type(
    name = "mojito_core.runtime_pack",
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
    name = "mojito_core.param",
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
    name = "mojito_core.place",
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
            Origin::Union(_) => Err(A1Error::unsupported_type("a union origin")),
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
                .map(|argument| match argument {
                    TyArg::Ty(ty) => import_type(ctx, ty).map(CoreArg::Type),
                    TyArg::Origin(origin) => CoreOrigin::from_origin(origin).map(CoreArg::Origin),
                    TyArg::Val(value) => import_constant(ctx, value).map(CoreArg::Value),
                })
                .collect::<Result<Vec<_>, _>>()?;
            NominalType::get(ctx, name.into(), args).into()
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
            .map(|argument| match argument {
                CoreArg::Type(ty) => export_type(ctx, *ty).map(TyArg::Ty),
                CoreArg::Origin(origin) => Ok(TyArg::Origin(origin.origin())),
                CoreArg::Value(value) => export_constant(ctx, *value).map(TyArg::Val),
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Ty::Struct(nominal.name.0.clone(), arguments));
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

/// The type of `value`'s place target, when `value` is a place.
pub fn place_target(ctx: &Context, value: pliron::value::Value) -> Option<TypeHandle> {
    value
        .get_type(ctx)
        .deref(ctx)
        .downcast_ref::<PlaceType>()
        .map(|place| place.target)
}

//! Shared semantic type representation.
//!
//! This is the type lattice used by the checker, but it also needs to be visible
//! to compile-time values once comptime can carry type values. Keeping `Ty` out
//! of `checker.rs` lets [`CtValue`](crate::ct::CtValue) represent `Type(Ty)`
//! without making the checker the owner of all type-level facts.

use std::collections::HashMap;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::sync::{Arc, LazyLock};

use crate::ct::CtValue;
use crate::param_expr::{
    HoleKind, MetaTy, ParamBindings, ParamContext, ParamError, ParamExpr, ParamId, ParamKind,
    ParamRef,
};
use mojito_ast::ast::{ArgConvention, Dtype};
use mojito_common::literal::{FloatLiteral, IntLiteral};

/// A loan-transfer effect inferred from a callable's body: an accepted store
/// into an outliving destination (`self` or a parameter) whose loan roots at
/// another parameter or `self`.
///
/// Call sites replay the effect against their actuals, installing the
/// caller-side loan the callee's store implies.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TransferEffect {
    pub dest: crate::origin::SigOrigin,
    pub src: crate::origin::SigOrigin,
    /// Whether the loan roots at the source parameter's own (borrowed)
    /// storage — a `mut`/`ref` actual's place is loaned at the call — as
    /// opposed to loans merely carried by an owned value moving through.
    pub src_is_place: bool,
    pub mutable: bool,
}

/// Inferred transfer effects riding a checked function type, so a call through
/// a function-typed VALUE replays the effects of the `def` the value came
/// from.
///
/// Transparent to type identity: two otherwise-equal function types never
/// differ by their inferred effects, and acceptance/coercion must not consult
/// them — a `def(...)` contract cannot spell effects (Mojo has no such
/// syntax), so soundness comes from call-site replay off the value's type,
/// never from acceptance filtering.
#[derive(Debug, Clone, Default, Eq, serde::Serialize, serde::Deserialize)]
pub struct TransferSet(pub Vec<TransferEffect>);

impl TransferSet {
    /// Iterate the canonical transfer effects retained by a callable type.
    pub fn iter(&self) -> impl Iterator<Item = &TransferEffect> {
        self.0.iter()
    }
}

impl std::hash::Hash for TransferSet {
    /// Hashes nothing, matching the always-equal `PartialEq` below: a type's
    /// hash must not see metadata its identity ignores.
    fn hash<H: std::hash::Hasher>(&self, _state: &mut H) {}
}

impl PartialEq for TransferSet {
    /// Always equal BY DESIGN: the set is metadata on the type, not part of
    /// its identity. See the type-level comment before relying on `==`.
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

/// A checked type expression denoted by a Type-meta-type parameter
/// expression.
///
/// The expression stays structural semantic data — no phase encodes or
/// recovers it from a synthesized name — and folds away to the [`Ty`] it
/// denotes once substitution closes it ([`DependentType::resolve`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum DependentType {
    /// A type-valued parameter expression: a finite type selection
    /// (`ParamKind::Select`), one element of a parameter list that is still
    /// a parameter (`ParamKind::ListGet` — a variadic pack, or the field
    /// types of a reflected symbolic type), or a reflected field type
    /// (`ParamKind::Reflect`).
    Parameter(ParamExpr),
}

impl DependentType {
    /// The type `expr` denotes: the contained type when it is a closed Type
    /// constant, the trait type of a trait constant (a trait is a type in
    /// KGEN), and the symbolic wrapper otherwise.
    pub fn resolve(expr: ParamExpr) -> Ty {
        match expr.as_constant() {
            Some(CtValue::Type(ty)) => (*ty).clone(),
            Some(CtValue::Trait(traits)) => TyKind::Trait(traits.clone()).into(),
            _ => match expr.kind() {
                ParamKind::TypeShape(ty) => ty.clone(),
                _ => TyKind::Dependent(Self::Parameter(expr)).into(),
            },
        }
    }

    /// The finite selection this type is, when it is one.
    pub fn selection(&self) -> Option<(&[Ty], &ParamExpr)> {
        let Self::Parameter(expr) = self;
        match expr.kind() {
            ParamKind::Select { elements, index } => Some((elements, index)),
            _ => None,
        }
    }

    /// The pack and index this type is an element of, when the pack is
    /// still a parameter.
    pub fn pack_element(&self) -> Option<(&ParamExpr, &ParamExpr)> {
        let Self::Parameter(expr) = self;
        match expr.kind() {
            ParamKind::ListGet { list, index } => Some((list, index)),
            _ => None,
        }
    }

    pub const fn expr(&self) -> &ParamExpr {
        let Self::Parameter(expr) = self;
        expr
    }

    /// Rebuild this type with `f` applied to each candidate of a finite
    /// selection. Any other expression holds no candidate types.
    #[must_use]
    pub fn map_types(&self, f: impl FnMut(&Ty) -> Ty) -> Ty {
        match self.selection() {
            Some((elements, index)) => ParamContext::detached()
                .select(elements.iter().map(f).collect(), index)
                .map_or_else(|_| TyKind::Dependent(self.clone()).into(), Self::resolve),
            None => TyKind::Dependent(self.clone()).into(),
        }
    }
}

/// The element type of a `SIMD` application: a concrete dtype, or a `DType`-valued
/// parameter expression while the enclosing declaration is still symbolic
/// (`Scalar[dt]` under source validation).
///
/// `Expr` is never a closed expression: [`simd_ty_from_slots`] folds one to
/// `Known`, so slot equality is type identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SimdDtype {
    Known(Dtype),
    Expr(ParamExpr),
}

impl SimdDtype {
    /// The dtype a concrete use reads: a known slot's, or the value an
    /// evaluated application in the slot established.
    pub fn known(&self) -> Option<Dtype> {
        match self {
            Self::Known(dtype) => Some(*dtype),
            Self::Expr(expr) => expr
                .require_constant()
                .ok()
                .as_ref()
                .and_then(dtype_of_value),
        }
    }

    pub const fn is_expr(&self) -> bool {
        matches!(self, Self::Expr(_))
    }

    /// Whether only an instantiation can say which dtype this is: an
    /// expression no evaluation has closed.
    pub fn is_symbolic(&self) -> bool {
        self.is_expr() && self.known().is_none()
    }

    /// Whether a dtype constraint holds here: a known dtype answers
    /// `predicate`; a symbolic one is licensed, since the constraint is the
    /// instantiation's to check (upstream defers `constrained[...]` the same
    /// way).
    pub fn licenses(&self, predicate: impl FnOnce(Dtype) -> bool) -> bool {
        self.known().is_none_or(predicate)
    }
}

impl fmt::Display for SimdDtype {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Known(dtype) => write!(f, "DType.{}", dtype.name()),
            Self::Expr(expr) => write!(f, "{expr}"),
        }
    }
}

/// The length of a `SIMD` application: a concrete lane count, or the
/// `SIMDLength` value the slot holds while the declaration is symbolic.
///
/// Upstream's slot is `SIMD[dtype, length: SIMDLength]`. The value is
/// a `SIMDLength` binder (`w` of `SIMD[dt, w]`), or a struct value spelled by
/// the construction that converted the width (`SIMDLength(n * 2)` for
/// `SIMD[dt, n * 2]`): its fields are its identity and its spelling what a
/// binder is solved from. Its lane count is its [`simd_length_image`]
/// ([`SimdWidth::lanes`]), which the slot displays.
///
/// The `SIMD[dt, _]` inference wildcard is an unbound typed hole, so it is
/// told apart from every length a program can name and from a binder the
/// instantiation still owes a value. `Expr` never holds a length whose lane
/// count is closed: [`SimdWidth::of_length`] makes one `Known`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SimdWidth {
    Known(i64),
    Expr(ParamExpr),
}

impl SimdWidth {
    /// The `SIMD[dt, _]` inference wildcard. The construction that spells it
    /// solves the hole from its argument count, so no later phase sees one.
    pub fn inferred(context: &ParamContext) -> Self {
        Self::Expr(context.hole(HoleKind::Unbound, MetaTy::value(simd_length_ty())))
    }

    /// The slot holding the `SIMDLength` value `length`: `Known` once its
    /// lane count is closed.
    pub fn of_length(context: &ParamContext, length: ParamExpr) -> Result<Self, ParamError> {
        if !length.meta().as_value().is_some_and(is_simd_length) {
            return Err(ParamError::TypeMismatch {
                operation: "SIMD width".to_string(),
                expected: SIMD_LENGTH_TYPE_NAME.to_string(),
                found: length.meta().to_string(),
            });
        }
        Ok(simd_length_image(context, &length)?
            .as_i64()
            .map_or(Self::Expr(length), Self::Known))
    }

    /// The lane count, an `Int`: a known slot's constant, or the image of
    /// the length the slot holds.
    pub fn lanes(&self, context: &ParamContext) -> Result<ParamExpr, ParamError> {
        match self {
            Self::Known(width) => context.constant(CtValue::Int(*width)),
            Self::Expr(length) => simd_length_image(context, length),
        }
    }

    /// The lane count a concrete use reads: a known slot's, or the one an
    /// evaluated application in the slot established.
    pub fn known(&self) -> Option<i64> {
        match self {
            Self::Known(width) => Some(*width),
            Self::Expr(_) => self.lanes(&ParamContext::detached()).ok()?.as_i64(),
        }
    }

    pub const fn is_expr(&self) -> bool {
        matches!(self, Self::Expr(_))
    }

    /// Whether only an instantiation can say how many lanes there are: an
    /// expression no evaluation has closed.
    pub fn is_symbolic(&self) -> bool {
        self.is_expr() && self.known().is_none()
    }

    /// Whether this slot is the [`SimdWidth::inferred`] wildcard.
    pub fn is_inferred(&self) -> bool {
        matches!(self, Self::Expr(expr) if expr.as_hole().is_some())
    }
}

impl fmt::Display for SimdWidth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Known(width) => write!(f, "{width}"),
            Self::Expr(length) => match self.lanes(&ParamContext::detached()) {
                Ok(lanes) => write!(f, "{lanes}"),
                Err(_) => write!(f, "{length}"),
            },
        }
    }
}

/// The size slot of a `!pop.array<size, element>`, an `index`.
///
/// A struct's parameter spells it (`` `!pop.array<`, Self.n.__mlir_index__(),
/// `, `, T, `>` ``) as upstream's `StaticTuple` does. `Expr` never holds a
/// closed size: [`ArraySize::of`] makes one `Known`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ArraySize {
    Known(u64),
    Expr(ParamExpr),
}

impl ArraySize {
    /// The slot holding the `index`-valued `size`: `Known` once it folds.
    pub fn of(size: ParamExpr) -> Result<Self, ParamError> {
        if !size
            .meta()
            .as_value()
            .is_some_and(|ty| matches!(ty.kind(), TyKind::Index))
        {
            return Err(ParamError::TypeMismatch {
                operation: "'!pop.array' size".to_string(),
                expected: MLIR_INDEX_TYPE_SPELLING.to_string(),
                found: size.meta().to_string(),
            });
        }
        match size.as_i64() {
            Some(known) => u64::try_from(known)
                .map(Self::Known)
                .map_err(|_| ParamError::NotConstant(format!("a negative array size {known}"))),
            None => Ok(Self::Expr(size)),
        }
    }

    /// The element count a concrete use reads.
    pub fn known(&self) -> Option<u64> {
        match self {
            Self::Known(size) => Some(*size),
            Self::Expr(size) => size.as_i64().and_then(|size| u64::try_from(size).ok()),
        }
    }

    /// Whether only an instantiation can say how many elements there are.
    pub fn is_symbolic(&self) -> bool {
        self.known().is_none()
    }

    /// The size as an `index`-valued parameter expression.
    pub fn expr(&self, context: &ParamContext) -> Result<ParamExpr, ParamError> {
        match self {
            Self::Known(size) => context.constant(CtValue::Index(*size as i64)),
            Self::Expr(size) => Ok(size.clone()),
        }
    }

    /// This slot after `rewrite` rewrote its expression.
    pub fn try_map(
        &self,
        rewrite: impl FnOnce(&ParamExpr) -> Result<ParamExpr, ParamError>,
    ) -> Result<Self, ParamError> {
        match self {
            Self::Known(_) => Ok(self.clone()),
            Self::Expr(size) => Self::of(rewrite(size)?),
        }
    }
}

impl fmt::Display for ArraySize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Known(size) => write!(f, "{size}"),
            Self::Expr(size) => write!(f, "{size}"),
        }
    }
}

/// Upstream's `SIMDLength`, the type of a `SIMD` width slot.
pub fn simd_length_ty() -> Ty {
    TyKind::Struct(SIMD_LENGTH_TYPE_NAME.to_string(), Vec::new()).into()
}

/// Whether `ty` is upstream's `SIMDLength`.
pub fn is_simd_length(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::Struct(name, args) if name == SIMD_LENGTH_TYPE_NAME && args.is_empty())
}

/// Whether `ty` is upstream's `Error`.
pub fn is_error(ty: &Ty) -> bool {
    ty.kind().is_error()
}

/// Whether `ty` is upstream's `DType`.
pub fn is_dtype(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::Struct(name, args) if name == DTYPE_TYPE_NAME && args.is_empty())
}

/// The closed `DType` value of `dtype`: upstream's `DType.<name>` member,
/// the struct over its one `#kgen.dtype.constant` field.
pub fn dtype_value(dtype: Dtype) -> CtValue {
    CtValue::Struct {
        name: DTYPE_TYPE_NAME.to_string(),
        fields: vec![(
            crate::ct::MLIR_VALUE_FIELD.to_string(),
            CtValue::Dtype(dtype),
        )],
    }
}

/// The dtype a closed `DType` value ([`dtype_value`]) is, or `None` for
/// any other value.
pub fn dtype_of_value(value: &CtValue) -> Option<Dtype> {
    match value {
        CtValue::Struct { name, fields } if name == DTYPE_TYPE_NAME => match fields.as_slice() {
            [(_, CtValue::Dtype(dtype))] => Some(*dtype),
            _ => None,
        },
        _ => None,
    }
}

/// The lane count a `SIMDLength` value is, `Int(mlir_value=length._mlir_value)`:
/// upstream's `SIMD.__init__(SIMDLength)` folded, which is what a SIMD
/// width slot holds. A closed length folds to its `Int`.
pub fn simd_length_image(
    context: &ParamContext,
    length: &ParamExpr,
) -> Result<ParamExpr, ParamError> {
    context.int_of_index(&context.field(
        length,
        crate::ct::MLIR_VALUE_FIELD,
        MetaTy::value(Ty::index()),
    )?)
}

/// The closed `SIMDLength(lanes)`: upstream's `__init__(Int)` folded, its
/// one `index` field the count.
pub fn simd_length_value(lanes: i64) -> CtValue {
    CtValue::Struct {
        name: SIMD_LENGTH_TYPE_NAME.to_string(),
        fields: vec![(
            crate::ct::MLIR_VALUE_FIELD.to_string(),
            CtValue::Index(lanes),
        )],
    }
}

/// The `SIMDLength` upstream's implicit `SIMDLength.__init__(Int)`, the
/// overload `conversion`, converts the lane count `lanes` to: the struct
/// value of its one `index` field, spelled by that application.
pub fn simd_length_converted(
    context: &ParamContext,
    conversion: &str,
    lanes: &ParamExpr,
) -> Result<ParamExpr, ParamError> {
    let spelling = context.apply(
        conversion,
        std::slice::from_ref(lanes),
        MetaTy::value(simd_length_ty()),
    );
    let field = (
        crate::ct::MLIR_VALUE_FIELD.to_string(),
        context.cast_to_builtin(lanes)?,
    );
    Ok(context.struct_value(&[field], &spelling))
}

/// The `SIMDLength` whose lane count `lanes` is, when `lanes` is that
/// length's [`simd_length_image`], the `Int` created from the storage cast
/// from its field: the inverse a width binder is solved by.
pub fn simd_length_of_image(lanes: &ParamExpr) -> Option<&ParamExpr> {
    let ParamKind::StructValue { fields, .. } = lanes.kind() else {
        return None;
    };
    let [(field, storage)] = fields.as_slice() else {
        return None;
    };
    let ParamKind::Op {
        op: crate::param_expr::ParamOp::CastFromBuiltin,
        operands,
    } = storage.kind()
    else {
        return None;
    };
    if field != crate::ct::MLIR_VALUE_FIELD
        || !lanes.meta().as_value().is_some_and(|ty| ty.kind().is_int())
    {
        return None;
    }
    let ParamKind::Field { base, name } = operands.first()?.kind() else {
        return None;
    };
    (name == crate::ct::MLIR_VALUE_FIELD && base.meta().as_value().is_some_and(is_simd_length))
        .then_some(base)
}

/// The alternative a `Variant` storage operation selects: the index
/// attribute upstream's `pop.variant` operations take, computed by
/// `_get_type_index[T, *Ts]()`.
///
/// An operation in a template over the variant's pack, or over a method's
/// own type parameter, keeps that query, which each instantiation closes;
/// one naming no alternative fails the instance, as upstream's
/// `Variant._check[T]()` does. Concrete MIR holds only `Known`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum VariantIndex {
    Known(usize),
    Expr(ParamExpr),
}

impl VariantIndex {
    pub const fn known(&self) -> Option<usize> {
        match self {
            Self::Known(index) => Some(*index),
            Self::Expr(_) => None,
        }
    }

    pub const fn is_symbolic(&self) -> bool {
        matches!(self, Self::Expr(_))
    }

    /// The index with its expression rebuilt through `f`, known once `f`
    /// closes it.
    pub fn try_map_expr<E>(
        &self,
        f: impl FnOnce(&ParamExpr) -> Result<ParamExpr, E>,
    ) -> Result<Self, E> {
        Ok(match self {
            Self::Known(_) => self.clone(),
            Self::Expr(expr) => {
                let expr = f(expr)?;
                match expr.as_i64().and_then(|index| usize::try_from(index).ok()) {
                    Some(index) => Self::Known(index),
                    None => Self::Expr(expr),
                }
            }
        })
    }

    /// Whether two operations may select the same alternative: two
    /// distinct known positions never do, and a symbolic one may fold to
    /// any position (two binders may bind one type, and a pack may repeat
    /// one).
    pub fn may_equal(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Known(left), Self::Known(right)) => left == right,
            _ => true,
        }
    }
}

impl fmt::Display for VariantIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Known(index) => write!(f, "{index}"),
            Self::Expr(expr) => write!(f, "{expr}"),
        }
    }
}

/// A lane gather's mask (`pop.simd.shuffle`'s mask parameter): result lane
/// `i` reads lane `mask[i]` of the source lanes, the first operand's
/// followed by the second's.
///
/// A `pop.simd.shuffle` op's mask a
/// template computes from its parameters (`_shuffle_list`'s, which
/// `shuffle` and `join` reach) is an `Attribute`, which each instance
/// closes and the op's verifier judges. Concrete MIR holds only `Known`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum LaneMask {
    Known(Vec<usize>),
    /// A `pop.simd.shuffle` mask a template computes from its parameters
    /// (`_shuffle_list`'s `mask._mlir_value`), a `!pop.array` attribute
    /// each instance closes ([`Self::attribute_lanes`]).
    Attribute(ParamExpr),
}

impl LaneMask {
    pub fn known(&self) -> Option<&[usize]> {
        match self {
            Self::Known(mask) => Some(mask),
            Self::Attribute(_) => None,
        }
    }

    pub const fn is_symbolic(&self) -> bool {
        !matches!(self, Self::Known(_))
    }

    /// The lanes of the closed `pop.simd.shuffle` mask attribute `value`: a
    /// `#pop.array` of `index` elements, each a single-field struct lowered
    /// to its field down to an `index` (a `StaticTuple[SIMDLength, n]`'s
    /// storage), as the pin lowers a struct attribute before its verifier
    /// reads the mask. `None` when an element lowers to no `index` (an
    /// `Int`, `!pop.scalar<index>` at the pin).
    pub fn attribute_lanes(value: &CtValue) -> Option<Vec<usize>> {
        fn lane(mut element: &CtValue) -> Option<usize> {
            loop {
                match element {
                    CtValue::Index(lane) => return usize::try_from(*lane).ok(),
                    CtValue::Struct { fields, .. } if fields.len() == 1 => {
                        element = &fields[0].1;
                    }
                    _ => return None,
                }
            }
        }
        let CtValue::PopArray { elements, .. } = value else {
            return None;
        };
        elements.iter().map(lane).collect()
    }

    /// The parameter expressions the mask reads.
    pub fn expressions(&self) -> Vec<&ParamExpr> {
        match self {
            Self::Known(_) => Vec::new(),
            Self::Attribute(value) => vec![value],
        }
    }

    /// The mask with each expression rebuilt through `f`.
    pub fn try_map_exprs<E>(
        &self,
        f: &mut impl FnMut(&ParamExpr) -> Result<ParamExpr, E>,
    ) -> Result<Self, E> {
        Ok(match self {
            Self::Known(_) => self.clone(),
            Self::Attribute(value) => Self::Attribute(f(value)?),
        })
    }
}

impl fmt::Display for LaneMask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Known(mask) => write!(f, "{mask:?}"),
            Self::Attribute(value) => write!(f, "attribute({value})"),
        }
    }
}

/// The variants of a [`Ty`], the storage its handle shares.
///
/// A `SIMD[dtype, length]` application is the declared struct's
/// [`TyKind::Struct`], as upstream's is, so `Int`, `UInt`, and `Float64` are
/// its width-one applications, upstream's `Scalar[DType.int]`,
/// `Scalar[DType.uint]`, and `Scalar[DType.float64]`. `Func` is synthesized from a `def` signature or
/// lowered from a function-type annotation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TyKind {
    None,
    /// Bottom type: no runtime value can inhabit it.
    Never,
    /// A direct initializer-inference hole written `_`. It is legal only in a
    /// type application whose constructor participates in literal inference and
    /// must be solved before checked HIR is produced.
    Infer,
    /// A non-generic function, synthesized from a `def` signature or lowered
    /// from a function-type annotation.
    Func(Box<FuncTy>),
    /// A generic function synthesized from a `def` with a `[params]` list.
    GenericFunc(Box<GenericFuncTy>),
    /// A source name that denotes multiple callable signatures. The checker
    /// resolves an overload set at each call site. The first implementation
    /// supports distinct call shapes/arity; keeping this as a first-class type
    /// leaves type-ranked overload resolution as a natural extension.
    Overload(Vec<Ty>),
    /// A type parameter (`T`) inside a generic body, carrying its trait bounds.
    /// The binder is the declaration's parameter by identity (`ParamId`); its
    /// spelling is diagnostic metadata, so two declarations that both spell
    /// `T` are different parameters and a `$` clone shares its template's.
    Param {
        binder: ParamRef,
        bounds: Vec<String>,
        /// Anonymous callable-trait contract from a declaration such as
        /// `F: def(T) -> T`. Unlike an ordinary trait name, the full checked
        /// signature is needed both to validate specializations and to type
        /// calls through `F` inside the generic body.
        callable_bound: Option<Ty>,
    },
    /// A symbolic associated type lookup such as `C.Element` where `C` is an
    /// opaque type parameter. It may resolve to a concrete type once `C` is
    /// substituted at a generic use site. `args` is the parameter application of
    /// a parameterized associated type (`C.IteratorType[o]`); it is empty for a
    /// bare `C.Element`. The arguments are retained so the projection can be
    /// resolved concretely once the base is a conforming struct.
    Assoc {
        base: Ty,
        name: String,
        args: Vec<TyArg>,
    },
    /// Structured dependent type metadata. Generic declarations may retain it,
    /// but executable uses must substitute its index to a concrete type.
    Dependent(DependentType),
    /// `Self` inside a trait method requirement.
    SelfType,
    /// A nominal struct type, named, with its parameter arguments: one
    /// `TyArg::Ty`/`TyArg::Val` per declared type or value parameter, in
    /// declaration order, followed by the **origin tail** — one
    /// `TyArg::Origin` per explicit `Origin`/`OriginSet` parameter, in source
    /// order. The tail is part of the checked identity (`P[origin_of(xs)]` and
    /// `P[origin_of(ys)]` are different types, as upstream) and erases from
    /// the runtime ABI: mangling, layout, and MIR verification ignore it.
    Struct(
        #[serde(serialize_with = "crate::symbol_ref::serialize")] String,
        Vec<TyArg>,
    ),
    /// Compile-time-only list shape used while materializing `CtValue::List`.
    /// Checked executable List values use `Struct("List", ...)`.
    ComptimeList(Ty),
    /// Upstream's raw parameter list `!kgen.param_list<T>`
    /// (`_MLIR.KGENParamListType[T]`, spelled `KGENParamList[T]`): the type
    /// of `ParameterList`'s `values` parameter and of a value pack's
    /// `vs.values`. It is compile-time only; no runtime register holds one.
    ParamList(Ty),
    /// Upstream's trait metatype `AnyTraitType`, spelled `type_of(Trait)`:
    /// the type of a trait used as a value, displayed `AnyTrait[A & B]`. The
    /// list is the bound's canonical trait set ([`trait_set`]). It is
    /// compile-time only; no runtime register holds one.
    AnyTrait(Vec<String>),
    /// Upstream's `TraitType` (`!lit.trait<@A, @B>`): a trait, or a `&`
    /// composition, used as a type, the type of every type conforming to the
    /// canonical set ([`trait_set`]), displayed `A & B`. It is a raw list's
    /// element in `KGENParamList[AnyType]`, a list of types. It is
    /// compile-time only; no runtime register holds one.
    Trait(Vec<String>),
    /// Upstream's KGEN metatype `!kgen.type` (`_MLIR.KGENTypeType`), the
    /// type of every type and trait, displayed `__TypeOfAllTypes`. A
    /// parameter it types is a type binder bound by [`TYPE_OF_ALL_TYPES`].
    /// It is compile-time only; no runtime register holds one.
    TypeType,
    /// Upstream's KGEN string type `!kgen.string`, the type of a
    /// `StringLiteral`'s `value` parameter, displayed
    /// ``__mlir_type.`!kgen.string` ``. Its values are
    /// [`CtValue::KgenString`](crate::ct::CtValue::KgenString). A run-time
    /// one is the VM's attribute value and a native backend's string-literal
    /// descriptor, which `pop.string.size` and `pop.string.address` read.
    KgenString,
    /// Upstream's `!pop.int_literal`, the type of an `IntLiteral`'s `value`
    /// parameter, displayed ``__mlir_type.`!pop.int_literal` ``. Its values
    /// are [`CtValue::PopIntLiteral`](crate::ct::CtValue::PopIntLiteral).
    /// It is compile-time only; no runtime register holds one.
    PopIntLiteral,
    /// Upstream's `!pop.float_literal`, the type of a `FloatLiteral`'s
    /// `value` parameter, displayed ``__mlir_type.`!pop.float_literal` ``.
    /// Its values are
    /// [`CtValue::PopFloatLiteral`](crate::ct::CtValue::PopFloatLiteral).
    /// It is compile-time only; no runtime register holds one.
    PopFloatLiteral,
    /// Upstream's `!kgen.deferred`, the type of an untyped `__mlir_attr`
    /// and of a function result that returns one (`FastMathFlag._mlir_attr`),
    /// displayed ``__mlir_type.`!kgen.deferred` ``. Its values are
    /// [`CtValue::MlirAttr`](crate::ct::CtValue::MlirAttr). It conforms to
    /// no trait and is compile-time only; no runtime register holds one.
    KgenDeferred,
    /// Upstream's `!kgen.target`, the type of a compilation target's
    /// attribute (`CompilationTarget._mlir_value`), displayed
    /// ``__mlir_type.`!kgen.target` ``. Its values are
    /// [`CtValue::KgenTarget`](crate::ct::CtValue::KgenTarget). It conforms
    /// to no trait and has no LLVM type ([`has_no_llvm_type`]).
    KgenTarget,
    /// Upstream's MLIR builtin `index` (`__mlir_type.index`), what
    /// `Indexer.__mlir_index__` returns, displayed `__mlir_type.index`.
    /// `pop.cast_from_builtin` and `pop.cast_to_builtin` convert it to and
    /// from an `Int`'s `!kgen.scalar<index>` storage. Both backends hold it
    /// as they hold an `Int`.
    Index,
    /// Upstream's MLIR builtin `i1` (`__mlir_type.i1`), what
    /// `Bool.__mlir_i1__` returns and `Bool`'s `@implicit` initializer
    /// takes, displayed `__mlir_type.i1`. `pop.cast_from_builtin` and
    /// `pop.cast_to_builtin` convert it to and from a `Bool`'s
    /// `!kgen.scalar<bool>` storage. It has no members and conforms to no
    /// trait. MIR holds it as a register type of its own, which both
    /// backends store as they store a `Bool`.
    I1,
    /// Upstream's MLIR builtin `ui8` (`__mlir_type.ui8`), what
    /// `pop.dtype.to_ui8` returns and `pop.dtype.from_ui8` takes, displayed
    /// `__mlir_type.ui8`. `pop.cast_from_builtin` and `pop.cast_to_builtin`
    /// convert it to and from a `UInt8`'s `!kgen.scalar<ui8>` storage. It
    /// has no members and conforms to no trait. Both backends hold it as
    /// they hold a `UInt8`.
    Ui8,
    /// Upstream's `!kgen.dtype`, the type of a `DType`'s `_mlir_value`,
    /// displayed ``__mlir_type.`!kgen.dtype` ``. Its constants are
    /// `#kgen.dtype.constant<mnemonic>` attributes, held as
    /// [`CtValue::Dtype`](crate::ct::CtValue::Dtype), and
    /// `pop.dtype.to_ui8` and `pop.dtype.from_ui8` convert it to and from
    /// its one-byte code. It has no members and conforms to no trait. Both
    /// backends hold it as its one-byte code, and a `DType` as this one
    /// field.
    KgenDtype,
    /// Upstream's KGEN vector storage `!kgen.simd<width, dtype>`, the type
    /// of a `SIMD`'s `_mlir_value` field, displayed
    /// ``__mlir_type.`!kgen.simd<4, si32>` `` (``__mlir_type.`!kgen.scalar<index>` ``
    /// at width one). It is distinct from the `SIMD` that stores it:
    /// `pop` ops take it, and it has no members and conforms to no
    /// numeric trait. Either slot is a parameter expression while its
    /// declaration is symbolic. Both backends hold it as they hold the
    /// vector.
    KgenSimd {
        dtype: SimdDtype,
        width: SimdWidth,
    },
    /// Upstream's KGEN vector of addresses `!kgen.simd<width, address>`,
    /// displayed ``__mlir_type.`!kgen.simd<4, address>` ``: `width`
    /// untyped pointers, LLVM's `<width x ptr>`, the base-pointer operand
    /// `llvm.masked.gather` and `llvm.masked.scatter` take. Its lane is
    /// KGEN's `address` dtype, which no public `DType` names, so it is
    /// apart from [`TyKind::KgenSimd`]. It has no members and conforms to
    /// no trait. The width is a parameter expression while its declaration
    /// is symbolic. Both backends lay it out as `width` pointers.
    KgenAddressSimd {
        width: SimdWidth,
    },
    /// Upstream's generator type `!kgen.generator`, spelled
    /// `__generator_type[idx: Int] Int`: the type of a parameterized
    /// `comptime` declaration read unapplied, and of a parameter that takes
    /// one. It is compile-time only; no runtime register holds one.
    Generator(Box<GeneratorTy>),
    /// Compiler-private `__RuntimeTuple[T1, ..., Tn]` storage. Public
    /// `Tuple[T1, ..., Tn]` values are nominal standard-library structs.
    Tuple(Vec<Ty>),
    /// Upstream's fixed-size array `!pop.array<size, element>`
    /// (`_MLIR.POPArrayType[size, element]`): `StaticTuple`'s storage,
    /// which `pop.array.repeat`, `pop.array.get`, and `pop.array.replace`
    /// build and read, and the array of element pointers a call builds for
    /// a `*xs: T` callee's `VariadicList`, in a slot of its own
    /// (`__passed_varargs__`).
    PopArray {
        size: ArraySize,
        element: Ty,
    },
    /// Internal checked ABI type for a compile-time-specialized heterogeneous
    /// runtime parameter pack. Unlike a source `Tuple[...]` used as the element
    /// type of an ordinary homogeneous `*args`, each entry describes one
    /// positional argument and the collector uses private tuple-shaped storage.
    /// This type cannot be written directly in Mojo source.
    RuntimePack(Vec<Ty>),
    /// Internal checked ABI type for an ordinary homogeneous runtime variadic.
    /// Source `List[T]` is a nominal standard-library struct; a `*args: T`
    /// collector is compiler storage and must therefore not masquerade as that
    /// user-facing collection.
    VariadicPack(Ty),
    /// The built-in tagged union `Variant[T1, ..., Tn]`.  The ordering is part
    /// of the type: it determines the runtime tag used by typed projection.
    Variant(Vec<Ty>),
    /// The built-in `UnsafePointer[T, origin]`.  The VM erases the origin, but
    /// the checked and MIR types retain it for lifetime/aggregate validation.
    Pointer {
        element: Ty,
        origin: crate::origin::PointerOrigin,
    },
    /// Upstream's raw address `!kgen.pointer<T>` (`Pointer[T, o]._mlir_type`),
    /// the one field a `Pointer` wraps, displayed
    /// ``__mlir_type.`!kgen.pointer<T>` ``. It carries no origin, so it lends
    /// nothing, and it has no members. Both backends hold it as they hold the
    /// `Pointer` it came from.
    KgenPointer(Ty),
    /// Upstream's reference value `!lit.ref<T, origin>`
    /// (`Pointer[T, o]._mlir_lit_ref`), what `__get_mvalue_as_litref(place)`
    /// gives and `Pointer(_mlir_value=)` takes, displayed `ref[origin] T`.
    /// Its origin is part of its type, and it is not its referent: no read
    /// goes through it. Both backends hold it as they hold a `Pointer`.
    LitRef {
        element: Ty,
        origin: crate::origin::PointerOrigin,
    },
    /// Upstream's reference pack `!lit.ref.pack<:param_list<Trait> Ts,
    /// origin>` (`VariadicPack._mlir_type`): one `!lit.ref` to each of a
    /// type list's elements, all under one origin. Its `elements` are a
    /// tuple's, a type-pack spread (`*Ts`) standing for the pack until it
    /// is bound. Both backends hold it as an aggregate of the elements'
    /// addresses, the `!kgen.struct` of pointers it lowers to.
    LitRefPack {
        elements: Vec<Ty>,
        origin: crate::origin::PointerOrigin,
    },
    /// A reference value. Origins and permissions are checked statically; its
    /// runtime representation is introduced only after loan checking exists.
    Ref(crate::origin::RefTy),
}

/// A node is sized for its common variants: the callable payloads, the
/// largest, sit behind a box.
const _: () = assert!(std::mem::size_of::<TyKind>() <= 72);

/// A type in mojito's semantic lattice: a handle to immutable, shared storage.
///
/// Cloning a type copies the handle, and every clone shares one node; a
/// node's children are handles too, so a nested type (`W[W[Int]]`) copied
/// into every register, key, and binding that names it holds each level
/// once. A write is explicit — [`Ty::update`] unshares one node, and a
/// rewriting walk rebuilds through [`Ty::reusing`] — so a walk that
/// changes nothing never copies the levels it passes through.
///
/// Each node caches two facts about everything below it, computed once when
/// it is built: its hash, so hashing a type reads one word and equality
/// compares the handles first, then the digests, and the variants only when
/// both agree; and whether it is [closed](Ty::is_closed), so a walk that only
/// looks for or rewrites open positions stops there. A deeply nested type
/// then costs each walk its own level, not its depth.
#[derive(Clone)]
pub struct Ty(Arc<TyNode>);

/// A handle is one pointer, and an absent type costs nothing beside it.
const _: () = assert!(std::mem::size_of::<Ty>() <= 8);
const _: () = assert!(std::mem::size_of::<Option<Ty>>() <= 8);

macro_rules! dataless_types {
    ($($(#[$doc:meta])* $name:ident => $variant:ident),* $(,)?) => {
        impl Ty {
            $(
                $(#[$doc])*
                pub fn $name() -> Self {
                    static TY: LazyLock<Ty> =
                        LazyLock::new(|| Ty(Arc::new(TyNode::new(TyKind::$variant))));
                    TY.clone()
                }
            )*

            /// The shared node of a variant that carries no data, or of one
            /// of the three named scalars.
            fn dataless(kind: &TyKind) -> Option<Self> {
                match kind {
                    $(TyKind::$variant => Some(Self::$name()),)*
                    _ => None,
                }
            }
        }
    };
}

impl Ty {
    /// `Bool`, the struct `std.builtin.bool` declares.
    pub fn bool() -> Self {
        static TY: LazyLock<Ty> = LazyLock::new(|| {
            Ty(Arc::new(TyNode::new(TyKind::Struct(
                BOOL_TYPE_NAME.to_string(),
                Vec::new(),
            ))))
        });
        TY.clone()
    }
    /// `Int`, `Scalar[DType.int]`.
    pub fn int() -> Self {
        static TY: LazyLock<Ty> =
            LazyLock::new(|| simd_node(SimdDtype::Known(Dtype::Int), SimdWidth::Known(1)));
        TY.clone()
    }
    /// `UInt`, `Scalar[DType.uint]`.
    pub fn uint() -> Self {
        static TY: LazyLock<Ty> =
            LazyLock::new(|| simd_node(SimdDtype::Known(Dtype::UInt), SimdWidth::Known(1)));
        TY.clone()
    }
    /// `Float64`, `Scalar[DType.float64]`.
    pub fn float64() -> Self {
        static TY: LazyLock<Ty> =
            LazyLock::new(|| simd_node(SimdDtype::Known(Dtype::Float64), SimdWidth::Known(1)));
        TY.clone()
    }
    /// `DType`, the struct `std.builtin.dtype` declares over one
    /// `!kgen.dtype` field.
    pub fn dtype() -> Self {
        static TY: LazyLock<Ty> = LazyLock::new(|| {
            Ty(Arc::new(TyNode::new(TyKind::Struct(
                DTYPE_TYPE_NAME.to_string(),
                Vec::new(),
            ))))
        });
        TY.clone()
    }
    /// `Error`, the struct `std.builtin.error` declares over its message.
    pub fn error() -> Self {
        static TY: LazyLock<Ty> = LazyLock::new(|| {
            Ty(Arc::new(TyNode::new(TyKind::Struct(
                ERROR_TYPE_NAME.to_string(),
                Vec::new(),
            ))))
        });
        TY.clone()
    }
}

dataless_types! {
    /// `None`.
    none => None,
    /// `Never`.
    never => Never,
    /// An inference hole, `_`.
    infer => Infer,
    /// `Self` inside a trait.
    self_type => SelfType,
    /// The MLIR builtin `index`.
    index => Index,
    /// The MLIR builtin `i1`.
    i1 => I1,
    /// The MLIR builtin `ui8`.
    ui8 => Ui8,
    /// Upstream's `!kgen.dtype`.
    kgen_dtype => KgenDtype,
}

impl Ty {
    /// The generator type over `params` yielding `result`
    /// ([`TyKind::Generator`]).
    pub fn generator(params: Vec<ParamDecl>, result: Self) -> Self {
        Self::new(TyKind::Generator(Box::new(GeneratorTy { params, result })))
    }

    /// The type of the string literal `text`, `StringLiteral[text]`.
    pub fn string_literal_of(text: &str) -> Self {
        Self::string_literal_at(CtValue::KgenString(text.to_string()))
    }

    /// `StringLiteral[slot]`, the application of the struct
    /// `std.builtin.string_literal` declares at `slot`: a known
    /// [`CtValue::KgenString`] or a `!kgen.string` expression.
    pub fn string_literal_at(slot: CtValue) -> Self {
        Self::new(TyKind::Struct(
            STRING_LITERAL_TYPE_NAME.to_string(),
            vec![TyArg::Val(slot)],
        ))
    }

    /// Whether this is a string literal's type, `StringLiteral[value]`.
    pub fn is_string_literal(&self) -> bool {
        self.kind().is_string_literal()
    }

    /// The type of the integer literal `value`, `IntLiteral[value]`.
    pub fn int_literal_of(value: &IntLiteral) -> Self {
        Self::int_literal_at(CtValue::PopIntLiteral(value.clone()))
    }

    /// The type of the float literal `value`, `FloatLiteral[value]`.
    pub fn float_literal_of(value: &FloatLiteral) -> Self {
        Self::float_literal_at(CtValue::PopFloatLiteral(value.clone()))
    }

    /// `IntLiteral[slot]`, the application of the struct
    /// `std.builtin.int_literal` declares at `slot`: a known
    /// [`CtValue::PopIntLiteral`] or a `!pop.int_literal` expression.
    pub fn int_literal_at(slot: CtValue) -> Self {
        Self::new(TyKind::Struct(
            INT_LITERAL_TYPE_NAME.to_string(),
            vec![TyArg::Val(slot)],
        ))
    }

    /// `FloatLiteral[slot]`, as [`Ty::int_literal_at`] is over a
    /// `!pop.float_literal` slot.
    pub fn float_literal_at(slot: CtValue) -> Self {
        Self::new(TyKind::Struct(
            FLOAT_LITERAL_TYPE_NAME.to_string(),
            vec![TyArg::Val(slot)],
        ))
    }

    /// The type of the literal compile-time value `value`, the
    /// `IntLiteral[v]` or `FloatLiteral[v]` it carries, or `None` for any
    /// other value.
    pub fn of_literal_value(value: &CtValue) -> Option<Self> {
        match value {
            CtValue::IntLiteral(value) => Some(Self::int_literal_of(value)),
            CtValue::FloatLiteral(value) => Some(Self::float_literal_of(value)),
            _ => None,
        }
    }

    /// Whether this is an integer literal's type, value-less or not.
    pub fn is_int_literal(&self) -> bool {
        self.kind().is_int_literal()
    }

    /// Whether this is a float literal's type, value-less or not.
    pub fn is_float_literal(&self) -> bool {
        self.kind().is_float_literal()
    }

    /// Whether this is a numeric literal's type, value-less or not.
    pub fn is_numeric_literal(&self) -> bool {
        self.kind().is_numeric_literal()
    }

    /// The type of variant `kind`. A variant that carries no data shares one
    /// node per compilation.
    ///
    /// A `SIMD[dtype, length]` application canonicalizes its slots
    /// ([`canonical_simd_application`]): a closed slot folds, so `Int`,
    /// `Scalar[DType.int]`, and `SIMD[DType.int, SIMDLength(1)]` are one
    /// type. A `StringLiteral[value]` application folds a closed slot to
    /// its text ([`canonical_string_literal_application`]), and an
    /// `IntLiteral[value]` or `FloatLiteral[value]` one to its attribute
    /// ([`canonical_numeric_literal_application`]). `Bool` shares one node
    /// ([`Ty::bool`]).
    pub fn new(kind: TyKind) -> Self {
        Self::dataless(&kind)
            .or_else(|| canonical_simd_application(&kind))
            .or_else(|| canonical_string_literal_application(&kind))
            .or_else(|| canonical_numeric_literal_application(&kind))
            .or_else(|| shared_bool(&kind))
            .unwrap_or_else(|| Self(Arc::new(TyNode::new(kind))))
    }

    /// The type's variant.
    pub fn kind(&self) -> &TyKind {
        &self.0.kind
    }

    /// The type's variant, owned: moved out when this handle is the node's
    /// only one, copied otherwise.
    pub fn into_kind(self) -> TyKind {
        Arc::unwrap_or_clone(self.0).kind
    }

    /// Apply `write` to this type's variant, unshared from every other
    /// handle first, and refresh the node's cached facts after it.
    pub fn update<R>(&mut self, write: impl FnOnce(&mut TyKind) -> R) -> R {
        let node = Arc::make_mut(&mut self.0);
        let result = write(&mut node.kind);
        node.digest = TyNode::digest(&node.kind);
        node.closed = closed_kind(&node.kind);
        result
    }

    /// Whether the type, at any depth, is a concrete runtime-shaped type
    /// whose compile-time values are scalars and whose origins are unbound.
    ///
    /// A closed type holds no parameter, associated or dependent projection,
    /// `Self`, inference hole, literal type or value, callable, pointer,
    /// reference, expression, deferred slot, marker, or bound origin. So
    /// [`is_symbolic`] is false of it, substitution and the literal, origin,
    /// and callable-environment canonicalizers leave it as it is, and a
    /// [`TyRewrite`] that rewrites only those positions passes it whole.
    pub fn is_closed(&self) -> bool {
        self.0.closed
    }

    /// The type of variant `rebuilt`, keeping this handle when a rewrite
    /// left the variant as it was.
    #[must_use]
    pub fn reusing(&self, rebuilt: TyKind) -> Self {
        if *self.kind() == rebuilt {
            self.clone()
        } else {
            Self::new(rebuilt)
        }
    }

    /// Whether the two handles share one node.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// The address of the node this handle shares.
    pub(crate) fn address(&self) -> usize {
        Arc::as_ptr(&self.0).addr()
    }

    /// The node's cached hash of its variant.
    pub(crate) fn digest(&self) -> u64 {
        self.0.digest
    }

    /// Whether the type, at any depth, carries a decoration its equality
    /// leaves out: a callable's transfer effects, or a dictionary or set
    /// value's display spelling.
    pub fn carries_decorations(&self) -> bool {
        !self.is_closed() && decorated(self)
    }

    /// The signature of a callable type, generic or not.
    pub fn callable(&self) -> Option<&FuncTy> {
        self.kind().callable()
    }

    /// The signature of a non-generic function type.
    pub fn as_func(&self) -> Option<&FuncTy> {
        self.kind().as_func()
    }

    /// The signature of a generic function type.
    pub fn as_generic_func(&self) -> Option<&GenericFuncTy> {
        self.kind().as_generic_func()
    }

    /// A `SIMD[dtype, width]` vector over slots as given: ones read off
    /// another vector or a MIR instruction, already canonical. New slots go
    /// through [`simd_ty_from_slots`].
    pub fn simd(dtype: SimdDtype, width: SimdWidth) -> Self {
        simd_ty_from_slots(dtype.clone(), width.clone()).unwrap_or_else(|_| simd_node(dtype, width))
    }
}

impl TyKind {
    /// Whether this is `Bool`, the struct `std.builtin.bool` declares.
    pub fn is_bool(&self) -> bool {
        matches!(self, Self::Struct(name, args) if name == BOOL_TYPE_NAME && args.is_empty())
    }

    /// Whether this is `Int`.
    pub fn is_int(&self) -> bool {
        self.known_simd() == Some((Dtype::Int, 1))
    }

    /// Whether this is `UInt`.
    pub fn is_uint(&self) -> bool {
        self.known_simd() == Some((Dtype::UInt, 1))
    }

    /// Whether this is `Float64`.
    pub fn is_float64(&self) -> bool {
        self.known_simd() == Some((Dtype::Float64, 1))
    }

    /// Whether this is a `SIMD` vector, its slots symbolic or not. An exact
    /// numeric literal is not one ([`simd_shape`] reports what it
    /// materializes to).
    pub fn is_simd(&self) -> bool {
        simd_struct_slots(self).is_some()
    }

    /// The name and arguments of a struct application other than a `SIMD`
    /// or a literal one. A rule written for a declared struct reads this
    /// rather than matching [`TyKind::Struct`], since `SIMD`'s copies and
    /// lanes are still the compiler's own rules (R1575), a string literal's
    /// construction and member table are still the compiler's (R1972), and a
    /// numeric literal's materialization is still the compiler's (R2064),
    /// though each application is the declared struct.
    pub fn as_non_simd_struct(&self) -> Option<(&String, &Vec<TyArg>)> {
        match self {
            Self::Struct(name, arguments)
                if !self.is_simd() && !self.is_string_literal() && !self.is_numeric_literal() =>
            {
                Some((name, arguments))
            }
            _ => None,
        }
    }

    /// Whether this is a struct application other than a `SIMD` one
    /// ([`TyKind::as_non_simd_struct`]).
    pub fn is_non_simd_struct(&self) -> bool {
        self.as_non_simd_struct().is_some()
    }

    /// The dtype and width slots of a `SIMD` vector, symbolic or not.
    pub fn as_simd(&self) -> Option<(SimdDtype, SimdWidth)> {
        simd_struct_slots(self)
    }

    /// The dtype and lane count of a `SIMD` vector whose slots are both
    /// known. Unlike [`simd_shape`], an exact literal answers `None`.
    pub fn known_simd(&self) -> Option<(Dtype, i64)> {
        match simd_struct_slots(self)? {
            (SimdDtype::Known(dtype), SimdWidth::Known(width)) => Some((dtype, width)),
            _ => None,
        }
    }

    /// Whether this is a string literal's type, `StringLiteral[value]`.
    pub fn is_string_literal(&self) -> bool {
        self.string_literal_value().is_some()
    }

    /// Whether this is upstream's `Error`, the `std.builtin.error` struct.
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Struct(name, args) if name == ERROR_TYPE_NAME && args.is_empty())
    }

    /// The `value` argument of a `StringLiteral[value]` application whose
    /// slot is a string: a known [`CtValue::KgenString`] or a symbolic
    /// `!kgen.string` [`CtValue::Expr`]. A bare, erased, or deferred
    /// application answers `None`.
    pub fn string_literal_value(&self) -> Option<&CtValue> {
        match self {
            Self::Struct(name, args) if name == STRING_LITERAL_TYPE_NAME => match args.as_slice() {
                [TyArg::Val(value @ CtValue::KgenString(_))] => Some(value),
                [TyArg::Val(value @ CtValue::Expr(expr))]
                    if matches!(expr.meta().as_value().map(Ty::kind), Some(Self::KgenString)) =>
                {
                    Some(value)
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// The `value` argument of an `IntLiteral[value]` application whose
    /// slot is an integer literal: a known [`CtValue::PopIntLiteral`] or a
    /// symbolic `!pop.int_literal` [`CtValue::Expr`]. A bare, erased, or
    /// deferred application answers `None`.
    pub fn int_literal_slot(&self) -> Option<&CtValue> {
        numeric_literal_application_slot(self, false)
    }

    /// The `value` argument of a `FloatLiteral[value]` application, as
    /// [`TyKind::int_literal_slot`] is over a `!pop.float_literal` slot.
    pub fn float_literal_slot(&self) -> Option<&CtValue> {
        numeric_literal_application_slot(self, true)
    }

    /// The slot of an integer or float literal's type.
    pub fn numeric_literal_slot(&self) -> Option<&CtValue> {
        self.int_literal_slot()
            .or_else(|| self.float_literal_slot())
    }

    /// The `value` parameter of a literal type's application, string or
    /// numeric: the slot upstream's `StringLiteral.value`,
    /// `IntLiteral.value`, and `FloatLiteral.value` read.
    pub fn literal_value_slot(&self) -> Option<&CtValue> {
        self.string_literal_value()
            .or_else(|| self.numeric_literal_slot())
    }

    /// Whether this is an integer literal's type.
    pub fn is_int_literal(&self) -> bool {
        self.int_literal_slot().is_some()
    }

    /// Whether this is a float literal's type.
    pub fn is_float_literal(&self) -> bool {
        self.float_literal_slot().is_some()
    }

    /// Whether this is a numeric literal's type.
    pub fn is_numeric_literal(&self) -> bool {
        self.numeric_literal_slot().is_some()
    }

    /// The value of an integer literal whose type carries it.
    pub fn int_literal_value(&self) -> Option<&IntLiteral> {
        match self.int_literal_slot()? {
            CtValue::PopIntLiteral(value) => Some(value),
            _ => None,
        }
    }

    /// The value of a float literal whose type carries it.
    pub fn float_literal_value(&self) -> Option<&FloatLiteral> {
        match self.float_literal_slot()? {
            CtValue::PopFloatLiteral(value) => Some(value),
            _ => None,
        }
    }

    /// The text of a string literal whose value is known.
    pub fn string_literal_text(&self) -> Option<&str> {
        match self.string_literal_value()? {
            CtValue::KgenString(text) => Some(text),
            _ => None,
        }
    }

    /// The signature of a callable type, generic or not.
    pub fn callable(&self) -> Option<&FuncTy> {
        match self {
            Self::Func(func) => Some(func),
            Self::GenericFunc(generic) => Some(&generic.func),
            _ => None,
        }
    }

    /// The signature of a non-generic function type.
    pub fn as_func(&self) -> Option<&FuncTy> {
        match self {
            Self::Func(func) => Some(func),
            _ => None,
        }
    }

    /// The signature of a non-generic function type, for rewriting.
    pub fn as_func_mut(&mut self) -> Option<&mut FuncTy> {
        match self {
            Self::Func(func) => Some(func),
            _ => None,
        }
    }

    /// The signature of a generic function type.
    pub fn as_generic_func(&self) -> Option<&GenericFuncTy> {
        match self {
            Self::GenericFunc(generic) => Some(generic),
            _ => None,
        }
    }

    /// The signature of a generic function type, for rewriting.
    pub fn as_generic_func_mut(&mut self) -> Option<&mut GenericFuncTy> {
        match self {
            Self::GenericFunc(generic) => Some(generic),
            _ => None,
        }
    }

    /// The signature of a callable type, generic or not, for rewriting.
    pub fn callable_mut(&mut self) -> Option<&mut FuncTy> {
        match self {
            Self::Func(func) => Some(func),
            Self::GenericFunc(generic) => Some(&mut generic.func),
            _ => None,
        }
    }
}

impl From<TyKind> for Ty {
    fn from(kind: TyKind) -> Self {
        Self::new(kind)
    }
}

impl Deref for Ty {
    type Target = TyKind;

    fn deref(&self) -> &TyKind {
        self.kind()
    }
}

impl PartialEq for Ty {
    fn eq(&self, other: &Self) -> bool {
        self.ptr_eq(other) || (self.0.digest == other.0.digest && self.0.kind == other.0.kind)
    }
}

impl Eq for Ty {}

impl PartialEq<TyKind> for Ty {
    fn eq(&self, other: &TyKind) -> bool {
        self.kind() == other
    }
}

impl PartialEq<Ty> for TyKind {
    fn eq(&self, other: &Ty) -> bool {
        self == other.kind()
    }
}

impl std::hash::Hash for Ty {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_u64(self.0.digest);
    }
}

impl fmt::Debug for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind().fmt(f)
    }
}

/// A type encodes as its variant, or inside a package's type section
/// ([`crate::type_section`]) as the index of its entry there.
impl serde::Serialize for Ty {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match crate::type_section::encoded(self) {
            Some(Ok(index)) => serializer.serialize_u32(index),
            Some(Err(error)) => Err(serde::ser::Error::custom(error)),
            None => self.kind().serialize(serializer),
        }
    }
}

impl<'de> serde::Deserialize<'de> for Ty {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if let Some(section) = crate::type_section::decoding_section() {
            let index = u32::deserialize(deserializer)?;
            return section.get(index).map_err(serde::de::Error::custom);
        }
        TyKind::deserialize(deserializer).map(Self::new)
    }
}

/// A function signature. `params`/`names` describe the regular parameters;
/// `required[i]` is true when regular parameter `i` has no default. The
/// marker fields are indexes into this regular-parameter list.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct FuncTy {
    /// Checked callable-environment contract. This is semantic type
    /// information even though the VM erases it at execution.
    pub environment: crate::origin::CallableEnvironment,
    pub params: Vec<Ty>,
    pub names: Vec<String>,
    pub ret: Ty,
    pub required: Vec<bool>,
    pub variadic: Option<Ty>,
    /// Homogeneous element type collected by `**kwargs`, when present.
    pub kw_variadic: Option<Ty>,
    pub positional_only: Option<usize>,
    pub keyword_only: Option<usize>,
    pub raises: bool,
    pub error: Option<Ty>,
    /// The argument convention of each regular parameter.
    pub conventions: Vec<Option<ArgConvention>>,
    pub ref_params: Vec<Option<crate::origin::RefSig>>,
    pub ref_return: Option<Box<crate::origin::RefSig>>,
    /// Identity-transparent inferred transfer effects; see
    /// [`TransferSet`].
    pub transfers: TransferSet,
}

impl FuncTy {
    /// Every parameter, result, variadic, and error type, in that order.
    pub fn types(&self) -> impl Iterator<Item = &Ty> {
        self.params
            .iter()
            .chain(std::iter::once(&self.ret))
            .chain(self.variadic.as_ref())
            .chain(self.kw_variadic.as_ref())
            .chain(self.error.as_ref())
    }

    /// This signature with every parameter, result, variadic, and error type
    /// rewritten by `f`, in that order.
    #[must_use]
    pub fn map_types(&self, mut f: impl FnMut(&Ty) -> Ty) -> Self {
        let Ok(func) = self.try_map_types(|ty| Ok::<_, std::convert::Infallible>(f(ty)));
        func
    }

    /// [`FuncTy::map_types`] with a rewrite that may fail.
    pub fn try_map_types<E>(&self, mut f: impl FnMut(&Ty) -> Result<Ty, E>) -> Result<Self, E> {
        let params = self.params.iter().map(&mut f).collect::<Result<_, _>>()?;
        let ret = f(&self.ret)?;
        let variadic = self.variadic.as_ref().map(&mut f).transpose()?;
        let kw_variadic = self.kw_variadic.as_ref().map(&mut f).transpose()?;
        let error = self.error.as_ref().map(&mut f).transpose()?;
        Ok(Self {
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            environment: self.environment.clone(),
            names: self.names.clone(),
            required: self.required.clone(),
            positional_only: self.positional_only,
            keyword_only: self.keyword_only,
            raises: self.raises,
            conventions: self.conventions.clone(),
            ref_params: self.ref_params.clone(),
            ref_return: self.ref_return.clone(),
            transfers: self.transfers.clone(),
        })
    }
}

/// A generic function's signature: its declared parameters over the
/// signature they bind.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct GenericFuncTy {
    pub decls: Vec<ParamDecl>,
    pub func: FuncTy,
}

impl Deref for GenericFuncTy {
    type Target = FuncTy;

    fn deref(&self) -> &FuncTy {
        &self.func
    }
}

impl DerefMut for GenericFuncTy {
    fn deref_mut(&mut self) -> &mut FuncTy {
        &mut self.func
    }
}

/// A generator type's input parameters over the type of what applying
/// them yields, upstream's `GeneratorType { inputParamTypes, body }`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct GeneratorTy {
    pub params: Vec<ParamDecl>,
    pub result: Ty,
}

impl GeneratorTy {
    /// The parameters a binding of `bound` leaves, in declaration order.
    pub fn unbound_params<T>(&self, bound: &[Option<T>]) -> Vec<ParamDecl> {
        self.params
            .iter()
            .zip(bound)
            .filter(|(_, slot)| slot.is_none())
            .map(|(decl, _)| decl.clone())
            .collect()
    }
}

pub const ARRAY_TYPE_NAME: &str = "Array";

/// The owner of a canonicalized generic contract's own binders
/// ([`canonical_generic_signature`]): slot `i` of every contract is one
/// identity, so contracts differing only in binder spelling compare equal.
pub const CONTRACT_BINDER_OWNER: &str = "$contract";

pub const LIST_TYPE_NAME: &str = "List";

pub const SET_TYPE_NAME: &str = "Set";

pub const DICT_TYPE_NAME: &str = "Dict";

pub const TUPLE_TYPE_NAME: &str = "Tuple";

/// The name of upstream's `ParameterList`, declared in
/// `std.builtin.variadics`: the type a value pack's visible binder has.
pub const PARAMETER_LIST_TYPE_NAME: &str = "ParameterList";

/// The name of upstream's `TypeList`, declared in `std.builtin.variadics`:
/// the type a type pack's visible binder has.
pub const TYPE_LIST_TYPE_NAME: &str = "TypeList";

/// The name of upstream's `SIMDLength`, the type of `SIMD`'s width.
pub const SIMD_LENGTH_TYPE_NAME: &str = "SIMDLength";

/// The name of upstream's `DType`, declared in `std.builtin.dtype`: the
/// type of `SIMD`'s element slot.
pub const DTYPE_TYPE_NAME: &str = "DType";

/// The name of upstream's `Error`, declared in `std.builtin.error`.
pub const ERROR_TYPE_NAME: &str = "Error";

/// The name of upstream's `SIMD`, declared in `std.simd`.
pub const SIMD_TYPE_NAME: &str = "SIMD";

/// The symbol of upstream's `@implicit SIMD.__init__(IntLiteral)`, which
/// splats an integer literal across a vector's lanes.
pub const SIMD_FROM_INT_LITERAL: &str = "SIMD.__init__$ov$IntLiteral";

/// The symbol of upstream's `@implicit SIMD.__init__(FloatLiteral)`, which
/// splats a float literal across a vector's lanes.
pub const SIMD_FROM_FLOAT_LITERAL: &str = "SIMD.__init__$ov$FloatLiteral";

/// The name of upstream's `Bool`, declared in `std.builtin.bool`
/// ([`Ty::is_bool`]).
pub const BOOL_TYPE_NAME: &str = "Bool";

/// The name `std.builtin.string_literal` declares a string literal's struct
/// under ([`TyKind::string_literal_value`]).
pub const STRING_LITERAL_TYPE_NAME: &str = "StringLiteral";

/// The name `std.builtin.int_literal` declares an integer literal's struct
/// under ([`TyKind::int_literal_slot`]).
pub const INT_LITERAL_TYPE_NAME: &str = "IntLiteral";

/// The name `std.builtin.float_literal` declares a float literal's struct
/// under ([`TyKind::float_literal_slot`]).
pub const FLOAT_LITERAL_TYPE_NAME: &str = "FloatLiteral";

/// The bound of a type binder whose type is upstream's `!kgen.type`
/// ([`TyKind::TypeType`]), the name the pin prints for it: every type
/// satisfies it, and it implies no trait, not even `AnyType`.
pub const TYPE_OF_ALL_TYPES: &str = "__TypeOfAllTypes";

/// The spelling of upstream's `!kgen.string` type ([`TyKind::KgenString`]),
/// as the pin prints it.
pub const KGEN_STRING_TYPE_SPELLING: &str = "__mlir_type.`!kgen.string`";

/// The spelling of upstream's `!pop.int_literal` type
/// ([`TyKind::PopIntLiteral`]), as the pin prints it.
pub const POP_INT_LITERAL_TYPE_SPELLING: &str = "__mlir_type.`!pop.int_literal`";

/// The spelling of upstream's `!pop.float_literal` type
/// ([`TyKind::PopFloatLiteral`]), as the pin prints it.
pub const POP_FLOAT_LITERAL_TYPE_SPELLING: &str = "__mlir_type.`!pop.float_literal`";

/// The spelling of upstream's `!kgen.deferred` type
/// ([`TyKind::KgenDeferred`]), as the pin prints it.
pub const KGEN_DEFERRED_TYPE_SPELLING: &str = "__mlir_type.`!kgen.deferred`";

/// The spelling of upstream's `!kgen.target` type ([`TyKind::KgenTarget`]),
/// as the pin prints it.
pub const KGEN_TARGET_TYPE_SPELLING: &str = "__mlir_type.`!kgen.target`";

/// The spelling of upstream's MLIR builtin `index` type ([`TyKind::Index`]),
/// as the pin prints it.
pub const MLIR_INDEX_TYPE_SPELLING: &str = "__mlir_type.index";

/// The spelling of upstream's MLIR builtin `i1` type ([`TyKind::I1`]), as
/// the pin prints it.
pub const MLIR_I1_TYPE_SPELLING: &str = "__mlir_type.i1";

/// The spelling of upstream's MLIR builtin `ui8` type ([`TyKind::Ui8`]), as
/// the pin prints it.
pub const MLIR_UI8_TYPE_SPELLING: &str = "__mlir_type.ui8";

/// The spelling of upstream's `!kgen.dtype` type ([`TyKind::KgenDtype`]),
/// as the pin prints it.
pub const KGEN_DTYPE_TYPE_SPELLING: &str = "__mlir_type.`!kgen.dtype`";

pub const TSTRING_TYPE_NAME: &str = "TString";
/// The public `Variant` struct's name.
pub const VARIANT_TYPE_NAME: &str = "Variant";

pub const RANGE_TYPE_NAME: &str = "Range";

/// The nominal scalar range family mirroring current Mojo's three private
/// range structs, in `range(...)` arity order (1, 2, 3 arguments).
pub const SCALAR_RANGE_FAMILY: [&str; 3] =
    ["_ZeroStartingRange", "_SequentialRange", "_StridedRange"];

/// The floating-point strided range: upstream's `_StridedRange` float path,
/// which iterates by index through a fused multiply-add rather than by value.
pub const FLOAT_STRIDED_RANGE: &str = "_FloatStridedRange";

/// The MLIR type an `` __mlir_type.`name` `` spelling names.
///
/// It is one of the types the subset reads: the builtins `index`
/// ([`TyKind::Index`]), `i1` ([`TyKind::I1`]), and `ui8` ([`TyKind::Ui8`]),
/// `!kgen.dtype` ([`TyKind::KgenDtype`]), a scalar's `!kgen.scalar<dtype>` storage
/// ([`TyKind::KgenSimd`], `!kgen.scalar<index>` for an `Int`), a vector's
/// `!kgen.simd<n, dtype>` storage or a `!kgen.simd<n, address>` vector of
/// addresses ([`TyKind::KgenAddressSimd`]) of a literal width, or the
/// compile-time-only `!kgen.type` ([`TyKind::TypeType`]), `!kgen.string`
/// ([`TyKind::KgenString`]), `!pop.int_literal` ([`TyKind::PopIntLiteral`]),
/// `!pop.float_literal` ([`TyKind::PopFloatLiteral`]), `!kgen.deferred`
/// ([`TyKind::KgenDeferred`]), and `!kgen.target` ([`TyKind::KgenTarget`]).
pub fn mlir_type_named(name: &str) -> Option<Ty> {
    match name {
        "index" => Some(Ty::index()),
        "i1" => Some(Ty::i1()),
        "ui8" => Some(Ty::ui8()),
        "!kgen.dtype" => Some(Ty::kgen_dtype()),
        "!kgen.type" => Some(TyKind::TypeType.into()),
        "!kgen.string" => Some(TyKind::KgenString.into()),
        "!pop.int_literal" => Some(TyKind::PopIntLiteral.into()),
        "!pop.float_literal" => Some(TyKind::PopFloatLiteral.into()),
        "!kgen.deferred" => Some(TyKind::KgenDeferred.into()),
        "!kgen.target" => Some(TyKind::KgenTarget.into()),
        _ => name
            .strip_prefix("!kgen.scalar<")
            .and_then(|rest| rest.strip_suffix('>'))
            .and_then(Dtype::from_kgen_mnemonic)
            .map(|dtype| kgen_simd_ty(dtype, 1))
            .or_else(|| kgen_simd_named(name))
            .or_else(|| pop_array_named(name)),
    }
}

/// The `!kgen.simd<n, lane>` of a literal width over a dtype mnemonic
/// ([`TyKind::KgenSimd`]) or KGEN's `address` ([`TyKind::KgenAddressSimd`]).
fn kgen_simd_named(name: &str) -> Option<Ty> {
    let (width, lane) = name
        .strip_prefix("!kgen.simd<")?
        .strip_suffix('>')?
        .split_once(',')?;
    let width = width.trim().parse().ok().filter(|width| *width > 0)?;
    match lane.trim() {
        "address" => Some(
            TyKind::KgenAddressSimd {
                width: SimdWidth::Known(width),
            }
            .into(),
        ),
        lane => Dtype::from_kgen_mnemonic(lane).map(|dtype| kgen_simd_ty(dtype, width)),
    }
}

/// The `!pop.array<n, element>` of a literal size over a type
/// [`mlir_type_named`] names.
fn pop_array_named(name: &str) -> Option<Ty> {
    let (size, element) = name
        .strip_prefix("!pop.array<")?
        .strip_suffix('>')?
        .split_once(',')?;
    Some(
        TyKind::PopArray {
            size: ArraySize::Known(size.trim().parse().ok()?),
            element: mlir_type_named(element.trim())?,
        }
        .into(),
    )
}

/// The `!kgen.simd` storage of a `dtype` vector of `width` lanes
/// ([`TyKind::KgenSimd`]).
pub fn kgen_simd_ty(dtype: Dtype, width: i64) -> Ty {
    TyKind::KgenSimd {
        dtype: SimdDtype::Known(dtype),
        width: SimdWidth::Known(width),
    }
    .into()
}

/// The `!kgen.simd` storage type of a SIMD type, its `_mlir_value`
/// field's type, or `None` for any other type.
pub fn simd_storage(ty: &Ty) -> Option<Ty> {
    let (dtype, width) = ty.kind().as_simd()?;
    Some(TyKind::KgenSimd { dtype, width }.into())
}

/// The SIMD type whose storage is the `!kgen.simd` type `ty`, or `None`
/// for any other type.
pub fn storage_simd(ty: &Ty) -> Option<Ty> {
    match ty.kind() {
        TyKind::KgenSimd { dtype, width } => simd_ty_from_slots(dtype.clone(), width.clone()).ok(),
        _ => None,
    }
}

/// The vector a `!kgen.simd` storage type stores, whose constants and
/// lanes are the storage's, as upstream's `pop` folds read a `!pop.simd`
/// attribute; any other type itself.
pub fn storage_value_ty(ty: &Ty) -> Ty {
    storage_simd(ty).unwrap_or_else(|| ty.clone())
}

/// The slots of a `!kgen.simd` storage type.
pub fn kgen_simd_slots(ty: &TyKind) -> Option<(SimdDtype, SimdWidth)> {
    match ty {
        TyKind::KgenSimd { dtype, width } => Some((dtype.clone(), width.clone())),
        _ => None,
    }
}

/// The type LLVM translation sees for a vector, its `!kgen.simd` storage,
/// as upstream's `LowerKGENToLLVM` lowers the `SIMD` struct to its field;
/// any other type itself.
pub fn storage_ty(ty: &Ty) -> Ty {
    simd_storage(ty).unwrap_or_else(|| ty.clone())
}

/// A bracketed `__mlir_type[...]` spelling as KGEN concatenates it.
///
/// The text pieces (the backtick-stropped arguments no plain identifier
/// spells) join without whitespace, each other argument a `{}` hole, beside
/// those operands in order.
pub fn mlir_type_spelling(
    args: &[mojito_ast::ast::ParamArg],
) -> (String, Vec<&mojito_ast::ast::ParamArg>) {
    mlir_bracket_pieces(args, |text| text.split_whitespace().collect())
}

/// A bracketed `__mlir_attr[...]` spelling as KGEN concatenates it.
///
/// The text pieces join verbatim, each operand a `{}` hole, beside those
/// operands in order, the pieces as [`mlir_type_spelling`] reads them.
pub fn mlir_attribute_spelling(
    args: &[mojito_ast::ast::ParamArg],
) -> (String, Vec<&mojito_ast::ast::ParamArg>) {
    mlir_bracket_pieces(args, str::to_string)
}

/// The parameter an MLIR type operand reads the `_mlir_value` of.
///
/// `n._mlir_value` and `Self.dtype._mlir_value` read a `SIMDLength`'s
/// `index` storage and a `DType`'s `!kgen.dtype`, which an MLIR type
/// spelling reads through the struct value itself. `None` for an operand
/// that reads no `_mlir_value`.
pub fn mlir_value_operand(arg: &mojito_ast::ast::ParamArg) -> Option<mojito_ast::ast::ParamArg> {
    use mojito_ast::ast::{Expr, ExprKind, ParamArg, Type};
    match arg {
        ParamArg::Value(Expr {
            kind: ExprKind::Member { object, field },
            ..
        }) if field == crate::ct::MLIR_VALUE_FIELD => Some(ParamArg::Value((**object).clone())),
        ParamArg::Type(Type::Assoc { base, name, args })
            if name == crate::ct::MLIR_VALUE_FIELD && args.is_empty() =>
        {
            Some(ParamArg::Type((**base).clone()))
        }
        _ => None,
    }
}

/// Decompose a checker-abstract scalar-range type — a struct naming a
/// [`SCALAR_RANGE_FAMILY`] member or [`FLOAT_STRIDED_RANGE`] (plain or
/// module-qualified) with one concrete dtype value argument.
///
/// This form exists only in the discovery round: the specialization fixpoint
/// rewrites every occurrence into a construction of the linked range struct
/// at its dtype before MIR lowering.
pub fn scalar_range_parts(ty: &TyKind) -> Option<(&'static str, mojito_ast::ast::Dtype)> {
    let TyKind::Struct(name, arguments) = ty else {
        return None;
    };
    let family = SCALAR_RANGE_FAMILY
        .iter()
        .chain(std::iter::once(&FLOAT_STRIDED_RANGE))
        .find(|family| *family == name || name.ends_with(&format!("${family}")))?;
    let [TyArg::Val(dtype)] = arguments.as_slice() else {
        return None;
    };
    Some((family, dtype_of_value(dtype)?))
}

pub const OPTIONAL_TYPE_NAME: &str = "Optional";

/// Compiler-private inline possibly-uninitialized storage, the field type of
/// `MaybeUninit`.
///
/// An unregistered nominal: resolvable only from bundled standard-library
/// sources, with every capability special-cased explicitly.
pub const UNINIT_STORAGE_TYPE_NAME: &str = "__UninitStorage";

/// Compiler-private tagged-union storage, the field type of the self-hosted
/// `Variant`.
///
/// The intrinsic `TyKind::Variant` spelled from bundled standard-library sources
/// only (`var _storage: __VariantStorage[*Ts]`).
pub const VARIANT_STORAGE_TYPE_NAME: &str = "__VariantStorage";

/// Compiler-private case-indexed storage, the field type of every `enum`.
///
/// It is the intrinsic `TyKind::Variant` over the case payload types
/// (`__EnumStorage[*Ts]`), which its operations select by case index, never
/// by type, so cases may share a payload type; upstream's is a library
/// struct over `!kgen.variant`.
pub const ENUM_STORAGE_TYPE_NAME: &str = "__EnumStorage";

/// Construct a nominal standard-library type from ordinary type arguments.
pub fn nominal_type(name: impl Into<String>, arguments: Vec<Ty>) -> Ty {
    TyKind::Struct(name.into(), arguments.into_iter().map(TyArg::Ty).collect()).into()
}

pub fn list_type(element: Ty) -> Ty {
    nominal_type(LIST_TYPE_NAME, vec![element])
}

pub fn array_type(element: Ty, length: i64) -> Ty {
    array_type_of(element, CtValue::Int(length))
}

/// `Array[element, length]` whose length may still be symbolic, as a
/// reflected list over a type parameter is sized by its field count.
pub fn array_type_of(element: Ty, length: CtValue) -> Ty {
    TyKind::Struct(
        ARRAY_TYPE_NAME.into(),
        vec![TyArg::Ty(element), TyArg::Val(length)],
    )
    .into()
}

pub fn array_parts(ty: &TyKind) -> Option<(&Ty, i64)> {
    let TyKind::Struct(_, arguments) = ty else {
        return None;
    };
    let element = array_element(ty)?;
    let Some(TyArg::Val(CtValue::Int(length))) = arguments.last() else {
        return None;
    };
    Some((element, *length))
}

/// The element of any `Array` instantiation, including a struct-body template
/// whose `length` is still a symbolic `CtValue::Expr`.
pub fn array_element(ty: &TyKind) -> Option<&Ty> {
    let TyKind::Struct(name, arguments) = ty else {
        return None;
    };
    if name != ARRAY_TYPE_NAME && !name.ends_with(&format!("${ARRAY_TYPE_NAME}")) {
        return None;
    }
    let [TyArg::Ty(element), TyArg::Val(_)] = arguments.as_slice() else {
        return None;
    };
    Some(element)
}

/// The template a struct symbol names: a backend-monomorphized instance
/// (`List$mono$…`) names the struct it instantiates.
pub fn struct_template(name: &str) -> &str {
    name.split_once("$mono$")
        .map_or(name, |(template, _)| template)
}

/// `ty` with every struct named by the template it instantiates.
///
/// A backend-monomorphized instance (`Box$mono$TInt[Int]`) is the type its
/// source spelling (`Box[Int]`) names; a type comparison decides over this
/// form.
pub fn template_spelled(ty: &Ty) -> Ty {
    rewrite_ty(ty, &mut TemplateSpelling).unwrap_or_else(|_| ty.clone())
}

/// Whether two compile-time types are one type, as a type comparison
/// (`T == Int`) decides it.
pub fn same_type(left: &Ty, right: &Ty) -> bool {
    template_spelled(left) == template_spelled(right)
}

/// Whether the struct symbol `name` is the bundled `nominal` struct, by its
/// bare or module-qualified spelling, a backend-monomorphized instance
/// included.
pub fn is_nominal_instance(name: &str, nominal: &str) -> bool {
    let template = struct_template(name);
    template
        .strip_suffix(nominal)
        .is_some_and(|module| module.is_empty() || module.ends_with('$'))
}

/// Whether `name` names compiler-private inline uninit storage, as linked,
/// specialization-mangled, or backend-monomorphized.
pub fn is_uninit_storage_name(name: &str) -> bool {
    let name = name.split("$mono").next().unwrap_or(name);
    name == UNINIT_STORAGE_TYPE_NAME || name.ends_with(&format!("${UNINIT_STORAGE_TYPE_NAME}"))
}

/// The payload type of compiler-private inline uninit storage
/// (`__UninitStorage[T]`), including specialization-mangled and
/// backend-monomorphized (`…$mono$…`) instantiations.
///
/// Mono renames the struct while keeping its substituted argument list, so the
/// payload stays recoverable from the arguments.
pub fn uninit_storage_element(ty: &Ty) -> Option<&Ty> {
    let TyKind::Struct(name, arguments) = ty.kind() else {
        return None;
    };
    if !is_uninit_storage_name(name) {
        return None;
    }
    let [TyArg::Ty(element)] = arguments.as_slice() else {
        return None;
    };
    Some(element)
}

/// The bundled `VariadicList`, the struct a run-time homogeneous collector
/// (`*xs: T`) is, as upstream's.
pub const VARIADIC_LIST_TYPE_NAME: &str = "VariadicList";

/// The bundled `Span`, the struct a `VariadicList` holds its element
/// pointers in, as upstream's.
pub const SPAN_TYPE_NAME: &str = "Span";

/// `VariadicList[element, is_owned]`, the type a `*xs: element` collector
/// has in its callee's body: owned for a `var` collector.
pub fn variadic_list_type(element: Ty, is_owned: bool) -> Ty {
    TyKind::Struct(
        VARIADIC_LIST_TYPE_NAME.to_string(),
        vec![TyArg::Ty(element), TyArg::Val(CtValue::Bool(is_owned))],
    )
    .into()
}

/// The bundled `VariadicPack`, upstream's heterogeneous argument
/// collector.
pub const VARIADIC_PACK_TYPE_NAME: &str = "VariadicPack";

/// `VariadicPack[element_trait=trait, is_owned, *Ts]` over the type pack
/// `pack` (`*Ts`), a `read` heterogeneous collector's type in its body.
///
/// It is spelled as the checker resolves that spelling: the infer-only
/// element trait and pack list, the ownership flag, and the pack's
/// zero-sized `TypeList` binder. The trait is the pack binder's bound.
pub fn variadic_pack_type(pack: Ty, is_owned: bool) -> Ty {
    let traits = pack_traits(&pack);
    variadic_pack_over(traits, TyArg::Ty(pack), is_owned)
}

/// [`variadic_pack_type`] with the pack `pack` bound to `elements`, the
/// list an instance's arguments hold in its place.
pub fn bound_variadic_pack_type(pack: &Ty, elements: &[Ty], is_owned: bool) -> Ty {
    variadic_pack_over(
        pack_traits(pack),
        TyArg::Val(CtValue::Tuple(
            elements.iter().cloned().map(CtValue::Type).collect(),
        )),
        is_owned,
    )
}

/// Whether `ty` is a `VariadicPack`, an elaborated instance included, and
/// if so whether it is owned.
pub fn variadic_pack_owned(ty: &TyKind) -> Option<bool> {
    let TyKind::Struct(name, arguments) = ty else {
        return None;
    };
    (struct_template(name) == VARIADIC_PACK_TYPE_NAME).then(|| {
        arguments
            .iter()
            .any(|argument| matches!(argument, TyArg::Val(CtValue::Bool(true))))
    })
}

/// The element types of a `VariadicPack` whose pack is bound: the list its
/// arguments hold, or the `RuntimePack` an elaborated declaration binds.
pub fn variadic_pack_elements(ty: &TyKind) -> Option<Vec<Ty>> {
    variadic_pack_owned(ty)?;
    let TyKind::Struct(_, arguments) = ty else {
        return None;
    };
    arguments.iter().find_map(|argument| match argument {
        TyArg::Val(CtValue::Tuple(values)) => values
            .iter()
            .map(|value| match value {
                CtValue::Type(ty) => Some(ty.clone()),
                _ => None,
            })
            .collect(),
        TyArg::Ty(pack) => match pack.kind() {
            TyKind::RuntimePack(elements) | TyKind::Tuple(elements) => Some(elements.clone()),
            _ => None,
        },
        _ => None,
    })
}

/// The type pack (`*Ts`) a heterogeneous collector's type spreads while
/// the pack is still a parameter: a `VariadicPack`'s, or an owned
/// collector's pack storage's.
pub fn collector_pack_spread(ty: &Ty) -> Option<&Ty> {
    match ty.kind() {
        TyKind::VariadicPack(element) => pack_spread(std::slice::from_ref(element)),
        TyKind::Struct(_, arguments) if variadic_pack_owned(ty.kind()).is_some() => {
            arguments.iter().find_map(|argument| match argument {
                TyArg::Ty(pack) => pack_spread(std::slice::from_ref(pack)),
                _ => None,
            })
        }
        _ => None,
    }
}

/// The literal struct of a `VariadicPack`'s loaded elements, upstream's
/// `_loaded_kgen_pack_type`: the tuple of its pack (`*Ts`) or of its bound
/// elements. `None` for any other type.
pub fn loaded_pack_type(ty: &Ty) -> Option<Ty> {
    let elements = match collector_pack_spread(ty) {
        Some(pack) if variadic_pack_owned(ty.kind()).is_some() => vec![pack.clone()],
        _ => variadic_pack_elements(ty.kind())?,
    };
    Some(TyKind::Tuple(elements).into())
}

/// The type a positional collector of `element`s (`*xs: element`) has in
/// its callee's body.
///
/// A `read` or `mut` heterogeneous one (`*args: *Ts`) is upstream's
/// `VariadicPack` over the pack; an owned one (`var *args: *Ts`) the
/// compiler's pack storage, the tuple of its elements once specialized
/// (`$pack`); and a homogeneous one upstream's `VariadicList[element,
/// is_owned]`, owned for a `var` collector.
pub fn variadic_collector_type(element: Ty, convention: Option<ArgConvention>) -> Ty {
    let owned = matches!(convention, Some(ArgConvention::Var));
    let heterogeneous = matches!(element.kind(), TyKind::RuntimePack(_))
        || pack_spread(std::slice::from_ref(&element)).is_some();
    match element.kind() {
        TyKind::RuntimePack(elements) if !owned => {
            bound_variadic_pack_type(&element, elements, false)
        }
        _ if heterogeneous && !owned => variadic_pack_type(element, false),
        TyKind::RuntimePack(elements) => TyKind::Tuple(elements.clone()).into(),
        _ if heterogeneous => TyKind::VariadicPack(element).into(),
        _ => variadic_list_type(element, owned),
    }
}

/// Whether `ty` is a heterogeneous collector's type: upstream's
/// `VariadicPack`, or an owned collector's pack storage.
pub fn is_heterogeneous_collector(ty: &Ty) -> bool {
    match ty.kind() {
        TyKind::RuntimePack(_) => true,
        TyKind::VariadicPack(element) => pack_spread(std::slice::from_ref(element)).is_some(),
        TyKind::Tuple(elements) => pack_spread(elements).is_some(),
        kind => variadic_pack_owned(kind).is_some(),
    }
}

/// The element type of a `VariadicList[T, is_owned]`, an elaborated
/// instance included.
pub fn variadic_list_element(ty: &TyKind) -> Option<&Ty> {
    let TyKind::Struct(name, arguments) = ty else {
        return None;
    };
    if struct_template(name) != VARIADIC_LIST_TYPE_NAME {
        return None;
    }
    match arguments.first() {
        Some(TyArg::Ty(element)) => Some(element),
        _ => None,
    }
}

pub fn list_element(ty: &TyKind) -> Option<&Ty> {
    let arguments = nominal_type_arguments(ty, LIST_TYPE_NAME)?;
    let [element] = arguments.as_slice() else {
        return None;
    };
    Some(*element)
}

pub fn set_type(element: Ty) -> Ty {
    nominal_type(SET_TYPE_NAME, vec![element])
}

/// `Set[T, H]` with an explicit hasher argument.
pub fn set_type_with(element: Ty, hasher: Ty) -> Ty {
    nominal_type(SET_TYPE_NAME, vec![element, hasher])
}

pub fn set_element(ty: &TyKind) -> Option<&Ty> {
    let arguments = nominal_type_arguments(ty, SET_TYPE_NAME)?;
    let ([element] | [element, _]) = arguments.as_slice() else {
        return None;
    };
    Some(*element)
}

pub fn optional_element(ty: &TyKind) -> Option<&Ty> {
    let arguments = nominal_type_arguments(ty, OPTIONAL_TYPE_NAME)?;
    let [element] = arguments.as_slice() else {
        return None;
    };
    Some(*element)
}

pub fn owned_pointer_element(ty: &TyKind) -> Option<&Ty> {
    let arguments = nominal_type_arguments(ty, "OwnedPointer")?;
    let [element] = arguments.as_slice() else {
        return None;
    };
    Some(*element)
}

pub fn dict_type(key: Ty, value: Ty) -> Ty {
    nominal_type(DICT_TYPE_NAME, vec![key, value])
}

/// `Dict[K, V, H]` with an explicit hasher argument.
pub fn dict_type_with(key: Ty, value: Ty, hasher: Ty) -> Ty {
    nominal_type(DICT_TYPE_NAME, vec![key, value, hasher])
}

pub fn dict_elements(ty: &TyKind) -> Option<(&Ty, &Ty)> {
    let arguments = nominal_type_arguments(ty, DICT_TYPE_NAME)?;
    let ([key, value] | [key, value, _]) = arguments.as_slice() else {
        return None;
    };
    Some((*key, *value))
}

pub fn tuple_type(elements: Vec<Ty>) -> Ty {
    nominal_type(TUPLE_TYPE_NAME, elements)
}

/// Current Mojo's spelling of a value argument inside a type name.
///
/// A scalar spells `value : Type` (`3 : SIMD[DType.int, 1]`, a vector key `[0,
/// 0, 0, 0] : SIMD[DType.uint64, 4]`), a `Bool` spells bare `True`/`False`,
/// and a type-valued argument spells as a type name.
pub fn unqualified_value_argument(value: &CtValue) -> String {
    match value {
        CtValue::Bool(value) => (if *value { "True" } else { "False" }).to_string(),
        CtValue::Int(_) | CtValue::IntLiteral(_) => format!("{value} : SIMD[DType.int, 1]"),
        CtValue::UInt(value) => format!("{value} : SIMD[DType.uint, 1]"),
        CtValue::Float(bits) => {
            format!("{:?} : SIMD[DType.float64, 1]", f64::from_bits(*bits))
        }
        CtValue::FloatLiteral(_) => format!("{value} : SIMD[DType.float64, 1]"),
        CtValue::Type(ty) => unqualified_type_name(ty),
        other => other.to_string(),
    }
}

/// Current Mojo's unqualified type-name spelling (`_unqualified_type_name`)
/// for the proof subset.
///
/// The scalar aliases spell through their `SIMD` identity (`Int` is
/// `SIMD[DType.int, 1]`), nominal structs drop their module qualification, and
/// applied arguments are spelled recursively.
///
/// A minted value specialization reached here spells its symbol's base name
/// only; `mojito_symbol::symbol::unqualified_instance_name` is the spelling
/// that decodes the baked arguments at every nesting level.
pub fn unqualified_type_name(ty: &Ty) -> String {
    match ty.kind() {
        kind if kind.is_int_literal() => "SIMD[DType.int, 1]".to_string(),
        kind if kind.is_float_literal() => "SIMD[DType.float64, 1]".to_string(),
        kind if let Some((dtype, width)) = kind.as_simd() => format!("SIMD[{dtype}, {width}]"),
        kind if kind.is_string_literal() => "StringLiteral".to_string(),
        TyKind::None => "NoneType".to_string(),
        TyKind::Struct(name, args) => {
            // A public tuple's specialization symbol encodes its elements;
            // spell it as upstream's `Tuple[...]`.
            if let Some(elements) = tuple_elements(ty) {
                let elements = elements
                    .into_iter()
                    .map(unqualified_type_name)
                    .collect::<Vec<_>>()
                    .join(", ");
                return format!("Tuple[{elements}]");
            }
            let base = name
                .strip_prefix("__module$")
                .map_or(name.as_str(), |rest| {
                    rest.rsplit('$').next().unwrap_or(rest)
                });
            // A specialization suffix (`Name$...`) is never part of the name.
            let base = base.split('$').next().unwrap_or(base);
            // The origin tail erases from the runtime ABI, so it is no part
            // of an instance name (diagnostics that show origins render
            // them separately).
            let arguments = args
                .iter()
                .filter_map(|argument| match argument {
                    TyArg::Ty(ty) => Some(unqualified_type_name(ty)),
                    // A bound type pack (`TypeNames[Int, String]`) spells
                    // its element types.
                    TyArg::Val(CtValue::Tuple(values))
                        if values.iter().all(|value| matches!(value, CtValue::Type(_))) =>
                    {
                        Some(
                            values
                                .iter()
                                .map(unqualified_value_argument)
                                .collect::<Vec<_>>()
                                .join(", "),
                        )
                    }
                    TyArg::Val(value) => Some(unqualified_value_argument(value)),
                    TyArg::Origin(_) => None,
                })
                .collect::<Vec<_>>();
            if arguments.is_empty() {
                base.to_string()
            } else {
                format!("{base}[{}]", arguments.join(", "))
            }
        }
        TyKind::Variant(alternatives) => format!(
            "Variant[{}]",
            alternatives
                .iter()
                .map(unqualified_type_name)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => other.to_string(),
    }
}

/// The variadic pack a type-argument list spreads, if it is exactly one.
///
/// The pack is still a parameter (`Tuple[*Self.Ts]` inside the template that
/// declares `*Ts`), and its starred spelling is its `TyKind::Param` name.
pub fn pack_spread(elements: &[Ty]) -> Option<&Ty> {
    match elements {
        [pack] if matches!(pack.kind(), TyKind::Param { binder, .. } if binder.name.starts_with('*')) => {
            Some(pack)
        }
        _ => None,
    }
}

/// `ty` with every spread of the pack `pack` replaced by `elements`.
///
/// A spread is a whole argument list (see [`pack_spread`]), so this is the
/// one place a single type becomes several: `Tuple[*Ts]` at `Ts = [Int,
/// Bool]` is `Tuple[Int, Bool]`. Any other struct binds its pack as one
/// list, the spelling its annotation resolves to: `Variant[*Ts]` becomes
/// `Variant[(Int, Bool)]`.
pub fn expand_pack_spread(ty: &Ty, pack: &str, elements: &[Ty]) -> Ty {
    let spreads = |list: &[Ty]| {
        matches!(pack_spread(list).map(Ty::kind), Some(TyKind::Param { binder, .. })
            if binder.name.trim_start_matches('*') == pack)
    };
    let expand = |list: &[Ty]| -> Vec<Ty> {
        if spreads(list) {
            elements.to_vec()
        } else {
            list.iter()
                .map(|ty| expand_pack_spread(ty, pack, elements))
                .collect()
        }
    };
    match ty.kind() {
        // A spread of a list computed from packs closes over the bound
        // pack, to its elements once every length in it is known.
        TyKind::Struct(name, arguments) if let Some(list) = list_spread_argument(arguments) => {
            let context = ParamContext::detached();
            context
                .type_list(elements.to_vec())
                .and_then(|bound| {
                    let mut bindings = ParamBindings::new();
                    bindings.bind_name(pack, bound);
                    context.replace(list, &bindings)
                })
                .and_then(|list| spread_arguments(name, &list))
                .map_or_else(
                    |_| ty.clone(),
                    |arguments| TyKind::Struct(name.clone(), arguments).into(),
                )
        }
        TyKind::Struct(name, arguments) => {
            // A type pack's list sits beside its visible binder's
            // zero-sized `TypeList`, which an elementwise spelling drops.
            let spread = |argument: &TyArg| matches!(argument, TyArg::Ty(ty) if spreads(std::slice::from_ref(ty)));
            let arguments = if arguments.iter().any(spread) && binds_pack_elementwise(name) {
                arguments
                    .iter()
                    .flat_map(|argument| match argument {
                        argument if spread(argument) => {
                            elements.iter().cloned().map(TyArg::Ty).collect()
                        }
                        TyArg::Val(value) if ParamDecl::is_pack_value(value) => Vec::new(),
                        argument => vec![argument.clone()],
                    })
                    .collect()
            } else if arguments.iter().any(spread) {
                arguments
                    .iter()
                    .map(|argument| {
                        if spread(argument) {
                            TyArg::Val(CtValue::Tuple(
                                elements.iter().cloned().map(CtValue::Type).collect(),
                            ))
                        } else {
                            argument.clone()
                        }
                    })
                    .collect()
            } else {
                map_tyargs(arguments, |ty| expand_pack_spread(ty, pack, elements))
            };
            TyKind::Struct(name.clone(), arguments).into()
        }
        TyKind::Tuple(list) => TyKind::Tuple(expand(list)).into(),
        TyKind::RuntimePack(list) => TyKind::RuntimePack(expand(list)).into(),
        TyKind::Variant(list) => TyKind::Variant(expand(list)).into(),
        TyKind::PopArray { size, element } => TyKind::PopArray {
            size: size.clone(),
            element: expand_pack_spread(element, pack, elements),
        }
        .into(),
        TyKind::Pointer { element, origin } => TyKind::Pointer {
            element: expand_pack_spread(element, pack, elements),
            origin: origin.clone(),
        }
        .into(),
        TyKind::KgenPointer(element) => {
            TyKind::KgenPointer(expand_pack_spread(element, pack, elements)).into()
        }
        TyKind::LitRef { element, origin } => TyKind::LitRef {
            element: expand_pack_spread(element, pack, elements),
            origin: origin.clone(),
        }
        .into(),
        TyKind::LitRefPack { elements, origin } => TyKind::LitRefPack {
            elements: expand(elements),
            origin: origin.clone(),
        }
        .into(),
        TyKind::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = expand_pack_spread(&reference.referent, pack, elements);
            TyKind::Ref(reference).into()
        }
        _ => ty.clone(),
    }
}

/// [`pack_spread`] over a type-argument list.
pub fn pack_spread_argument(arguments: &[TyArg]) -> Option<&Ty> {
    match arguments {
        [TyArg::Ty(pack)] => pack_spread(std::slice::from_ref(pack)),
        _ => None,
    }
}

/// The list a type spreads, when it is one computed from packs still open.
///
/// `*Ts.reverse()` and `*TypeList._concat[..]()` are such lists; a pack
/// itself is [`pack_spread`]'s.
pub fn list_spread(ty: &Ty) -> Option<&ParamExpr> {
    match ty.kind() {
        TyKind::Dependent(list) if matches!(list.expr().meta(), MetaTy::ParamList(_)) => {
            Some(list.expr())
        }
        _ => None,
    }
}

/// [`list_spread`] over a type-argument list.
pub fn list_spread_argument(arguments: &[TyArg]) -> Option<&ParamExpr> {
    match arguments {
        [TyArg::Ty(list)] => list_spread(list),
        _ => None,
    }
}

/// The argument list a spread of `list` gives the struct `name`.
///
/// Its elements once every length in it is known, element by element or as
/// one bound list as [`expand_pack_spread`] spells a pack, and the spread
/// itself while a length is open.
pub fn spread_arguments(name: &str, list: &ParamExpr) -> Result<Vec<TyArg>, ParamError> {
    let Some(elements) = ParamContext::detached().list_elements(list)? else {
        return Ok(vec![TyArg::Ty(
            TyKind::Dependent(DependentType::Parameter(list.clone())).into(),
        )]);
    };
    let types = elements.into_iter().map(|element| match element.kind() {
        ParamKind::TypeShape(ty) => ty.clone(),
        _ => DependentType::resolve(element),
    });
    Ok(if binds_pack_elementwise(name) {
        types.map(TyArg::Ty).collect()
    } else {
        vec![TyArg::Val(CtValue::Tuple(
            types.map(CtValue::Type).collect(),
        ))]
    })
}

/// Whether a struct spells its bound pack element by element.
///
/// The public `Tuple`, `TString`, and `Variant` do, by public name or
/// generated specialization symbol: their identity is that spelling
/// (`Variant[Int, String]`).
pub fn binds_pack_elementwise(name: &str) -> bool {
    let template = struct_template(name);
    [TUPLE_TYPE_NAME, TSTRING_TYPE_NAME, VARIANT_TYPE_NAME]
        .iter()
        .any(|public| template == *public || template.ends_with(&format!("${public}")))
}

/// The canonical argument list of an application of the struct `name`.
///
/// A pack bound as one list of types is spread element by element for a
/// struct that [`binds_pack_elementwise`], so `Tuple[Int, Bool]` is one
/// type however its pack was solved.
pub fn canonical_pack_arguments(name: &str, arguments: Vec<TyArg>) -> Vec<TyArg> {
    if !binds_pack_elementwise(name) {
        return arguments;
    }
    // The pack's visible binder is its zero-sized `TypeList`, which the
    // list beside it spells.
    let mut canonical: Vec<TyArg> = arguments
        .into_iter()
        .filter(
            |argument| !matches!(argument, TyArg::Val(value) if ParamDecl::is_pack_value(value)),
        )
        .collect();
    let bound = canonical.iter().position(|argument| {
        matches!(argument, TyArg::Val(CtValue::Tuple(values))
            if values.iter().all(|value| matches!(value, CtValue::Type(_))))
    });
    let Some(position) = bound else {
        return canonical;
    };
    let TyArg::Val(CtValue::Tuple(values)) = canonical.remove(position) else {
        unreachable!("the bound pack was matched above");
    };
    canonical.splice(
        position..position,
        values.into_iter().map(|value| match value {
            CtValue::Type(ty) => TyArg::Ty(ty),
            _ => unreachable!("every element is a type"),
        }),
    );
    canonical
}

/// The positions of a pack's list and of its visible binder among a
/// struct's `arguments`.
///
/// The visible binder holds its zero-sized `ParameterList` or `TypeList`,
/// and the list is the first argument that is a parameter list. `None`
/// when the struct binds no pack.
pub fn pack_list_arguments(arguments: &[TyArg]) -> Option<(usize, usize)> {
    let is_list = |argument: &TyArg| match argument {
        TyArg::Val(CtValue::Tuple(_)) => true,
        TyArg::Val(CtValue::Expr(expr)) => matches!(expr.meta(), MetaTy::ParamList(_)),
        TyArg::Ty(ty) => {
            pack_spread(std::slice::from_ref(ty)).is_some()
                || list_spread(ty).is_some()
                || matches!(ty.kind(), TyKind::RuntimePack(_))
        }
        TyArg::Val(_) | TyArg::Origin(_) => false,
    };
    let pack = arguments.iter().position(
        |argument| matches!(argument, TyArg::Val(value) if ParamDecl::is_pack_value(value)),
    )?;
    Some((arguments.iter().position(is_list)?, pack))
}

/// The argument list of an application of the struct `name` aligned with
/// its declared parameters `decls`.
///
/// [`canonical_pack_arguments`] undone: the types a struct that
/// [`binds_pack_elementwise`] spells element by element regroup into the
/// one list of types its pack binds, so `Tuple[Int, Bool]` binds `Ts` to
/// `[Int, Bool]`, and the pack's visible binder its zero-sized `TypeList`.
/// A pack spread or computed list stays the one argument it is, and any
/// other list is returned unchanged.
pub fn declared_pack_arguments<'a>(
    name: &str,
    decls: &[ParamDecl],
    arguments: &'a [TyArg],
) -> std::borrow::Cow<'a, [TyArg]> {
    let pack = decls
        .iter()
        .position(|decl| matches!(decl, ParamDecl::Type { variadic: true, .. }));
    let visible = decls.iter().position(ParamDecl::is_type_pack);
    let declared_visible = arguments
        .iter()
        .any(|argument| matches!(argument, TyArg::Val(value) if ParamDecl::is_pack_value(value)));
    let extra = usize::from(visible.is_some()) + 1;
    let (Some(position), false, Some(count)) = (
        pack,
        declared_visible,
        (arguments.len() + extra).checked_sub(decls.len()),
    ) else {
        return arguments.into();
    };
    if !binds_pack_elementwise(name) || position + count > arguments.len() {
        return arguments.into();
    }
    let elements = &arguments[position..position + count];
    let list =
        if pack_spread_argument(elements).is_some() || list_spread_argument(elements).is_some() {
            elements[0].clone()
        } else {
            let types: Option<Vec<CtValue>> = elements
                .iter()
                .map(|argument| match argument {
                    TyArg::Ty(ty) => Some(CtValue::Type(ty.clone())),
                    TyArg::Val(_) | TyArg::Origin(_) => None,
                })
                .collect();
            let Some(types) = types else {
                return arguments.into();
            };
            TyArg::Val(CtValue::Tuple(types))
        };
    let mut declared = arguments[..position].to_vec();
    declared.push(list);
    declared.extend_from_slice(&arguments[position + count..]);
    if let Some(visible) = visible
        && visible <= declared.len()
    {
        declared.insert(visible, TyArg::Val(ParamDecl::type_list_value()));
    }
    declared.into()
}

/// The arguments of an application of a struct keyed on its lone type pack
/// (`decls` [`ParamDecl::sole_type_pack`]) aligned with `decls`.
///
/// The origins are dropped, and the pack's list is a whole argument (a
/// bound list, a runtime pack, a spread) or the types spelled element by
/// element (`Tuple[Int, Bool]`) gathered into one, beside its visible
/// binder's zero-sized `TypeList`. `None` for another struct, or arguments
/// already aligned.
pub fn lone_pack_arguments(decls: &[ParamDecl], arguments: &[TyArg]) -> Option<Vec<TyArg>> {
    ParamDecl::sole_type_pack(decls)?;
    let arguments: Vec<&TyArg> = arguments
        .iter()
        .filter(|argument| !matches!(argument, TyArg::Origin(_)))
        .collect();
    if arguments
        .iter()
        .any(|argument| matches!(argument, TyArg::Val(value) if ParamDecl::is_pack_value(value)))
    {
        return None;
    }
    let whole = |ty: &Ty| {
        matches!(ty.kind(), TyKind::RuntimePack(_))
            || pack_spread(std::slice::from_ref(ty)).is_some()
            || list_spread(ty).is_some()
    };
    let list = match arguments.as_slice() {
        [list @ TyArg::Val(_)] => (*list).clone(),
        [list @ TyArg::Ty(ty)] if whole(ty) => (*list).clone(),
        elements => TyArg::Val(CtValue::Tuple(
            elements
                .iter()
                .map(|argument| match argument {
                    TyArg::Ty(ty) => Some(CtValue::Type(ty.clone())),
                    TyArg::Val(_) | TyArg::Origin(_) => None,
                })
                .collect::<Option<Vec<_>>>()?,
        )),
    };
    let mut declared = vec![list];
    if decls.len() == 2 {
        declared.push(TyArg::Val(ParamDecl::type_list_value()));
    }
    Some(declared)
}

pub fn tuple_elements(ty: &TyKind) -> Option<Vec<&Ty>> {
    let TyKind::Struct(name, arguments) = ty else {
        return None;
    };
    // `$` cannot be written in a source identifier: besides the ordinary
    // name, accept the historically module-qualified public one and a
    // backend-monomorphized instance, which keeps its element arguments.
    if !is_nominal_instance(name, TUPLE_TYPE_NAME) {
        return None;
    }
    arguments
        .iter()
        .map(|argument| match argument {
            TyArg::Ty(ty) => Some(ty),
            TyArg::Val(_) | TyArg::Origin(_) => None,
        })
        .collect()
}

pub fn range_type() -> Ty {
    nominal_type(RANGE_TYPE_NAME, Vec::new())
}

pub fn is_range_type(ty: &TyKind) -> bool {
    nominal_type_arguments(ty, RANGE_TYPE_NAME).is_some_and(|arguments| arguments.is_empty())
}

/// Whether a `write_to`/`write_repr_to` parameter is the `Writable` protocol's writer.
///
/// The writer is `mut writer: Some[Writer]` or a `Writer`-bounded type
/// parameter. A method with any other parameter is an ordinary overload, and
/// the value displays through the reflective default instead.
pub fn is_writer_parameter(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::Param { bounds, .. } if bounds.iter().any(|bound| bound == "Writer"))
}

pub fn contains_infer(ty: &Ty) -> bool {
    mentions_open(ty, &|ty| matches!(ty.kind(), TyKind::Infer))
}

/// Whether the compile-time `StringLiteral` occurs anywhere in `ty`.
///
/// Its values are the empty literal struct rather than the nominal `String`
/// struct's, so an instantiation argument that mentions it never selects a
/// nominal-`String` instance.
pub fn contains_string_literal(ty: &Ty) -> bool {
    mentions_open(ty, &|ty| ty.is_string_literal())
}

/// Whether a struct type in `ty`, itself or nested, has an argument its
/// instance's generator erased ([`CtValue::RemovedArg`]).
pub fn erases_an_argument(ty: &Ty) -> bool {
    mentions(ty, &|ty| {
        matches!(ty.kind(), TyKind::Struct(_, arguments)
            if arguments.iter().any(|argument| matches!(argument, TyArg::Val(CtValue::RemovedArg))))
    })
}

/// Whether `predicate` holds for `ty` or any type nested in it (struct
/// arguments, elements, callable signatures).
pub fn mentions(ty: &Ty, predicate: &dyn Fn(&Ty) -> bool) -> bool {
    mentions_below(ty, predicate, false)
}

/// [`mentions`] for a `predicate` that holds of no type a
/// [closed](Ty::is_closed) argument list can hold — a parameter, a
/// literal or inference type, a callable — so a closed level is not walked.
pub fn mentions_open(ty: &Ty, predicate: &dyn Fn(&Ty) -> bool) -> bool {
    mentions_below(ty, predicate, true)
}

/// A declared compile-time parameter of a generic `struct`/`def`, classified
/// from `[name: X]` by whether `X` is a trait or a type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ParamDecl {
    /// A type parameter `T: Trait & ...`.
    Type {
        /// The binder's identity: its declaration and slot. Every `TyKind::Param`
        /// naming this binder carries the same id.
        id: ParamId,
        name: String,
        bounds: Vec<String>,
        /// The trait-valued sibling binder whose value bounds this one
        /// (`T: Tr` over `Tr: type_of(Copyable)`), upstream's `ParamType`
        /// metatype. `bounds` then holds that binder's declared trait set,
        /// all a generic body may assume of `T`; a use site checks the
        /// argument against the trait `Tr` receives.
        trait_param: Option<ParamRef>,
        /// Checked anonymous callable-trait contract, when this parameter was
        /// declared with `F: def(...) -> ...`.
        callable_bound: Option<Ty>,
        default: Option<Ty>,
        infer_only: bool,
        variadic: bool,
        constraints: Vec<GenericConstraint>,
    },
    /// A value parameter such as `n: Int` or `label: String`.  Retaining the
    /// declared type is essential: compile-time values participate in generic
    /// identity, but only values representable by this type may bind here.
    Value {
        /// The binder's identity; a `DeclRef` to this parameter carries it.
        id: ParamId,
        name: String,
        /// The declared type. A pack (`*values: T`, `variadic`) is the
        /// visible binder `values: ParameterList[T, values.values]` beside
        /// the implicit infer-only `values.values: KGENParamList[T]`
        /// ([`ParamDecl::push_value_pack`]), as upstream declares it;
        /// [`ParamDecl::value_element_ty`] reads the element each bound
        /// value has. A type pack's visible binder (`*Ts: Tr`) is one too,
        /// `Ts: TypeList[Tr, Ts.values]` beside its list binder
        /// ([`ParamDecl::push_type_pack`]).
        ty: Ty,
        default: Option<ParamExpr>,
        /// A callable default is deliberately not a `CtValue`: captured
        /// closures contain frame-relative runtime state and therefore cannot
        /// be serialized into generic identity.  This symbolic plan is
        /// evaluated in declaration order when the call frame is built.
        callable_default: Option<CallableDefault>,
        infer_only: bool,
        variadic: bool,
        constraints: Vec<GenericConstraint>,
    },
}

/// Symbolic default for a compile-time callable-value parameter.
///
/// Static functions lower to their checker-selected symbol, aliases reuse an
/// earlier reified callable parameter, and conditional defaults select between
/// two such plans using ordinary scalar compile-time parameters.  No variant
/// stores a closure payload.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CallableDefault {
    Symbol(#[serde(serialize_with = "crate::symbol_ref::serialize")] String),
    /// An earlier callable parameter of the same declaration.
    Parameter(ParamRef),
    If {
        condition: ParamExpr,
        then_value: Box<Self>,
        else_value: Box<Self>,
    },
}

impl CallableDefault {
    /// This default with each unbound parameter reference bound in `scopes`
    /// ([`GenericConstraint::bind`]).
    #[must_use]
    pub fn bind(&self, scopes: &[&[ParamDecl]]) -> Self {
        match self {
            Self::Symbol(_) => self.clone(),
            Self::Parameter(reference) => Self::Parameter(bind_reference(reference, scopes)),
            Self::If {
                condition,
                then_value,
                else_value,
            } => Self::If {
                condition: condition.clone(),
                then_value: Box::new(then_value.bind(scopes)),
                else_value: Box::new(else_value.bind(scopes)),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ConstraintOperand {
    /// A parameter, named by its declaration's identity.
    Param(ParamRef),
    Value(CtValue),
    Type(Ty),
    /// `TypeList[Ts.values]().length`, upstream's size of a type list: the
    /// operand is a pack binder, or once inlined at a concrete binding the
    /// bound element list.
    PackLength(Box<Self>),
    /// An arithmetic operand over value parameters (`n + 1` in `where n + 1 ==
    /// m`), in canonical form. It is retained symbolically and discharged by
    /// replacement at each application.
    Expr(ParamExpr),
}

/// The per-element predicate of a `TypeList` `any`/`all` proposition.
///
/// A builtin `IsTrivially*` spelling or a Bool-bodied predicate alias with one
/// type parameter, applied to each element of the bound pack.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PackPredicateRef {
    Trivial(TrivialLifecycle),
    Alias(String),
}

/// The lifecycle facet queried by the
/// `IsTrivially{Movable,Copyable,Deinitable}[T]` comptime predicates.
///
/// The type conforms to `TrivialRegisterPassable`, or the base capability
/// holds and the corresponding lifecycle operation is compiler-generated with
/// recursively trivial fields (a bitwise move/copy or a no-op destructor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TrivialLifecycle {
    Movable,
    Copyable,
    Deinitable,
}

/// Recognize an `IsTrivially*` comptime-predicate name.
///
/// These are Bool-valued predicates, not traits: they are valid in `where`
/// clauses, conformance conditions, and `comptime if`, but not as
/// type-parameter bounds.
pub fn trivial_predicate_name(name: &str) -> Option<TrivialLifecycle> {
    match name {
        "IsTriviallyMovable" => Some(TrivialLifecycle::Movable),
        "IsTriviallyCopyable" => Some(TrivialLifecycle::Copyable),
        "IsTriviallyDeinitable" => Some(TrivialLifecycle::Deinitable),
        _ => None,
    }
}

/// The predicate spelling of a [`TrivialLifecycle`] facet.
///
/// The inverse of [`trivial_predicate_name`], used to record a `where
/// IsTrivially*[T]` fact as a body-side assumption and to look it up during
/// capability queries.
pub const fn trivial_predicate_spelling(kind: TrivialLifecycle) -> &'static str {
    match kind {
        TrivialLifecycle::Movable => "IsTriviallyMovable",
        TrivialLifecycle::Copyable => "IsTriviallyCopyable",
        TrivialLifecycle::Deinitable => "IsTriviallyDeinitable",
    }
}

/// The `comptime` member through which a struct decides a
/// [`TrivialLifecycle`] facet itself.
///
/// These are upstream's `__del__is_trivial`, `__copy_ctor_is_trivial`, and
/// `__move_ctor_is_trivial` witnesses: a struct that declares one is
/// trivial for the facet exactly where its value is `True` at the
/// instance's arguments (`comptime __del__is_trivial: Bool = not
/// Self.is_owned`), whatever its lifecycle members and fields.
pub const fn trivial_hook_name(kind: TrivialLifecycle) -> &'static str {
    match kind {
        TrivialLifecycle::Movable => "__move_ctor_is_trivial",
        TrivialLifecycle::Copyable => "__copy_ctor_is_trivial",
        TrivialLifecycle::Deinitable => "__del__is_trivial",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum GenericConstraint {
    /// A top-level `where (condition, "message")` clause. The message affects
    /// only the failed-specialization diagnostic; semantic operations recurse
    /// through the wrapped condition.
    WithMessage(Box<Self>, String),
    /// `conforms_to(operand, Trait)`, upstream's `type_conforms_to_trait`:
    /// the operand is a type binder, or once inlined at a concrete binding
    /// the bound type.
    Conforms {
        operand: ConstraintOperand,
        trait_name: String,
    },
    /// `conforms_to(Ts.values, Trait)`: the operand is a pack binder, or once
    /// inlined at a concrete binding the bound element list.
    ConformsPack {
        operand: ConstraintOperand,
        trait_name: String,
    },
    /// `TypeList[Ts.values]().any[P]()` / `.all[P]()`: `P` holds for at
    /// least one / every element type. The operand is a pack binder, or once
    /// inlined at a concrete binding the bound element list.
    PackPredicate {
        operand: ConstraintOperand,
        predicate: PackPredicateRef,
        all: bool,
    },
    /// `TypeList[Ts.values]().contains[T]()`: the element type equals some
    /// element of the operand, a pack binder or a bound element list.
    PackContains {
        operand: ConstraintOperand,
        element: ConstraintOperand,
    },
    /// `IsTrivially{Movable,Copyable,Deinitable}[operand]`.
    Trivial(TrivialLifecycle, ConstraintOperand),
    Eq(ConstraintOperand, ConstraintOperand),
    Ne(ConstraintOperand, ConstraintOperand),
    Lt(ConstraintOperand, ConstraintOperand),
    Le(ConstraintOperand, ConstraintOperand),
    Gt(ConstraintOperand, ConstraintOperand),
    Ge(ConstraintOperand, ConstraintOperand),
    And(Box<Self>, Box<Self>),
    Or(Box<Self>, Box<Self>),
    Not(Box<Self>),
    Bool(bool),
}

impl GenericConstraint {
    /// This constraint with every binder reference replaced by `binder` and
    /// every other operand by `operand`.
    #[must_use]
    pub fn map(
        &self,
        binder: &dyn Fn(&ParamRef) -> ParamRef,
        operand: &dyn Fn(&ConstraintOperand) -> ConstraintOperand,
    ) -> Self {
        let mapped: Result<Self, std::convert::Infallible> = self.try_map(&|leaf| {
            Ok(match leaf {
                ConstraintOperand::Param(param) => ConstraintOperand::Param(binder(param)),
                other => operand(other),
            })
        });
        match mapped {
            Ok(mapped) => mapped,
            Err(never) => match never {},
        }
    }

    /// This constraint with every operand, a parameter one included, replaced
    /// by `leaf`; the first failure is returned. A `PackLength` operand is
    /// not itself a leaf: `leaf` replaces the list it measures.
    pub fn try_map<E>(
        &self,
        leaf: &dyn Fn(&ConstraintOperand) -> Result<ConstraintOperand, E>,
    ) -> Result<Self, E> {
        let recurse = |inner: &Self| inner.try_map(leaf).map(Box::new);
        let leaf = |operand: &ConstraintOperand| operand.try_map(leaf);
        Ok(match self {
            Self::WithMessage(inner, message) => {
                Self::WithMessage(recurse(inner)?, message.clone())
            }
            Self::Conforms {
                operand,
                trait_name,
            } => Self::Conforms {
                operand: leaf(operand)?,
                trait_name: trait_name.clone(),
            },
            Self::ConformsPack {
                operand,
                trait_name,
            } => Self::ConformsPack {
                operand: leaf(operand)?,
                trait_name: trait_name.clone(),
            },
            Self::PackPredicate {
                operand,
                predicate,
                all,
            } => Self::PackPredicate {
                operand: leaf(operand)?,
                predicate: predicate.clone(),
                all: *all,
            },
            Self::PackContains { operand, element } => Self::PackContains {
                operand: leaf(operand)?,
                element: leaf(element)?,
            },
            Self::Trivial(kind, inner) => Self::Trivial(*kind, leaf(inner)?),
            Self::Eq(a, b) => Self::Eq(leaf(a)?, leaf(b)?),
            Self::Ne(a, b) => Self::Ne(leaf(a)?, leaf(b)?),
            Self::Lt(a, b) => Self::Lt(leaf(a)?, leaf(b)?),
            Self::Le(a, b) => Self::Le(leaf(a)?, leaf(b)?),
            Self::Gt(a, b) => Self::Gt(leaf(a)?, leaf(b)?),
            Self::Ge(a, b) => Self::Ge(leaf(a)?, leaf(b)?),
            Self::And(a, b) => Self::And(recurse(a)?, recurse(b)?),
            Self::Or(a, b) => Self::Or(recurse(a)?, recurse(b)?),
            Self::Not(inner) => Self::Not(recurse(inner)?),
            Self::Bool(value) => Self::Bool(*value),
        })
    }

    /// This constraint with each reference no declaration binds yet
    /// ([`ParamRef::unbound`]) bound to the binder of that spelling in
    /// `scopes`, innermost scope first. A reference no scope declares stays
    /// unbound.
    #[must_use]
    pub fn bind(&self, scopes: &[&[ParamDecl]]) -> Self {
        self.map(
            &|reference| bind_reference(reference, scopes),
            &ConstraintOperand::clone,
        )
    }
}

impl fmt::Display for GenericConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WithMessage(inner, message) => {
                write!(f, "({inner}, {message:?})")
            }
            Self::Conforms {
                operand,
                trait_name,
            } => {
                write!(f, "conforms_to({operand}, {trait_name})")
            }
            Self::ConformsPack {
                operand,
                trait_name,
            } => {
                write!(f, "conforms_to({operand}.values, {trait_name})")
            }
            Self::PackPredicate {
                operand,
                predicate,
                all,
            } => {
                let reduction = if *all { "all" } else { "any" };
                let predicate = match predicate {
                    PackPredicateRef::Trivial(kind) => trivial_predicate_spelling(*kind),
                    PackPredicateRef::Alias(name) => name,
                };
                write!(f, "TypeList[{operand}.values]().{reduction}[{predicate}]()")
            }
            Self::PackContains { operand, element } => {
                write!(f, "TypeList[{operand}.values]().contains[{element}]()")
            }
            Self::Trivial(kind, operand) => {
                write!(f, "{}[{operand}]", trivial_predicate_spelling(*kind))
            }
            Self::Eq(a, b) => write!(f, "{a} == {b}"),
            Self::Ne(a, b) => write!(f, "{a} != {b}"),
            Self::Lt(a, b) => write!(f, "{a} < {b}"),
            Self::Le(a, b) => write!(f, "{a} <= {b}"),
            Self::Gt(a, b) => write!(f, "{a} > {b}"),
            Self::Ge(a, b) => write!(f, "{a} >= {b}"),
            Self::And(a, b) => write!(f, "{a} and {b}"),
            Self::Or(a, b) => write!(f, "{a} or {b}"),
            Self::Not(inner) => write!(f, "not {inner}"),
            Self::Bool(value) => {
                write!(f, "{}", if *value { "True" } else { "False" })
            }
        }
    }
}

impl ConstraintOperand {
    /// This operand with every leaf replaced by `leaf`: the operand itself,
    /// or for a `PackLength` the list it measures.
    pub fn try_map<E>(&self, leaf: &dyn Fn(&Self) -> Result<Self, E>) -> Result<Self, E> {
        match self {
            Self::PackLength(list) => Ok(Self::PackLength(Box::new(list.try_map(leaf)?))),
            other => leaf(other),
        }
    }
}

impl fmt::Display for ConstraintOperand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Param(name) => write!(f, "{name}"),
            Self::Value(value) => write!(f, "{value}"),
            Self::Type(ty) => write!(f, "{ty}"),
            Self::PackLength(name) => {
                write!(f, "TypeList[{name}.values]().length")
            }
            Self::Expr(expr) => write!(f, "{expr}"),
        }
    }
}

impl ParamDecl {
    pub fn name(&self) -> &str {
        match self {
            Self::Type { name, .. } | Self::Value { name, .. } => name,
        }
    }

    pub const fn id(&self) -> &ParamId {
        match self {
            Self::Type { id, .. } | Self::Value { id, .. } => id,
        }
    }

    /// This declaration with each unbound reference of its `where` clauses
    /// and its callable default bound in `scopes`
    /// ([`GenericConstraint::bind`]).
    #[must_use]
    pub fn bind_references(&self, scopes: &[&[Self]]) -> Self {
        let mut bound = self.clone();
        match &mut bound {
            Self::Type { constraints, .. } => bind_constraints(constraints, scopes),
            Self::Value {
                constraints,
                callable_default,
                ..
            } => {
                bind_constraints(constraints, scopes);
                if let Some(default) = callable_default {
                    *default = default.bind(scopes);
                }
            }
        }
        bound
    }

    /// The meta-type a reference to this binder has: a value binder's
    /// declared type's, which for a pack's visible binder (`*values: T`) is
    /// its `ParameterList` and for its implicit `values.values` the raw
    /// list, as for a raw-list binder (`values: KGENParamList[T]`); a type
    /// binder's `Type`, or a type list for a type pack.
    pub fn binder_meta(&self) -> MetaTy {
        match self {
            Self::Value { ty, .. } => MetaTy::value(ty.clone()),
            Self::Type { variadic: true, .. } => MetaTy::type_list(),
            Self::Type { .. } => MetaTy::Type,
        }
    }

    /// Declare the value pack `visible` (`*vs: T`, its declared type the
    /// element `T`) after `decls` as upstream's `typeCheckVariadicParams`
    /// declares it: an implicit infer-only `vs.values: KGENParamList[T]`
    /// binder, inserted after the leading infer-only binders as
    /// `addImplicitTypeParams(append=false)` inserts one, and the visible
    /// `vs: ParameterList[T, vs.values]` at the end. Each binder after the
    /// insertion point moves up a slot, as does every reference to one.
    ///
    /// Returns each moved binder's old identity with its new one.
    pub fn push_value_pack(decls: &mut Vec<Self>, visible: Self) -> Vec<(ParamId, ParamId)> {
        let end = decls.len();
        Self::insert_value_pack(decls, end, visible)
    }

    /// [`Self::push_value_pack`] for a pack at source position `at` that
    /// binders `decls` already declares follow (`*fs: def() thin -> None,
    /// n: Int`, whose callable contract is lowered after `n` is declared).
    pub fn insert_value_pack(
        decls: &mut Vec<Self>,
        at: usize,
        visible: Self,
    ) -> Vec<(ParamId, ParamId)> {
        let Self::Value { ty: element, .. } = &visible else {
            decls.insert(at, visible);
            return Vec::new();
        };
        let element = element.clone();
        insert_pack(decls, at, visible, |list, bindings| {
            let element = bind_signature_slots(&element, bindings);
            let list = list.with_value_type(TyKind::ParamList(element.clone()).into());
            let reference = ParamContext::detached().decl_ref(
                list.id().clone(),
                list.name(),
                list.binder_meta(),
            );
            let ty = TyKind::Struct(
                PARAMETER_LIST_TYPE_NAME.to_string(),
                vec![TyArg::Ty(element), TyArg::Val(CtValue::Expr(reference))],
            );
            (list, ty.into())
        })
    }

    /// Declare the type pack `list` (`*Ts: Tr`, classified as the type
    /// binder whose values are the pack's types) after `decls` as upstream's
    /// `typeCheckVariadicParams` declares it: `list` becomes the implicit
    /// infer-only `Ts.values: KGENParamList[Tr]` binder, inserted after the
    /// leading infer-only binders, and the visible `Ts: TypeList[Tr,
    /// Ts.values]`, whose `Trait` argument is `trait_value`, goes at the
    /// end. Each binder after the insertion point moves up a slot, as does
    /// every reference to one.
    ///
    /// Returns each moved binder's old identity with its new one.
    pub fn push_type_pack(
        decls: &mut Vec<Self>,
        list: Self,
        trait_value: &CtValue,
    ) -> Vec<(ParamId, ParamId)> {
        if !matches!(list, Self::Type { .. }) {
            decls.push(list);
            return Vec::new();
        }
        push_pack(decls, list, |list, bindings| {
            let trait_value = match trait_value {
                CtValue::Expr(expr) => CtValue::Expr(
                    ParamContext::detached()
                        .replace(expr, bindings)
                        .unwrap_or_else(|_| expr.clone()),
                ),
                value => value.clone(),
            };
            let reference = list.list_reference(list.id());
            let ty = TyKind::Struct(
                TYPE_LIST_TYPE_NAME.to_string(),
                vec![
                    TyArg::Val(trait_value),
                    TyArg::Val(CtValue::Expr(reference)),
                ],
            );
            (list, ty.into())
        })
    }

    /// The value a value pack's visible binder (`vs: ParameterList[T,
    /// vs.values]`) always has: the zero-sized `ParameterList` whose type
    /// carries the list.
    pub fn parameter_list_value() -> CtValue {
        CtValue::Struct {
            name: PARAMETER_LIST_TYPE_NAME.to_string(),
            fields: Vec::new(),
        }
    }

    /// The value a type pack's visible binder (`Ts: TypeList[Tr,
    /// Ts.values]`) always has: the zero-sized `TypeList` whose type
    /// carries the list.
    pub fn type_list_value() -> CtValue {
        CtValue::Struct {
            name: TYPE_LIST_TYPE_NAME.to_string(),
            fields: Vec::new(),
        }
    }

    /// Whether `value` is a pack's visible binder's value: the zero-sized
    /// `ParameterList` or `TypeList`.
    pub fn is_pack_value(value: &CtValue) -> bool {
        matches!(value, CtValue::Struct { name, fields }
            if fields.is_empty()
                && (name == PARAMETER_LIST_TYPE_NAME || name == TYPE_LIST_TYPE_NAME))
    }

    /// The value this pack's visible binder always has
    /// ([`Self::parameter_list_value`] or [`Self::type_list_value`]).
    /// `None` for any other binder.
    pub fn pack_value(&self) -> Option<CtValue> {
        self.pack_list_ref()?;
        Some(if self.is_type_pack() {
            Self::type_list_value()
        } else {
            Self::parameter_list_value()
        })
    }

    /// Whether this is a type pack's visible binder (`Ts: TypeList[Tr,
    /// Ts.values]`).
    pub fn is_type_pack(&self) -> bool {
        self.pack_list_struct() == Some(TYPE_LIST_TYPE_NAME)
    }

    /// Whether this is a value pack's visible binder (`vs: ParameterList[T,
    /// vs.values]`).
    pub fn is_value_pack(&self) -> bool {
        self.pack_list_struct() == Some(PARAMETER_LIST_TYPE_NAME)
    }

    /// The list binder (`Ts.values`) of the type pack `decls` declares as
    /// `name` (`Ts` or `*Ts`), which a body's compile-time reads of the
    /// pack read.
    pub fn type_pack_list<'a>(decls: &'a [Self], name: &str) -> Option<&'a Self> {
        let name = name.trim_start_matches('*');
        decls
            .iter()
            .find(|decl| decl.is_type_pack() && decl.name().trim_start_matches('*') == name)
            .and_then(|visible| Self::pack_list(decls, visible))
            .or_else(|| {
                decls.iter().find(|decl| {
                    matches!(decl, Self::Type { variadic: true, .. })
                        && !decl.is_pack_list()
                        && decl.name().trim_start_matches('*') == name
                })
            })
    }

    /// The list binder of `decls` when they declare exactly one type pack
    /// and nothing else (`[Ts.values, Ts]`), as `Tuple` and `Variant` do.
    pub fn sole_type_pack(decls: &[Self]) -> Option<&Self> {
        match decls {
            [list @ Self::Type { variadic: true, .. }, visible] if visible.is_type_pack() => {
                Some(list)
            }
            [list @ Self::Type { variadic: true, .. }] => Some(list),
            _ => None,
        }
    }

    /// The implicit list binder (`vs.values`, `Ts.values`) of the pack
    /// `visible` declares, among `decls`: the binder its `ParameterList` or
    /// `TypeList` type names.
    pub fn pack_list<'a>(decls: &'a [Self], visible: &Self) -> Option<&'a Self> {
        let list = visible.pack_list_ref()?;
        decls.iter().find(|decl| *decl.id() == list.id)
    }

    /// The reference a pack's visible binder's `ParameterList` or
    /// `TypeList` type makes to its implicit list binder. `None` for any
    /// other binder.
    pub fn pack_list_ref(&self) -> Option<ParamRef> {
        let Self::Value { ty, .. } = self else {
            return None;
        };
        self.pack_list_struct()?;
        let TyKind::Struct(_, arguments) = ty.kind() else {
            return None;
        };
        let [_, TyArg::Val(CtValue::Expr(list))] = arguments.as_slice() else {
            return None;
        };
        list.as_decl_ref().cloned()
    }

    /// The argument `arguments`, aligned with `decls`, gives `decls[index]`
    /// as a source parameter reads it: a value pack's list, which its
    /// implicit list binder holds, and any other binder's own.
    pub fn source_argument<'a>(
        decls: &[Self],
        arguments: &'a [TyArg],
        index: usize,
    ) -> Option<&'a TyArg> {
        let list = decls.get(index)?.pack_list_ref().and_then(|list| {
            decls
                .iter()
                .position(|decl| *decl.id() == list.id)
                .and_then(|slot| arguments.get(slot))
        });
        list.or_else(|| arguments.get(index))
    }

    /// Whether this is a pack's implicit list binder (`vs.values`,
    /// `Ts.values`), whose dotted name no source binder can spell.
    pub fn is_pack_list(&self) -> bool {
        self.name().contains('.')
    }

    /// The value pack whose implicit list binder (`vs.values`) this is,
    /// among `decls`. `None` for any other binder.
    pub fn list_pack<'a>(&self, decls: &'a [Self]) -> Option<&'a Self> {
        decls.iter().find(|decl| {
            decl.pack_list_ref()
                .is_some_and(|list| list.id == *self.id())
        })
    }

    /// The type each value this binder binds has: a pack's element (the
    /// element type upstream's `ASTType::getParameterListInfo` reads off
    /// the visible binder's `ParameterList`), any other value binder's
    /// declared type. `None` for a type binder.
    pub fn value_element_ty(&self) -> Option<&Ty> {
        match self {
            Self::Value { .. } if self.is_type_pack() => None,
            Self::Value {
                ty, variadic: true, ..
            } => Some(match ty.kind() {
                TyKind::Struct(name, arguments) if name == PARAMETER_LIST_TYPE_NAME => {
                    match arguments.first() {
                        Some(TyArg::Ty(element)) => element,
                        _ => ty,
                    }
                }
                TyKind::ParamList(element) => element,
                _ => ty,
            }),
            Self::Value { ty, .. } => Some(ty),
            Self::Type { .. } => None,
        }
    }

    /// Whether this is a value binder whose values are callables (`f: def()
    /// -> None`, or a pack `*fs` of them).
    pub fn binds_callables(&self) -> bool {
        self.value_element_ty()
            .is_some_and(|ty| matches!(ty.kind(), TyKind::Func(_) | TyKind::GenericFunc(_)))
    }

    /// Whether a call by name reads this binder as its parameter constant:
    /// a value binder with a callable default, or one of callables that is
    /// neither a value pack nor a pack's list binder.
    pub fn is_callable_binder(&self) -> bool {
        match self {
            Self::Value {
                callable_default, ..
            } => {
                callable_default.is_some()
                    || (self.binds_callables() && !self.is_value_pack() && !self.is_pack_list())
            }
            Self::Type { .. } => false,
        }
    }

    /// The reference a use of this binder carries: its identity with its
    /// spelling attached.
    ///
    /// A type pack's list binder (`Ts.values`) is spelled as its spread,
    /// `*Ts`, which every type a body names it by is.
    pub fn binder(&self) -> ParamRef {
        let name = match self {
            Self::Type {
                name,
                variadic: true,
                ..
            } if let Some(pack) = name.strip_suffix(".values") => {
                std::sync::Arc::from(format!("*{pack}"))
            }
            _ => std::sync::Arc::from(self.name()),
        };
        ParamRef {
            id: self.id().clone(),
            name,
        }
    }

    /// The name a body's reads of this binder spell: a pack's unstarred,
    /// and a type pack's list binder's (`Ts.values`) the pack's, `Ts`,
    /// which every read of the pack reads.
    pub fn spelling(&self) -> &str {
        let name = self.name().trim_start_matches('*');
        match self {
            Self::Type { variadic: true, .. } => name.strip_suffix(".values").unwrap_or(name),
            Self::Type { .. } | Self::Value { .. } => name,
        }
    }

    /// A reference to this list binder at the identity `id`, as a pack's
    /// element, length, or `TypeList` reads it. A type pack's list is
    /// spelled as the pack, `Ts`, as its spread is `*Ts`.
    pub fn list_reference(&self, id: &ParamId) -> ParamExpr {
        ParamContext::detached().decl_ref(
            id.clone(),
            self.binder().name.trim_start_matches('*'),
            self.binder_meta(),
        )
    }

    /// This binder spelled `name` instead, as a diagnostic about a pack's
    /// list binder names the pack.
    #[must_use]
    pub fn named(&self, name: &str) -> Self {
        let mut named = self.clone();
        match &mut named {
            Self::Type { name: own, .. } | Self::Value { name: own, .. } => {
                *own = name.to_string();
            }
        }
        named
    }

    /// This value binder declared with the type `ty` instead, as an
    /// application that solved the type binders its type names sees it; a
    /// type binder is returned unchanged.
    #[must_use]
    pub fn with_value_type(&self, ty: Ty) -> Self {
        let mut typed = self.clone();
        if let Self::Value { ty: declared, .. } = &mut typed {
            *declared = ty;
        }
        typed
    }

    /// The argument this binder contributes to its declaration's own `Self`
    /// type: a type binder as its `TyKind::Param`, a value binder as a reference
    /// to itself.
    pub fn own_argument(&self) -> TyArg {
        match self {
            Self::Type {
                bounds,
                callable_bound,
                ..
            } => TyArg::Ty(
                TyKind::Param {
                    binder: self.binder(),
                    bounds: bounds.clone(),
                    callable_bound: callable_bound.clone(),
                }
                .into(),
            ),
            Self::Value { .. } if let Some(value) = self.pack_value() => TyArg::Val(value),
            Self::Value { .. } => TyArg::Val(CtValue::Expr(ParamContext::detached().decl_ref(
                self.id().clone(),
                self.name().trim_start_matches('*'),
                self.binder_meta(),
            ))),
        }
    }

    /// The list struct a pack's visible binder is declared as:
    /// `ParameterList` or `TypeList`. `None` for any other binder.
    fn pack_list_struct(&self) -> Option<&'static str> {
        let Self::Value {
            ty, variadic: true, ..
        } = self
        else {
            return None;
        };
        match ty.kind() {
            TyKind::Struct(name, _) if name == PARAMETER_LIST_TYPE_NAME => {
                Some(PARAMETER_LIST_TYPE_NAME)
            }
            TyKind::Struct(name, _) if name == TYPE_LIST_TYPE_NAME => Some(TYPE_LIST_TYPE_NAME),
            _ => None,
        }
    }
}

impl mojito_ast::call::CompileTimeParam for ParamDecl {
    fn keyword(&self) -> &str {
        self.name().trim_start_matches('*')
    }

    fn infer_only(&self) -> bool {
        match self {
            Self::Type { infer_only, .. } | Self::Value { infer_only, .. } => *infer_only,
        }
    }

    fn variadic(&self) -> bool {
        match self {
            Self::Type { variadic, .. } | Self::Value { variadic, .. } => *variadic,
        }
    }
}

/// A type substitution: the solution for each type binder, by identity.
pub type TySubst = HashMap<ParamId, Ty>;

/// One argument in a struct type's parameter list: a type, a compile-time
/// value, or an origin.
///
/// Part of a struct type's identity, so `FixedBuffer[8] != FixedBuffer[9]`.
/// Origins participate in checked identity but erase from the runtime ABI,
/// exactly like `TyKind::Pointer` origins — a parameterized iterator's `origin`
/// argument distinguishes checked types without changing lowering.
#[derive(Debug, Clone, Eq, serde::Serialize, serde::Deserialize)]
pub enum TyArg {
    Ty(Ty),
    Val(CtValue),
    Origin(crate::origin::Origin),
}

impl PartialEq for TyArg {
    /// Value arguments compare by parameter identity
    /// ([`crate::param_expr::identity_eq`]): a residual by its canonical
    /// node, a constant by its value, with a display's spelling left out.
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Ty(left), Self::Ty(right)) => left == right,
            (Self::Val(left), Self::Val(right)) => crate::param_expr::identity_eq(left, right),
            (Self::Origin(left), Self::Origin(right)) => left == right,
            _ => false,
        }
    }
}

impl std::hash::Hash for TyArg {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
        match self {
            Self::Ty(ty) => ty.hash(state),
            Self::Val(value) => crate::param_expr::identity_hash(value, state),
            Self::Origin(origin) => origin.hash(state),
        }
    }
}

impl TyArg {
    /// The compile-time value this argument binds in a CTFE/elaboration
    /// scope. Origins erase from runtime state and bind no value.
    pub fn ct_value(&self) -> Option<CtValue> {
        match self {
            Self::Ty(ty) => Some(CtValue::Type(ty.clone())),
            Self::Val(value) => Some(value.clone()),
            Self::Origin(_) => None,
        }
    }
}

impl fmt::Display for TyArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ty(t) => write!(f, "{t}"),
            Self::Val(v) => write!(f, "{v}"),
            Self::Origin(o) => write!(f, "{o}"),
        }
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind().fmt(f)
    }
}

impl fmt::Display for TyKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some((dtype, width)) = self.as_simd() {
            return fmt_simd(f, &dtype, &width);
        }
        if let Some(value) = self.float_literal_value() {
            return write!(f, "FloatLiteral[{}]", value.pop_attribute_spelling());
        }
        match self {
            Self::Infer => write!(f, "_"),
            Self::None => write!(f, "None"),
            Self::Never => write!(f, "Never"),
            Self::Func(func) => write_callable(f, None, func, false),
            Self::GenericFunc(generic) => write_callable(f, Some(&generic.decls), generic, false),
            Self::Overload(candidates) => {
                write!(f, "overload(")?;
                for (i, candidate) in candidates.iter().enumerate() {
                    if i > 0 {
                        write!(f, " | ")?;
                    }
                    write!(f, "{candidate}")?;
                }
                write!(f, ")")
            }
            Self::Param { binder, .. } => write!(f, "{}", binder.name),
            Self::Assoc { base, name, args } => {
                write!(f, "{base}.{name}")?;
                if !args.is_empty() {
                    write!(f, "[")?;
                    for (position, argument) in args.iter().enumerate() {
                        if position > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{argument}")?;
                    }
                    write!(f, "]")?;
                }
                Ok(())
            }
            Self::Dependent(dependent) => write!(f, "{}", dependent.expr()),
            Self::SelfType => write!(f, "Self"),
            Self::Pointer { element, origin } => {
                write!(f, "Pointer[{element}, ")?;
                fmt_pointer_origin(f, origin)?;
                write!(f, "]")
            }
            Self::KgenPointer(element) => write!(f, "__mlir_type.`!kgen.pointer<{element}>`"),
            Self::LitRef { element, origin } => {
                write!(f, "ref[")?;
                fmt_pointer_origin(f, origin)?;
                write!(f, "] {element}")
            }
            Self::LitRefPack { elements, origin } => {
                write!(f, "__mlir_type.`!lit.ref.pack<[")?;
                for (index, element) in elements.iter().enumerate() {
                    if index > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{element}")?;
                }
                write!(f, "], ")?;
                fmt_pointer_origin(f, origin)?;
                write!(f, ">`")
            }
            Self::Ref(reference) => write!(f, "ref {}", reference.referent),
            Self::ComptimeList(elem) => write!(f, "<comptime-list[{elem}]>"),
            Self::ParamList(elem) => write!(f, "KGENParamList[{elem}]"),
            Self::AnyTrait(traits) => write!(f, "AnyTrait[{}]", traits.join(" & ")),
            Self::Trait(traits) => f.write_str(&traits.join(" & ")),
            Self::TypeType => write!(f, "{TYPE_OF_ALL_TYPES}"),
            Self::KgenString => f.write_str(KGEN_STRING_TYPE_SPELLING),
            Self::PopIntLiteral => f.write_str(POP_INT_LITERAL_TYPE_SPELLING),
            Self::PopFloatLiteral => f.write_str(POP_FLOAT_LITERAL_TYPE_SPELLING),
            Self::KgenDeferred => f.write_str(KGEN_DEFERRED_TYPE_SPELLING),
            Self::KgenTarget => f.write_str(KGEN_TARGET_TYPE_SPELLING),
            Self::Index => f.write_str(MLIR_INDEX_TYPE_SPELLING),
            Self::I1 => f.write_str(MLIR_I1_TYPE_SPELLING),
            Self::Ui8 => f.write_str(MLIR_UI8_TYPE_SPELLING),
            Self::KgenDtype => f.write_str(KGEN_DTYPE_TYPE_SPELLING),
            Self::KgenSimd { dtype, width } => {
                let dtype = match dtype {
                    SimdDtype::Known(dtype) => dtype.kgen_mnemonic().to_string(),
                    SimdDtype::Expr(_) => dtype.to_string(),
                };
                match width {
                    SimdWidth::Known(1) => write!(f, "__mlir_type.`!kgen.scalar<{dtype}>`"),
                    width => write!(f, "__mlir_type.`!kgen.simd<{width}, {dtype}>`"),
                }
            }
            Self::KgenAddressSimd { width } => {
                write!(f, "__mlir_type.`!kgen.simd<{width}, address>`")
            }
            Self::Generator(generator) => {
                f.write_str("__generator_type")?;
                if !generator.params.is_empty() {
                    write_param_decls(f, &generator.params)?;
                }
                write!(f, " {}", generator.result)
            }
            Self::Tuple(elems) => {
                write!(f, "Tuple[")?;
                for (i, t) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{t}")?;
                }
                write!(f, "]")
            }
            Self::PopArray { size, element } => write!(f, "!pop.array<{size}, {element}>"),
            Self::RuntimePack(elems) => {
                write!(f, "$pack[")?;
                for (i, t) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{t}")?;
                }
                write!(f, "]")
            }
            Self::VariadicPack(element) => write!(f, "$variadic[{element}]"),
            Self::Variant(alternatives) => {
                write!(f, "Variant[")?;
                for (i, ty) in alternatives.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{ty}")?;
                }
                write!(f, "]")
            }
            Self::Struct(name, args) => {
                if args.iter().all(|arg| matches!(arg, TyArg::Origin(_)))
                    && let Some(contract) = crate::closure_trait::closure_trait(name)
                {
                    return match contract.kind() {
                        Self::Func(func) => write_callable(f, None, func, true),
                        Self::GenericFunc(generic) => {
                            write_callable(f, Some(&generic.decls), generic, true)
                        }
                        contract => contract.fmt(f),
                    };
                }
                // The prelude-qualified nominal String prints its public
                // spelling rather than leaking the module-qualified symbol.
                if args.is_empty() && is_stdlib_string_struct(name) {
                    return write!(f, "String");
                }
                write!(f, "{name}")?;
                // The origin tail is checked identity, not part of the
                // printed spelling (specialization symbols and reflection
                // read this text; diagnostics that show origins render them
                // through the checker, which knows the places' names).
                let mut shown = displayed_pack_arguments(args)
                    .into_iter()
                    .filter(|argument| !matches!(argument, TyArg::Origin(_)))
                    .peekable();
                if shown.peek().is_some() {
                    write!(f, "[")?;
                    for (i, a) in shown.enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{a}")?;
                    }
                    write!(f, "]")?;
                }
                Ok(())
            }
        }
    }
}

/// A struct's arguments as its source spells them: a pack's list, which
/// its implicit list binder holds, in the place of the visible binder's
/// zero-sized `ParameterList` or `TypeList`, and the list binder's slot
/// dropped. The list is the first argument that is one.
fn displayed_pack_arguments(arguments: &[TyArg]) -> Vec<&TyArg> {
    let Some((list, pack)) = pack_list_arguments(arguments) else {
        return arguments.iter().collect();
    };
    arguments
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != list)
        .map(|(index, argument)| {
            if index == pack {
                &arguments[list]
            } else {
                argument
            }
        })
        .collect()
}

fn nominal_type_arguments<'a>(ty: &'a TyKind, expected: &str) -> Option<Vec<&'a Ty>> {
    let TyKind::Struct(name, arguments) = ty else {
        return None;
    };
    // Linked stdlib declarations used module-qualified symbols historically.
    // Accept that spelling during the representation migration; the implicit
    // prelude canonicalizes new programs to the unqualified public identity.
    if name != expected && !name.ends_with(&format!("${expected}")) {
        return None;
    }
    arguments
        .iter()
        .map(|argument| match argument {
            TyArg::Ty(ty) => Some(ty),
            TyArg::Val(_) | TyArg::Origin(_) => None,
        })
        .collect()
}

/// The type with every struct origin-tail entry reset to `Origin::Unbound`.
///
/// Origins erase from every generated clone and from the runtime ABI, so the
/// instantiation records the checker hands the elaborator carry none: a place
/// origin names a per-check owner, which would make the same instantiation
/// look new on every discovery round.
pub fn erase_origin_arguments(ty: &Ty) -> Ty {
    let recur = |ty: &Ty| erase_origin_arguments(ty);
    match ty.kind() {
        TyKind::Struct(..) if ty.is_closed() => ty.clone(),
        TyKind::Struct(name, arguments) => ty.reusing(TyKind::Struct(
            name.clone(),
            arguments
                .iter()
                .map(|argument| match argument {
                    TyArg::Ty(inner) => TyArg::Ty(recur(inner)),
                    TyArg::Val(value) => TyArg::Val(value.clone()),
                    TyArg::Origin(_) => TyArg::Origin(crate::origin::Origin::Unbound),
                })
                .collect(),
        )),
        TyKind::Tuple(elements) => TyKind::Tuple(elements.iter().map(recur).collect()).into(),
        TyKind::RuntimePack(elements) => {
            TyKind::RuntimePack(elements.iter().map(recur).collect()).into()
        }
        TyKind::Variant(alternatives) => {
            TyKind::Variant(alternatives.iter().map(recur).collect()).into()
        }
        TyKind::ComptimeList(element) => TyKind::ComptimeList(recur(element)).into(),
        TyKind::ParamList(element) => TyKind::ParamList(recur(element)).into(),
        TyKind::VariadicPack(element) => TyKind::VariadicPack(recur(element)).into(),
        TyKind::PopArray { size, element } => TyKind::PopArray {
            size: size.clone(),
            element: recur(element),
        }
        .into(),
        TyKind::Pointer { element, origin } => TyKind::Pointer {
            element: recur(element),
            origin: origin.clone(),
        }
        .into(),
        TyKind::KgenPointer(element) => TyKind::KgenPointer(recur(element)).into(),
        TyKind::LitRef { element, origin } => TyKind::LitRef {
            element: recur(element),
            origin: origin.clone(),
        }
        .into(),
        TyKind::LitRefPack { elements, origin } => TyKind::LitRefPack {
            elements: elements.iter().map(recur).collect(),
            origin: origin.clone(),
        }
        .into(),
        TyKind::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = recur(&reference.referent);
            TyKind::Ref(reference).into()
        }
        _ => ty.clone(),
    }
}

/// `ty` with every struct origin argument unbound and every pointer origin
/// static.
///
/// It is the one origin erasure an instance's key and its bindings share,
/// so an instance's body spells the types a caller binds without their
/// origins. Origins erase from the runtime ABI and from the instance symbol.
pub fn erase_origins(ty: &Ty) -> Ty {
    let recur = erase_origins;
    match ty.kind() {
        TyKind::Struct(..) if ty.is_closed() => ty.clone(),
        TyKind::Struct(name, arguments) => ty.reusing(TyKind::Struct(
            name.clone(),
            arguments
                .iter()
                .map(|argument| match argument {
                    TyArg::Ty(inner) => TyArg::Ty(recur(inner)),
                    TyArg::Val(value) => TyArg::Val(value.clone()),
                    TyArg::Origin(_) => TyArg::Origin(crate::origin::Origin::Unbound),
                })
                .collect(),
        )),
        TyKind::Pointer { element, .. } => TyKind::Pointer {
            element: recur(element),
            origin: crate::origin::PointerOrigin::Static,
        }
        .into(),
        TyKind::KgenPointer(element) => TyKind::KgenPointer(recur(element)).into(),
        TyKind::LitRef { element, .. } => TyKind::LitRef {
            element: recur(element),
            origin: crate::origin::PointerOrigin::Static,
        }
        .into(),
        TyKind::LitRefPack { elements, .. } => TyKind::LitRefPack {
            elements: elements.iter().map(recur).collect(),
            origin: crate::origin::PointerOrigin::Static,
        }
        .into(),
        TyKind::Tuple(elements) => TyKind::Tuple(elements.iter().map(recur).collect()).into(),
        TyKind::PopArray { size, element } => TyKind::PopArray {
            size: size.clone(),
            element: recur(element),
        }
        .into(),
        TyKind::RuntimePack(elements) => {
            TyKind::RuntimePack(elements.iter().map(recur).collect()).into()
        }
        TyKind::Variant(alternatives) => {
            TyKind::Variant(alternatives.iter().map(recur).collect()).into()
        }
        TyKind::ComptimeList(element) => TyKind::ComptimeList(recur(element)).into(),
        TyKind::ParamList(element) => TyKind::ParamList(recur(element)).into(),
        TyKind::VariadicPack(element) => TyKind::VariadicPack(recur(element)).into(),
        TyKind::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = recur(&reference.referent);
            TyKind::Ref(reference).into()
        }
        _ => ty.clone(),
    }
}

/// The type with each origin parameter `origins` binds replaced by its
/// argument.
///
/// It reaches struct and associated-type applications and references: a
/// parameterized associated type's template (`_ListIter[Self.T, o]`) at an
/// application's origin arguments. An origin id is a slot number, so this
/// runs while the template names only its own origins, before its type
/// binders are substituted.
pub fn substitute_origin_params<S: std::hash::BuildHasher>(
    ty: &Ty,
    origins: &HashMap<u32, crate::origin::Origin, S>,
) -> Ty {
    let recur = |ty: &Ty| substitute_origin_params(ty, origins);
    let map_args = |args: &[TyArg]| -> Vec<TyArg> {
        args.iter()
            .map(|argument| match argument {
                TyArg::Ty(inner) => TyArg::Ty(recur(inner)),
                TyArg::Val(value) => TyArg::Val(value.clone()),
                TyArg::Origin(origin) => TyArg::Origin(origin.substitute_params(origins)),
            })
            .collect()
    };
    match ty.kind() {
        TyKind::Struct(name, args) => TyKind::Struct(name.clone(), map_args(args)).into(),
        TyKind::Assoc { base, name, args } => TyKind::Assoc {
            base: recur(base),
            name: name.clone(),
            args: map_args(args),
        }
        .into(),
        TyKind::Pointer { element, origin } => TyKind::Pointer {
            element: recur(element),
            origin: origin.clone(),
        }
        .into(),
        TyKind::KgenPointer(element) => TyKind::KgenPointer(recur(element)).into(),
        TyKind::LitRef { element, origin } => TyKind::LitRef {
            element: recur(element),
            origin: origin.clone(),
        }
        .into(),
        TyKind::LitRefPack { elements, origin } => TyKind::LitRefPack {
            elements: elements.iter().map(recur).collect(),
            origin: origin.clone(),
        }
        .into(),
        TyKind::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = recur(&reference.referent);
            reference.origin = reference.origin.substitute_params(origins);
            TyKind::Ref(reference).into()
        }
        TyKind::Tuple(elements) => TyKind::Tuple(elements.iter().map(recur).collect()).into(),
        TyKind::RuntimePack(elements) => {
            TyKind::RuntimePack(elements.iter().map(recur).collect()).into()
        }
        TyKind::VariadicPack(element) => TyKind::VariadicPack(recur(element)).into(),
        TyKind::PopArray { size, element } => TyKind::PopArray {
            size: size.clone(),
            element: recur(element),
        }
        .into(),
        TyKind::Variant(alternatives) => {
            TyKind::Variant(alternatives.iter().map(recur).collect()).into()
        }
        TyKind::ComptimeList(element) => TyKind::ComptimeList(recur(element)).into(),
        TyKind::ParamList(element) => TyKind::ParamList(recur(element)).into(),
        TyKind::Dependent(dependent) => dependent.map_types(recur),
        _ => ty.clone(),
    }
}

/// A declaration's parameter list as its source spells it, `[a: Int, T:
/// Copyable]`.
pub fn write_param_decls(f: &mut fmt::Formatter<'_>, decls: &[ParamDecl]) -> fmt::Result {
    write!(f, "[")?;
    for (index, decl) in decls.iter().filter(|decl| !decl.is_pack_list()).enumerate() {
        if index > 0 {
            write!(f, ", ")?;
        }
        match decl {
            ParamDecl::Type { name, bounds, .. } => {
                write!(f, "{name}")?;
                if !bounds.is_empty() {
                    write!(f, ": {}", bounds.join(" & "))?;
                }
            }
            // A type pack's bounds are its list binder's.
            ParamDecl::Value { name, .. }
                if let Some(ParamDecl::Type { bounds, .. }) = ParamDecl::pack_list(decls, decl) =>
            {
                write!(f, "{name}")?;
                if !bounds.is_empty() {
                    write!(f, ": {}", bounds.join(" & "))?;
                }
            }
            ParamDecl::Value { name, ty, .. } => {
                write!(f, "{name}: {}", decl.value_element_ty().unwrap_or(ty))?;
            }
        }
    }
    write!(f, "]")
}

/// Whether two generators' parameter lists declare the same signature.
///
/// Position by position, the same kind, declared type or bounds, infer-only
/// and variadic markers, as upstream compares generator types. Names are
/// the declarations' own and do not take part.
pub fn same_generator_signature(left: &[ParamDecl], right: &[ParamDecl]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|pair| match pair {
            (
                ParamDecl::Type {
                    bounds,
                    infer_only,
                    variadic,
                    ..
                },
                ParamDecl::Type {
                    bounds: more_bounds,
                    infer_only: more_infer_only,
                    variadic: more_variadic,
                    ..
                },
            ) => {
                bounds.len() == more_bounds.len()
                    && bounds.iter().all(|bound| more_bounds.contains(bound))
                    && infer_only == more_infer_only
                    && variadic == more_variadic
            }
            (
                ParamDecl::Value {
                    ty,
                    infer_only,
                    variadic,
                    ..
                },
                ParamDecl::Value {
                    ty: more_ty,
                    infer_only: more_infer_only,
                    variadic: more_variadic,
                    ..
                },
            ) => ty == more_ty && infer_only == more_infer_only && variadic == more_variadic,
            _ => false,
        })
}

/// Whether a generator of type `actual` converts to the generator type
/// `expected`, as upstream converts a generator argument.
///
/// The two have the same signature ([`same_generator_signature`]), and
/// `actual`'s result coerces to `expected`'s once `actual`'s own type
/// binders are `expected`'s.
pub fn generator_converts(actual: &Ty, expected: &Ty) -> bool {
    let (TyKind::Generator(actual), TyKind::Generator(expected)) = (actual.kind(), expected.kind())
    else {
        return false;
    };
    if !same_generator_signature(&actual.params, &expected.params) {
        return false;
    }
    let renamed: TySubst = actual
        .params
        .iter()
        .zip(&expected.params)
        .filter_map(|pair| match pair {
            (
                ParamDecl::Type { id, .. },
                ParamDecl::Type {
                    id: target,
                    name,
                    bounds,
                    ..
                },
            ) => Some((
                id.clone(),
                TyKind::Param {
                    binder: ParamRef {
                        id: target.clone(),
                        name: std::sync::Arc::from(name.as_str()),
                    },
                    bounds: bounds.clone(),
                    callable_bound: None,
                }
                .into(),
            )),
            _ => None,
        })
        .collect();
    let result = substitute(&actual.result, &renamed);
    result == expected.result || coerces(&result, &expected.result)
}

/// The checker's value-coercion predicate, shared with MIR verification so the
/// verifier never re-derives conversion rules.
pub fn value_coerces(from: &Ty, to: &Ty) -> bool {
    coerces(from, to)
}

/// The `Ty` for a SIMD of `dtype`/`width`. `Int`, `UInt`, and `Float64` are
/// its width-one `int`, `uint`, and `float64` instances.
///
/// Every phase that names a SIMD leaf type (checker annotations, the VM's
/// runtime leaf classification, the hasher's leaf instances) agrees through this
/// one function.
pub fn canonical_simd_ty(dtype: Dtype, width: i64) -> Ty {
    match (dtype, width) {
        (Dtype::Int, 1) => Ty::int(),
        (Dtype::UInt, 1) => Ty::uint(),
        (Dtype::Float64, 1) => Ty::float64(),
        _ => simd_node(SimdDtype::Known(dtype), SimdWidth::Known(width)),
    }
}

/// The `SIMD[dtype, length]` struct node of two canonical slots: a
/// `DType` value or expression, and a `SIMDLength` value or expression.
/// It is built directly, since its slots are already canonical.
fn simd_node(dtype: SimdDtype, width: SimdWidth) -> Ty {
    let dtype = match dtype {
        SimdDtype::Known(dtype) => dtype_value(dtype),
        SimdDtype::Expr(expr) => CtValue::Expr(expr),
    };
    let length = match width {
        SimdWidth::Known(lanes) => simd_length_value(lanes),
        SimdWidth::Expr(length) => CtValue::Expr(length),
    };
    Ty(Arc::new(TyNode::new(TyKind::Struct(
        SIMD_TYPE_NAME.to_string(),
        vec![TyArg::Val(dtype), TyArg::Val(length)],
    ))))
}

/// The slots of a canonical `SIMD[dtype, length]` application, as
/// [`simd_node`] spells them, or `None` for any other type.
fn simd_struct_slots(kind: &TyKind) -> Option<(SimdDtype, SimdWidth)> {
    let TyKind::Struct(name, args) = kind else {
        return None;
    };
    if name != SIMD_TYPE_NAME {
        return None;
    }
    let [TyArg::Val(dtype), TyArg::Val(length)] = args.as_slice() else {
        return None;
    };
    let width = match length {
        CtValue::Struct { .. } => SimdWidth::Known(known_simd_length(length)?),
        CtValue::Expr(expr) => SimdWidth::Expr(expr.clone()),
        _ => return None,
    };
    Some((simd_dtype_slot(dtype)?, width))
}

/// A `SIMD` application's dtype argument as its slot: a `DType` value, or a
/// `DType`-typed expression.
fn simd_dtype_slot(value: &CtValue) -> Option<SimdDtype> {
    if let Some(dtype) = dtype_of_value(value) {
        return Some(SimdDtype::Known(dtype));
    }
    match value {
        CtValue::Expr(expr) if expr.meta().as_value().is_some_and(is_dtype) => {
            Some(SimdDtype::Expr(expr.clone()))
        }
        _ => None,
    }
}

/// The lane count of a closed `SIMDLength` value in its canonical form,
/// its one field an `Index`.
fn known_simd_length(value: &CtValue) -> Option<i64> {
    match value {
        CtValue::Struct { name, fields } if name == SIMD_LENGTH_TYPE_NAME => {
            match fields.as_slice() {
                [(_, CtValue::Index(lanes))] => Some(*lanes),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The canonical form of a `SIMD[dtype, length]` application, upstream's
/// struct applied to a dtype value and a `SIMDLength` value, closed or
/// symbolic: a closed slot folds, so `Int`, `Scalar[DType.int]`, and
/// `SIMD[DType.int, SIMDLength(1)]` are one type. Any other shape (the
/// bare template, an erased or deferred argument) is not rewritten.
fn canonical_simd_application(kind: &TyKind) -> Option<Ty> {
    let TyKind::Struct(name, args) = kind else {
        return None;
    };
    let [TyArg::Val(dtype), TyArg::Val(length)] = args.as_slice() else {
        return None;
    };
    if name != SIMD_TYPE_NAME {
        return None;
    }
    let dtype = simd_dtype_slot(dtype)?;
    let width = match (length, known_simd_length(length)) {
        (_, Some(lanes)) => SimdWidth::Known(lanes),
        (CtValue::Struct { name, .. }, None) if name == SIMD_LENGTH_TYPE_NAME => {
            SimdWidth::of_length(
                &ParamContext::detached(),
                ParamContext::detached().constant(length.clone()).ok()?,
            )
            .ok()?
        }
        (CtValue::Expr(expr), None) => {
            SimdWidth::of_length(&ParamContext::detached(), expr.clone()).ok()?
        }
        _ => return None,
    };
    simd_ty_from_slots(dtype, width).ok()
}

/// The SIMD type of two slots that may still be symbolic.
///
/// A slot whose expression is a closed constant becomes `Known`, and two
/// known slots canonicalize through [`canonical_simd_ty`]. This is the
/// constructor of a symbolic `SIMD` application from new slots, so `Expr` never
/// holds a closed expression; [`Ty::simd`] only reassembles slots read
/// off a vector or a MIR instruction.
pub fn simd_ty_from_slots(dtype: SimdDtype, width: SimdWidth) -> Result<Ty, ParamError> {
    let dtype = match dtype {
        SimdDtype::Expr(expr) => match expr.as_constant() {
            Some(value) if let Some(dtype) = dtype_of_value(value) => SimdDtype::Known(dtype),
            Some(other) => {
                return Err(ParamError::TypeMismatch {
                    operation: "SIMD element type".to_string(),
                    expected: "DType".to_string(),
                    found: other.to_string(),
                });
            }
            None => SimdDtype::Expr(expr),
        },
        known @ SimdDtype::Known(_) => known,
    };
    let width = match width {
        SimdWidth::Expr(length) => SimdWidth::of_length(&ParamContext::detached(), length)?,
        known @ SimdWidth::Known(_) => known,
    };
    Ok(match (dtype, width) {
        (SimdDtype::Known(dtype), SimdWidth::Known(width)) => canonical_simd_ty(dtype, width),
        (dtype, width) => simd_node(dtype, width),
    })
}

/// The `(dtype, width)` shape of a SIMD-valued type — an exact literal
/// reports the width-one vector it materializes to — or `None` for a non-SIMD
/// type.
///
/// `Bool` is not a SIMD value (upstream's `Bool` is its own struct;
/// `Scalar[DType.bool]` is the vector).
pub fn simd_shape(ty: &TyKind) -> Option<(Dtype, i64)> {
    Some(match ty {
        kind if kind.is_int_literal() => (Dtype::Int, 1),
        kind if kind.is_float_literal() => (Dtype::Float64, 1),
        _ => return ty.known_simd(),
    })
}

/// The slots of a SIMD-valued type, symbolic or not — [`simd_shape`] for a
/// declaration that is still a template.
pub fn simd_slots(ty: &TyKind) -> Option<(SimdDtype, SimdWidth)> {
    ty.as_simd().or_else(|| {
        simd_shape(ty).map(|(dtype, width)| (SimdDtype::Known(dtype), SimdWidth::Known(width)))
    })
}

/// The declared struct `ty` is an instance of, and its arguments.
///
/// A struct application answers its own name and arguments, a literal's
/// (`StringLiteral[value]`, `IntLiteral[value]`) included.
pub fn nominal_parts(ty: &Ty) -> Option<(&str, std::borrow::Cow<'_, [TyArg]>)> {
    match ty.kind() {
        TyKind::Struct(name, args) => Some((name, std::borrow::Cow::Borrowed(args))),
        _ => None,
    }
}

/// The type of a string literal's bytes' address: a `Pointer[Byte]` into
/// the static memory that lives for the whole program.
pub fn static_string_address_ty() -> Ty {
    TyKind::Pointer {
        element: canonical_simd_ty(Dtype::UInt8, 1),
        origin: crate::origin::PointerOrigin::Static,
    }
    .into()
}

/// Whether `ty` is a width-one `SIMD` (a scalar alias such as `Int`
/// or `Float32`), whatever its dtype slot.
pub fn is_scalar_simd(ty: &TyKind) -> bool {
    matches!(ty.as_simd(), Some((_, SimdWidth::Known(1))))
}

/// Whether `ty` is `Int`, `UInt`, or `Float64`: a width-one `int`, `uint`,
/// or `float64` vector, whose value both backends hold as a host scalar.
///
/// Above the waist those three keep an infix operator path of their own
/// where upstream's are ordinary `SIMD` instances: it serves their `**`
/// and materializes a literal operand. Below it, a host
/// scalar's `binary` comparison defines its `Bool` directly, where any
/// other vector's defines a lane mask.
pub fn has_host_scalar_value(ty: &TyKind) -> bool {
    ty.is_int() || ty.is_uint() || ty.is_float64()
}

/// The message of the `comptime assert Self.length == 1` of upstream's
/// implicit `SIMD.__init__(out self: SIMD[DType.bool, Self.length], value:
/// Bool)`, verbatim.
pub const BOOL_MASK_CONVERSION_MESSAGE: &str =
    "must be a scalar; use the `fill` keyword instead for explicit splatting";

/// The message of the `comptime assert Self.dtype.is_floating_point()` of
/// upstream's implicit `SIMD.__init__(FloatLiteral)`, verbatim.
///
/// That is the overload an `Int(3.9)` or `Int8(3.9)` binds, ahead of the
/// generic `Intable` one.
pub const FLOAT_LITERAL_CONVERSION_MESSAGE: &str = "the SIMD type must be floating point";

/// The message of the `comptime assert target_dtype.is_integral()` of
/// upstream's `SIMD.__init__[T: Intable, target_dtype](value: T)`, verbatim.
pub const INTABLE_CONVERSION_MESSAGE: &str =
    "constructing from an `Intable` value requires an integral dtype";

/// The known dtype of a width-one `SIMD`.
pub fn scalar_simd_dtype(ty: &TyKind) -> Option<Dtype> {
    match ty.known_simd()? {
        (dtype, 1) => Some(dtype),
        _ => None,
    }
}

/// The float an integer literal of known value converts to.
///
/// It is the `FloatLiteral[v]` value an `IntLiteral[n]` argument binds at a
/// float literal pattern: upstream's `@implicit
/// FloatLiteral.__init__(IntLiteral[_])` (`#pop<int_to_float_literal>`)
/// applied before deduction, so `g(3)` over `def g(x: FloatLiteral)` solves
/// `x.value = 3.0`. `None` for any other type, a symbolic value included.
pub fn int_literal_as_float(actual: &Ty) -> Option<CtValue> {
    actual
        .kind()
        .int_literal_value()
        .map(|value| CtValue::PopFloatLiteral(FloatLiteral::from_int(value)))
}

/// The runtime type an exact literal materializes to when no context selects
/// another (`IntLiteral` → `Int`, `FloatLiteral` → `Float64`), applied through
/// struct arguments, packs, and variants.
pub fn default_literal(ty: &Ty) -> Ty {
    match ty.kind() {
        kind if kind.is_int_literal() => Ty::int(),
        kind if kind.is_float_literal() => Ty::float64(),
        TyKind::Struct(..) if ty.is_closed() => ty.clone(),
        TyKind::Struct(name, arguments) => ty.reusing(TyKind::Struct(
            name.clone(),
            arguments
                .iter()
                .map(|argument| match argument {
                    TyArg::Ty(ty) => TyArg::Ty(default_literal(ty)),
                    TyArg::Val(value) => TyArg::Val(value.clone()),
                    TyArg::Origin(origin) => TyArg::Origin(origin.clone()),
                })
                .collect(),
        )),
        // Internal heterogeneous pack storage also materializes its elements.
        TyKind::Tuple(elems) => TyKind::Tuple(elems.iter().map(default_literal).collect()).into(),
        TyKind::VariadicPack(element) => TyKind::VariadicPack(default_literal(element)).into(),
        TyKind::RuntimePack(elems) => {
            TyKind::RuntimePack(elems.iter().map(default_literal).collect()).into()
        }
        TyKind::Variant(alternatives) => {
            TyKind::Variant(alternatives.iter().map(default_literal).collect()).into()
        }
        _ => ty.clone(),
    }
}

/// Whether a value of type `from` can be used where `to` is required.
///
/// Only the literal types coerce (to the concrete numeric types, or
/// `IntLiteral` up to `FloatLiteral`); everything else must match exactly.
///
/// # Panics
///
/// Panics if the `Tuple` guard above admits a type without element types,
/// which the type lattice forbids.
pub fn coerces(from: &Ty, to: &Ty) -> bool {
    if from == to {
        return true;
    }
    if let Some((to_dtype, to_width)) = to.kind().as_simd() {
        return coerces_to_simd(from.kind(), &to_dtype, &to_width);
    }
    match (from.kind(), to.kind()) {
        (TyKind::Param { binder: a, .. }, TyKind::Param { binder: b, .. }) => a == b,
        // Two carried values agree only when equal, and an integer carries
        // over to the float its `#pop<int_to_float_literal>` is, upstream's
        // `@implicit FloatLiteral.__init__(IntLiteral[_])`.
        (from_kind, to_kind) if from_kind.is_int_literal() && to_kind.is_float_literal() => {
            match (
                from.kind().int_literal_value(),
                to.kind().float_literal_value(),
            ) {
                (Some(int), Some(float)) => FloatLiteral::from_int(int) == *float,
                _ => false,
            }
        }
        (TyKind::Struct(an, aargs), TyKind::Struct(bn, bargs)) => {
            an == bn
                && aargs.len() == bargs.len()
                && aargs.iter().zip(bargs).all(|(a, b)| match (a, b) {
                    (TyArg::Ty(a), TyArg::Ty(b)) => coerces(a, b),
                    (TyArg::Val(a), TyArg::Val(b)) => a == b,
                    // The origin tail is part of the struct's identity: an
                    // unbound slot infers, a bound one must match.
                    (TyArg::Origin(a), TyArg::Origin(b)) => a.coerces_to(b),
                    _ => false,
                })
        }
        (TyKind::ComptimeList(a), TyKind::ComptimeList(b)) => coerces(a, b),
        // A generator value converts as a generator argument does, but its
        // type spells its binders, so their names must agree too.
        (TyKind::Generator(a), TyKind::Generator(b)) => {
            a.params
                .iter()
                .zip(&b.params)
                .all(|(a, b)| a.name() == b.name())
                && generator_converts(from, to)
        }
        (TyKind::ParamList(a), TyKind::ParamList(b)) => a == b,
        (TyKind::AnyTrait(a), TyKind::AnyTrait(b)) => a == b,
        (TyKind::Trait(a), TyKind::Trait(b)) => a == b,
        (TyKind::TypeType, TyKind::TypeType)
        | (TyKind::KgenString, TyKind::KgenString)
        | (TyKind::PopIntLiteral, TyKind::PopIntLiteral)
        | (TyKind::PopFloatLiteral, TyKind::PopFloatLiteral)
        | (TyKind::KgenDeferred, TyKind::KgenDeferred)
        | (TyKind::KgenTarget, TyKind::KgenTarget)
        | (TyKind::Index, TyKind::Index)
        | (TyKind::I1, TyKind::I1)
        | (TyKind::Ui8, TyKind::Ui8)
        | (TyKind::KgenDtype, TyKind::KgenDtype) => true,
        (
            TyKind::Pointer {
                element: a,
                origin: ao,
            },
            TyKind::Pointer {
                element: b,
                origin: bo,
            },
        ) => coerces(a, b) && ao.coerces_to(bo),
        (TyKind::KgenPointer(a), TyKind::KgenPointer(b)) => coerces(a, b),
        (
            TyKind::LitRef {
                element: a,
                origin: ao,
            },
            TyKind::LitRef {
                element: b,
                origin: bo,
            },
        ) => coerces(a, b) && ao.coerces_to(bo),
        (
            TyKind::LitRefPack {
                elements: a,
                origin: ao,
            },
            TyKind::LitRefPack {
                elements: b,
                origin: bo,
            },
        ) => a.len() == b.len() && a.iter().zip(b).all(|(a, b)| coerces(a, b)) && ao.coerces_to(bo),
        (TyKind::Func(from), TyKind::Func(to)) => {
            let FuncTy {
                environment: from_environment,
                params: from_params,
                ret: from_ret,
                required,
                variadic,
                conventions,
                raises: from_raises,
                error: from_error,
                ref_params: from_ref_params,
                ref_return: from_ref_return,
                ..
            } = &**from;
            let FuncTy {
                environment: to_environment,
                params: to_params,
                ret: to_ret,
                required: to_required,
                variadic: to_variadic,
                conventions: to_conventions,
                raises: to_raises,
                error: to_error,
                ref_params: to_ref_params,
                ref_return: to_ref_return,
                ..
            } = &**to;
            callable_environment_value_coerces(from_environment, to_environment)
                && required == to_required
                && variadic.is_none()
                && to_variadic.is_none()
                && conventions == to_conventions
                // Reference conventions are not represented by the ordinary
                // parameter/result `Ty`s. They carry the storage origin and
                // permission contract, so erasing them here could coerce a
                // value-returning callable to a reference-returning contract,
                // or silently rebase a result from one argument to another.
                && from_ref_params == to_ref_params
                && from_ref_return == to_ref_return
                && (!from_raises || *to_raises)
                && match (from_error.as_ref().map(Ty::kind), to_error.as_ref().map(Ty::kind)) {
                    (None, None) => true,
                    (None, Some(TyKind::Never)) => true,
                    (None, Some(_)) => true,
                    (Some(from), Some(to)) if to.is_error() => *from != TyKind::Never,
                    (Some(from), Some(to)) => from == to,
                    (Some(TyKind::Never), None) => true,
                    (Some(_), None) => false,
                }
                && from_params.len() == to_params.len()
                && from_params
                    .iter()
                    .zip(to_params)
                    .all(|(from, to)| from == to)
                && from_ret == to_ret
        }
        // A tuple coerces element-wise (same arity) — so a literal element
        // materializes: `(1, 2.0)` fits `Tuple[Float64, Float64]`.
        (TyKind::Tuple(a), TyKind::Tuple(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| coerces(x, y))
        }
        (TyKind::Variant(a), TyKind::Variant(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| coerces(x, y))
        }
        _ => false,
    }
}

/// Whether a value of type `from` coerces to `SIMD[to_dtype, to_width]`
/// through one of the struct's `@implicit` initializers.
fn coerces_to_simd(from: &TyKind, to_dtype: &SimdDtype, to_width: &SimdWidth) -> bool {
    match (from, from.as_simd()) {
        (_, _) if matches!(to_width, SimdWidth::Known(1)) && splats_to(from, to_dtype) => true,
        // Upstream's `@implicit SIMD.__init__(out self: SIMD[.bool,
        // Self.length], value: Bool)`, whose `comptime assert` of one lane
        // each reached instance judges.
        (kind, _) if kind.is_bool() => {
            *to_dtype == SimdDtype::Known(Dtype::Bool) && !to_width.is_inferred()
        }
        // Upstream's `@implicit SIMD.__init__(IntLiteral)` and
        // `(FloatLiteral)` splat an exact literal across every lane, a
        // symbolic lane count included.
        (kind, _) if kind.is_numeric_literal() => {
            !to_width.is_inferred() && splats_to(from, to_dtype)
        }
        (_, Some((from_dtype, from_width))) => {
            to_width.is_inferred()
                && from_dtype == *to_dtype
                && from_width.known().is_none_or(|width| width > 0)
        }
        _ => false,
    }
}

/// Whether a value of type `ty` can be a `dtype` SIMD element (a construction
/// argument, or the non-SIMD operand of an elementwise operator that splats).
///
/// An integer literal fits any lane, as upstream's unconstrained
/// `SIMD.__init__(IntLiteral)` does (a `bool` lane is whether it is
/// nonzero); a float literal fits a float lane; a same-dtype width-1 SIMD
/// fits. Into a symbolic dtype a float literal fits too (the literal's fit
/// is the instantiation's to check, as upstream), a concrete scalar never
/// does, and a width-one vector of the same expression does.
pub fn splats_to(ty: &TyKind, dtype: &SimdDtype) -> bool {
    match (ty, dtype) {
        (kind, _) if kind.is_int_literal() => true,
        (kind, SimdDtype::Known(dtype)) if kind.is_float_literal() => dtype.is_float(),
        (kind, SimdDtype::Expr(_)) if kind.is_float_literal() => true,
        (kind, SimdDtype::Known(dtype)) if kind.is_bool() => *dtype == Dtype::Bool,
        (kind, dtype) => {
            matches!(kind.as_simd(), Some((d, SimdWidth::Known(1))) if d == *dtype)
        }
    }
}

/// The value-coercion policy for callable environments.
///
/// A capturing callable does not bind to an unqualified `def(...)` value
/// position, while a thin function value still binds.
///
/// Comptime callable *bounds* stay on the permissive
/// `callable_environment_coerces` below.
pub fn callable_environment_value_coerces(
    from: &crate::origin::CallableEnvironment,
    to: &crate::origin::CallableEnvironment,
) -> bool {
    use crate::origin::CallableEnvironment;
    match (from, to) {
        (CallableEnvironment::Capturing(_), CallableEnvironment::Default) => false,
        _ => callable_environment_coerces(from, to),
    }
}

pub fn callable_environment_coerces(
    from: &crate::origin::CallableEnvironment,
    to: &crate::origin::CallableEnvironment,
) -> bool {
    use crate::origin::{CallableEnvironment, CaptureOriginSet};
    if from == to {
        return true;
    }
    match (from, to) {
        // An unqualified callable contract does not constrain the environment
        // in the *bound* channel: a supplied callable argument against
        // `F: def(...)` may capture (upstream accepts this — see
        // `subscript_call_contracts.mojo`), so `unify`,
        // `callable_bound_accepts`, and MIR verify stay on this permissive
        // predicate. Runtime value coercion uses the strict
        // `callable_environment_value_coerces` above.
        (
            CallableEnvironment::Thin | CallableEnvironment::Capturing(_),
            CallableEnvironment::Default,
        ) => true,
        // A non-capturing callable satisfies every `capturing[...]` contract:
        // its capture set is empty, a subset of any allowed origin set
        // (upstream accepts a thin function for a capturing funarg).
        (CallableEnvironment::Thin, CallableEnvironment::Capturing(_)) => true,
        (
            CallableEnvironment::Capturing(CaptureOriginSet(actual)),
            CallableEnvironment::Capturing(CaptureOriginSet(allowed)),
        ) => actual.iter().all(|capture| allowed.contains(capture)),
        _ => false,
    }
}

/// Whether `Copyable.copy` on a value of this type has no callee.
///
/// The compiler's own value types (`Bool`, tuples, packs, and variants)
/// copy by the ordinary value read. Structs (a `SIMD`, a `StringLiteral`,
/// and a numeric literal application included),
/// parametric, associated, and reference types resolve their `copy`
/// through declarations or trait dispatch instead.
pub const fn builtin_copy_is_value_read(ty: &TyKind) -> bool {
    !matches!(ty, TyKind::Struct(..))
        && !matches!(
            ty,
            TyKind::Param { .. }
                | TyKind::Assoc { .. }
                | TyKind::Dependent(_)
                | TyKind::Ref(_)
                | TyKind::SelfType
                | TyKind::Func(_)
                | TyKind::GenericFunc(_)
                | TyKind::Overload(_)
        )
}

/// Whether a body reads a value binder of this type from a frame slot
/// rather than as upstream's `kgen.param.constant` of the binder.
///
/// A pack and a raw parameter list keep their own readings. Every other
/// value binder's read, a callable binder's included, is its parameter
/// constant and takes no storage.
pub const fn binder_read_takes_slot(ty: &TyKind) -> bool {
    matches!(ty, TyKind::VariadicPack(_) | TyKind::ParamList(_))
}

/// Whether a copy of a value of this type is its operand.
///
/// The value is a scalar, pointer, `None`, or one SIMD lane, held whole in
/// one register on both backends, so copying it is a bit copy and no
/// `__copyinit__` runs. A lane's dtype may still be a parameter; every
/// instance closes it to a scalar. A copy of a value held in storage is the
/// identity too when its type is register-passable trivial:
/// `MirDeclarations::copy_is_identity` answers for those.
pub fn copy_is_identity(ty: &TyKind) -> bool {
    matches!(
        ty,
        TyKind::Index
            | TyKind::I1
            | TyKind::Ui8
            | TyKind::KgenDtype
            | TyKind::None
            | TyKind::Pointer { .. }
            | TyKind::KgenPointer(_)
            | TyKind::LitRef { .. }
            | TyKind::LitRefPack { .. }
            | TyKind::KgenSimd {
                width: SimdWidth::Known(1),
                ..
            }
    ) || is_scalar_simd(ty)
        || ty.is_numeric_literal()
}

/// Whether this compiler-known type is one upstream declares
/// `TrivialRegisterPassable`: a [`copy_is_identity`] value, a SIMD vector
/// of any dtype and width, or a `StringLiteral`. Its copy is a bit copy.
pub fn trivially_register_passable_leaf(ty: &TyKind) -> bool {
    copy_is_identity(ty)
        || ty.is_simd()
        || ty.is_string_literal()
        || matches!(ty, TyKind::KgenSimd { .. } | TyKind::KgenAddressSimd { .. })
}

/// Whether this KGEN type's values are attributes only.
///
/// The compile-time interpreter holds them as values, and upstream's LLVM
/// type converter has no type for them, so translation rejects one that
/// survives the post-elaboration pipeline.
pub const fn has_no_llvm_type(ty: &TyKind) -> bool {
    matches!(
        ty,
        TyKind::KgenDeferred | TyKind::KgenTarget | TyKind::PopIntLiteral | TyKind::PopFloatLiteral
    )
}

/// Recover the monomorphic or generic callable contract carried either directly
/// by a function type or indirectly by a callable-bounded type parameter.
pub fn callable_contract_ty(ty: &Ty) -> Option<&Ty> {
    match ty.kind() {
        TyKind::Func(_) | TyKind::GenericFunc(_) => Some(ty),
        TyKind::Param {
            callable_bound: Some(bound),
            ..
        } => callable_contract_ty(bound),
        _ => None,
    }
}

/// Whether a concrete monomorphic callable implementation fulfills an
/// anonymous `def(...)` trait contract.
///
/// This is intentionally directional: non-raising/read-only implementations
/// may fulfill raising/mutable contracts, but not vice versa. Binder
/// constraints are directional the other way (upstream 2026-08): every `where`
/// constraint the implementation declares must be declared by the contract —
/// otherwise calls through the contract could violate the implementation's
/// precondition — while an unconstrained implementation may serve a
/// constrained contract.
pub fn callable_bound_accepts(actual: &Ty, contract: &Ty) -> bool {
    if matches!(actual.kind(), TyKind::GenericFunc(_))
        || matches!(contract.kind(), TyKind::GenericFunc(_))
    {
        let (Some((actual_decls, actual)), Some((contract_decls, contract))) = (
            erase_generic_callable_binders(actual),
            erase_generic_callable_binders(contract),
        ) else {
            return false;
        };
        let strip = |decl: &ParamDecl| {
            let mut decl = decl.clone();
            match &mut decl {
                ParamDecl::Type { constraints, .. } | ParamDecl::Value { constraints, .. } => {
                    constraints.clear();
                }
            }
            decl
        };
        let constraints_of = |decl: &ParamDecl| -> Vec<GenericConstraint> {
            match decl {
                ParamDecl::Type { constraints, .. } | ParamDecl::Value { constraints, .. } => {
                    constraints.clone()
                }
            }
        };
        let structural = actual_decls.len() == contract_decls.len()
            && actual_decls
                .iter()
                .zip(&contract_decls)
                .all(|(actual, contract)| strip(actual) == strip(contract));
        let constraints_declared =
            actual_decls
                .iter()
                .zip(&contract_decls)
                .all(|(actual, contract)| {
                    let declared = constraints_of(contract);
                    constraints_of(actual)
                        .iter()
                        .all(|constraint| declared.contains(constraint))
                });
        return structural && constraints_declared && callable_bound_accepts(&actual, &contract);
    }

    let (TyKind::Func(actual_func), TyKind::Func(contract_func)) = (actual.kind(), contract.kind())
    else {
        return false;
    };
    let FuncTy {
        environment: actual_environment,
        params: actual_params,
        ret: actual_ret,
        required: actual_required,
        variadic: actual_variadic,
        kw_variadic: actual_kw_variadic,
        positional_only: actual_positional_only,
        keyword_only: actual_keyword_only,
        raises: actual_raises,
        error: actual_error,
        conventions: actual_conventions,
        ref_params: actual_ref_params,
        ref_return: actual_ref_return,
        ..
    } = &**actual_func;
    let FuncTy {
        environment: contract_environment,
        params: contract_params,
        ret: contract_ret,
        required: contract_required,
        variadic: contract_variadic,
        kw_variadic: contract_kw_variadic,
        positional_only: contract_positional_only,
        keyword_only: contract_keyword_only,
        raises: contract_raises,
        error: contract_error,
        conventions: contract_conventions,
        ref_params: contract_ref_params,
        ref_return: contract_ref_return,
        ..
    } = &**contract_func;

    callable_environment_coerces(actual_environment, contract_environment)
        && actual_params.len() == contract_params.len()
        && actual_params
            .iter()
            .zip(contract_params)
            .all(|(actual, contract)| actual == contract)
        && coerces(actual_ret, contract_ret)
        && actual_required.len() == contract_required.len()
        && actual_required
            .iter()
            .zip(contract_required)
            .all(|(actual, contract)| !*actual || *contract)
        && actual_variadic.is_none()
        && contract_variadic.is_none()
        && actual_kw_variadic.is_none()
        && contract_kw_variadic.is_none()
        && actual_positional_only == contract_positional_only
        && actual_keyword_only == contract_keyword_only
        && actual_conventions.len() == contract_conventions.len()
        && actual_conventions
            .iter()
            .zip(contract_conventions)
            .all(|(actual, contract)| callable_convention_accepts(*actual, *contract))
        && actual_ref_params == contract_ref_params
        && actual_ref_return == contract_ref_return
        && (!*actual_raises || *contract_raises)
        && match (
            actual_error.as_ref().map(Ty::kind),
            contract_error.as_ref().map(Ty::kind),
        ) {
            (None | Some(TyKind::Never), _) => true,
            (Some(_), None) => false,
            (Some(actual), Some(contract)) if contract.is_error() => *actual != TyKind::Never,
            (Some(actual), Some(contract)) => actual == contract,
        }
}

/// Alpha-normalize a generic anonymous callable into its declaration list and
/// a monomorphic callable shape whose parameter occurrences use canonical `$N`
/// names.
///
/// Generic callable compatibility can then reuse the ordinary directional
/// callable-contract rules without making source binder spelling part of the
/// type identity.
pub fn erase_generic_callable_binders(callable: &Ty) -> Option<(Vec<ParamDecl>, Ty)> {
    let TyKind::GenericFunc(generic) = callable.kind() else {
        return None;
    };
    let GenericFuncTy {
        decls,
        func:
            FuncTy {
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
            },
    } = &**generic;

    let mut signature = params.clone();
    let variadic_index = variadic.as_ref().map(|parameter| {
        let index = signature.len();
        signature.push((*parameter).clone());
        index
    });
    let kw_variadic_index = kw_variadic.as_ref().map(|parameter| {
        let index = signature.len();
        signature.push((*parameter).clone());
        index
    });
    let return_index = signature.len();
    signature.push((*ret).clone());
    let error_index = error.as_ref().map(|error| {
        let index = signature.len();
        signature.push((*error).clone());
        index
    });
    let (decls, signature) = canonical_generic_signature(decls, &signature);

    Some((
        decls,
        TyKind::Func(Box::new(FuncTy {
            environment: environment.clone(),
            params: signature[..params.len()].to_vec(),
            names: names.clone(),
            ret: signature[return_index].clone(),
            required: required.clone(),
            variadic: variadic_index.map(|index| signature[index].clone()),
            kw_variadic: kw_variadic_index.map(|index| signature[index].clone()),
            positional_only: *positional_only,
            keyword_only: *keyword_only,
            raises: *raises,
            error: error_index.map(|index| signature[index].clone()),
            conventions: conventions.clone(),
            ref_params: ref_params.clone(),
            ref_return: ref_return.clone(),
            transfers: transfers.clone(),
        }))
        .into(),
    ))
}

pub fn canonical_generic_signature(
    decls: &[ParamDecl],
    params: &[Ty],
) -> (Vec<ParamDecl>, Vec<Ty>) {
    let identity_constraints = |constraints: &[GenericConstraint]| {
        constraints
            .iter()
            .map(|constraint| match constraint {
                GenericConstraint::WithMessage(condition, _) => (**condition).clone(),
                constraint => constraint.clone(),
            })
            .collect()
    };
    let mut subst = TySubst::new();
    let mut binders: HashMap<ParamId, ParamRef> = HashMap::new();
    // A binder's references become signature slots — a type binder the
    // contract-owned identity `($contract, index)`, a value binder an index
    // reference — so two contracts that differ only in their binders'
    // spelling are one identity.
    let mut value_slots = ParamBindings::new();
    let canonical_decls = decls
        .iter()
        .enumerate()
        .map(|(index, decl)| match decl {
            ParamDecl::Type {
                id,
                name: _,
                bounds,
                trait_param,
                callable_bound,
                default: _,
                infer_only: _,
                variadic,
                constraints,
            } => {
                let canonical_name = format!("${index}");
                let canonical_id = ParamId::new(CONTRACT_BINDER_OWNER, index);
                let canonical_callable_bound = callable_bound
                    .as_ref()
                    .map(|bound| bind_signature_slots(&substitute(bound, &subst), &value_slots));
                subst.insert(
                    id.clone(),
                    TyKind::Param {
                        binder: ParamRef {
                            id: canonical_id.clone(),
                            name: canonical_name.as_str().into(),
                        },
                        bounds: bounds.clone(),
                        callable_bound: canonical_callable_bound.clone(),
                    }
                    .into(),
                );
                // A pack is a parameter list a dependent type reads
                // (`Us[1]`), so its references there become the slot too.
                if *variadic {
                    value_slots.bind(
                        id.clone(),
                        ParamContext::detached().decl_ref(
                            canonical_id.clone(),
                            &canonical_name,
                            decl.binder_meta(),
                        ),
                    );
                }
                binders.insert(
                    id.clone(),
                    ParamRef {
                        id: canonical_id.clone(),
                        name: canonical_name.as_str().into(),
                    },
                );
                ParamDecl::Type {
                    id: canonical_id,
                    name: canonical_name,
                    bounds: bounds.clone(),
                    trait_param: trait_param.as_ref().map(|binder| {
                        binders
                            .get(&binder.id)
                            .cloned()
                            .unwrap_or_else(|| binder.clone())
                    }),
                    callable_bound: canonical_callable_bound,
                    // Binder defaults and the `//` inference marker govern a
                    // call through the contract; current Mojo does not make
                    // either part of generic callable conformance identity.
                    default: None,
                    infer_only: false,
                    variadic: *variadic,
                    constraints: identity_constraints(constraints),
                }
            }
            ParamDecl::Value {
                id,
                name,
                ty,
                default: _,
                callable_default: _,
                infer_only: _,
                variadic,
                constraints,
            } => {
                let canonical_name = format!("${index}");
                let canonical_id = ParamId::new(CONTRACT_BINDER_OWNER, index);
                let canonical_ty = bind_signature_slots(&substitute(ty, &subst), &value_slots);
                value_slots.bind_name(
                    name,
                    ParamContext::detached().index_ref(
                        0,
                        u32::try_from(index).unwrap_or(u32::MAX),
                        MetaTy::value((*ty).clone()),
                    ),
                );
                binders.insert(
                    id.clone(),
                    ParamRef {
                        id: canonical_id.clone(),
                        name: canonical_name.as_str().into(),
                    },
                );
                ParamDecl::Value {
                    id: canonical_id,
                    name: canonical_name,
                    ty: canonical_ty,
                    default: None,
                    callable_default: None,
                    infer_only: false,
                    variadic: *variadic,
                    constraints: identity_constraints(constraints),
                }
            }
        })
        .collect();
    let canonical_params = params
        .iter()
        .map(|ty| bind_signature_slots(&substitute(ty, &subst), &value_slots))
        .collect();
    // Second pass: alpha-rename binder references INSIDE the retained
    // constraints, so `def[w: Int](…) where w > 0` and `def[n: Int](…) where
    // n > 0` share one canonical identity. The maps are complete only after
    // the fold above (a clause on the last binder may reference any of them);
    // names the contract does not bind (an enclosing declaration's
    // parameters) stay as-is and correctly distinguish contracts.
    let mut canonical_decls: Vec<ParamDecl> = canonical_decls;
    for decl in &mut canonical_decls {
        match decl {
            ParamDecl::Type { constraints, .. } | ParamDecl::Value { constraints, .. } => {
                *constraints = constraints
                    .iter()
                    .map(|constraint| {
                        rename_constraint_parameters(constraint, &binders, &subst, &value_slots)
                    })
                    .collect();
            }
        }
    }
    (canonical_decls, canonical_params)
}

/// `decls` owned by the declaration `owner` instead.
///
/// Each binder keeps its slot and spelling, and the bindings returned move a
/// reference to one of them ([`replace_parameters`]). Upstream renames a
/// trait function's binders into the wrapper a conformer gets for its
/// default this way (`replaceIndexRefsWithNamedRefs`).
pub fn reown_generic_signature(
    decls: &[ParamDecl],
    owner: &str,
) -> (Vec<ParamDecl>, ParamBindings) {
    reown_parameters(decls, owner, |_, decl| decl.id().slot)
}

/// `decls` declared as `owner`'s own, each at its position in `decls`.
///
/// They are gathered from the declarations they are in scope of, and the
/// bindings returned move a reference to one of them
/// ([`reown_generic_signature`]). Upstream's `liftClosure` declares the
/// enclosing binders a closure names as its storage struct's own this way,
/// reusing their spellings.
pub fn reown_as_own_parameters(
    decls: &[ParamDecl],
    owner: &str,
) -> (Vec<ParamDecl>, ParamBindings) {
    reown_parameters(decls, owner, |position, _| position)
}

/// The bindings moving a reference to each of `decls` to the binder of the
/// same position in `own`, the declarations [`reown_as_own_parameters`]
/// gave them as another declaration's own.
pub fn own_parameter_bindings(decls: &[ParamDecl], own: &[ParamDecl]) -> ParamBindings {
    let mut bindings = ParamBindings::new();
    for (decl, own) in decls.iter().zip(own) {
        bind_reowned(&mut bindings, decl, &own.binder());
    }
    bindings
}

/// The substitution mapping a struct's type-parameter names to an instance's
/// type arguments (`[T] @ [Int]` ⟹ `{T: Int}`), for [`substitute`].
///
/// Value parameters and arguments are skipped; [`struct_field_types`]
/// binds them too, for a value binder a field's lane slot or value
/// argument names. Empty for a non-generic struct.
pub fn struct_argument_substitution(decls: &[ParamDecl], arguments: &[TyArg]) -> TySubst {
    decls
        .iter()
        .zip(arguments)
        .filter_map(|(decl, argument)| match (decl, argument) {
            (ParamDecl::Type { id, .. }, TyArg::Ty(ty)) => Some((id.clone(), ty.clone())),
            _ => None,
        })
        .collect()
}

/// A struct's declared `fields` at an application's `arguments`.
///
/// Upstream's `!kgen.struct` element types are the field types at the
/// struct's arguments. Each type and value binder `decls` declares is bound
/// to its argument and replaced through [`replace_parameters`] (a lane slot
/// or value argument re-canonicalizes), and a type pack bound whole to its
/// element types is expanded in their place (`Tuple([*Ts])` at `Ts = [A, B]`
/// is `Tuple([A, B])`); the fields are borrowed when none names a binder.
/// Origin arguments are left as declared.
///
/// `None` when a field still names one of the struct's own binders: a
/// missing, erased (`CtValue::RemovedArg`), or unbindable argument, or a
/// pack the substitution does not expand.
pub fn struct_field_types<'a>(
    decls: &[ParamDecl],
    arguments: &[TyArg],
    fields: &'a [(String, Ty)],
) -> Option<std::borrow::Cow<'a, [(String, Ty)]>> {
    let binders: Vec<ParamRef> = decls.iter().map(ParamDecl::binder).collect();
    let names_own_binder = |ty: &Ty| binders.iter().any(|binder| names_binder(ty, binder));
    if !fields.iter().any(|(_, field)| names_own_binder(field)) {
        return Some(std::borrow::Cow::Borrowed(fields));
    }
    let context = ParamContext::detached();
    let bindings = struct_argument_bindings(&context, decls, arguments);
    let packs = struct_pack_arguments(decls, arguments);
    fields
        .iter()
        .map(|(name, field)| {
            let field = if packs.is_empty() {
                std::borrow::Cow::Borrowed(field)
            } else {
                std::borrow::Cow::Owned(substitute_packs(field, &TySubst::new(), &packs, &[]))
            };
            replace_parameters(&context, &field, &bindings, 0)
                .ok()
                .filter(|field| !names_own_binder(field))
                .map(|field| (name.clone(), field))
        })
        .collect::<Option<Vec<_>>>()
        .map(std::borrow::Cow::Owned)
}

/// The bindings a struct application's `arguments` give its declared
/// non-variadic binders `decls`.
///
/// Each binds by identity: a type binder its type and a value binder its
/// constant. An argument the elaborator removed binds nothing.
pub fn struct_argument_bindings(
    context: &ParamContext,
    decls: &[ParamDecl],
    arguments: &[TyArg],
) -> ParamBindings {
    let mut bindings = ParamBindings::new();
    for (decl, argument) in decls.iter().zip(arguments) {
        match (decl, argument) {
            (
                ParamDecl::Type {
                    id,
                    variadic: false,
                    ..
                },
                TyArg::Ty(ty),
            ) => {
                bindings.bind(id.clone(), context.type_shape(ty.clone()));
                bindings.bind_type(id.clone(), ty.clone());
            }
            (
                ParamDecl::Value {
                    id,
                    variadic: false,
                    ..
                },
                TyArg::Val(value),
            ) if !matches!(value, CtValue::RemovedArg) => {
                if let Ok(value) = context.constant(value.clone()) {
                    bindings.bind(id.clone(), value);
                }
            }
            _ => {}
        }
    }
    bindings
}

/// [`substitute`] under the solutions of type packs and of compile-time
/// values as well.
///
/// A pack binder stands for its element list: bare, it is the runtime pack
/// of the elements; a variadic collector over it is the tuple of the
/// elements, as a specialized collector binds; a spread of it flattens into
/// the argument list it spreads. A dependent type is replaced under every
/// binding — a pack's elements, a value binder such as an unrolled `comptime
/// for`'s index — and folds to the type it then denotes, so `Ts[i]` at
/// `Ts = [Int, Bool]`, `i = 1` is `Bool`. A lane slot or a struct's value
/// argument naming a value binder closes the same way.
pub fn substitute_packs<S: std::hash::BuildHasher>(
    ty: &Ty,
    subst: &TySubst,
    packs: &HashMap<ParamId, Vec<Ty>, S>,
    values: &[(ParamId, CtValue)],
) -> Ty {
    substitute(&expand_packs(ty, subst, packs, values), subst)
}

/// Replace every `TyKind::Param` in `ty` with its solution from `subst` (leaving an
/// unsolved parameter untouched), including types embedded in parameter
/// expressions and SIMD slots.
pub fn substitute(ty: &Ty, subst: &TySubst) -> Ty {
    match ty.kind() {
        TyKind::Param {
            binder,
            bounds,
            callable_bound,
        } => subst.get(&binder.id).cloned().unwrap_or_else(|| {
            TyKind::Param {
                binder: binder.clone(),
                bounds: bounds.clone(),
                callable_bound: callable_bound
                    .as_ref()
                    .map(|bound| substitute(bound, subst)),
            }
            .into()
        }),
        TyKind::Struct(..) if ty.is_closed() => ty.clone(),
        TyKind::Struct(name, args) => {
            let mut args = map_tyargs(args, |t| substitute(t, subst));
            // A value argument's expression names type binders too (`Box[T.K]`).
            if !subst.is_empty()
                && args
                    .iter()
                    .any(|argument| matches!(argument, TyArg::Val(CtValue::Expr(_))))
            {
                let bindings = type_bindings(subst);
                let context = ParamContext::detached();
                for argument in &mut args {
                    if let TyArg::Val(value @ CtValue::Expr(_)) = argument
                        && let Ok(replaced) = replace_value_parameters(&context, value, &bindings)
                    {
                        *value = replaced;
                    }
                }
            }
            ty.reusing(TyKind::Struct(name.clone(), args))
        }
        TyKind::Dependent(_) | TyKind::KgenSimd { .. } | TyKind::KgenAddressSimd { .. }
            if is_symbolic(ty) =>
        {
            replace_parameters(&ParamContext::detached(), ty, &type_bindings(subst), 0)
                .unwrap_or_else(|_| ty.clone())
        }
        TyKind::ComptimeList(elem) => TyKind::ComptimeList(substitute(elem, subst)).into(),
        TyKind::ParamList(elem) => TyKind::ParamList(substitute(elem, subst)).into(),
        TyKind::Tuple(elems) => {
            TyKind::Tuple(elems.iter().map(|t| substitute(t, subst)).collect()).into()
        }
        TyKind::RuntimePack(elems) => {
            TyKind::RuntimePack(elems.iter().map(|t| substitute(t, subst)).collect()).into()
        }
        TyKind::VariadicPack(element) => TyKind::VariadicPack(substitute(element, subst)).into(),
        TyKind::PopArray { size, element } => TyKind::PopArray {
            size: match size {
                ArraySize::Expr(_) if !subst.is_empty() => {
                    let context = ParamContext::detached();
                    size.try_map(|size| context.replace(size, &type_bindings(subst)))
                        .unwrap_or_else(|_| size.clone())
                }
                _ => size.clone(),
            },
            element: substitute(element, subst),
        }
        .into(),
        TyKind::Variant(alternatives) => TyKind::Variant(
            alternatives
                .iter()
                .map(|ty| substitute(ty, subst))
                .collect(),
        )
        .into(),
        TyKind::Pointer { element, origin } => TyKind::Pointer {
            element: substitute(element, subst),
            origin: origin.clone(),
        }
        .into(),
        TyKind::KgenPointer(element) => TyKind::KgenPointer(substitute(element, subst)).into(),
        TyKind::LitRef { element, origin } => TyKind::LitRef {
            element: substitute(element, subst),
            origin: origin.clone(),
        }
        .into(),
        TyKind::LitRefPack { elements, origin } => TyKind::LitRefPack {
            elements: elements.iter().map(|t| substitute(t, subst)).collect(),
            origin: origin.clone(),
        }
        .into(),
        TyKind::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = substitute(&reference.referent, subst);
            TyKind::Ref(reference).into()
        }
        TyKind::Assoc { base, name, args } => TyKind::Assoc {
            base: substitute(base, subst),
            name: name.clone(),
            args: map_tyargs(args, |t| substitute(t, subst)),
        }
        .into(),
        TyKind::Func(func) => {
            TyKind::Func(Box::new(func.map_types(|ty| substitute(ty, subst)))).into()
        }
        TyKind::GenericFunc(generic) => {
            let decls = &generic.decls;
            // An anonymous callable's own binders shadow names from the
            // surrounding substitution. Outer parameters may still occur in
            // its bounds and signature, so substitute with only those shadowed
            // entries removed.
            let mut nested = subst.clone();
            for declaration in decls {
                nested.remove(declaration.id());
            }
            let decls = decls
                .iter()
                .map(|declaration| match declaration {
                    ParamDecl::Type {
                        id,
                        name,
                        bounds,
                        trait_param,
                        callable_bound,
                        default,
                        infer_only,
                        variadic,
                        constraints,
                    } => ParamDecl::Type {
                        id: id.clone(),
                        name: name.clone(),
                        bounds: bounds.clone(),
                        trait_param: trait_param.clone(),
                        callable_bound: callable_bound
                            .as_ref()
                            .map(|bound| substitute(bound, &nested)),
                        default: default.as_ref().map(|default| substitute(default, &nested)),
                        infer_only: *infer_only,
                        variadic: *variadic,
                        constraints: constraints.clone(),
                    },
                    ParamDecl::Value {
                        id,
                        name,
                        ty,
                        default,
                        callable_default,
                        infer_only,
                        variadic,
                        constraints,
                    } => ParamDecl::Value {
                        id: id.clone(),
                        name: name.clone(),
                        ty: substitute(ty, &nested),
                        default: default.clone(),
                        callable_default: callable_default.clone(),
                        infer_only: *infer_only,
                        variadic: *variadic,
                        constraints: constraints.clone(),
                    },
                })
                .collect();
            TyKind::GenericFunc(Box::new(GenericFuncTy {
                decls,
                func: generic.map_types(|ty| substitute(ty, &nested)),
            }))
            .into()
        }
        TyKind::Overload(candidates) => TyKind::Overload(
            candidates
                .iter()
                .map(|candidate| substitute(candidate, subst))
                .collect(),
        )
        .into(),
        TyKind::Generator(generator) => {
            let mut nested = subst.clone();
            for declaration in &generator.params {
                nested.remove(declaration.id());
            }
            let params = generator
                .params
                .iter()
                .map(|declaration| match declaration {
                    ParamDecl::Value { ty, .. } => {
                        let mut declaration = declaration.clone();
                        if let ParamDecl::Value { ty: slot, .. } = &mut declaration {
                            *slot = substitute(ty, &nested);
                        }
                        declaration
                    }
                    ParamDecl::Type { .. } => declaration.clone(),
                })
                .collect();
            Ty::generator(params, substitute(&generator.result, &nested))
        }
        _ => ty.clone(),
    }
}

/// A structural rebuild of a type.
///
/// This is the one traversal behind parameter replacement and
/// signature-binder canonicalization. An implementation says
/// what a type parameter, a value argument's expression, and a signature's
/// own binders mean; [`rewrite_ty`] visits every type, value argument,
/// default, and aggregate child.
pub trait TyRewrite {
    /// The replacement for a whole type, if any, taken before its children
    /// are rebuilt.
    fn whole(&mut self, _ty: &Ty) -> Option<Ty> {
        None
    }

    /// The replacement for the type parameter `binder`, if any.
    fn param(&mut self, _binder: &ParamRef) -> Option<Ty> {
        None
    }

    /// Rewrite one parameter expression.
    fn expr(&mut self, expr: &ParamExpr) -> Result<ParamExpr, ParamError>;

    /// Observe a compile-time value before it is rebuilt.
    fn value(&mut self, _value: &CtValue) {}

    /// A generic callable's own binders come into scope: they shadow
    /// same-spelled outer names and sit one signature level further in.
    fn enter_signature(&mut self, _decls: &[ParamDecl]) {}

    fn exit_signature(&mut self) {}

    /// Whether a [closed](Ty::is_closed) struct type is walked rather
    /// than passed whole. Nothing inside one reaches `param` or `expr`, so
    /// only a rewrite whose `whole` or `value` hook must see inside it opts
    /// in.
    fn visits_closed(&self) -> bool {
        false
    }
}

/// Rebuild `ty` through `rewrite`. Origins, conventions, and transfer effects
/// pass through: they are checked decorations, not parameter positions.
pub fn rewrite_ty(ty: &Ty, rewrite: &mut dyn TyRewrite) -> Result<Ty, ParamError> {
    if let Some(replacement) = rewrite.whole(ty) {
        return Ok(replacement);
    }
    let all = |types: &[Ty], rewrite: &mut dyn TyRewrite| {
        types
            .iter()
            .map(|ty| rewrite_ty(ty, rewrite))
            .collect::<Result<Vec<_>, _>>()
    };
    let optional = |ty: &Option<Ty>, rewrite: &mut dyn TyRewrite| {
        ty.as_ref().map(|ty| rewrite_ty(ty, rewrite)).transpose()
    };
    let list = |types: &[Ty], rewrite: &mut dyn TyRewrite| match bound_pack(types, rewrite) {
        Some(elements) => Ok(elements),
        None => all(types, rewrite),
    };
    Ok(match ty.kind() {
        TyKind::Param {
            binder,
            bounds,
            callable_bound,
        } => match rewrite.param(binder) {
            Some(replacement) => replacement,
            None => TyKind::Param {
                binder: binder.clone(),
                bounds: bounds.clone(),
                callable_bound: optional(callable_bound, rewrite)?,
            }
            .into(),
        },
        TyKind::Struct(..) if ty.is_closed() && !rewrite.visits_closed() => ty.clone(),
        kind if let Some((dtype, width)) = kind.as_simd() => {
            let dtype = match dtype {
                SimdDtype::Expr(expr) => SimdDtype::Expr(rewrite.expr(&expr)?),
                known @ SimdDtype::Known(_) => known,
            };
            let width = match width {
                SimdWidth::Expr(expr) => SimdWidth::Expr(rewrite.expr(&expr)?),
                known @ SimdWidth::Known(_) => known,
            };
            simd_ty_from_slots(dtype, width)?
        }
        // A spread list closes into its elements once the rewrite knows
        // every length in it.
        // A spread of a pack the rewrite binds is its elements, spelled as
        // `expand_pack_spread` spells them.
        TyKind::Struct(name, arguments) => {
            let rewrite = std::cell::RefCell::new(rewrite);
            let spread = spread_struct_arguments(
                name,
                arguments,
                |argument| {
                    let rewrite = &mut **rewrite.borrow_mut();
                    if let Some(list) = list_spread(argument) {
                        return spread_arguments(name, &rewrite.expr(list)?).map(Some);
                    }
                    Ok(bound_pack(std::slice::from_ref(argument), rewrite)
                        .map(|elements| pack_elements_arguments(name, elements)))
                },
                |argument| {
                    rewrite_tyargs(std::slice::from_ref(argument), &mut **rewrite.borrow_mut())
                        .map(|mut rewritten| rewritten.remove(0))
                },
            )?;
            let arguments = match spread {
                Some(arguments) => arguments,
                None => rewrite_tyargs(arguments, *rewrite.borrow_mut())?,
            };
            ty.reusing(TyKind::Struct(name.clone(), arguments))
        }
        TyKind::Dependent(dependent) => DependentType::resolve(rewrite.expr(dependent.expr())?),
        TyKind::ComptimeList(element) => TyKind::ComptimeList(rewrite_ty(element, rewrite)?).into(),
        TyKind::ParamList(element) => TyKind::ParamList(rewrite_ty(element, rewrite)?).into(),
        TyKind::VariadicPack(element) => TyKind::VariadicPack(rewrite_ty(element, rewrite)?).into(),
        TyKind::PopArray { size, element } => TyKind::PopArray {
            size: size.try_map(|size| rewrite.expr(size))?,
            element: rewrite_ty(element, rewrite)?,
        }
        .into(),
        TyKind::Tuple(elements) => TyKind::Tuple(list(elements, rewrite)?).into(),
        TyKind::RuntimePack(elements) => TyKind::RuntimePack(list(elements, rewrite)?).into(),
        TyKind::Variant(alternatives) => TyKind::Variant(list(alternatives, rewrite)?).into(),
        TyKind::Overload(candidates) => TyKind::Overload(all(candidates, rewrite)?).into(),
        TyKind::Pointer { element, origin } => TyKind::Pointer {
            element: rewrite_ty(element, rewrite)?,
            origin: origin.clone(),
        }
        .into(),
        TyKind::KgenPointer(element) => TyKind::KgenPointer(rewrite_ty(element, rewrite)?).into(),
        TyKind::LitRef { element, origin } => TyKind::LitRef {
            element: rewrite_ty(element, rewrite)?,
            origin: origin.clone(),
        }
        .into(),
        TyKind::LitRefPack { elements, origin } => TyKind::LitRefPack {
            elements: list(elements, rewrite)?,
            origin: origin.clone(),
        }
        .into(),
        TyKind::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = rewrite_ty(&reference.referent, rewrite)?;
            TyKind::Ref(reference).into()
        }
        TyKind::Assoc { base, name, args } => TyKind::Assoc {
            base: rewrite_ty(base, rewrite)?,
            name: name.clone(),
            args: rewrite_tyargs(args, rewrite)?,
        }
        .into(),
        TyKind::Func(func) => {
            TyKind::Func(Box::new(func.try_map_types(|ty| rewrite_ty(ty, rewrite))?)).into()
        }
        TyKind::GenericFunc(generic) => {
            rewrite.enter_signature(&generic.decls);
            let rebuilt = (|| {
                Ok(TyKind::GenericFunc(Box::new(GenericFuncTy {
                    decls: generic
                        .decls
                        .iter()
                        .map(|decl| rewrite_decl(decl, rewrite))
                        .collect::<Result<_, _>>()?,
                    func: generic.try_map_types(|ty| rewrite_ty(ty, rewrite))?,
                })))
            })();
            rewrite.exit_signature();
            Ty::new(rebuilt?)
        }
        TyKind::Generator(generator) => {
            rewrite.enter_signature(&generator.params);
            let rebuilt = (|| {
                Ok(TyKind::Generator(Box::new(GeneratorTy {
                    params: generator
                        .params
                        .iter()
                        .map(|decl| rewrite_decl(decl, rewrite))
                        .collect::<Result<_, _>>()?,
                    result: rewrite_ty(&generator.result, rewrite)?,
                })))
            })();
            rewrite.exit_signature();
            Ty::new(rebuilt?)
        }
        TyKind::KgenSimd { dtype, width } => {
            let vector = rewrite_ty(&simd_node(dtype.clone(), width.clone()), rewrite)?;
            simd_storage(&vector).unwrap_or(vector)
        }
        TyKind::KgenAddressSimd { width } => {
            // The width rewrites as an `int` vector's would, the lane
            // standing aside.
            let vector = rewrite_ty(
                &simd_node(SimdDtype::Known(Dtype::Int), width.clone()),
                rewrite,
            )?;
            match vector.kind().as_simd() {
                Some((_, width)) => TyKind::KgenAddressSimd { width }.into(),
                None => vector,
            }
        }
        TyKind::None
        | TyKind::Never
        | TyKind::Infer
        | TyKind::AnyTrait(_)
        | TyKind::Trait(_)
        | TyKind::TypeType
        | TyKind::KgenString
        | TyKind::PopIntLiteral
        | TyKind::PopFloatLiteral
        | TyKind::KgenDeferred
        | TyKind::KgenTarget
        | TyKind::Index
        | TyKind::I1
        | TyKind::Ui8
        | TyKind::KgenDtype
        | TyKind::SelfType => ty.clone(),
    })
}

/// [`rewrite_ty`] over a parameter-argument list. Origins pass through.
pub fn rewrite_tyargs(
    arguments: &[TyArg],
    rewrite: &mut dyn TyRewrite,
) -> Result<Vec<TyArg>, ParamError> {
    arguments
        .iter()
        .map(|argument| {
            Ok(match argument {
                TyArg::Ty(ty) => TyArg::Ty(rewrite_ty(ty, rewrite)?),
                TyArg::Val(value) => TyArg::Val(rewrite_value(value, rewrite)?),
                TyArg::Origin(origin) => TyArg::Origin(origin.clone()),
            })
        })
        .collect()
}

/// [`rewrite_ty`] over a compile-time value: residual expressions, embedded
/// types, and every aggregate child. A rewritten expression that folds
/// becomes its ordinary concrete value.
pub fn rewrite_value(value: &CtValue, rewrite: &mut dyn TyRewrite) -> Result<CtValue, ParamError> {
    let all = |values: &[CtValue], rewrite: &mut dyn TyRewrite| {
        values
            .iter()
            .map(|value| rewrite_value(value, rewrite))
            .collect::<Result<Vec<_>, _>>()
    };
    rewrite.value(value);
    Ok(match value {
        CtValue::Pointer { memory, offset } => CtValue::Pointer {
            memory: memory
                .as_ref()
                .map(|slots| {
                    slots
                        .iter()
                        .map(|slot| {
                            slot.as_ref()
                                .map(|value| rewrite_value(value, rewrite))
                                .transpose()
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?,
            offset: *offset,
        },
        CtValue::StoreToMem(referent) => {
            CtValue::StoreToMem(Box::new(rewrite_value(referent, rewrite)?))
        }
        CtValue::Expr(expr) => rewrite.expr(expr)?.into_value(),
        CtValue::Type(ty) => CtValue::Type(rewrite_ty(ty, rewrite)?),
        CtValue::Tuple(values) => CtValue::Tuple(all(values, rewrite)?),
        CtValue::List(values) => CtValue::List(all(values, rewrite)?),
        CtValue::PopArray { element, elements } => CtValue::PopArray {
            element: rewrite_ty(element, rewrite)?,
            elements: all(elements, rewrite)?,
        },
        CtValue::Set { spelling, elements } => CtValue::Set {
            spelling: spelling.clone(),
            elements: all(elements, rewrite)?,
        },
        CtValue::Dict { spelling, entries } => CtValue::Dict {
            spelling: spelling.clone(),
            entries: entries
                .iter()
                .map(|(key, value)| {
                    Ok((rewrite_value(key, rewrite)?, rewrite_value(value, rewrite)?))
                })
                .collect::<Result<_, ParamError>>()?,
        },
        CtValue::Struct { name, fields } => CtValue::Struct {
            name: name.clone(),
            fields: fields
                .iter()
                .map(|(field, value)| Ok((field.clone(), rewrite_value(value, rewrite)?)))
                .collect::<Result<_, ParamError>>()?,
        },
        CtValue::Variant {
            alternatives,
            index,
            payload,
        } => CtValue::Variant {
            alternatives: alternatives
                .iter()
                .map(|ty| rewrite_ty(ty, rewrite))
                .collect::<Result<_, _>>()?,
            index: *index,
            payload: payload
                .as_deref()
                .map(|payload| rewrite_value(payload, rewrite).map(Box::new))
                .transpose()?,
        },
        CtValue::Int(_)
        | CtValue::UInt(_)
        | CtValue::Index(_)
        | CtValue::Float(_)
        | CtValue::IntLiteral(_)
        | CtValue::FloatLiteral(_)
        | CtValue::Bool(_)
        | CtValue::None
        | CtValue::Str(_)
        | CtValue::Dtype(_)
        | CtValue::Trait(_)
        | CtValue::KgenString(_)
        | CtValue::PopIntLiteral(_)
        | CtValue::PopFloatLiteral(_)
        | CtValue::MlirAttr(_)
        | CtValue::KgenTarget(_)
        | CtValue::Generator(_)
        | CtValue::Simd { .. }
        | CtValue::Deferred(_)
        | CtValue::Marker(_)
        | CtValue::RemovedArg => value.clone(),
    })
}

/// Replace declared parameters, signature slots, and type parameters in `ty`.
///
/// Replacement re-enters the canonicalizing constructors of `context`.
/// `depth` is the number of signature binders already descended through.
pub fn replace_parameters(
    context: &ParamContext,
    ty: &Ty,
    bindings: &ParamBindings,
    depth: u32,
) -> Result<Ty, ParamError> {
    let mut replacer = Replacer {
        context,
        scopes: vec![bindings.clone()],
        depth,
    };
    rewrite_ty(ty, &mut replacer)
}

/// [`replace_parameters`] over one compile-time value.
pub fn replace_value_parameters(
    context: &ParamContext,
    value: &CtValue,
    bindings: &ParamBindings,
) -> Result<CtValue, ParamError> {
    let mut replacer = Replacer {
        context,
        scopes: vec![bindings.clone()],
        depth: 0,
    };
    rewrite_value(value, &mut replacer)
}

/// [`replace_parameters`] over one parameter argument.
pub fn replace_argument_parameters(
    context: &ParamContext,
    argument: &TyArg,
    bindings: &ParamBindings,
) -> Result<TyArg, ParamError> {
    let mut replacer = Replacer {
        context,
        scopes: vec![bindings.clone()],
        depth: 0,
    };
    rewrite_tyargs(std::slice::from_ref(argument), &mut replacer)
        .map(|mut arguments| arguments.remove(0))
}

/// [`replace_parameters`] over one generic constraint.
///
/// A bound value binder's operand becomes its expression, a closed one its
/// constant; a bound type binder's becomes its type, and a bound pack
/// binder's its element list, so a pack proposition at a concrete binding
/// asks of that list.
pub fn replace_constraint_parameters(
    context: &ParamContext,
    cond: &GenericConstraint,
    bindings: &ParamBindings,
) -> Result<GenericConstraint, ParamError> {
    let operand = |expr: ParamExpr| match (expr.as_constant(), expr.as_decl_ref()) {
        (Some(value), _) => ConstraintOperand::Value(value.clone()),
        (None, Some(param)) => ConstraintOperand::Param(param.clone()),
        (None, None) => ConstraintOperand::Expr(expr),
    };
    cond.try_map(&|leaf| {
        Ok(match leaf {
            ConstraintOperand::Param(param) => {
                match (bindings.types().get(&param.id), bindings.lookup(param)) {
                    // A pack bound to its elements is also bound to them as
                    // the runtime pack a spread expands to; as an operand it
                    // is the type list, upstream's `param_list`.
                    (Some(ty), Some(expr)) if matches!(ty.kind(), TyKind::RuntimePack(_)) => {
                        operand(expr.clone())
                    }
                    (Some(ty), _) => match ty.kind() {
                        TyKind::Param { binder, .. } => ConstraintOperand::Param(binder.clone()),
                        _ => ConstraintOperand::Type(ty.clone()),
                    },
                    (None, Some(expr)) => operand(expr.clone()),
                    (None, None) => leaf.clone(),
                }
            }
            // `try_map` replaces the list a length measures.
            ConstraintOperand::PackLength(_) => leaf.clone(),
            ConstraintOperand::Value(value) => {
                ConstraintOperand::Value(replace_value_parameters(context, value, bindings)?)
            }
            ConstraintOperand::Type(ty) => {
                ConstraintOperand::Type(replace_parameters(context, ty, bindings, 0)?)
            }
            ConstraintOperand::Expr(expr) => operand(context.replace(expr, bindings)?),
        })
    })
}

/// Whether `ty` mentions anything no signature inside it binds: a binder
/// outside a generic callable's own `def[...]` list, an inference hole, an
/// associated projection, or `Self`.
///
/// A parametric function type closed over its own binders (`def[dtype:
/// DType, //](SIMD[dtype, 1]) thin -> SIMD[dtype, 1]`) has none, so it is a
/// concrete type argument (`OptionalReg[G]`), as upstream's
/// `!kgen.generator` type is, though no value of it is.
pub fn has_free_binders(ty: &Ty) -> bool {
    struct Finder {
        free: bool,
        bound: Vec<ParamId>,
        marks: Vec<usize>,
    }
    impl TyRewrite for Finder {
        fn whole(&mut self, ty: &Ty) -> Option<Ty> {
            self.free |= matches!(
                ty.kind(),
                TyKind::Infer | TyKind::Assoc { .. } | TyKind::SelfType
            );
            None
        }

        fn param(&mut self, binder: &ParamRef) -> Option<Ty> {
            self.free |= !self.bound.contains(&binder.id);
            None
        }

        fn expr(&mut self, expr: &ParamExpr) -> Result<ParamExpr, ParamError> {
            self.free |= expr
                .free_parameters()
                .iter()
                .any(|reference| !self.bound.contains(&reference.id));
            let mut nested = Vec::new();
            expr.visit(&mut |node| nested.extend(node.embedded_types().into_iter().cloned()));
            for ty in &nested {
                rewrite_ty(ty, self)?;
            }
            Ok(expr.clone())
        }

        fn enter_signature(&mut self, decls: &[ParamDecl]) {
            self.marks.push(self.bound.len());
            self.bound
                .extend(decls.iter().map(|decl| decl.id().clone()));
        }

        fn exit_signature(&mut self) {
            if let Some(mark) = self.marks.pop() {
                self.bound.truncate(mark);
            }
        }
    }
    let mut finder = Finder {
        free: false,
        bound: Vec::new(),
        marks: Vec::new(),
    };
    // The finder never fails; the rebuilt type is discarded.
    let _ = rewrite_ty(ty, &mut finder);
    finder.free
}

/// The declared parameters referenced free anywhere in `ty`, by name.
///
/// It reaches every expression operand, embedded type, default, and aggregate
/// child. A generic callable's own binders are bound, not free.
pub fn referenced_parameters<S: std::hash::BuildHasher>(
    ty: &Ty,
    output: &mut std::collections::HashSet<String, S>,
) {
    struct Collector<'a, S> {
        output: &'a mut std::collections::HashSet<String, S>,
        bound: Vec<String>,
        marks: Vec<usize>,
    }
    impl<S: std::hash::BuildHasher> TyRewrite for Collector<'_, S> {
        fn expr(&mut self, expr: &ParamExpr) -> Result<ParamExpr, ParamError> {
            let mut names = std::collections::HashSet::new();
            expr.referenced_parameters(&mut names);
            self.output
                .extend(names.into_iter().filter(|name| !self.bound.contains(name)));
            let mut nested = Vec::new();
            expr.visit(&mut |node| nested.extend(node.embedded_types().into_iter().cloned()));
            for ty in &nested {
                rewrite_ty(ty, self)?;
            }
            Ok(expr.clone())
        }

        fn enter_signature(&mut self, decls: &[ParamDecl]) {
            self.marks.push(self.bound.len());
            self.bound
                .extend(decls.iter().map(|decl| decl.spelling().to_string()));
        }

        fn exit_signature(&mut self) {
            if let Some(mark) = self.marks.pop() {
                self.bound.truncate(mark);
            }
        }
    }
    // The collector never fails; the rebuilt type is discarded.
    let _ = rewrite_ty(
        ty,
        &mut Collector {
            output,
            bound: Vec::new(),
            marks: Vec::new(),
        },
    );
}

/// Whether a deferred slot ([`CtValue::Deferred`]) or an elaborator marker
/// ([`CtValue::Marker`]) occurs in a value argument anywhere in `ty`.
pub fn mentions_deferred_value(ty: &Ty) -> bool {
    struct Finder(bool);
    impl TyRewrite for Finder {
        fn expr(&mut self, expr: &ParamExpr) -> Result<ParamExpr, ParamError> {
            Ok(expr.clone())
        }

        fn value(&mut self, value: &CtValue) {
            self.0 |= matches!(value, CtValue::Deferred(_) | CtValue::Marker(_));
        }
    }
    let mut finder = Finder(false);
    // The finder never fails; the rebuilt type is discarded.
    let _ = rewrite_ty(ty, &mut finder);
    finder.0
}

/// Apply `f` to each type argument of a struct's parameter list, passing value
/// arguments through unchanged.
pub fn map_tyargs(args: &[TyArg], mut f: impl FnMut(&Ty) -> Ty) -> Vec<TyArg> {
    args.iter()
        .map(|a| match a {
            TyArg::Ty(t) => TyArg::Ty(f(t)),
            // A pack bound whole (`Pair[T, Int]` as one list) holds type
            // positions too: its element types map like `TyArg::Ty`.
            TyArg::Val(CtValue::Tuple(values)) => TyArg::Val(CtValue::Tuple(
                values
                    .iter()
                    .map(|value| match value {
                        CtValue::Type(ty) => CtValue::Type(f(ty)),
                        other => other.clone(),
                    })
                    .collect(),
            )),
            TyArg::Val(v) => TyArg::Val(v.clone()),
            // Origin substitution is threaded separately; pass origins through.
            TyArg::Origin(o) => TyArg::Origin(o.clone()),
        })
        .collect()
}

/// `reference` bound to the binder of its spelling in `scopes`, innermost
/// scope first, when no declaration binds it yet.
pub fn bind_reference(reference: &ParamRef, scopes: &[&[ParamDecl]]) -> ParamRef {
    if !reference.is_unbound() {
        return reference.clone();
    }
    let spelling = reference.name.trim_start_matches('*');
    scopes
        .iter()
        .find_map(|decls| {
            decls
                .iter()
                .find(|decl| decl.name().trim_start_matches('*') == spelling)
        })
        .map_or_else(
            || reference.clone(),
            // A type pack's name reads its list binder.
            |decl| ParamRef {
                id: decl
                    .pack_list_ref()
                    .filter(|_| decl.is_type_pack())
                    .map_or_else(|| decl.id().clone(), |list| list.id),
                name: reference.name.clone(),
            },
        )
}

/// Alpha-rename the binder references inside one canonicalized constraint.
///
/// A reference to one of the contract's own binders takes that binder's
/// canonical identity from `binders`, and embedded types canonicalize exactly
/// like signature types.
#[allow(clippy::implicit_hasher, reason = "TODO: generalize over BuildHasher")]
pub fn rename_constraint_parameters(
    constraint: &GenericConstraint,
    binders: &HashMap<ParamId, ParamRef>,
    subst: &TySubst,
    value_slots: &ParamBindings,
) -> GenericConstraint {
    constraint.map(
        &|reference| {
            binders
                .get(&reference.id)
                .cloned()
                .unwrap_or_else(|| reference.clone())
        },
        &|operand| match operand {
            ConstraintOperand::Expr(expr) => ConstraintOperand::Expr(
                ParamContext::detached()
                    .replace(expr, value_slots)
                    .unwrap_or_else(|_| expr.clone()),
            ),
            ConstraintOperand::Type(ty) => {
                ConstraintOperand::Type(bind_signature_slots(&substitute(ty, subst), value_slots))
            }
            other => other.clone(),
        },
    )
}

pub fn callable_convention_accepts(
    actual: Option<ArgConvention>,
    contract: Option<ArgConvention>,
) -> bool {
    let actual = actual.unwrap_or(ArgConvention::Imm);
    let contract = contract.unwrap_or(ArgConvention::Imm);
    match (actual, contract) {
        // A read-only callee demands less access than a mutable callable
        // contract promises to supply, so it is a valid implementation.
        (ArgConvention::Imm, ArgConvention::Imm | ArgConvention::Mut) => true,
        (ArgConvention::Mut, ArgConvention::Mut) => true,
        // Ownership-changing and parametric-reference conventions retain their
        // exact ABI until their full subtyping rules are modeled.
        (actual, contract) => actual == contract,
    }
}

pub const STDLIB_STRING_STRUCT: &str = "__module$std$string$String";

/// Whether `name` is the bundled nominal `String` struct — the linked
/// qualified identity, or the bare name in unlinked/focused contexts.
pub fn is_stdlib_string_struct(name: &str) -> bool {
    name == "String" || name == STDLIB_STRING_STRUCT
}

/// Whether `ty` is the nominal `String`.
pub fn is_nominal_string(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::Struct(name, args)
        if args.is_empty() && is_stdlib_string_struct(name))
}

/// The bundled borrowed string view.
///
/// Unlike `String`, `StringSpan` is prelude-bare in MIR (no module-qualified
/// takeover identity), so its bare name is its checked identity — a user
/// struct spelled `StringSpan` shadows it, the caveat the other stdlib
/// collection name lists share.
pub const STDLIB_STRING_SPAN_STRUCT: &str = "StringSpan";

/// Whether `name` is the bundled `StringSpan` view struct, whose
/// `StringLiteral` constructor the backends replace with a literal bridge.
pub fn is_stdlib_string_span_struct(name: &str) -> bool {
    name == STDLIB_STRING_SPAN_STRUCT
}

/// Whether `name` is the bundled `RawFileDescriptor` struct (`std.io`), the only
/// accepted `file=` argument of `print`.
///
/// Its checked identity is the linker's module-qualified spelling of
/// `std/io/file_descriptor.mojo`; the bare name is the prelude spelling and
/// shares the other stdlib name lists' shadowing caveat.
pub fn is_stdlib_file_descriptor_struct(name: &str) -> bool {
    name == "RawFileDescriptor" || name.ends_with("file$descriptor$RawFileDescriptor")
}

/// Identity of one checked declaration, stable across the checked program.
///
/// Defined here (below the checked handoff) so symbol mangling can spell
/// declaration-qualified names without depending on the handoff crate.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct CheckedDeclId(pub u32);

/// The canonical spelling of a trait composition.
///
/// Each trait appears once, ordered by name descending, as the pin displays
/// a composition (`Copyable & Hashable` prints `Hashable & Copyable`).
/// Dropping a trait another one implies needs the trait table, so the
/// checker does that first.
pub fn trait_set<I: IntoIterator<Item = String>>(names: I) -> Vec<String> {
    let mut traits: Vec<String> = names.into_iter().collect();
    traits.sort_unstable_by(|a, b| b.cmp(a));
    traits.dedup();
    traits
}

/// The traits every type of the trait type `ty` conforms to.
///
/// That is the set a [`TyKind::Trait`] is, or the bound of the trait-valued
/// binder a dependent type reads (`Tr: type_of(Copyable)`); `None` for any
/// other type.
pub fn trait_type_bound(ty: &Ty) -> Option<&[String]> {
    match ty.kind() {
        TyKind::Trait(traits) => Some(traits),
        TyKind::Dependent(dependent) => match dependent.expr().meta().as_value()?.kind() {
            TyKind::AnyTrait(bound) => Some(bound),
            _ => None,
        },
        _ => None,
    }
}

/// Whether `ty` still mentions something only an instantiation can resolve.
///
/// A type parameter, an associated or dependent projection, `Self`, or an
/// unsolved inference variable, at any depth, makes it symbolic: such a type
/// has no runtime identity — no native layout, and nothing the VM can dispatch
/// on. Origins erase from the runtime ABI, so they never make a type symbolic.
pub fn is_symbolic(ty: &Ty) -> bool {
    match ty.kind() {
        TyKind::Infer
        | TyKind::Param { .. }
        | TyKind::Assoc { .. }
        | TyKind::Dependent(_)
        | TyKind::SelfType => true,
        TyKind::Struct(..) if ty.is_closed() => false,
        TyKind::Struct(_, arguments) => arguments.iter().any(|argument| match argument {
            TyArg::Ty(ty) => is_symbolic(ty),
            TyArg::Val(value) => ct_value_is_symbolic(value),
            TyArg::Origin(_) => false,
        }),
        TyKind::Func(_) => ty
            .callable()
            .is_some_and(|func| func.types().any(is_symbolic)),
        // The callable's own binders are bound inside it.
        TyKind::GenericFunc(_) => has_free_binders(ty),
        // The generator's own binders are bound inside it.
        TyKind::Generator(generator) => {
            let bound: Vec<&ParamId> = generator.params.iter().map(ParamDecl::id).collect();
            let free = |ty: &Ty| {
                mentions(ty, &|inner| match inner.kind() {
                    TyKind::Param { binder, .. } => !bound.contains(&&binder.id),
                    TyKind::Infer | TyKind::Assoc { .. } | TyKind::SelfType => true,
                    TyKind::Dependent(_) => is_symbolic(inner),
                    _ => false,
                })
            };
            generator.params.iter().any(|decl| match decl {
                ParamDecl::Value { ty, .. } => free(ty),
                ParamDecl::Type { .. } => false,
            }) || free(&generator.result)
        }
        TyKind::Overload(types)
        | TyKind::Tuple(types)
        | TyKind::RuntimePack(types)
        | TyKind::Variant(types) => types.iter().any(is_symbolic),
        TyKind::ComptimeList(element)
        | TyKind::ParamList(element)
        | TyKind::VariadicPack(element)
        | TyKind::Pointer { element, .. }
        | TyKind::KgenPointer(element)
        | TyKind::LitRef { element, .. } => is_symbolic(element),
        TyKind::LitRefPack { elements, .. } => elements.iter().any(is_symbolic),
        TyKind::PopArray { size, element } => size.is_symbolic() || is_symbolic(element),
        TyKind::Ref(reference) => is_symbolic(&reference.referent),
        TyKind::KgenSimd { dtype, width } => dtype.is_symbolic() || width.is_symbolic(),
        TyKind::KgenAddressSimd { width } => width.is_symbolic(),
        TyKind::AnyTrait(_)
        | TyKind::Trait(_)
        | TyKind::TypeType
        | TyKind::KgenString
        | TyKind::PopIntLiteral
        | TyKind::PopFloatLiteral
        | TyKind::KgenDeferred
        | TyKind::KgenTarget
        | TyKind::Index
        | TyKind::I1
        | TyKind::Ui8
        | TyKind::KgenDtype
        | TyKind::None
        | TyKind::Never => false,
    }
}

/// Whether `ty` mentions a parameter that nothing inside it binds.
///
/// This is the *closedness* question, which is not [`is_symbolic`]'s: a
/// generic callable value binds the parameters its own signature mentions, so
/// `def[T](T) -> T` is closed — it is a compile-time type constant, and
/// `DependentType::resolve` unwraps it — even though a `TyKind::Param` named `T`
/// occurs inside it. A binder nested below another's signature is left to
/// [`is_symbolic`]'s conservative answer.
pub fn has_free_parameters(ty: &Ty) -> bool {
    let TyKind::GenericFunc(generic) = ty.kind() else {
        return is_symbolic(ty);
    };
    let bound: Vec<&ParamId> = generic.decls.iter().map(ParamDecl::id).collect();
    mentions_open(ty, &|inner| match inner.kind() {
        TyKind::Param { binder, .. } => !bound.contains(&&binder.id),
        TyKind::Infer | TyKind::Assoc { .. } | TyKind::Dependent(_) | TyKind::SelfType => true,
        _ => false,
    })
}

/// Whether `ty` names the declared parameter `binder`.
///
/// It counts as a type parameter, or in a parameter expression of a value
/// position — a SIMD width, a value argument, a pack element's index, a
/// query over the pack it is — or of a type embedded in one.
pub fn names_binder(ty: &Ty, binder: &ParamRef) -> bool {
    struct Finder<'a> {
        binder: &'a ParamRef,
        found: bool,
    }
    impl TyRewrite for Finder<'_> {
        fn param(&mut self, binder: &ParamRef) -> Option<Ty> {
            self.found |= binder == self.binder;
            None
        }

        fn expr(&mut self, expr: &ParamExpr) -> Result<ParamExpr, ParamError> {
            expr.visit(&mut |node| {
                self.found |= matches!(node.kind(), ParamKind::DeclRef(reference) if reference == self.binder)
                    || node
                        .embedded_types()
                        .into_iter()
                        .any(|ty| names_binder(ty, self.binder));
            });
            Ok(expr.clone())
        }
    }
    let mut finder = Finder {
        binder,
        found: false,
    };
    // The finder never fails; the rebuilt type is discarded.
    let _ = rewrite_ty(ty, &mut finder);
    finder.found
}

/// The [`is_symbolic`] companion for compile-time values: a value parameter, or
/// any collection or type handle carrying one.
pub fn ct_value_is_symbolic(value: &CtValue) -> bool {
    match value {
        CtValue::Pointer { memory, .. } => {
            memory.iter().flatten().flatten().any(ct_value_is_symbolic)
        }
        CtValue::StoreToMem(referent) => ct_value_is_symbolic(referent),
        CtValue::Expr(_) | CtValue::Deferred(_) | CtValue::Marker(_) => true,
        CtValue::Tuple(values)
        | CtValue::List(values)
        | CtValue::Set {
            elements: values, ..
        } => values.iter().any(ct_value_is_symbolic),
        CtValue::Dict { entries, .. } => entries
            .iter()
            .any(|(key, value)| ct_value_is_symbolic(key) || ct_value_is_symbolic(value)),
        CtValue::Type(ty) => is_symbolic(ty),
        CtValue::Struct { fields, .. } => {
            fields.iter().any(|(_, value)| ct_value_is_symbolic(value))
        }
        CtValue::PopArray { element, elements } => {
            is_symbolic(element) || elements.iter().any(ct_value_is_symbolic)
        }
        CtValue::Variant {
            alternatives,
            payload,
            ..
        } => {
            alternatives.iter().any(is_symbolic)
                || payload.as_deref().is_some_and(ct_value_is_symbolic)
        }
        CtValue::Int(_)
        | CtValue::UInt(_)
        | CtValue::Index(_)
        | CtValue::Float(_)
        | CtValue::IntLiteral(_)
        | CtValue::FloatLiteral(_)
        | CtValue::Bool(_)
        | CtValue::None
        | CtValue::Dtype(_)
        | CtValue::Simd { .. }
        | CtValue::Str(_)
        | CtValue::Trait(_)
        | CtValue::KgenString(_)
        | CtValue::PopIntLiteral(_)
        | CtValue::PopFloatLiteral(_)
        | CtValue::MlirAttr(_)
        | CtValue::KgenTarget(_)
        | CtValue::Generator(_)
        | CtValue::RemovedArg => false,
    }
}

/// The replacing [`TyRewrite`]: one binding scope per signature level, the
/// innermost masking the names its own binders shadow.
struct Replacer<'a> {
    context: &'a ParamContext,
    scopes: Vec<ParamBindings>,
    depth: u32,
}

impl Replacer<'_> {
    fn bindings(&self) -> &ParamBindings {
        self.scopes
            .last()
            .expect("a replacer always holds its outermost scope")
    }
}

impl TyRewrite for Replacer<'_> {
    /// A collector over a pack bound to its element list is the tuple of
    /// the elements, as [`expand_packs`] spells it in an instance.
    fn whole(&mut self, ty: &Ty) -> Option<Ty> {
        let TyKind::VariadicPack(element) = ty.kind() else {
            return None;
        };
        let TyKind::Param { binder, .. } = pack_spread(std::slice::from_ref(element))?.kind()
        else {
            return None;
        };
        match self.bindings().types().get(&binder.id).map(Ty::kind) {
            Some(TyKind::RuntimePack(elements)) => Some(TyKind::Tuple(elements.clone()).into()),
            _ => None,
        }
    }

    fn param(&mut self, binder: &ParamRef) -> Option<Ty> {
        self.bindings().types().get(&binder.id).cloned()
    }

    fn expr(&mut self, expr: &ParamExpr) -> Result<ParamExpr, ParamError> {
        self.context
            .replace_at(expr, self.bindings(), self.depth, &mut HashMap::new())
    }

    fn enter_signature(&mut self, decls: &[ParamDecl]) {
        let mut inner = self.bindings().clone();
        inner.mask(decls.iter().map(ParamDecl::name));
        inner.mask_types(decls.iter().map(ParamDecl::id));
        self.scopes.push(inner);
        self.depth += 1;
    }

    fn exit_signature(&mut self) {
        self.scopes.pop();
        self.depth -= 1;
    }
}

/// The [`TyRewrite`] behind [`template_spelled`]: a monomorphized struct
/// instance is respelled by its template, its arguments rewritten alike.
struct TemplateSpelling;

impl TyRewrite for TemplateSpelling {
    fn whole(&mut self, ty: &Ty) -> Option<Ty> {
        let TyKind::Struct(name, arguments) = ty.kind() else {
            return None;
        };
        let template = struct_template(name);
        (template != name).then(|| {
            let arguments = rewrite_tyargs(arguments, self).unwrap_or_else(|_| arguments.clone());
            ty.reusing(TyKind::Struct(template.to_string(), arguments))
        })
    }

    fn expr(&mut self, expr: &ParamExpr) -> Result<ParamExpr, ParamError> {
        Ok(expr.clone())
    }

    fn visits_closed(&self) -> bool {
        true
    }
}

/// [`mentions`], passing a closed level whole when `skip_closed`.
fn mentions_below(ty: &Ty, predicate: &dyn Fn(&Ty) -> bool, skip_closed: bool) -> bool {
    let mentions =
        |ty: &Ty, predicate: &dyn Fn(&Ty) -> bool| mentions_below(ty, predicate, skip_closed);
    if predicate(ty) {
        return true;
    }
    let argument_mentions = |argument: &TyArg| match argument {
        TyArg::Ty(ty) => mentions(ty, predicate),
        TyArg::Val(CtValue::Expr(expr)) => expr_mentions(expr, predicate),
        // A pack bound whole holds type positions (see `map_tyargs`).
        TyArg::Val(CtValue::Tuple(values)) => values.iter().any(|value| match value {
            CtValue::Type(ty) => mentions(ty, predicate),
            _ => false,
        }),
        TyArg::Val(_) | TyArg::Origin(_) => false,
    };
    match ty.kind() {
        TyKind::Struct(..) if skip_closed && ty.is_closed() => false,
        TyKind::Struct(_, arguments) => arguments.iter().any(argument_mentions),
        TyKind::ComptimeList(element)
        | TyKind::ParamList(element)
        | TyKind::VariadicPack(element)
        | TyKind::Pointer { element, .. }
        | TyKind::KgenPointer(element)
        | TyKind::LitRef { element, .. } => mentions(element, predicate),
        TyKind::LitRefPack { elements, .. } => {
            elements.iter().any(|element| mentions(element, predicate))
        }
        TyKind::PopArray { size, element } => {
            matches!(size, ArraySize::Expr(expr) if expr_mentions(expr, predicate))
                || mentions(element, predicate)
        }
        TyKind::Dependent(dependent) => expr_mentions(dependent.expr(), predicate),
        TyKind::Tuple(elements) | TyKind::RuntimePack(elements) | TyKind::Variant(elements) => {
            elements.iter().any(|element| mentions(element, predicate))
        }
        TyKind::Assoc { base, args, .. } => {
            mentions(base, predicate) || args.iter().any(argument_mentions)
        }
        TyKind::Func(_) | TyKind::GenericFunc(_) if let Some(func) = ty.callable() => {
            let FuncTy {
                params,
                ret,
                variadic,
                kw_variadic,
                ..
            } = func;
            params.iter().any(|param| mentions(param, predicate))
                || mentions(ret, predicate)
                || variadic
                    .as_ref()
                    .is_some_and(|variadic| mentions(variadic, predicate))
                || kw_variadic
                    .as_ref()
                    .is_some_and(|variadic| mentions(variadic, predicate))
        }
        TyKind::Overload(candidates) => candidates
            .iter()
            .any(|candidate| mentions(candidate, predicate)),
        _ => false,
    }
}

/// [`Ty::carries_decorations`], below the closedness check.
fn decorated(ty: &Ty) -> bool {
    let callable = |func: &FuncTy| {
        !func.transfers.0.is_empty()
            || func
                .params
                .iter()
                .chain(std::iter::once(&func.ret))
                .chain(&func.variadic)
                .chain(&func.kw_variadic)
                .chain(&func.error)
                .any(Ty::carries_decorations)
    };
    match ty.kind() {
        TyKind::Func(func) => callable(func),
        TyKind::GenericFunc(generic) => callable(&generic.func),
        TyKind::Overload(types)
        | TyKind::Tuple(types)
        | TyKind::RuntimePack(types)
        | TyKind::Variant(types) => types.iter().any(Ty::carries_decorations),
        TyKind::Param { callable_bound, .. } => {
            callable_bound.as_ref().is_some_and(Ty::carries_decorations)
        }
        TyKind::Assoc { base, args, .. } => {
            base.carries_decorations() || args.iter().any(argument_decorated)
        }
        TyKind::Struct(_, arguments) => arguments.iter().any(argument_decorated),
        TyKind::ComptimeList(element)
        | TyKind::ParamList(element)
        | TyKind::VariadicPack(element)
        | TyKind::Pointer { element, .. }
        | TyKind::KgenPointer(element)
        | TyKind::LitRef { element, .. } => element.carries_decorations(),
        TyKind::LitRefPack { elements, .. } => elements.iter().any(Ty::carries_decorations),
        TyKind::PopArray { size, element } => {
            matches!(size, ArraySize::Expr(expr) if expr.carries_decorations())
                || element.carries_decorations()
        }
        TyKind::Ref(reference) => reference.referent.carries_decorations(),
        TyKind::Dependent(dependent) => dependent.expr().carries_decorations(),
        TyKind::KgenSimd { dtype, width } => {
            matches!(dtype, SimdDtype::Expr(expr) if expr.carries_decorations())
                || matches!(width, SimdWidth::Expr(expr) if expr.carries_decorations())
        }
        TyKind::KgenAddressSimd {
            width: SimdWidth::Expr(expr),
        } => expr.carries_decorations(),
        _ => false,
    }
}

fn argument_decorated(argument: &TyArg) -> bool {
    match argument {
        TyArg::Ty(ty) => ty.carries_decorations(),
        TyArg::Val(value) => value_decorated(value),
        TyArg::Origin(_) => false,
    }
}

/// Whether a compile-time value carries a display spelling or a decorated
/// type.
fn value_decorated(value: &CtValue) -> bool {
    match value {
        CtValue::Dict { spelling, entries } => {
            spelling.is_some()
                || entries
                    .iter()
                    .any(|(key, value)| value_decorated(key) || value_decorated(value))
        }
        CtValue::Set { spelling, elements } => {
            spelling.is_some() || elements.iter().any(value_decorated)
        }
        CtValue::Tuple(values) | CtValue::List(values) => values.iter().any(value_decorated),
        CtValue::Struct { fields, .. } => fields.iter().any(|(_, field)| value_decorated(field)),
        CtValue::Type(ty) => ty.carries_decorations(),
        CtValue::Expr(expr) => expr.carries_decorations(),
        _ => false,
    }
}

/// The storage behind a [`Ty`]: its variant with the variant's cached facts.
#[derive(Clone)]
struct TyNode {
    kind: TyKind,
    digest: u64,
    closed: bool,
}

impl TyNode {
    fn new(kind: TyKind) -> Self {
        Self {
            digest: Self::digest(&kind),
            closed: closed_kind(&kind),
            kind,
        }
    }

    /// A fixed-key hash, so equal variants digest alike in every node.
    fn digest(kind: &TyKind) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::hash::DefaultHasher::new();
        kind.hash(&mut hasher);
        hasher.finish()
    }
}

/// Whether one struct argument is closed; see [`Ty::is_closed`].
fn closed_argument(argument: &TyArg) -> bool {
    match argument {
        TyArg::Ty(ty) => ty.is_closed(),
        TyArg::Val(value) => {
            matches!(
                value,
                CtValue::Int(_)
                    | CtValue::UInt(_)
                    | CtValue::Index(_)
                    | CtValue::Float(_)
                    | CtValue::Bool(_)
                    | CtValue::None
                    | CtValue::Str(_)
                    | CtValue::Dtype(_)
                    | CtValue::Simd { .. } // A `SIMD`'s known lane count.
            ) || known_simd_length(value).is_some()
                || dtype_of_value(value).is_some()
        }
        TyArg::Origin(origin) => matches!(origin, crate::origin::Origin::Unbound),
    }
}

/// Whether a variant is closed, from its children's cached closedness.
fn closed_kind(kind: &TyKind) -> bool {
    match kind {
        TyKind::None
        | TyKind::Never
        | TyKind::Index
        | TyKind::I1
        | TyKind::Ui8
        | TyKind::KgenDtype => true,
        TyKind::KgenSimd { dtype, width } => !dtype.is_expr() && !width.is_expr(),
        TyKind::KgenAddressSimd { width } => !width.is_expr(),
        TyKind::Struct(_, arguments) => arguments.iter().all(closed_argument),
        TyKind::Tuple(elements) | TyKind::RuntimePack(elements) | TyKind::Variant(elements) => {
            elements.iter().all(Ty::is_closed)
        }
        TyKind::ComptimeList(element)
        | TyKind::ParamList(element)
        | TyKind::VariadicPack(element) => element.is_closed(),
        TyKind::PopArray { size, element } => {
            matches!(size, ArraySize::Known(_)) && element.is_closed()
        }
        _ => false,
    }
}

fn bind_constraints(constraints: &mut [GenericConstraint], scopes: &[&[ParamDecl]]) {
    for constraint in constraints {
        *constraint = constraint.bind(scopes);
    }
}

/// [`rewrite_ty`] over a binder declaration: its type, callable bound,
/// and defaults. Its constraints pass through.
pub fn rewrite_decl(
    decl: &ParamDecl,
    rewrite: &mut dyn TyRewrite,
) -> Result<ParamDecl, ParamError> {
    let optional = |ty: &Option<Ty>, rewrite: &mut dyn TyRewrite| {
        ty.as_ref().map(|ty| rewrite_ty(ty, rewrite)).transpose()
    };
    Ok(match decl {
        ParamDecl::Type {
            id,
            name,
            bounds,
            trait_param,
            callable_bound,
            default,
            infer_only,
            variadic,
            constraints,
        } => ParamDecl::Type {
            id: id.clone(),
            name: name.clone(),
            bounds: bounds.clone(),
            trait_param: trait_param.clone(),
            callable_bound: optional(callable_bound, rewrite)?,
            default: optional(default, rewrite)?,
            infer_only: *infer_only,
            variadic: *variadic,
            constraints: constraints.clone(),
        },
        ParamDecl::Value {
            id,
            name,
            ty,
            default,
            callable_default,
            infer_only,
            variadic,
            constraints,
        } => ParamDecl::Value {
            id: id.clone(),
            name: name.clone(),
            ty: rewrite_ty(ty, rewrite)?,
            default: default
                .as_ref()
                .map(|value| rewrite.expr(value))
                .transpose()?,
            callable_default: callable_default
                .as_ref()
                .map(|default| rewrite_callable_default(default, rewrite))
                .transpose()?,
            infer_only: *infer_only,
            variadic: *variadic,
            constraints: constraints.clone(),
        },
    })
}

/// The elements of the pack `types` spreads, when `rewrite` binds it to a
/// list of types, as the elaborator binds a pack to a `RuntimePack`.
fn bound_pack(types: &[Ty], rewrite: &mut dyn TyRewrite) -> Option<Vec<Ty>> {
    let TyKind::Param { binder, .. } = pack_spread(types)?.kind() else {
        return None;
    };
    match rewrite.param(binder)?.kind() {
        TyKind::RuntimePack(elements) => Some(elements.clone()),
        _ => None,
    }
}

/// The arguments of the struct `name` with the argument that spreads a
/// pack replaced by `spread`'s answer for it, spelled as
/// [`expand_pack_spread`] spells a bound pack: element by element for a
/// struct that [`binds_pack_elementwise`], its type pack's visible
/// `TypeList` dropped beside them, and as the one argument `spread`
/// answers for any other. Every other argument is `other`'s. `None` when
/// `spread` answers for no argument.
fn spread_struct_arguments<E>(
    name: &str,
    arguments: &[TyArg],
    mut spread: impl FnMut(&Ty) -> Result<Option<Vec<TyArg>>, E>,
    mut other: impl FnMut(&TyArg) -> Result<TyArg, E>,
) -> Result<Option<Vec<TyArg>>, E> {
    let mut expanded = None;
    for (index, argument) in arguments.iter().enumerate() {
        if let TyArg::Ty(ty) = argument
            && let Some(elements) = spread(ty)?
        {
            expanded = Some((index, elements));
            break;
        }
    }
    let Some((position, elements)) = expanded else {
        return Ok(None);
    };
    let elementwise = binds_pack_elementwise(name);
    let mut spelled = Vec::with_capacity(arguments.len() + elements.len());
    for (index, argument) in arguments.iter().enumerate() {
        if index == position {
            spelled.extend(elements.iter().cloned());
        } else if !(elementwise
            && matches!(argument, TyArg::Val(value) if ParamDecl::is_pack_value(value)))
        {
            spelled.push(other(argument)?);
        }
    }
    Ok(Some(spelled))
}

/// A bound pack's `elements` as the argument list a spread of it gives the
/// struct `name`: element by element, or one bound list.
fn pack_elements_arguments(name: &str, elements: Vec<Ty>) -> Vec<TyArg> {
    if binds_pack_elementwise(name) {
        elements.into_iter().map(TyArg::Ty).collect()
    } else {
        vec![TyArg::Val(CtValue::Tuple(
            elements.into_iter().map(CtValue::Type).collect(),
        ))]
    }
}

fn rewrite_callable_default(
    default: &CallableDefault,
    rewrite: &mut dyn TyRewrite,
) -> Result<CallableDefault, ParamError> {
    Ok(match default {
        CallableDefault::Symbol(_) | CallableDefault::Parameter(_) => default.clone(),
        CallableDefault::If {
            condition,
            then_value,
            else_value,
        } => CallableDefault::If {
            condition: rewrite.expr(condition)?,
            then_value: Box::new(rewrite_callable_default(then_value, rewrite)?),
            else_value: Box::new(rewrite_callable_default(else_value, rewrite)?),
        },
    })
}

/// [`substitute_packs`] before the type binders are substituted: the pack
/// spellings expanded, and every dependent type replaced under the packs,
/// the values, and the types alike.
fn expand_packs<S: std::hash::BuildHasher>(
    ty: &Ty,
    subst: &TySubst,
    packs: &HashMap<ParamId, Vec<Ty>, S>,
    values: &[(ParamId, CtValue)],
) -> Ty {
    let recurse = |ty: &Ty| expand_packs(ty, subst, packs, values);
    let elements_of = |ty: &Ty| match ty.kind() {
        TyKind::Param { binder, .. } => packs.get(&binder.id),
        _ => None,
    };
    let expand = |list: &[Ty]| -> Vec<Ty> {
        list.iter()
            .flat_map(|element| match elements_of(element) {
                Some(elements) if pack_spread(std::slice::from_ref(element)).is_some() => {
                    elements.clone()
                }
                _ => vec![recurse(element)],
            })
            .collect()
    };
    match ty.kind() {
        TyKind::Param { .. } => elements_of(ty).map_or_else(
            || ty.clone(),
            |elements| TyKind::RuntimePack(elements.clone()).into(),
        ),
        TyKind::VariadicPack(element) => Ty::new(elements_of(element).map_or_else(
            || TyKind::VariadicPack(recurse(element)),
            |elements| TyKind::Tuple(elements.clone()),
        )),
        TyKind::Struct(name, arguments) => {
            let spread = spread_struct_arguments::<()>(
                name,
                arguments,
                |argument| {
                    Ok(pack_spread(std::slice::from_ref(argument))
                        .and_then(elements_of)
                        .map(|elements| pack_elements_arguments(name, elements.clone())))
                },
                |argument| Ok(map_tyargs(std::slice::from_ref(argument), recurse).remove(0)),
            )
            .ok()
            .flatten();
            let mut arguments = spread.unwrap_or_else(|| map_tyargs(arguments, recurse));
            // A value argument naming a folded value binder closes
            // (`_StridedRange[dtype]` at `dtype = DType.int`).
            if !values.is_empty() {
                let context = ParamContext::detached();
                let bindings = pack_bindings(&context, subst, packs, values);
                for argument in &mut arguments {
                    if let TyArg::Val(value) = argument
                        && let Ok(closed) = replace_value_parameters(&context, value, &bindings)
                    {
                        *value = closed;
                    }
                }
            }
            TyKind::Struct(name.clone(), arguments).into()
        }
        TyKind::Tuple(list) => TyKind::Tuple(expand(list)).into(),
        TyKind::RuntimePack(list) => TyKind::RuntimePack(expand(list)).into(),
        TyKind::Variant(list) => TyKind::Variant(expand(list)).into(),
        TyKind::PopArray { size, element } => TyKind::PopArray {
            size: match size {
                ArraySize::Expr(_) if !values.is_empty() => {
                    let context = ParamContext::detached();
                    let bindings = pack_bindings(&context, subst, packs, values);
                    size.try_map(|size| context.replace(size, &bindings))
                        .unwrap_or_else(|_| size.clone())
                }
                _ => size.clone(),
            },
            element: recurse(element),
        }
        .into(),
        TyKind::Pointer { element, origin } => TyKind::Pointer {
            element: recurse(element),
            origin: origin.clone(),
        }
        .into(),
        TyKind::KgenPointer(element) => TyKind::KgenPointer(recurse(element)).into(),
        TyKind::LitRef { element, origin } => TyKind::LitRef {
            element: recurse(element),
            origin: origin.clone(),
        }
        .into(),
        TyKind::LitRefPack { elements, origin } => TyKind::LitRefPack {
            elements: expand(elements),
            origin: origin.clone(),
        }
        .into(),
        TyKind::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = recurse(&reference.referent);
            TyKind::Ref(reference).into()
        }
        TyKind::Dependent(dependent) => {
            let context = ParamContext::detached();
            context
                .replace(
                    dependent.expr(),
                    &pack_bindings(&context, subst, packs, values),
                )
                .map_or_else(|_| ty.clone(), DependentType::resolve)
        }
        // A lane dtype or width naming a folded value binder closes and
        // re-canonicalizes (`Scalar[dt]` at `dt = DType.int` is `Int`).
        TyKind::KgenSimd { .. } | TyKind::KgenAddressSimd { .. }
            if !values.is_empty() && is_symbolic(ty) =>
        {
            let context = ParamContext::detached();
            replace_parameters(
                &context,
                ty,
                &pack_bindings(&context, subst, packs, values),
                0,
            )
            .unwrap_or_else(|_| ty.clone())
        }
        _ => ty.clone(),
    }
}

/// `subst` as the bindings [`replace_parameters`] takes.
fn type_bindings(subst: &TySubst) -> ParamBindings {
    let mut bindings = ParamBindings::new();
    for (id, ty) in subst {
        bindings.bind_type(id.clone(), ty.clone());
    }
    bindings
}

/// The bindings [`expand_packs`] replaces a dependent or lane-shaped type
/// under: the types, each pack as the tuple of its elements, and the values.
fn pack_bindings<S: std::hash::BuildHasher>(
    context: &ParamContext,
    subst: &TySubst,
    packs: &HashMap<ParamId, Vec<Ty>, S>,
    values: &[(ParamId, CtValue)],
) -> ParamBindings {
    let mut bindings = type_bindings(subst);
    let lists = packs.iter().map(|(id, elements)| {
        let list = elements
            .iter()
            .map(|element| CtValue::Type(element.clone()))
            .collect();
        (id, CtValue::Tuple(list))
    });
    let values = values.iter().map(|(id, value)| (id, value.clone()));
    for (id, value) in lists.chain(values) {
        if let Ok(constant) = context.constant(value) {
            bindings.bind(id.clone(), constant);
        }
    }
    bindings
}

/// Bind a generic signature's own value binders to their slots. A type the
/// slots do not type (a binder declared over another binder) stays as it is.
fn bind_signature_slots(ty: &Ty, slots: &ParamBindings) -> Ty {
    replace_parameters(&ParamContext::detached(), ty, slots, 0).unwrap_or_else(|_| ty.clone())
}

fn expr_mentions(expr: &ParamExpr, predicate: &dyn Fn(&Ty) -> bool) -> bool {
    let mut found = false;
    expr.visit(&mut |node| {
        found |= node
            .embedded_types()
            .into_iter()
            .any(|ty| mentions(ty, predicate));
    });
    found
}

/// Render a callable signature; a generic contract (`decls`) renders its
/// binders and trailing `where` constraints so a constrained-vs-unconstrained
/// mismatch is visible in diagnostics. A closure's storage struct
/// ([`crate::closure_trait`]) renders as its closure trait, which names its
/// parameters and no environment.
fn write_callable(
    f: &mut fmt::Formatter<'_>,
    decls: Option<&[ParamDecl]>,
    func: &FuncTy,
    closure: bool,
) -> fmt::Result {
    let FuncTy {
        environment,
        params,
        ret,
        raises,
        ..
    } = func;
    if let Some(decls) = decls {
        write!(f, "def")?;
        write_param_decls(f, decls)?;
        write!(f, "(")?;
    } else {
        write!(f, "def(")?;
    }
    for (i, p) in params.iter().enumerate() {
        if i > 0 {
            write!(f, ", ")?;
        }
        match func.names.get(i) {
            Some(name) if closure => write!(f, "{name}: {p}")?,
            _ => write!(f, "{p}")?,
        }
    }
    write!(f, ")")?;
    match environment {
        _ if closure => {}
        crate::origin::CallableEnvironment::Default => {}
        crate::origin::CallableEnvironment::Thin => write!(f, " thin")?,
        crate::origin::CallableEnvironment::Capturing(origins) => {
            write!(f, " capturing[")?;
            for (index, capture) in origins.0.iter().enumerate() {
                if index > 0 {
                    write!(f, ", ")?;
                }
                if capture.access == crate::origin::CaptureAccess::Write {
                    write!(f, "mut ")?;
                }
                match &capture.origin {
                    crate::origin::Origin::Param(id) => {
                        write!(f, "origin#{}", id.0)?;
                    }
                    crate::origin::Origin::Place(place) => {
                        write!(f, "origin@{}", place.root.0)?;
                    }
                    crate::origin::Origin::SelfParam => {
                        write!(f, "origin_of(self)")?;
                    }
                    crate::origin::Origin::Static => write!(f, "static")?,
                    crate::origin::Origin::Untracked { mutable: true } => {
                        write!(f, "mut-untracked")?;
                    }
                    crate::origin::Origin::Untracked { mutable: false } => {
                        write!(f, "immut-untracked")?;
                    }
                    crate::origin::Origin::Union(_) => {
                        write!(f, "origin-union")?;
                    }
                    crate::origin::Origin::Unbound => write!(f, "_")?,
                }
            }
            write!(f, "]")?;
        }
    }
    if *raises {
        write!(f, " raises")?;
    }
    write!(f, " -> {ret}")?;
    if let Some(decls) = decls {
        for decl in decls {
            let (ParamDecl::Type { constraints, .. } | ParamDecl::Value { constraints, .. }) = decl;
            for constraint in constraints {
                write!(f, " where {constraint}")?;
            }
        }
    }
    Ok(())
}

/// The element types a struct application's `arguments` bind each of its
/// declared type packs `decls` to, where the pack is bound whole, as its
/// element tuple or runtime pack, to types none of which spreads another
/// pack. A struct applied element by element (`Tuple[A, B]`) binds none.
fn struct_pack_arguments(decls: &[ParamDecl], arguments: &[TyArg]) -> HashMap<ParamId, Vec<Ty>> {
    decls
        .iter()
        .zip(arguments)
        .filter_map(|(decl, argument)| {
            let ParamDecl::Type { variadic: true, .. } = decl else {
                return None;
            };
            let elements = match argument {
                TyArg::Val(CtValue::Tuple(values)) => values
                    .iter()
                    .map(|value| match value {
                        CtValue::Type(ty) => Some(ty.clone()),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>()?,
                TyArg::Ty(pack) => match pack.kind() {
                    TyKind::RuntimePack(elements) => elements.clone(),
                    _ => return None,
                },
                _ => return None,
            };
            elements
                .iter()
                .all(|element| pack_spread(std::slice::from_ref(element)).is_none())
                .then(|| (decl.binder().id, elements))
        })
        .collect()
}

/// The shared node of the declared `Bool` struct.
fn shared_bool(kind: &TyKind) -> Option<Ty> {
    kind.is_bool().then(Ty::bool)
}

/// The canonical form of a `StringLiteral[value]` application: a closed
/// `!kgen.string` slot folds to its text, so `StringLiteral[Expr("ab")]`
/// and the [`Ty::string_literal_of`] of `"ab"` are one type. Any other shape (the
/// bare template, an erased or deferred argument) is not rewritten.
fn canonical_string_literal_application(kind: &TyKind) -> Option<Ty> {
    let TyKind::Struct(name, args) = kind else {
        return None;
    };
    let [TyArg::Val(CtValue::Expr(expr))] = args.as_slice() else {
        return None;
    };
    if name != STRING_LITERAL_TYPE_NAME {
        return None;
    }
    let value @ CtValue::KgenString(_) = expr.as_constant()? else {
        return None;
    };
    Some(Ty(Arc::new(TyNode::new(TyKind::Struct(
        name.clone(),
        vec![TyArg::Val(value.clone())],
    )))))
}

/// The canonical form of an `IntLiteral[value]` or `FloatLiteral[value]`
/// application: a closed slot folds to its `#pop.int_literal` or
/// `#pop.float_literal` attribute, so `IntLiteral[(4).value]` and the
/// [`Ty::int_literal_of`] of 4 are one type. Any other shape (the bare
/// template, a symbolic, erased, or deferred argument) is not rewritten.
fn canonical_numeric_literal_application(kind: &TyKind) -> Option<Ty> {
    let TyKind::Struct(name, args) = kind else {
        return None;
    };
    let [TyArg::Val(CtValue::Expr(expr))] = args.as_slice() else {
        return None;
    };
    let float = match name.as_str() {
        INT_LITERAL_TYPE_NAME => false,
        FLOAT_LITERAL_TYPE_NAME => true,
        _ => return None,
    };
    let value = expr.as_constant()?;
    numeric_literal_attribute(value, float).then(|| {
        Ty(Arc::new(TyNode::new(TyKind::Struct(
            name.clone(),
            vec![TyArg::Val(value.clone())],
        ))))
    })
}

/// The slot of an `IntLiteral[value]` (or, when `float`, a
/// `FloatLiteral[value]`) application whose argument is that family's
/// attribute or a symbolic expression of its type.
fn numeric_literal_application_slot(kind: &TyKind, float: bool) -> Option<&CtValue> {
    let TyKind::Struct(name, args) = kind else {
        return None;
    };
    let family = if float {
        FLOAT_LITERAL_TYPE_NAME
    } else {
        INT_LITERAL_TYPE_NAME
    };
    let [TyArg::Val(value)] = args.as_slice() else {
        return None;
    };
    if name != family {
        return None;
    }
    let literal = match value {
        CtValue::Expr(expr) => matches!(
            (expr.meta().as_value().map(Ty::kind), float),
            (Some(TyKind::PopIntLiteral), false) | (Some(TyKind::PopFloatLiteral), true)
        ),
        value => numeric_literal_attribute(value, float),
    };
    literal.then_some(value)
}

/// Whether `value` is a known integer (or, when `float`, float) literal
/// attribute.
const fn numeric_literal_attribute(value: &CtValue, float: bool) -> bool {
    matches!(
        (value, float),
        (CtValue::PopIntLiteral(_), false) | (CtValue::PopFloatLiteral(_), true)
    )
}

/// The pieces of an MLIR bracket spelling, each text piece through `piece`
/// and each other argument a `{}` hole, beside those operands in order.
///
/// A text piece is a backtick-stropped argument: one no plain identifier
/// spells, or one whose source span covers its backticks (`` `8` ``,
/// `` `fast` ``), which the lexer drops from the name. A linked name
/// (`__module$std$sys$info$_empty_target`) is an identifier.
fn mlir_bracket_pieces(
    args: &[mojito_ast::ast::ParamArg],
    piece: impl Fn(&str) -> String,
) -> (String, Vec<&mojito_ast::ast::ParamArg>) {
    use mojito_ast::ast::{Expr, ExprKind, ParamArg};
    let mut operands = Vec::new();
    let text = args
        .iter()
        .map(|arg| match arg {
            ParamArg::Value(Expr {
                kind: ExprKind::Identifier(text),
                span: (start, end),
                ..
            }) if !text
                .chars()
                .all(|c| c == '_' || c == '$' || c.is_alphanumeric())
                || end.checked_sub(*start) == Some(text.len() + 2) =>
            {
                piece(text)
            }
            operand => {
                operands.push(operand);
                "{}".to_string()
            }
        })
        .collect();
    (text, operands)
}

/// A `SIMD` application as diagnostics spell it: a scalar alias
/// (`Int`, `Float32`), `Scalar[dtype]` at width one, else `SIMD[dtype, width]`.
fn fmt_simd(f: &mut fmt::Formatter<'_>, dtype: &SimdDtype, width: &SimdWidth) -> fmt::Result {
    match (dtype, width) {
        (SimdDtype::Known(dtype), SimdWidth::Known(1)) => match dtype.repr_alias() {
            Some(alias) => write!(f, "{alias}"),
            None => write!(f, "SIMD[DType.{}, 1]", dtype.name()),
        },
        (dtype, SimdWidth::Known(1)) => write!(f, "Scalar[{dtype}]"),
        (dtype, width) => write!(f, "SIMD[{dtype}, {width}]"),
    }
}

/// A pointer origin as a `Pointer` or a `!lit.ref` displays it.
fn fmt_pointer_origin(
    f: &mut fmt::Formatter<'_>,
    origin: &crate::origin::PointerOrigin,
) -> fmt::Result {
    use crate::origin::{Mutability, PointerOrigin};
    match origin {
        PointerOrigin::Place {
            place,
            mutable: false,
        } => {
            write!(f, "ImmOrigin(origin@{})", place.root.0)?;
        }
        PointerOrigin::Place { place, .. } => {
            write!(f, "origin@{}", place.root.0)?;
        }
        PointerOrigin::Param {
            id,
            mutability,
            interior,
            subtree,
        } => {
            let immutable = *mutability == Mutability::Immutable;
            write!(
                f,
                "{}origin#{}",
                if immutable { "ImmOrigin(" } else { "" },
                id.0
            )?;
            for tag in interior {
                write!(f, "._get_owned_interior[{tag}]")?;
            }
            if *subtree {
                write!(f, "._subtree")?;
            }
            if immutable {
                write!(f, ")")?;
            }
        }
        PointerOrigin::SelfPlace { .. } => {
            write!(f, "origin_of(self)")?;
        }
        PointerOrigin::Static => write!(f, "ImmStaticOrigin")?,
        PointerOrigin::Untracked { mutable: true } => {
            write!(f, "MutUntrackedOrigin")?;
        }
        PointerOrigin::Untracked { mutable: false } => {
            write!(f, "ImmUntrackedOrigin")?;
        }
        PointerOrigin::UnsafeAny { mutable: true } => {
            write!(f, "MutUnsafeAnyOrigin")?;
        }
        PointerOrigin::UnsafeAny { mutable: false } => {
            write!(f, "ImmUnsafeAnyOrigin")?;
        }
    }
    Ok(())
}

/// Declare a pack after `decls` as two binders: its implicit infer-only
/// list binder, inserted after the leading infer-only binders, and the
/// visible binder at the end.
///
/// `pack` is the binder the pack was classified as: a type pack's is its
/// list, a value pack's its visible binder. `list_and_ty` receives the
/// list binder at its slot (a value pack's an untyped placeholder) and the
/// bindings that move every binder, and answers the list binder and the
/// visible binder's type. Returns each moved binder's old identity with
/// its new one.
fn push_pack(
    decls: &mut Vec<ParamDecl>,
    pack: ParamDecl,
    list_and_ty: impl FnOnce(ParamDecl, &ParamBindings) -> (ParamDecl, Ty),
) -> Vec<(ParamId, ParamId)> {
    let end = decls.len();
    insert_pack(decls, end, pack, list_and_ty)
}

/// [`push_pack`] for a pack whose source position `at` is followed by
/// binders `decls` already declares from `at` on: the visible binder
/// takes slot `at` among the source binders, and each later binder moves
/// up a slot past the inserted list binder, as does every reference to
/// one.
fn insert_pack(
    decls: &mut Vec<ParamDecl>,
    at: usize,
    pack: ParamDecl,
    list_and_ty: impl FnOnce(ParamDecl, &ParamBindings) -> (ParamDecl, Ty),
) -> Vec<(ParamId, ParamId)> {
    let owner = pack.id().owner.clone();
    let type_pack = matches!(pack, ParamDecl::Type { .. });
    let position = decls
        .iter()
        .take_while(|decl| mojito_ast::call::CompileTimeParam::infer_only(*decl))
        .count()
        .min(at);
    let list_name = format!("{}.values", pack.name().trim_start_matches('*'));
    let mut ordered = decls.clone();
    ordered.insert(at, pack);
    let (mut moved, bindings) = reown_parameters(&ordered, &owner, |slot, _| {
        if slot < position {
            slot
        } else if type_pack && slot == at {
            position
        } else {
            slot + 1
        }
    });
    let renumbered = ordered
        .iter()
        .zip(&moved)
        .filter(|(old, new)| old.id() != new.id())
        .map(|(old, new)| (old.id().clone(), new.id().clone()))
        .collect();
    if moved.len() <= at {
        return renumbered;
    }
    let moved_pack = moved.remove(at);
    let list_id = ParamId::new(&owner, position);
    let visible_id = ParamId::new(&owner, at + 1);
    let (list, visible) = match moved_pack {
        ParamDecl::Type {
            name,
            bounds,
            trait_param,
            callable_bound,
            default,
            constraints,
            ..
        } => (
            ParamDecl::Type {
                id: list_id,
                name: list_name,
                bounds,
                trait_param,
                callable_bound,
                default,
                infer_only: true,
                variadic: true,
                constraints,
            },
            ParamDecl::Value {
                id: visible_id,
                name,
                ty: Ty::none(),
                default: None,
                callable_default: None,
                infer_only: false,
                variadic: true,
                constraints: Vec::new(),
            },
        ),
        visible @ ParamDecl::Value { .. } => (
            ParamDecl::Value {
                id: list_id,
                name: list_name,
                ty: Ty::none(),
                default: None,
                callable_default: None,
                infer_only: true,
                variadic: false,
                constraints: Vec::new(),
            },
            visible,
        ),
    };
    let (list, ty) = list_and_ty(list, &bindings);
    moved.insert(at, visible.with_value_type(ty));
    moved.insert(position, list);
    *decls = moved;
    renumbered
}

fn reown_parameters(
    decls: &[ParamDecl],
    owner: &str,
    slot: impl Fn(usize, &ParamDecl) -> usize,
) -> (Vec<ParamDecl>, ParamBindings) {
    let mut bindings = ParamBindings::new();
    let mut binders: HashMap<ParamId, ParamRef> = HashMap::new();
    let mut subst = TySubst::new();
    for (position, decl) in decls.iter().enumerate() {
        let declared = decl.binder();
        let reowned = ParamRef {
            id: ParamId::new(owner, slot(position, decl)),
            name: declared.name.clone(),
        };
        if let Some(ty) = bind_reowned(&mut bindings, decl, &reowned) {
            subst.insert(declared.id.clone(), ty);
        }
        binders.insert(declared.id, reowned);
    }
    let moved = |ty: &Ty| bind_signature_slots(ty, &bindings);
    let reowned = decls
        .iter()
        .map(|decl| {
            let mut decl = decl.clone();
            let (id, constraints) = match &mut decl {
                ParamDecl::Type {
                    id,
                    trait_param,
                    callable_bound,
                    default,
                    constraints,
                    ..
                } => {
                    for ty in callable_bound.iter_mut().chain(default.iter_mut()) {
                        *ty = moved(ty);
                    }
                    if let Some(binder) = trait_param
                        && let Some(reowned) = binders.get(&binder.id)
                    {
                        *binder = reowned.clone();
                    }
                    (id, constraints)
                }
                ParamDecl::Value {
                    id,
                    ty,
                    default,
                    constraints,
                    ..
                } => {
                    *ty = moved(ty);
                    if let Some(default) = default {
                        *default = ParamContext::detached()
                            .replace(default, &bindings)
                            .unwrap_or_else(|_| default.clone());
                    }
                    (id, constraints)
                }
            };
            *id = binders[id].id.clone();
            *constraints = constraints
                .iter()
                .map(|constraint| {
                    rename_constraint_parameters(constraint, &binders, &subst, &bindings)
                })
                .collect();
            decl
        })
        .collect();
    (reowned, bindings)
}

/// Bind `decl`'s binder to `reowned`, the binder declaring it as another
/// declaration's own, in `bindings`: a type binder to the parameter type
/// over `reowned` (returned), and a type pack's list binder or a value
/// binder to a reference to it.
fn bind_reowned(bindings: &mut ParamBindings, decl: &ParamDecl, reowned: &ParamRef) -> Option<Ty> {
    let declared = decl.binder();
    match decl {
        ParamDecl::Type {
            bounds,
            callable_bound,
            ..
        } => {
            let ty = Ty::new(TyKind::Param {
                binder: reowned.clone(),
                bounds: bounds.clone(),
                callable_bound: callable_bound.clone(),
            });
            bindings.bind_type(declared.id.clone(), ty.clone());
            // A type pack's list binder is read as a list too
            // (`TypeList[Tr, Ts.values]`).
            if let ParamDecl::Type { variadic: true, .. } = decl {
                bindings.bind(declared.id, decl.list_reference(&reowned.id));
            }
            Some(ty)
        }
        ParamDecl::Value { name, ty, .. } => {
            bindings.bind(
                declared.id,
                ParamContext::detached().decl_ref(
                    reowned.id.clone(),
                    name.trim_start_matches('*'),
                    crate::param_expr::MetaTy::value((*ty).clone()),
                ),
            );
            None
        }
    }
}

/// The `VariadicPack` with the element trait `traits` over `pack`, the
/// pack's argument.
fn variadic_pack_over(traits: Vec<String>, pack: TyArg, is_owned: bool) -> Ty {
    TyKind::Struct(
        VARIADIC_PACK_TYPE_NAME.to_string(),
        vec![
            TyArg::Val(CtValue::Trait(traits)),
            pack,
            TyArg::Val(CtValue::Bool(is_owned)),
            TyArg::Val(ParamDecl::type_list_value()),
        ],
    )
    .into()
}

/// The trait a type pack's elements conform to: its binder's bounds,
/// `AnyType` for a pack with none.
fn pack_traits(pack: &Ty) -> Vec<String> {
    match pack.kind() {
        TyKind::Param { bounds, .. } if !bounds.is_empty() => bounds.clone(),
        _ => vec!["AnyType".to_string()],
    }
}

#[cfg(test)]
mod simd_slot_tests {
    use super::*;
    use crate::param_expr::{MetaTy, ParamId};

    fn dtype_binder(context: &ParamContext) -> ParamExpr {
        context.decl_ref(ParamId::new("f", 0), "dt", MetaTy::value(Ty::dtype()))
    }

    /// `SIMDLength(width)` over the `Int` binder `width`, as `SIMD[dt,
    /// width]` holds it.
    fn width_binder(context: &ParamContext) -> ParamExpr {
        let width = context.decl_ref(ParamId::new("f", 1), "width", MetaTy::int());
        simd_length_converted(context, "SIMDLength.__init__$ov$Int", &width)
            .expect("an Int converts")
    }

    #[test]
    fn closed_slots_fold_to_known_and_canonicalize() {
        let context = ParamContext::detached();
        let float64 = context
            .constant(dtype_value(Dtype::Float64))
            .expect("a constant");
        assert_eq!(
            simd_ty_from_slots(SimdDtype::Expr(float64), SimdWidth::Known(1)).expect("closed"),
            Ty::float64()
        );
        let one = context.constant(CtValue::Int(1)).expect("a constant");
        let one = simd_length_converted(&context, "SIMDLength.__init__$ov$Int", &one)
            .expect("an Int converts");
        assert_eq!(
            simd_ty_from_slots(SimdDtype::Known(Dtype::Int32), SimdWidth::Expr(one))
                .expect("closed"),
            canonical_simd_ty(Dtype::Int32, 1)
        );
        let symbolic =
            simd_ty_from_slots(SimdDtype::Expr(dtype_binder(&context)), SimdWidth::Known(1))
                .expect("symbolic");
        assert!(is_symbolic(&symbolic));
        assert_eq!(symbolic.to_string(), "Scalar[dt]");
        assert!(simd_shape(&symbolic).is_none());
        assert!(is_scalar_simd(&symbolic) && scalar_simd_dtype(&symbolic).is_none());
    }

    #[test]
    fn literals_splat_into_a_symbolic_lane_and_concrete_scalars_do_not() {
        let context = ParamContext::detached();
        let lane = SimdDtype::Expr(dtype_binder(&context));
        assert!(splats_to(&Ty::int_literal_of(&0.into()), &lane));
        assert!(splats_to(
            &Ty::float_literal_of(&FloatLiteral::from_int(&0.into())),
            &lane
        ));
        for scalar in [
            Ty::int(),
            Ty::float64(),
            Ty::bool(),
            canonical_simd_ty(Dtype::Int32, 1),
        ] {
            assert!(!splats_to(&scalar, &lane), "{scalar}");
        }
        let same = Ty::simd(lane.clone(), SimdWidth::Known(1));
        assert!(splats_to(&same, &lane));
        assert!(coerces(
            &Ty::float_literal_of(&FloatLiteral::from_int(&0.into())),
            &same
        ));
        assert!(!coerces(&same, &Ty::float64()));
        let vector = Ty::simd(lane.clone(), SimdWidth::Expr(width_binder(&context)));
        assert!(!coerces(&vector, &canonical_simd_ty(Dtype::Int32, 4)));
        assert!(coerces(
            &vector,
            &Ty::simd(lane, SimdWidth::inferred(&context))
        ));
    }

    #[test]
    fn exact_literals_splat_into_a_known_multi_lane_vector() {
        let int32x4 = canonical_simd_ty(Dtype::Int32, 4);
        let float32x4 = canonical_simd_ty(Dtype::Float32, 4);
        assert!(coerces(&Ty::int_literal_of(&0.into()), &int32x4));
        assert!(coerces(&Ty::int_literal_of(&0.into()), &float32x4));
        assert!(coerces(
            &Ty::float_literal_of(&FloatLiteral::from_int(&0.into())),
            &float32x4
        ));
        assert!(!coerces(
            &Ty::float_literal_of(&FloatLiteral::from_int(&0.into())),
            &int32x4
        ));
        assert!(coerces(&Ty::bool(), &canonical_simd_ty(Dtype::Bool, 4)));
        assert!(!coerces(&canonical_simd_ty(Dtype::Int32, 1), &int32x4));
        assert!(!coerces(&Ty::int(), &canonical_simd_ty(Dtype::Int, 4)));
    }

    #[test]
    fn replacement_closes_symbolic_slots_through_the_canonical_form() {
        let context = ParamContext::detached();
        let scalar = Ty::simd(
            SimdDtype::Expr(dtype_binder(&context)),
            SimdWidth::Expr(width_binder(&context)),
        );
        let mut referenced = std::collections::HashSet::new();
        referenced_parameters(&scalar, &mut referenced);
        assert!(referenced.contains("dt") && referenced.contains("width"));
        let values = HashMap::from([
            ("dt".to_string(), dtype_value(Dtype::Float64)),
            ("width".to_string(), CtValue::Int(1)),
        ]);
        let bindings = ParamBindings::from_named_values(&context, &values);
        assert_eq!(
            replace_parameters(&context, &scalar, &bindings, 0).expect("closed"),
            Ty::float64()
        );
    }
}

#[cfg(test)]
mod collection_representation_tests {
    use super::*;

    #[test]
    fn public_collection_helpers_construct_only_nominal_types() {
        let runtime_list = list_type(Ty::int());
        for ty in [
            runtime_list.clone(),
            set_type(Ty::int()),
            dict_type(Ty::string_literal_of("k"), Ty::int()),
            range_type(),
        ] {
            assert!(matches!(ty.kind(), TyKind::Struct(..)), "got {ty:?}");
        }
        assert_ne!(runtime_list, TyKind::ComptimeList(Ty::int()));
        assert_eq!(
            TyKind::ComptimeList(Ty::int()).to_string(),
            "<comptime-list[Int]>"
        );
    }

    #[test]
    fn uninit_storage_element_recognizes_every_mangled_spelling() {
        let storage =
            |name: &str| Ty::new(TyKind::Struct(name.to_string(), vec![TyArg::Ty(Ty::int())]));
        for name in [
            "__UninitStorage",
            "mono_test$__UninitStorage",
            "__UninitStorage$mono$TInt",
            "mono_test$__UninitStorage$mono$TRecorder",
        ] {
            assert_eq!(
                uninit_storage_element(&storage(name)),
                Some(&Ty::int()),
                "{name}"
            );
        }
        assert_eq!(uninit_storage_element(&storage("Storageish")), None);
        assert_eq!(
            uninit_storage_element(&Ty::new(TyKind::Struct("__UninitStorage".into(), vec![]))),
            None
        );
    }
}

#[cfg(test)]
mod encoding_tests {
    use super::*;
    use crate::origin::{Origin, OriginPlace, OwnerId, RefSig, SigMutability, SigOrigin};
    use crate::param_expr::{MetaTy, ParamId};
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};

    fn round_trip<T>(value: &T) -> T
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
    {
        postcard::from_bytes(&postcard::to_stdvec(value).expect("encode")).expect("decode")
    }

    fn hash_of(value: &impl std::hash::Hash) -> u64 {
        BuildHasherDefault::<DefaultHasher>::default().hash_one(value)
    }

    #[test]
    fn checked_vocabulary_round_trips() {
        let context = ParamContext::new();
        let n = context.decl_ref(ParamId::new("f", 1), "n", MetaTy::int());
        let doubled = context
            .infix(mojito_ast::ast::InfixOp::Mul, &n, &n)
            .expect("n * n");
        let decls = vec![
            ParamDecl::Type {
                id: ParamId::new("f", 0),
                name: "T".to_string(),
                bounds: vec!["Copyable".to_string()],
                trait_param: None,
                callable_bound: None,
                default: None,
                infer_only: false,
                variadic: false,
                constraints: vec![GenericConstraint::Conforms {
                    operand: ConstraintOperand::Param(n.as_decl_ref().expect("a binder").clone()),
                    trait_name: "Writable".to_string(),
                }],
            },
            ParamDecl::Value {
                id: ParamId::new("f", 1),
                name: "n".to_string(),
                ty: Ty::int(),
                default: Some(context.constant(CtValue::Int(4)).expect("4")),
                callable_default: None,
                infer_only: false,
                variadic: false,
                constraints: Vec::new(),
            },
        ];
        let array = Ty::new(TyKind::Struct(
            "Array".to_string(),
            vec![TyArg::Ty(Ty::int()), TyArg::Val(CtValue::Expr(doubled))],
        ));
        let function = TyKind::GenericFunc(Box::new(GenericFuncTy {
            decls,
            func: FuncTy {
                environment: crate::origin::CallableEnvironment::Default,
                params: vec![array.clone()],
                names: vec!["xs".to_string()],
                ret: array,
                required: vec![true],
                variadic: None,
                kw_variadic: None,
                positional_only: None,
                keyword_only: None,
                raises: false,
                error: None,
                conventions: vec![None],
                ref_params: vec![Some(RefSig {
                    origin: SigOrigin::Bound(Origin::Param(crate::origin::OriginParamId(0))),
                    mutability: SigMutability::Immutable,
                })],
                ref_return: None,
                transfers: TransferSet::default(),
            },
        }));
        let decoded = round_trip(&function);
        assert_eq!(decoded, function);
        assert_eq!(hash_of(&decoded), hash_of(&function));
    }

    #[test]
    fn every_encoded_kind_round_trips() {
        let context = ParamContext::new();
        let n = context.decl_ref(ParamId::new("f", 1), "n", MetaTy::int());
        let field = context.field(&n, "rows", MetaTy::int()).expect("a field");
        assert_eq!(round_trip(&field), field);
        let pack = |owner: &str| {
            context.decl_ref(
                ParamId::new(owner, 0),
                "Ts",
                MetaTy::ParamList(Box::new(MetaTy::Type)),
            )
        };
        let concat = context
            .list_concat(&[pack("f"), pack("g")])
            .expect("a concatenation");
        assert_eq!(round_trip(&concat), concat);
    }

    #[test]
    fn a_binding_owner_is_never_encoded() {
        let place = Origin::Place(OriginPlace {
            root: OwnerId(3),
            path: Vec::new(),
        });
        assert!(postcard::to_stdvec(&place).is_err());
    }
}

#[cfg(test)]
mod parameter_replacement_tests {
    use super::*;
    use crate::param_expr::ParamId;

    #[test]
    fn constraint_replacement_binds_values_and_renames_type_binders() {
        let context = ParamContext::detached();
        let reference = |owner: &str, name: &str| ParamRef {
            id: ParamId::new(owner, 0),
            name: Arc::from(name),
        };
        let (n, t, u) = (
            reference("f", "n"),
            reference("g", "T"),
            reference("h", "U"),
        );
        let mut bindings = ParamBindings::new();
        bindings.bind(
            n.id.clone(),
            context.constant(CtValue::Int(3)).expect("a constant"),
        );
        let binder = |binder: ParamRef| -> Ty {
            TyKind::Param {
                binder,
                bounds: Vec::new(),
                callable_bound: None,
            }
            .into()
        };
        bindings.bind_type(t.id.clone(), binder(u.clone()));
        let compared = GenericConstraint::Eq(
            ConstraintOperand::Param(n),
            ConstraintOperand::Value(CtValue::Int(0)),
        );
        assert_eq!(
            replace_constraint_parameters(&context, &compared, &bindings).expect("replaced"),
            GenericConstraint::Eq(
                ConstraintOperand::Value(CtValue::Int(3)),
                ConstraintOperand::Value(CtValue::Int(0)),
            )
        );
        let conforms = |operand| GenericConstraint::Conforms {
            operand,
            trait_name: "Stringable".to_string(),
        };
        assert_eq!(
            replace_constraint_parameters(
                &context,
                &conforms(ConstraintOperand::Param(t.clone())),
                &bindings
            )
            .expect("renamed"),
            conforms(ConstraintOperand::Param(u))
        );
        bindings.bind_type(t.id.clone(), Ty::int());
        assert_eq!(
            replace_constraint_parameters(
                &context,
                &conforms(ConstraintOperand::Param(t)),
                &bindings
            )
            .expect("bound"),
            conforms(ConstraintOperand::Type(Ty::int()))
        );
    }

    /// A pack query at a concrete binding asks of the bound list, as
    /// upstream's `TypeList` queries take the list as an operand.
    #[test]
    fn pack_query_replacement_holds_the_bound_list() {
        let context = ParamContext::detached();
        let pack = ParamRef {
            id: ParamId::new("f", 0),
            name: Arc::from("Ts"),
        };
        let list = CtValue::Tuple(vec![CtValue::Type(Ty::int()), CtValue::Type(Ty::bool())]);
        let mut bindings = ParamBindings::new();
        bindings.bind(
            pack.id.clone(),
            context.constant(list.clone()).expect("a constant"),
        );
        let queries = |operand: ConstraintOperand| {
            GenericConstraint::And(
                Box::new(GenericConstraint::PackContains {
                    operand: operand.clone(),
                    element: ConstraintOperand::Type(Ty::int()),
                }),
                Box::new(GenericConstraint::Eq(
                    ConstraintOperand::PackLength(Box::new(operand)),
                    ConstraintOperand::Value(CtValue::Int(2)),
                )),
            )
        };
        assert_eq!(
            replace_constraint_parameters(
                &context,
                &queries(ConstraintOperand::Param(pack)),
                &bindings
            )
            .expect("bound"),
            queries(ConstraintOperand::Value(list))
        );
    }
}

#[cfg(test)]
mod canonical_signature_tests {
    use super::*;
    use crate::param_expr::ParamId;

    /// The canonical parameter type of `def get[*Us](x: Us[index])` whose
    /// pack `Us` the declaration `owner` declares.
    fn pack_element_signature(owner: &str, index: i64) -> Ty {
        let context = ParamContext::detached();
        let pack = ParamDecl::Type {
            id: ParamId::new(owner, 0),
            name: "*Us".to_string(),
            bounds: vec!["AnyType".to_string()],
            trait_param: None,
            callable_bound: None,
            default: None,
            infer_only: false,
            variadic: true,
            constraints: Vec::new(),
        };
        let element = context
            .list_get(
                &context.decl_ref(ParamId::new(owner, 0), "Us", pack.binder_meta()),
                &context.constant(CtValue::Int(index)).expect("a constant"),
            )
            .expect("an element");
        let (_, params) = canonical_generic_signature(
            &[pack],
            &[TyKind::Dependent(DependentType::Parameter(element)).into()],
        );
        params.into_iter().next().expect("one parameter")
    }

    #[test]
    fn a_pack_read_by_a_dependent_type_is_its_signature_slot() {
        assert_eq!(
            pack_element_signature("Getter.get", 1),
            pack_element_signature("S.get", 1)
        );
        assert_ne!(
            pack_element_signature("Getter.get", 1),
            pack_element_signature("S.get", 0)
        );
    }
}

#[cfg(test)]
mod struct_field_types_tests {
    use super::*;
    use std::borrow::Cow;

    fn value_param(owner: &str) -> ParamDecl {
        ParamDecl::Value {
            id: ParamId::new(owner, 0),
            name: "n".to_string(),
            ty: Ty::int(),
            default: None,
            callable_default: None,
            infer_only: false,
            variadic: false,
            constraints: Vec::new(),
        }
    }

    fn type_param(owner: &str) -> ParamDecl {
        ParamDecl::Type {
            id: ParamId::new(owner, 0),
            name: "T".to_string(),
            bounds: vec!["Copyable".to_string()],
            trait_param: None,
            callable_bound: None,
            default: None,
            infer_only: false,
            variadic: false,
            constraints: Vec::new(),
        }
    }

    /// `SIMD[DType.int64, n]` over the value binder `decl`.
    fn lanes_over(decl: &ParamDecl) -> Ty {
        let context = ParamContext::detached();
        let width = context.decl_ref(decl.id().clone(), "n", decl.binder_meta());
        let width = simd_length_converted(&context, "SIMDLength.__init__$ov$Int", &width)
            .expect("an Int converts");
        simd_ty_from_slots(SimdDtype::Known(Dtype::Int64), SimdWidth::Expr(width))
            .expect("a symbolic SIMD type")
    }

    #[test]
    fn struct_field_types_are_the_fields_at_the_arguments() {
        let arr = value_param("Arr");
        let fields = vec![("v".to_string(), lanes_over(&arr))];
        let at = |arguments: &[TyArg]| {
            struct_field_types(std::slice::from_ref(&arr), arguments, &fields)
                .map(|fields| fields[0].1.clone())
        };
        assert_eq!(
            at(&[TyArg::Val(CtValue::Int(4))]),
            Some(Ty::simd(
                SimdDtype::Known(Dtype::Int64),
                SimdWidth::Known(4)
            ))
        );
        let caller = value_param("plain_nested");
        let forwarded =
            ParamContext::detached().decl_ref(caller.id().clone(), "n", caller.binder_meta());
        assert_eq!(
            at(&[TyArg::Val(CtValue::Expr(forwarded))]),
            Some(lanes_over(&caller))
        );
        assert_eq!(at(&[TyArg::Val(CtValue::RemovedArg)]), None);
        assert_eq!(at(&[]), None);

        let p = type_param("P");
        let fields = vec![(
            "x".to_string(),
            Ty::new(TyKind::Param {
                binder: p.binder(),
                bounds: vec!["Copyable".to_string()],
                callable_bound: None,
            }),
        )];
        let at = |arguments: &[TyArg]| {
            struct_field_types(std::slice::from_ref(&p), arguments, &fields)
                .map(|fields| fields[0].1.clone())
        };
        assert_eq!(at(&[TyArg::Ty(Ty::int())]), Some(Ty::int()));
        assert_eq!(at(&[TyArg::Val(CtValue::RemovedArg)]), None);
        assert_eq!(at(&[]), None);

        let concrete = vec![("x".to_string(), Ty::int())];
        assert!(matches!(
            struct_field_types(&[], &[], &concrete),
            Some(Cow::Borrowed(_))
        ));
    }

    #[test]
    fn a_type_pack_bound_whole_expands_in_a_field() {
        let pack = ParamDecl::Type {
            id: ParamId::new("Pack", 0),
            name: "*Ts".to_string(),
            bounds: vec!["TrivialRegisterPassable".to_string()],
            trait_param: None,
            callable_bound: None,
            default: None,
            infer_only: false,
            variadic: true,
            constraints: Vec::new(),
        };
        let fields = vec![(
            "_mlir_value".to_string(),
            Ty::new(TyKind::Tuple(vec![Ty::new(TyKind::Param {
                binder: pack.binder(),
                bounds: vec!["TrivialRegisterPassable".to_string()],
                callable_bound: None,
            })])),
        )];
        let at = |arguments: &[TyArg]| {
            struct_field_types(std::slice::from_ref(&pack), arguments, &fields)
                .map(|fields| fields[0].1.clone())
        };
        let elements = vec![Ty::int(), Ty::bool()];
        assert_eq!(
            at(&[TyArg::Val(CtValue::Tuple(
                elements.iter().cloned().map(CtValue::Type).collect()
            ))]),
            Some(Ty::new(TyKind::Tuple(elements)))
        );
        assert_eq!(at(&[]), None);
    }
}

#[cfg(test)]
mod value_pack_decl_tests {
    use super::*;

    /// `[n: Int, *vs: Int]` as two binders and the pack's list.
    fn value_pack() -> Vec<ParamDecl> {
        let value = |slot, name: &str| ParamDecl::Value {
            id: ParamId::new("f", slot),
            name: name.to_string(),
            ty: Ty::int(),
            default: None,
            callable_default: None,
            infer_only: false,
            variadic: name.starts_with('*'),
            constraints: Vec::new(),
        };
        let mut decls = vec![value(0, "n")];
        let moved = ParamDecl::push_value_pack(&mut decls, value(1, "*vs"));
        assert_eq!(
            moved,
            vec![
                (ParamId::new("f", 0), ParamId::new("f", 1)),
                (ParamId::new("f", 1), ParamId::new("f", 2)),
            ]
        );
        decls
    }

    struct Decls(Vec<ParamDecl>);

    impl fmt::Display for Decls {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write_param_decls(f, &self.0)
        }
    }

    #[test]
    fn a_value_pack_declares_its_list_after_the_infer_only_binders() {
        let decls = value_pack();
        assert_eq!(
            decls.iter().map(ParamDecl::name).collect::<Vec<_>>(),
            ["vs.values", "n", "*vs"]
        );
        assert_eq!(decls[0].id(), &ParamId::new("f", 0));
        assert_eq!(ParamDecl::pack_list(&decls, &decls[2]), Some(&decls[0]));
        assert_eq!(decls[0].list_pack(&decls), Some(&decls[2]));
        assert_eq!(
            decls[0].binder_meta(),
            MetaTy::ParamList(Box::new(MetaTy::value(Ty::int())))
        );
    }

    #[test]
    fn a_value_pack_binds_values_of_its_element_type() {
        assert_eq!(value_pack()[2].value_element_ty(), Some(&Ty::int()));
    }

    #[test]
    fn a_value_pack_displays_its_element_type() {
        assert_eq!(Decls(value_pack()).to_string(), "[n: Int, *vs: Int]");
    }
}

#[cfg(test)]
mod string_literal_application_tests {
    use super::*;

    fn application(args: Vec<TyArg>) -> Ty {
        Ty::new(TyKind::Struct(STRING_LITERAL_TYPE_NAME.to_string(), args))
    }

    #[test]
    fn a_string_literal_is_the_declared_struct_application() {
        let ab = Ty::string_literal_of("ab");
        assert!(matches!(ab.kind(), TyKind::Struct(name, _) if name == STRING_LITERAL_TYPE_NAME));
        assert!(ab.is_string_literal());
        assert_eq!(ab.kind().string_literal_text(), Some("ab"));
        assert!(!ab.kind().is_non_simd_struct());
        let (name, args) = nominal_parts(&ab).expect("a literal is an instance");
        assert_eq!(name, STRING_LITERAL_TYPE_NAME);
        assert_eq!(application(args.into_owned()), ab);
        assert_eq!(ab.to_string(), "StringLiteral[\"ab\"]");
    }

    #[test]
    fn a_closed_slot_folds_to_its_text() {
        let closed = ParamContext::detached()
            .constant(CtValue::KgenString("ab".to_string()))
            .expect("a KGEN string constant");
        assert_eq!(
            application(vec![TyArg::Val(CtValue::Expr(closed))]),
            Ty::string_literal_of("ab")
        );
    }

    #[test]
    fn a_literal_coerces_to_its_own_text_only() {
        let ab = Ty::string_literal_of("ab");
        assert!(coerces(&ab, &Ty::string_literal_of("ab")));
        assert!(!coerces(&ab, &Ty::string_literal_of("cd")));
    }

    #[test]
    fn a_bare_or_non_string_application_is_no_literal() {
        let bare = application(Vec::new());
        assert!(!bare.is_string_literal());
        assert!(bare.kind().is_non_simd_struct());
        let int_slot = application(vec![TyArg::Val(CtValue::Int(3))]);
        assert!(!int_slot.is_string_literal());
    }
}

#[cfg(test)]
mod numeric_literal_application_tests {
    use super::*;

    fn application(name: &str, value: CtValue) -> Ty {
        Ty::new(TyKind::Struct(name.to_string(), vec![TyArg::Val(value)]))
    }

    #[test]
    fn an_int_literal_application_is_the_literal_type_carrying_its_value() {
        let four = application(INT_LITERAL_TYPE_NAME, CtValue::PopIntLiteral(4.into()));
        assert_eq!(four, Ty::int_literal_of(&4.into()));
        let (name, args) = nominal_parts(&four).expect("a carried literal is an instance");
        let [TyArg::Val(value)] = args.as_ref() else {
            panic!("`IntLiteral` takes one value argument");
        };
        assert_eq!(application(name, value.clone()), four);
        assert!(!coerces(&four, &Ty::int_literal_of(&7.into())));
        assert!(coerces(&four, &Ty::int_literal_of(&4.into())));
        assert!(coerces(
            &four,
            &Ty::float_literal_of(&FloatLiteral::from_int(&4.into()))
        ));
        assert_eq!(four.to_string(), "IntLiteral[4]");
        assert!(matches!(four.kind(), TyKind::Struct(name, _) if name == INT_LITERAL_TYPE_NAME));
        assert!(!four.kind().is_non_simd_struct());
    }

    #[test]
    fn a_closed_slot_folds_to_its_attribute() {
        let context = crate::param_expr::ParamContext::new();
        let closed = context
            .constant(CtValue::PopIntLiteral(4.into()))
            .expect("a literal attribute is a constant");
        assert_eq!(
            application(INT_LITERAL_TYPE_NAME, CtValue::Expr(closed)),
            Ty::int_literal_of(&4.into())
        );
    }

    #[test]
    fn a_slot_of_the_other_family_is_no_literal() {
        let float_slot = application(
            INT_LITERAL_TYPE_NAME,
            CtValue::PopFloatLiteral(FloatLiteral::from_int(&4.into())),
        );
        assert!(!float_slot.is_numeric_literal());
        assert!(float_slot.kind().is_non_simd_struct());
    }

    #[test]
    fn a_float_literal_type_spells_its_value_as_the_pin() {
        let tenth = FloatLiteral::parse_decimal("0.1").expect("a decimal literal");
        assert_eq!(
            Ty::float_literal_of(&tenth).to_string(),
            "FloatLiteral[0.10000000000000001]"
        );
    }

    #[test]
    fn a_symbolic_literal_type_spells_its_attribute() {
        let context = crate::param_expr::ParamContext::new();
        let value = context.register(
            crate::param_expr::ParamId::new("f", 0),
            "value",
            crate::param_expr::MetaTy::value(TyKind::PopIntLiteral.into()),
        );
        let ty = Ty::int_literal_at(CtValue::Expr(value));
        assert!(ty.is_int_literal());
        assert!(is_symbolic(&ty));
        assert_eq!(ty.to_string(), "IntLiteral[value]");
        assert!(!coerces(&ty, &Ty::int_literal_of(&4.into())));
    }
}

#[cfg(test)]
mod type_pack_decl_tests {
    use super::*;

    /// `[n: Int, *Ts: Movable]` as two binders and the pack's list.
    fn type_pack() -> Vec<ParamDecl> {
        let mut decls = vec![ParamDecl::Value {
            id: ParamId::new("S", 0),
            name: "n".to_string(),
            ty: Ty::int(),
            default: None,
            callable_default: None,
            infer_only: false,
            variadic: false,
            constraints: Vec::new(),
        }];
        let list = ParamDecl::Type {
            id: ParamId::new("S", 1),
            name: "*Ts".to_string(),
            bounds: vec!["Movable".to_string()],
            trait_param: None,
            callable_bound: None,
            default: None,
            infer_only: false,
            variadic: true,
            constraints: Vec::new(),
        };
        let moved = ParamDecl::push_type_pack(
            &mut decls,
            list,
            &CtValue::Trait(vec!["Movable".to_string()]),
        );
        assert_eq!(
            moved,
            vec![
                (ParamId::new("S", 0), ParamId::new("S", 1)),
                (ParamId::new("S", 1), ParamId::new("S", 0)),
            ]
        );
        decls
    }

    #[test]
    fn a_type_pack_declares_its_list_after_the_infer_only_binders() {
        let decls = type_pack();
        assert_eq!(
            decls.iter().map(ParamDecl::name).collect::<Vec<_>>(),
            ["Ts.values", "n", "*Ts"]
        );
        assert!(decls[0].is_pack_list() && decls[2].is_type_pack());
        assert_eq!(ParamDecl::pack_list(&decls, &decls[2]), Some(&decls[0]));
        assert_eq!(ParamDecl::type_pack_list(&decls, "Ts"), Some(&decls[0]));
        assert_eq!(decls[0].binder_meta(), MetaTy::type_list());
        assert_eq!(&*decls[0].binder().name, "*Ts");
        assert_eq!(decls[2].value_element_ty(), None);
        assert_eq!(decls[2].pack_value(), Some(ParamDecl::type_list_value()));
    }

    #[test]
    fn an_elementwise_struct_regroups_its_pack_beside_the_type_list() {
        let mut decls = Vec::new();
        let list = ParamDecl::Type {
            id: ParamId::new("Tuple", 0),
            name: "*Ts".to_string(),
            bounds: vec!["Movable".to_string()],
            trait_param: None,
            callable_bound: None,
            default: None,
            infer_only: false,
            variadic: true,
            constraints: Vec::new(),
        };
        ParamDecl::push_type_pack(&mut decls, list, &CtValue::Trait(Vec::new()));
        let canonical = vec![TyArg::Ty(Ty::int()), TyArg::Ty(Ty::bool())];
        let declared = declared_pack_arguments(TUPLE_TYPE_NAME, &decls, &canonical).into_owned();
        assert_eq!(
            declared,
            vec![
                TyArg::Val(CtValue::Tuple(vec![
                    CtValue::Type(Ty::int()),
                    CtValue::Type(Ty::bool())
                ])),
                TyArg::Val(ParamDecl::type_list_value()),
            ]
        );
        assert_eq!(
            canonical_pack_arguments(TUPLE_TYPE_NAME, declared),
            canonical
        );
    }

    #[test]
    fn a_type_pack_displays_its_bounds() {
        struct Decls(Vec<ParamDecl>);

        impl fmt::Display for Decls {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write_param_decls(f, &self.0)
            }
        }

        assert_eq!(Decls(type_pack()).to_string(), "[n: Int, *Ts: Movable]");
    }
}

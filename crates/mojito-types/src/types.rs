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

/// The element type of a [`TyKind::Simd`]: a concrete dtype, or a `DType`-valued
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
            Self::Expr(expr) => match expr.require_constant() {
                Ok(CtValue::Dtype(dtype)) => Some(dtype),
                _ => None,
            },
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

/// The length of a [`TyKind::Simd`]: a concrete lane count, or the
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

/// Upstream's `SIMDLength`, the type of a `SIMD` width slot.
pub fn simd_length_ty() -> Ty {
    TyKind::Struct(SIMD_LENGTH_TYPE_NAME.to_string(), Vec::new()).into()
}

/// Whether `ty` is upstream's `SIMDLength`.
pub fn is_simd_length(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::Struct(name, args) if name == SIMD_LENGTH_TYPE_NAME && args.is_empty())
}

/// The lane count a `SIMDLength` value is, `Int(mlir_value=length._mlir_value)`:
/// upstream's `SIMD.__init__(SIMDLength)` folded, which is what a SIMD
/// width slot holds. A closed length folds to its `Int`.
pub fn simd_length_image(
    context: &ParamContext,
    length: &ParamExpr,
) -> Result<ParamExpr, ParamError> {
    context.cast_from_builtin(&context.field(
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
/// length's [`simd_length_image`]: the inverse a width binder is solved by.
pub fn simd_length_of_image(lanes: &ParamExpr) -> Option<&ParamExpr> {
    let ParamKind::Op {
        op: crate::param_expr::ParamOp::CastFromBuiltin,
        operands,
    } = lanes.kind()
    else {
        return None;
    };
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
/// `i` reads lane `mask[i]` of the source lanes, a join's being the
/// receiver's followed by its argument's.
///
/// A gather whose receiver width, or whose own compile-time arguments, name
/// a template's binders keeps its method's form, which the instantiation
/// closes and checks against the method's constraints, as the pin's
/// `comptime assert`s do. Concrete MIR holds only `Known`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum LaneMask {
    Known(Vec<usize>),
    /// `v.shuffle[i, j, …]()`: one `Int` lane index per receiver lane.
    Shuffle(Vec<ParamExpr>),
    /// `v.slice[count, offset=start]()`: `count` consecutive receiver lanes
    /// from `start`.
    Slice {
        start: ParamExpr,
        count: SimdWidth,
    },
    /// `v.join(w)`: every lane of the receiver, then of the argument.
    Join,
}

impl LaneMask {
    pub fn known(&self) -> Option<&[usize]> {
        match self {
            Self::Known(mask) => Some(mask),
            _ => None,
        }
    }

    pub const fn is_symbolic(&self) -> bool {
        !matches!(self, Self::Known(_))
    }

    /// The parameter expressions the mask reads.
    pub fn expressions(&self) -> Vec<&ParamExpr> {
        match self {
            Self::Known(_) | Self::Join => Vec::new(),
            Self::Shuffle(lanes) => lanes.iter().collect(),
            Self::Slice { start, count } => {
                let count = match count {
                    SimdWidth::Expr(expr) => Some(expr),
                    SimdWidth::Known(_) => None,
                };
                std::iter::once(start).chain(count).collect()
            }
        }
    }

    /// The mask with each expression rebuilt through `f`, a slice's lane
    /// count known once `f` closes it.
    pub fn try_map_exprs<E>(
        &self,
        f: &mut impl FnMut(&ParamExpr) -> Result<ParamExpr, E>,
    ) -> Result<Self, E> {
        Ok(match self {
            Self::Known(_) | Self::Join => self.clone(),
            Self::Shuffle(lanes) => {
                Self::Shuffle(lanes.iter().map(&mut *f).collect::<Result<_, _>>()?)
            }
            Self::Slice { start, count } => Self::Slice {
                start: f(start)?,
                count: match count {
                    SimdWidth::Known(_) => count.clone(),
                    SimdWidth::Expr(length) => {
                        let length = f(length)?;
                        SimdWidth::of_length(&ParamContext::detached(), length.clone())
                            .unwrap_or(SimdWidth::Expr(length))
                    }
                },
            },
        })
    }

    /// The mask with each expression `close` answers replaced by its
    /// constant; one it leaves open stays as written.
    #[must_use]
    pub fn close_with(&self, close: &impl Fn(&ParamExpr) -> Option<i64>) -> Self {
        let context = ParamContext::detached();
        let expr = |expr: &ParamExpr| {
            close(expr)
                .and_then(|value| context.constant(CtValue::Int(value)).ok())
                .unwrap_or_else(|| expr.clone())
        };
        let width = |width: &SimdWidth| match width {
            SimdWidth::Expr(_) => width
                .lanes(&context)
                .ok()
                .and_then(|lanes| close(&lanes))
                .map_or_else(|| width.clone(), SimdWidth::Known),
            SimdWidth::Known(_) => width.clone(),
        };
        match self {
            Self::Known(_) | Self::Join => self.clone(),
            Self::Shuffle(lanes) => Self::Shuffle(lanes.iter().map(expr).collect()),
            Self::Slice { start, count } => Self::Slice {
                start: expr(start),
                count: width(count),
            },
        }
    }

    /// The lane indices of a gather over a `width`-lane receiver, checked
    /// against its method's constraints, worded as the pin's: `None` while
    /// an expression is open.
    pub fn resolve(&self, width: i64) -> Option<Result<Vec<usize>, String>> {
        let range = |start: i64, count: i64| (start..start + count).map(|lane| lane as usize);
        let lanes = match self {
            Self::Known(mask) => return Some(Ok(mask.clone())),
            Self::Shuffle(lanes) => {
                let lanes = lanes
                    .iter()
                    .map(ParamExpr::as_i64)
                    .collect::<Option<Vec<_>>>()?;
                if lanes.len() as i64 != width {
                    return Some(Err("mismatch in the number of elements".to_string()));
                }
                if lanes.iter().any(|lane| !(0..width).contains(lane)) {
                    return Some(Err("invalid index in the shuffle operation".to_string()));
                }
                lanes.into_iter().map(|lane| lane as usize).collect()
            }
            Self::Slice { start, count } => {
                let (start, count) = (start.as_i64()?, count.known()?);
                if !(0 <= start && start < count + start && count + start <= width) {
                    return Some(Err(
                        "output width must be a positive integer less than simd size".to_string(),
                    ));
                }
                range(start, count).collect()
            }
            Self::Join => {
                let joined = width * 2;
                if joined > 1 << 15 {
                    return Some(Err(format!("SIMD width {joined} is too wide")));
                }
                range(0, joined).collect()
            }
        };
        Some(Ok(lanes))
    }
}

impl fmt::Display for LaneMask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Known(mask) => write!(f, "{mask:?}"),
            Self::Shuffle(lanes) => {
                let lanes: Vec<String> = lanes.iter().map(ToString::to_string).collect();
                write!(f, "shuffle[{}]", lanes.join(", "))
            }
            Self::Slice { start, count } => write!(f, "slice[{count}, offset={start}]"),
            Self::Join => write!(f, "join"),
        }
    }
}

/// The variants of a [`Ty`], the storage its handle shares.
///
/// `Int`, `UInt`, and `Float64` are width-one [`TyKind::Simd`]s, as
/// upstream's `Scalar[DType.int]`, `Scalar[DType.uint]`, and
/// `Scalar[DType.float64]` aliases are ([`TyKind::INT`], [`TyKind::UINT`],
/// [`TyKind::FLOAT64`]). `Func` is synthesized from a `def` signature or
/// lowered from a function-type annotation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TyKind {
    Bool,
    /// The compile-time string literal type (Mojo's `StringLiteral`). Its
    /// source spelling is concrete only as a parameter annotation; the
    /// nominal runtime `String` is the self-hosted stdlib struct.
    StringLiteral,
    None,
    /// Bottom type: no runtime value can inhabit it.
    Never,
    /// The flexible type of an integer literal: coerces to `Int`, `UInt`, or
    /// `Float64` (materializing to `Int` if nothing forces a choice).
    IntLiteral,
    /// The flexible type of a float literal: coerces to `Float64`.
    FloatLiteral,
    /// A direct initializer-inference hole written `_`. It is legal only in a
    /// type application whose constructor participates in literal inference and
    /// must be solved before checked HIR is produced.
    Infer,
    /// The compile-time-only type of a `[dtype: DType]` value parameter. No
    /// runtime value inhabits it; specialization folds every use to a
    /// concrete `DType.<dt>` spelling before checking.
    Dtype,
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
    /// A SIMD vector type `SIMD[DType.<dtype>, width]`. Either slot is a
    /// parameter expression while the declaration it belongs to is symbolic;
    /// a symbolic slot never crosses the MIR waist.
    Simd {
        dtype: SimdDtype,
        width: SimdWidth,
    },
    /// The built-in `Error` type.
    Error,
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
    /// [`CtValue::KgenString`](crate::ct::CtValue::KgenString). It is
    /// compile-time only; no runtime register holds one.
    KgenString,
    /// Upstream's MLIR builtin `index` (`__mlir_type.index`), what
    /// `Indexer.__mlir_index__` returns, displayed `__mlir_type.index`.
    /// `pop.cast_from_builtin` and `pop.cast_to_builtin` convert it to and
    /// from an `Int`'s `!kgen.scalar<index>` storage. Both backends hold it
    /// as they hold an `Int`.
    Index,
    /// Upstream's generator type `!kgen.generator`, spelled
    /// `__generator_type[idx: Int] Int`: the type of a parameterized
    /// `comptime` declaration read unapplied, and of a parameter that takes
    /// one. It is compile-time only; no runtime register holds one.
    Generator(Box<GeneratorTy>),
    /// Compiler-private `__RuntimeTuple[T1, ..., Tn]` storage. Public
    /// `Tuple[T1, ..., Tn]` values are nominal standard-library structs.
    Tuple(Vec<Ty>),
    /// Upstream's fixed-size array `!pop.array<size, element>`
    /// (`_MLIR.POPArrayType[size, element]`): the array of element pointers
    /// a call builds for a `*xs: T` callee's `VariadicList`, in a slot of
    /// its own (`__passed_varargs__`).
    PopArray {
        size: u64,
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
                    TyKind::INT => Some(Self::int()),
                    TyKind::UINT => Some(Self::uint()),
                    TyKind::FLOAT64 => Some(Self::float64()),
                    _ => None,
                }
            }
        }
    };
}

macro_rules! shared_scalar_types {
    ($($(#[$doc:meta])* $name:ident => $kind:ident),* $(,)?) => {
        impl Ty {
            $(
                $(#[$doc])*
                pub fn $name() -> Self {
                    static TY: LazyLock<Ty> =
                        LazyLock::new(|| Ty(Arc::new(TyNode::new(TyKind::$kind.clone()))));
                    TY.clone()
                }
            )*
        }
    };
}

shared_scalar_types! {
    /// `Int`, `Scalar[DType.int]`.
    int => INT,
    /// `UInt`, `Scalar[DType.uint]`.
    uint => UINT,
    /// `Float64`, `Scalar[DType.float64]`.
    float64 => FLOAT64,
}

dataless_types! {
    /// `Bool`.
    bool => Bool,
    /// `StringLiteral`.
    string_literal => StringLiteral,
    /// `None`.
    none => None,
    /// `Never`.
    never => Never,
    /// An integer literal's flexible type.
    int_literal => IntLiteral,
    /// A float literal's flexible type.
    float_literal => FloatLiteral,
    /// An inference hole, `_`.
    infer => Infer,
    /// The `DType` value-parameter type.
    dtype => Dtype,
    /// `Self` inside a trait.
    self_type => SelfType,
    /// The MLIR builtin `index`.
    index => Index,
    /// `Error`.
    error => Error,
}

impl Ty {
    /// The generator type over `params` yielding `result`
    /// ([`TyKind::Generator`]).
    pub fn generator(params: Vec<ParamDecl>, result: Self) -> Self {
        Self::new(TyKind::Generator(Box::new(GeneratorTy { params, result })))
    }

    /// The type of variant `kind`. A variant that carries no data shares one
    /// node per compilation.
    pub fn new(kind: TyKind) -> Self {
        Self::dataless(&kind).unwrap_or_else(|| Self(Arc::new(TyNode::new(kind))))
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
}

impl TyKind {
    /// `Int`, upstream's `Scalar[DType.int]`.
    pub const INT: &'static Self = &Self::Simd {
        dtype: SimdDtype::Known(Dtype::Int),
        width: SimdWidth::Known(1),
    };
    /// `UInt`, upstream's `Scalar[DType.uint]`.
    pub const UINT: &'static Self = &Self::Simd {
        dtype: SimdDtype::Known(Dtype::UInt),
        width: SimdWidth::Known(1),
    };
    /// `Float64`, upstream's `Scalar[DType.float64]`.
    pub const FLOAT64: &'static Self = &Self::Simd {
        dtype: SimdDtype::Known(Dtype::Float64),
        width: SimdWidth::Known(1),
    };

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

/// The name of upstream's `SIMDLength`, the type of `SIMD`'s width.
pub const SIMD_LENGTH_TYPE_NAME: &str = "SIMDLength";

/// The bound of a type binder whose type is upstream's `!kgen.type`
/// ([`TyKind::TypeType`]), the name the pin prints for it: every type
/// satisfies it, and it implies no trait, not even `AnyType`.
pub const TYPE_OF_ALL_TYPES: &str = "__TypeOfAllTypes";

/// The spelling of upstream's `!kgen.string` type ([`TyKind::KgenString`]),
/// as the pin prints it.
pub const KGEN_STRING_TYPE_SPELLING: &str = "__mlir_type.`!kgen.string`";

/// The spelling of upstream's MLIR builtin `index` type ([`TyKind::Index`]),
/// as the pin prints it.
pub const MLIR_INDEX_TYPE_SPELLING: &str = "__mlir_type.index";

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
/// It is one of the types the subset reads: the builtin `index`
/// ([`TyKind::Index`]), an `Int`'s `!kgen.scalar<index>` storage (which is
/// `Int` itself until a scalar's storage is a type of its own, R1397), or
/// the compile-time-only `!kgen.type` ([`TyKind::TypeType`]) and
/// `!kgen.string` ([`TyKind::KgenString`]).
pub fn mlir_type_named(name: &str) -> Option<Ty> {
    match name {
        "index" => Some(Ty::index()),
        "!kgen.scalar<index>" => Some(Ty::int()),
        "!kgen.type" => Some(TyKind::TypeType.into()),
        "!kgen.string" => Some(TyKind::KgenString.into()),
        _ => None,
    }
}

/// A bracketed `__mlir_type[...]` spelling as KGEN concatenates it.
///
/// The text pieces (the backtick-stropped arguments no plain identifier
/// spells) join without whitespace, each other argument a `{}` hole, beside
/// those operands in order.
pub fn mlir_type_spelling(
    args: &[mojito_ast::ast::ParamArg],
) -> (String, Vec<&mojito_ast::ast::ParamArg>) {
    use mojito_ast::ast::{Expr, ExprKind, ParamArg};
    let mut operands = Vec::new();
    let text = args
        .iter()
        .map(|arg| match arg {
            ParamArg::Value(Expr {
                kind: ExprKind::Identifier(text),
                ..
            }) if !text.chars().all(|c| c == '_' || c.is_alphanumeric()) => {
                text.split_whitespace().collect::<String>()
            }
            operand => {
                operands.push(operand);
                "{}".to_string()
            }
        })
        .collect();
    (text, operands)
}

/// The parameter an MLIR type operand reads the `_mlir_value` of.
///
/// `n._mlir_value` and `Self.dtype._mlir_value` read a `SIMDLength`'s
/// `index` storage, and a `DType`'s, which is the parameter itself while
/// `DType` is a compiler type (R160). `None` for an operand that reads no
/// `_mlir_value`.
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
    let [TyArg::Val(CtValue::Dtype(dtype))] = arguments.as_slice() else {
        return None;
    };
    Some((family, *dtype))
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

/// The type a positional collector of `element`s (`*xs: element`) has in
/// its callee's body.
///
/// A specialized heterogeneous one (`$pack`) is the tuple
/// of its elements, an unspecialized one (`*args: *Ts`) the compiler's pack
/// storage, and a homogeneous one upstream's `VariadicList[element,
/// is_owned]`, owned for a `var` collector.
pub fn variadic_collector_type(element: Ty, convention: Option<ArgConvention>) -> Ty {
    match element.kind() {
        TyKind::RuntimePack(elements) => TyKind::Tuple(elements.clone()).into(),
        _ if pack_spread(std::slice::from_ref(&element)).is_some() => {
            TyKind::VariadicPack(element).into()
        }
        _ => variadic_list_type(element, matches!(convention, Some(ArgConvention::Var))),
    }
}

/// Whether `ty` is a heterogeneous collector's type while its pack is
/// still a parameter (`*args: *Ts`), upstream's `VariadicPack`.
pub fn is_heterogeneous_collector(ty: &Ty) -> bool {
    match ty.kind() {
        TyKind::RuntimePack(_) => true,
        TyKind::VariadicPack(element) => pack_spread(std::slice::from_ref(element)).is_some(),
        TyKind::Tuple(elements) => pack_spread(elements).is_some(),
        _ => false,
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
        TyKind::IntLiteral => "SIMD[DType.int, 1]".to_string(),
        TyKind::FloatLiteral => "SIMD[DType.float64, 1]".to_string(),
        TyKind::Bool => "Bool".to_string(),
        TyKind::Simd { dtype, width } => format!("SIMD[{dtype}, {width}]"),
        TyKind::StringLiteral => "StringLiteral".to_string(),
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
            let spread = pack_spread_argument(arguments)
                .is_some_and(|spread| spreads(std::slice::from_ref(spread)));
            let arguments = if spread && binds_pack_elementwise(name) {
                elements.iter().cloned().map(TyArg::Ty).collect()
            } else if spread {
                vec![TyArg::Val(CtValue::Tuple(
                    elements.iter().cloned().map(CtValue::Type).collect(),
                ))]
            } else {
                map_tyargs(arguments, |ty| expand_pack_spread(ty, pack, elements))
            };
            TyKind::Struct(name.clone(), arguments).into()
        }
        TyKind::Tuple(list) => TyKind::Tuple(expand(list)).into(),
        TyKind::RuntimePack(list) => TyKind::RuntimePack(expand(list)).into(),
        TyKind::Variant(list) => TyKind::Variant(expand(list)).into(),
        TyKind::PopArray { size, element } => TyKind::PopArray {
            size: *size,
            element: expand_pack_spread(element, pack, elements),
        }
        .into(),
        TyKind::Pointer { element, origin } => TyKind::Pointer {
            element: expand_pack_spread(element, pack, elements),
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
    [TUPLE_TYPE_NAME, TSTRING_TYPE_NAME, VARIANT_TYPE_NAME]
        .iter()
        .any(|public| {
            name == *public
                || name.ends_with(&format!("${public}"))
                || name.starts_with(&format!("{public}$"))
                || name.contains(&format!("${public}$"))
        })
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
    let bound = arguments.iter().position(|argument| {
        matches!(argument, TyArg::Val(CtValue::Tuple(values))
            if values.iter().all(|value| matches!(value, CtValue::Type(_))))
    });
    let Some(position) = bound else {
        return arguments;
    };
    let mut canonical = arguments;
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

/// The argument list of an application of the struct `name` aligned with
/// its declared parameters `decls`.
///
/// [`canonical_pack_arguments`] undone: the types a struct that
/// [`binds_pack_elementwise`] spells element by element regroup into the
/// one list of types its pack binds, so `Tuple[Int, Bool]` binds `Ts` to
/// `[Int, Bool]`. A pack spread or computed list stays the one argument it
/// is, and any other list is returned unchanged.
pub fn declared_pack_arguments<'a>(
    name: &str,
    decls: &[ParamDecl],
    arguments: &'a [TyArg],
) -> std::borrow::Cow<'a, [TyArg]> {
    let pack = decls
        .iter()
        .position(|decl| matches!(decl, ParamDecl::Type { variadic: true, .. }));
    let (Some(position), Some(count)) = (pack, (arguments.len() + 1).checked_sub(decls.len()))
    else {
        return arguments.into();
    };
    let elements = &arguments[position..position + count];
    let types: Option<Vec<CtValue>> = elements
        .iter()
        .map(|argument| match argument {
            TyArg::Ty(ty) => Some(CtValue::Type(ty.clone())),
            TyArg::Val(_) | TyArg::Origin(_) => None,
        })
        .collect();
    match types {
        Some(types)
            if binds_pack_elementwise(name)
                && pack_spread_argument(elements).is_none()
                && list_spread_argument(elements).is_none() =>
        {
            let mut declared = arguments[..position].to_vec();
            declared.push(TyArg::Val(CtValue::Tuple(types)));
            declared.extend_from_slice(&arguments[position + count..]);
            declared.into()
        }
        _ => arguments.into(),
    }
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
/// Its runtime values are the literal representation rather than the nominal
/// `String` struct's, so an instantiation argument that mentions it never
/// selects a nominal-`String` instance.
pub fn contains_string_literal(ty: &Ty) -> bool {
    mentions_open(ty, &|ty| matches!(ty.kind(), TyKind::StringLiteral))
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
        /// The declared type. A pack (`*values: T`, `variadic`) is declared
        /// with the raw list it binds, `TyKind::ParamList(T)`, as upstream's
        /// `values.values` binder is; [`ParamDecl::value_element_ty`] reads
        /// the element each bound value has.
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
    /// declared type's, which for a pack (`*values: T`) is the raw list it
    /// binds, as for a raw-list binder (`values: KGENParamList[T]`); a type
    /// binder's `Type`, or a type list for a type pack.
    pub fn binder_meta(&self) -> MetaTy {
        match self {
            Self::Value { ty, .. } => MetaTy::value(ty.clone()),
            Self::Type { variadic: true, .. } => MetaTy::type_list(),
            Self::Type { .. } => MetaTy::Type,
        }
    }

    /// The type a value binder whose values have type `element` is
    /// declared with: a pack (`*vs: T`, `variadic`) binds the raw list
    /// `TyKind::ParamList(T)`, as upstream's implicit `vs.values:
    /// !kgen.param_list<T>` binder does; any other value binder `element`.
    pub fn value_list_ty(element: Ty, variadic: bool) -> Ty {
        if variadic {
            TyKind::ParamList(element).into()
        } else {
            element
        }
    }

    /// The type each value this binder binds has: a pack's element (the
    /// element type upstream's `ASTType::getParameterListInfo` reads off
    /// the declared list), any other value binder's declared type. `None`
    /// for a type binder.
    pub fn value_element_ty(&self) -> Option<&Ty> {
        match self {
            Self::Value {
                ty, variadic: true, ..
            } => Some(match ty.kind() {
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

    /// The reference a use of this binder carries: its identity with its
    /// spelling attached.
    pub fn binder(&self) -> ParamRef {
        ParamRef {
            id: self.id().clone(),
            name: std::sync::Arc::from(self.name()),
        }
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
            Self::Value { .. } => TyArg::Val(CtValue::Expr(ParamContext::detached().decl_ref(
                self.id().clone(),
                self.name().trim_start_matches('*'),
                self.binder_meta(),
            ))),
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
        match self {
            Self::IntLiteral => write!(f, "Int"),
            Self::Bool => write!(f, "Bool"),
            Self::StringLiteral => write!(f, "StringLiteral"),
            Self::FloatLiteral => write!(f, "Float64"),
            Self::Infer => write!(f, "_"),
            Self::Dtype => write!(f, "DType"),
            Self::None => write!(f, "None"),
            Self::Never => write!(f, "Never"),
            Self::Func(func) => write_callable(f, None, func),
            Self::GenericFunc(generic) => write_callable(f, Some(&generic.decls), generic),
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
            Self::Simd {
                dtype: SimdDtype::Known(dtype),
                width: SimdWidth::Known(1),
            } => match dtype.repr_alias() {
                Some(alias) => write!(f, "{alias}"),
                None => write!(f, "SIMD[DType.{}, 1]", dtype.name()),
            },
            Self::Simd {
                dtype,
                width: SimdWidth::Known(1),
            } => write!(f, "Scalar[{dtype}]"),
            Self::Simd { dtype, width } => write!(f, "SIMD[{dtype}, {width}]"),
            Self::Error => write!(f, "Error"),
            Self::Pointer { element, origin } => {
                write!(f, "Pointer[{element}")?;
                match origin {
                    crate::origin::PointerOrigin::Place {
                        place,
                        mutable: false,
                    } => {
                        write!(f, ", ImmOrigin(origin@{})", place.root.0)?;
                    }
                    crate::origin::PointerOrigin::Place { place, .. } => {
                        write!(f, ", origin@{}", place.root.0)?;
                    }
                    crate::origin::PointerOrigin::Param {
                        id,
                        mutability,
                        interior,
                        subtree,
                    } => {
                        let immutable = *mutability == crate::origin::Mutability::Immutable;
                        write!(
                            f,
                            ", {}origin#{}",
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
                    crate::origin::PointerOrigin::SelfPlace { .. } => {
                        write!(f, ", origin_of(self)")?;
                    }
                    crate::origin::PointerOrigin::Static => write!(f, ", ImmStaticOrigin")?,
                    crate::origin::PointerOrigin::Untracked { mutable: true } => {
                        write!(f, ", MutUntrackedOrigin")?;
                    }
                    crate::origin::PointerOrigin::Untracked { mutable: false } => {
                        write!(f, ", ImmUntrackedOrigin")?;
                    }
                    crate::origin::PointerOrigin::UnsafeAny { mutable: true } => {
                        write!(f, ", MutUnsafeAnyOrigin")?;
                    }
                    crate::origin::PointerOrigin::UnsafeAny { mutable: false } => {
                        write!(f, ", ImmUnsafeAnyOrigin")?;
                    }
                }
                write!(f, "]")
            }
            Self::Ref(reference) => write!(f, "ref {}", reference.referent),
            Self::ComptimeList(elem) => write!(f, "<comptime-list[{elem}]>"),
            Self::ParamList(elem) => write!(f, "KGENParamList[{elem}]"),
            Self::AnyTrait(traits) => write!(f, "AnyTrait[{}]", traits.join(" & ")),
            Self::Trait(traits) => f.write_str(&traits.join(" & ")),
            Self::TypeType => write!(f, "{TYPE_OF_ALL_TYPES}"),
            Self::KgenString => f.write_str(KGEN_STRING_TYPE_SPELLING),
            Self::Index => f.write_str(MLIR_INDEX_TYPE_SPELLING),
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
                let mut shown = args
                    .iter()
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
            size: *size,
            element: recur(element),
        }
        .into(),
        TyKind::Pointer { element, origin } => TyKind::Pointer {
            element: recur(element),
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
            size: *size,
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
    for (index, decl) in decls.iter().enumerate() {
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
    TyKind::Simd {
        dtype: SimdDtype::Known(dtype),
        width: SimdWidth::Known(width),
    }
    .into()
}

/// The SIMD type of two slots that may still be symbolic.
///
/// A slot whose expression is a closed constant becomes `Known`, and two
/// known slots canonicalize through [`canonical_simd_ty`]. This is the only
/// constructor of a symbolic `TyKind::Simd`, so `Expr` never holds a closed
/// expression.
pub fn simd_ty_from_slots(dtype: SimdDtype, width: SimdWidth) -> Result<Ty, ParamError> {
    let dtype = match dtype {
        SimdDtype::Expr(expr) => match expr.as_constant() {
            Some(CtValue::Dtype(dtype)) => SimdDtype::Known(*dtype),
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
        (dtype, width) => TyKind::Simd { dtype, width }.into(),
    })
}

/// The type a Hashable builtin leaf contributes to its hasher as.
///
/// A `DType` hashes its code as a `UInt8` (upstream's `DType.__hash__`), a
/// `Bool` as `Scalar[DType.bool]` (upstream's `Bool.__hash__`); every other
/// leaf hashes as itself.
pub fn hash_leaf_ty(ty: &Ty) -> Ty {
    match ty.kind() {
        TyKind::Dtype => canonical_simd_ty(Dtype::UInt8, 1),
        TyKind::Bool => canonical_simd_ty(Dtype::Bool, 1),
        _ => ty.clone(),
    }
}

/// The `(dtype, width)` shape of a SIMD-valued type — an exact literal
/// reports the width-one vector it materializes to — or `None` for a non-SIMD
/// type.
///
/// `Bool` is not a SIMD value (upstream's `Bool` is its own struct;
/// `Scalar[DType.bool]` is the vector).
pub const fn simd_shape(ty: &TyKind) -> Option<(Dtype, i64)> {
    Some(match ty {
        TyKind::IntLiteral => (Dtype::Int, 1),
        TyKind::FloatLiteral => (Dtype::Float64, 1),
        TyKind::Simd {
            dtype: SimdDtype::Known(dtype),
            width: SimdWidth::Known(width),
        } => (*dtype, *width),
        _ => return None,
    })
}

/// The slots of a SIMD-valued type, symbolic or not — [`simd_shape`] for a
/// declaration that is still a template.
pub fn simd_slots(ty: &TyKind) -> Option<(SimdDtype, SimdWidth)> {
    match ty {
        TyKind::Simd { dtype, width } => Some((dtype.clone(), width.clone())),
        _ => {
            simd_shape(ty).map(|(dtype, width)| (SimdDtype::Known(dtype), SimdWidth::Known(width)))
        }
    }
}

/// Whether `ty` is a width-one `TyKind::Simd` (a scalar alias such as `Int`
/// or `Float32`), whatever its dtype slot.
pub const fn is_scalar_simd(ty: &TyKind) -> bool {
    matches!(
        ty,
        TyKind::Simd {
            width: SimdWidth::Known(1),
            ..
        }
    )
}

/// Whether `ty` is `Int`, `UInt`, or `Float64`: a width-one `int`, `uint`,
/// or `float64` vector, whose value both backends hold as a host scalar.
///
/// It exists only because those three keep an infix operator path of their
/// own above the waist where upstream's are ordinary `SIMD` instances: a
/// width-one comparison types as a lane mask where upstream's is a `Bool`
/// (R185).
pub const fn has_host_scalar_value(ty: &TyKind) -> bool {
    matches!(ty, TyKind::INT | TyKind::UINT | TyKind::FLOAT64)
}

/// The known dtype of a width-one `TyKind::Simd`.
pub const fn scalar_simd_dtype(ty: &TyKind) -> Option<Dtype> {
    match ty {
        TyKind::Simd {
            dtype: SimdDtype::Known(dtype),
            width: SimdWidth::Known(1),
        } => Some(*dtype),
        _ => None,
    }
}

/// The type of one lane of a SIMD type: the canonical width-one type of its
/// dtype slot.
pub fn simd_lane(dtype: &SimdDtype) -> Ty {
    match dtype {
        SimdDtype::Known(dtype) => canonical_simd_ty(*dtype, 1),
        SimdDtype::Expr(_) => TyKind::Simd {
            dtype: dtype.clone(),
            width: SimdWidth::Known(1),
        }
        .into(),
    }
}

/// The runtime type an exact literal materializes to when no context selects
/// another (`IntLiteral` → `Int`, `FloatLiteral` → `Float64`), applied through
/// struct arguments, packs, and variants.
pub fn default_literal(ty: &Ty) -> Ty {
    match ty.kind() {
        TyKind::IntLiteral => Ty::int(),
        TyKind::FloatLiteral => Ty::float64(),
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
    if *from == TyKind::Never {
        return true;
    }
    if from == to {
        return true;
    }
    match (from.kind(), to.kind()) {
        (TyKind::Param { binder: a, .. }, TyKind::Param { binder: b, .. }) => a == b,
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
        (TyKind::ParamList(a), TyKind::ParamList(b)) => a == b,
        (TyKind::AnyTrait(a), TyKind::AnyTrait(b)) => a == b,
        (TyKind::Trait(a), TyKind::Trait(b)) => a == b,
        (TyKind::TypeType, TyKind::TypeType)
        | (TyKind::KgenString, TyKind::KgenString)
        | (TyKind::Index, TyKind::Index) => true,
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
                    (Some(from), Some(TyKind::Error)) => *from != TyKind::Never,
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
        (TyKind::IntLiteral, TyKind::FloatLiteral) => true,
        (
            _,
            TyKind::Simd {
                dtype,
                width: SimdWidth::Known(1),
            },
        ) if splats_to(from, dtype) => true,
        // Upstream's `@implicit SIMD.__init__(IntLiteral)` and
        // `(FloatLiteral)` splat an exact literal across every lane, a
        // symbolic lane count included.
        (TyKind::IntLiteral | TyKind::FloatLiteral, TyKind::Simd { dtype, width })
            if !width.is_inferred() && splats_to(from, dtype) =>
        {
            true
        }
        (
            TyKind::Simd {
                dtype: from_dtype,
                width: from_width,
            },
            TyKind::Simd {
                dtype: to_dtype,
                width: to_width,
            },
        ) if to_width.is_inferred() => {
            from_dtype == to_dtype && from_width.known().is_none_or(|width| width > 0)
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

/// Whether a value of type `ty` can be a `dtype` SIMD element (a construction
/// argument, or the non-SIMD operand of an elementwise operator that splats).
///
/// A numeric literal fits any matching-kind lane; a same-dtype width-1 SIMD
/// fits. Into a symbolic dtype an integer or floating literal fits (the
/// literal's fit is the instantiation's to check, as upstream), a concrete
/// scalar never does, and a width-one vector of the same expression does.
pub fn splats_to(ty: &TyKind, dtype: &SimdDtype) -> bool {
    match (ty, dtype) {
        (TyKind::IntLiteral, SimdDtype::Known(dtype)) => *dtype != Dtype::Bool,
        (TyKind::FloatLiteral, SimdDtype::Known(dtype)) => dtype.is_float(),
        (TyKind::IntLiteral | TyKind::FloatLiteral, SimdDtype::Expr(_)) => true,
        (TyKind::Bool, SimdDtype::Known(dtype)) => *dtype == Dtype::Bool,
        (
            TyKind::Simd {
                dtype: d,
                width: SimdWidth::Known(1),
            },
            dtype,
        ) => d == dtype,
        _ => false,
    }
}

/// Whether the runtime scalar `from` splats across the multi-lane vector `to`.
///
/// Upstream's `@implicit SIMD.__init__(Scalar[dtype])`: an `Int32` into
/// `SIMD[DType.int32, 4]`, or a `Scalar[dt]` into `SIMD[dt, width]` while
/// the width is a parameter expression, since the instance's may be above
/// one. A `Bool` is no `Scalar`, and an exact literal materializes through
/// its own initializer instead.
pub fn scalar_splats_into(from: &Ty, to: &Ty) -> bool {
    matches!(
        to.kind(),
        TyKind::Simd { dtype, width }
            if width.known().is_none_or(|width| width > 1)
                && !matches!(from.kind(), TyKind::Bool | TyKind::IntLiteral | TyKind::FloatLiteral)
                && splats_to(from, dtype)
    )
}

/// The value-coercion policy for callable environments.
///
/// Current Mojo rejects binding a capturing closure to an unqualified
/// `def(...)` value position — the contract must spell `capturing[...]` —
/// while a thin function value still binds.
///
/// Comptime callable *bounds* stay on the permissive
/// `callable_environment_coerces` below.
pub fn callable_environment_value_coerces(
    from: &crate::origin::CallableEnvironment,
    to: &crate::origin::CallableEnvironment,
) -> bool {
    use crate::origin::CallableEnvironment;
    if matches!(
        (from, to),
        (
            CallableEnvironment::Capturing(_) | CallableEnvironment::Storage(_),
            CallableEnvironment::Default
        )
    ) {
        return false;
    }
    callable_environment_coerces(from, to)
}

pub fn callable_environment_coerces(
    from: &crate::origin::CallableEnvironment,
    to: &crate::origin::CallableEnvironment,
) -> bool {
    use crate::origin::{CallableEnvironment, CaptureOriginSet};
    if from == to {
        return true;
    }
    // A closure's storage converts to the closure of its body, whose
    // environment is the storage's captures.
    if let CallableEnvironment::Storage(storage) = from {
        return !matches!(to, CallableEnvironment::Storage(_))
            && callable_environment_coerces(
                &CallableEnvironment::Capturing(storage.captures.clone()),
                to,
            );
    }
    match (from, to) {
        // An unqualified callable contract does not constrain the environment
        // in the *bound* channel: a supplied `@parameter`/comptime callable
        // argument against `F: def(...)` may capture (upstream accepts this —
        // see `subscript_call_contracts.mojo`), so `unify`,
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
            CallableEnvironment::Capturing(CaptureOriginSet::Concrete(_)),
            CallableEnvironment::Capturing(CaptureOriginSet::Infer | CaptureOriginSet::Param(_)),
        ) => true,
        (
            CallableEnvironment::Capturing(CaptureOriginSet::Concrete(actual)),
            CallableEnvironment::Capturing(CaptureOriginSet::Concrete(allowed)),
        ) => actual.iter().all(|capture| allowed.contains(capture)),
        _ => false,
    }
}

/// Whether a *concrete* built-in type has an intrinsic `__hash__` — the scalar
/// set the VM can hash directly (`Int`/`UInt`/`Bool`/`String`/`Float64`).
///
/// This lets a user key struct combine `self.field.__hash__()` values. Whether
/// `Copyable.copy` on a value of this type has no callee: built-in scalars,
/// literals, tuples, packs, and variants copy by the ordinary value read.
/// Nominal, parametric, associated, and reference types resolve their `copy`
/// through declarations or trait dispatch instead.
pub const fn builtin_copy_is_value_read(ty: &TyKind) -> bool {
    !matches!(
        ty,
        TyKind::Struct(..)
            | TyKind::Param { .. }
            | TyKind::Assoc { .. }
            | TyKind::Dependent(_)
            | TyKind::Ref(_)
            | TyKind::SelfType
            | TyKind::Func(_)
            | TyKind::GenericFunc(_)
            | TyKind::Overload(_)
            | TyKind::Error
    )
}

/// Whether a body reads a value binder of this type from a frame slot
/// rather than as upstream's `kgen.param.constant` of the binder.
///
/// A callable binder is still read from a slot, which the elaborator and
/// `inline-parametric` rewrite into the bound function (or a promoted
/// closure's storage); a pack and a raw parameter list keep their own
/// readings. Every other value binder's read is its parameter constant
/// and takes no storage.
pub const fn binder_read_takes_slot(ty: &TyKind) -> bool {
    matches!(
        ty,
        TyKind::Func(_)
            | TyKind::GenericFunc(_)
            | TyKind::VariadicPack(_)
            | TyKind::ParamList(_)
            | TyKind::KgenString
    )
}

/// Whether a copy of a value of this type is its operand.
///
/// The value is a scalar, pointer, `None`, or one SIMD lane, held whole in
/// one register on both backends, so copying it is a bit copy and no
/// `__copyinit__` runs. A lane's dtype may still be a parameter; every
/// instance closes it to a scalar. A copy of a value held in storage is the
/// identity too when its type is register-passable trivial:
/// `MirDeclarations::copy_is_identity` answers for those.
pub const fn copy_is_identity(ty: &TyKind) -> bool {
    matches!(
        ty,
        TyKind::Bool
            | TyKind::Dtype
            | TyKind::Index
            | TyKind::IntLiteral
            | TyKind::FloatLiteral
            | TyKind::None
            | TyKind::Pointer { .. }
            | TyKind::Simd {
                width: SimdWidth::Known(1),
                ..
            }
    )
}

/// Whether this compiler-known type is one upstream declares
/// `TrivialRegisterPassable`: a [`copy_is_identity`] value, a SIMD vector
/// of any dtype and width, or a `StringLiteral`. Its copy is a bit copy.
pub const fn trivially_register_passable_leaf(ty: &TyKind) -> bool {
    copy_is_identity(ty) || matches!(ty, TyKind::Simd { .. } | TyKind::StringLiteral)
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
            (Some(actual), Some(TyKind::Error)) => *actual != TyKind::Never,
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
    let mut bindings = ParamBindings::new();
    let mut binders: HashMap<ParamId, ParamRef> = HashMap::new();
    let mut subst = TySubst::new();
    for decl in decls {
        let declared = decl.binder();
        let reowned = ParamRef {
            id: ParamId::new(owner, declared.id.slot),
            name: declared.name.clone(),
        };
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
                subst.insert(declared.id.clone(), ty);
            }
            ParamDecl::Value { name, ty, .. } => bindings.bind(
                declared.id.clone(),
                ParamContext::detached().decl_ref(
                    reowned.id.clone(),
                    name.trim_start_matches('*'),
                    crate::param_expr::MetaTy::value((*ty).clone()),
                ),
            ),
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
/// or value argument re-canonicalizes); the fields are borrowed when none
/// names a binder. Origin arguments are left as declared.
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
    fields
        .iter()
        .map(|(name, field)| {
            replace_parameters(&context, field, &bindings, 0)
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
        TyKind::Dependent(_) | TyKind::Simd { .. } if is_symbolic(ty) => {
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
            size: *size,
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
        // A spread list closes into its elements once the rewrite knows
        // every length in it.
        TyKind::Struct(name, arguments) if let Some(list) = list_spread_argument(arguments) => ty
            .reusing(TyKind::Struct(
                name.clone(),
                spread_arguments(name, &rewrite.expr(list)?)?,
            )),
        // A spread of a pack the rewrite binds is its elements, spelled as
        // `expand_pack_spread` spells them.
        TyKind::Struct(name, arguments)
            if let Some(elements) = pack_spread_argument(arguments)
                .and_then(|pack| bound_pack(std::slice::from_ref(pack), rewrite)) =>
        {
            let arguments = if binds_pack_elementwise(name) {
                elements.into_iter().map(TyArg::Ty).collect()
            } else {
                vec![TyArg::Val(CtValue::Tuple(
                    elements.into_iter().map(CtValue::Type).collect(),
                ))]
            };
            ty.reusing(TyKind::Struct(name.clone(), arguments))
        }
        TyKind::Struct(name, arguments) => ty.reusing(TyKind::Struct(
            name.clone(),
            rewrite_tyargs(arguments, rewrite)?,
        )),
        TyKind::Dependent(dependent) => DependentType::resolve(rewrite.expr(dependent.expr())?),
        TyKind::ComptimeList(element) => TyKind::ComptimeList(rewrite_ty(element, rewrite)?).into(),
        TyKind::ParamList(element) => TyKind::ParamList(rewrite_ty(element, rewrite)?).into(),
        TyKind::VariadicPack(element) => TyKind::VariadicPack(rewrite_ty(element, rewrite)?).into(),
        TyKind::PopArray { size, element } => TyKind::PopArray {
            size: *size,
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
        TyKind::Simd { dtype, width } => {
            let dtype = match dtype {
                SimdDtype::Expr(expr) => SimdDtype::Expr(rewrite.expr(expr)?),
                known @ SimdDtype::Known(_) => known.clone(),
            };
            let width = match width {
                SimdWidth::Expr(expr) => SimdWidth::Expr(rewrite.expr(expr)?),
                known @ SimdWidth::Known(_) => known.clone(),
            };
            simd_ty_from_slots(dtype, width)?
        }
        TyKind::Bool
        | TyKind::StringLiteral
        | TyKind::None
        | TyKind::Never
        | TyKind::IntLiteral
        | TyKind::FloatLiteral
        | TyKind::Infer
        | TyKind::Dtype
        | TyKind::AnyTrait(_)
        | TyKind::Trait(_)
        | TyKind::TypeType
        | TyKind::KgenString
        | TyKind::Index
        | TyKind::SelfType
        | TyKind::Error => ty.clone(),
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
        | CtValue::Str(_)
        | CtValue::Dtype(_)
        | CtValue::Trait(_)
        | CtValue::KgenString(_)
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
            self.bound.extend(
                decls
                    .iter()
                    .map(|decl| decl.name().trim_start_matches('*').to_string()),
            );
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
            |decl| ParamRef {
                id: decl.id().clone(),
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

/// The bundled formatter behind `format` (upstream's `_FormatUtils`), and
/// its stand-in for `StringLiteral.format`, which a string literal's call
/// lowers to while the literal declares no methods.
pub const STDLIB_FORMAT_UTILS_STRUCT: &str = "__module$std$string$_FormatUtils";
pub const FORMAT_LITERAL_METHOD: &str = "format_literal";

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
        TyKind::Func(_) | TyKind::GenericFunc(_) => ty
            .callable()
            .is_some_and(|func| func.types().any(is_symbolic)),
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
        | TyKind::PopArray { element, .. }
        | TyKind::Pointer { element, .. } => is_symbolic(element),
        TyKind::Ref(reference) => is_symbolic(&reference.referent),
        TyKind::Simd { dtype, width } => dtype.is_symbolic() || width.is_symbolic(),
        TyKind::Dtype
        | TyKind::AnyTrait(_)
        | TyKind::Trait(_)
        | TyKind::TypeType
        | TyKind::KgenString
        | TyKind::Index
        | TyKind::Bool
        | TyKind::StringLiteral
        | TyKind::None
        | TyKind::Never
        | TyKind::IntLiteral
        | TyKind::FloatLiteral
        | TyKind::Error => false,
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
                    || matches!(node.kind(), ParamKind::PackQuery { pack, .. } if pack == self.binder)
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
        | CtValue::Dtype(_)
        | CtValue::Simd { .. }
        | CtValue::Str(_)
        | CtValue::Trait(_)
        | CtValue::KgenString(_)
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
        | TyKind::PopArray { element, .. }
        | TyKind::Pointer { element, .. } => mentions(element, predicate),
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
        | TyKind::PopArray { element, .. }
        | TyKind::Pointer { element, .. } => element.carries_decorations(),
        TyKind::Ref(reference) => reference.referent.carries_decorations(),
        TyKind::Dependent(dependent) => dependent.expr().carries_decorations(),
        TyKind::Simd { dtype, width } => {
            matches!(dtype, SimdDtype::Expr(expr) if expr.carries_decorations())
                || matches!(width, SimdWidth::Expr(expr) if expr.carries_decorations())
        }
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
        TyArg::Val(value) => matches!(
            value,
            CtValue::Int(_)
                | CtValue::UInt(_)
                | CtValue::Index(_)
                | CtValue::Float(_)
                | CtValue::Bool(_)
                | CtValue::Str(_)
                | CtValue::Dtype(_)
                | CtValue::Simd { .. }
        ),
        TyArg::Origin(origin) => matches!(origin, crate::origin::Origin::Unbound),
    }
}

/// Whether a variant is closed, from its children's cached closedness.
fn closed_kind(kind: &TyKind) -> bool {
    match kind {
        TyKind::Bool
        | TyKind::None
        | TyKind::Never
        | TyKind::Dtype
        | TyKind::Index
        | TyKind::Error => true,
        TyKind::Simd { dtype, width } => !dtype.is_expr() && !width.is_expr(),
        TyKind::Struct(_, arguments) => arguments.iter().all(closed_argument),
        TyKind::Tuple(elements) | TyKind::RuntimePack(elements) | TyKind::Variant(elements) => {
            elements.iter().all(Ty::is_closed)
        }
        TyKind::ComptimeList(element)
        | TyKind::ParamList(element)
        | TyKind::VariadicPack(element)
        | TyKind::PopArray { element, .. } => element.is_closed(),
        _ => false,
    }
}

fn bind_constraints(constraints: &mut [GenericConstraint], scopes: &[&[ParamDecl]]) {
    for constraint in constraints {
        *constraint = constraint.bind(scopes);
    }
}

fn rewrite_decl(decl: &ParamDecl, rewrite: &mut dyn TyRewrite) -> Result<ParamDecl, ParamError> {
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
            let mut arguments = match pack_spread_argument(arguments).and_then(elements_of) {
                Some(elements) => elements.iter().cloned().map(TyArg::Ty).collect(),
                None => map_tyargs(arguments, recurse),
            };
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
            size: *size,
            element: recurse(element),
        }
        .into(),
        TyKind::Pointer { element, origin } => TyKind::Pointer {
            element: recurse(element),
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
        TyKind::Simd { .. } if !values.is_empty() && is_symbolic(ty) => {
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
/// mismatch is visible in diagnostics.
fn write_callable(
    f: &mut fmt::Formatter<'_>,
    decls: Option<&[ParamDecl]>,
    func: &FuncTy,
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
    // A closure's own value displays as its closure trait, which names
    // its parameters.
    let named = matches!(environment, crate::origin::CallableEnvironment::Storage(_));
    for (i, p) in params.iter().enumerate() {
        if i > 0 {
            write!(f, ", ")?;
        }
        match func.names.get(i) {
            Some(name) if named => write!(f, "{name}: {p}")?,
            _ => write!(f, "{p}")?,
        }
    }
    write!(f, ")")?;
    match environment {
        crate::origin::CallableEnvironment::Default
        | crate::origin::CallableEnvironment::Storage(_) => {}
        crate::origin::CallableEnvironment::Thin => write!(f, " thin")?,
        crate::origin::CallableEnvironment::Capturing(origins) => {
            write!(f, " capturing[")?;
            match origins {
                crate::origin::CaptureOriginSet::Infer => write!(f, "_")?,
                crate::origin::CaptureOriginSet::Param(id) => {
                    write!(f, "origin_set#{}", id.0)?;
                }
                crate::origin::CaptureOriginSet::Concrete(members) => {
                    for (index, capture) in members.iter().enumerate() {
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
            .constant(CtValue::Dtype(Dtype::Float64))
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
        assert!(splats_to(&Ty::int_literal(), &lane));
        assert!(splats_to(&Ty::float_literal(), &lane));
        for scalar in [
            Ty::int(),
            Ty::float64(),
            Ty::bool(),
            canonical_simd_ty(Dtype::Int32, 1),
        ] {
            assert!(!splats_to(&scalar, &lane), "{scalar}");
        }
        let same = Ty::new(TyKind::Simd {
            dtype: lane.clone(),
            width: SimdWidth::Known(1),
        });
        assert!(splats_to(&same, &lane));
        assert!(coerces(&Ty::float_literal(), &same));
        assert!(!coerces(&same, &Ty::float64()));
        let vector = Ty::new(TyKind::Simd {
            dtype: lane.clone(),
            width: SimdWidth::Expr(width_binder(&context)),
        });
        assert!(!coerces(&vector, &canonical_simd_ty(Dtype::Int32, 4)));
        assert!(coerces(
            &vector,
            &Ty::new(TyKind::Simd {
                dtype: lane,
                width: SimdWidth::inferred(&context),
            })
        ));
    }

    #[test]
    fn exact_literals_splat_into_a_known_multi_lane_vector() {
        let int32x4 = canonical_simd_ty(Dtype::Int32, 4);
        let float32x4 = canonical_simd_ty(Dtype::Float32, 4);
        assert!(coerces(&Ty::int_literal(), &int32x4));
        assert!(coerces(&Ty::int_literal(), &float32x4));
        assert!(coerces(&Ty::float_literal(), &float32x4));
        assert!(!coerces(&Ty::float_literal(), &int32x4));
        assert!(!coerces(&Ty::bool(), &canonical_simd_ty(Dtype::Bool, 4)));
        assert!(!coerces(&canonical_simd_ty(Dtype::Int32, 1), &int32x4));
        assert!(!coerces(&Ty::int(), &canonical_simd_ty(Dtype::Int, 4)));
    }

    #[test]
    fn a_runtime_scalar_splats_only_into_a_vector_of_its_dtype() {
        let int32x4 = canonical_simd_ty(Dtype::Int32, 4);
        assert!(scalar_splats_into(
            &canonical_simd_ty(Dtype::Int32, 1),
            &int32x4
        ));
        assert!(scalar_splats_into(
            &Ty::int(),
            &canonical_simd_ty(Dtype::Int, 4)
        ));
        assert!(scalar_splats_into(
            &Ty::float64(),
            &canonical_simd_ty(Dtype::Float64, 2)
        ));
        assert!(!scalar_splats_into(
            &canonical_simd_ty(Dtype::Int32, 1),
            &canonical_simd_ty(Dtype::Int64, 4)
        ));
        assert!(!scalar_splats_into(
            &Ty::int(),
            &canonical_simd_ty(Dtype::Int64, 4)
        ));
        assert!(!scalar_splats_into(
            &Ty::bool(),
            &canonical_simd_ty(Dtype::Bool, 4)
        ));
        assert!(!scalar_splats_into(&Ty::int_literal(), &int32x4));
        assert!(!scalar_splats_into(
            &canonical_simd_ty(Dtype::Int32, 1),
            &canonical_simd_ty(Dtype::Int32, 1)
        ));
    }

    #[test]
    fn replacement_closes_symbolic_slots_through_the_canonical_form() {
        let context = ParamContext::detached();
        let scalar = Ty::new(TyKind::Simd {
            dtype: SimdDtype::Expr(dtype_binder(&context)),
            width: SimdWidth::Expr(width_binder(&context)),
        });
        let mut referenced = std::collections::HashSet::new();
        referenced_parameters(&scalar, &mut referenced);
        assert!(referenced.contains("dt") && referenced.contains("width"));
        let values = HashMap::from([
            ("dt".to_string(), CtValue::Dtype(Dtype::Float64)),
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
            dict_type(Ty::string_literal(), Ty::int()),
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
            Some(Ty::new(TyKind::Simd {
                dtype: SimdDtype::Known(Dtype::Int64),
                width: SimdWidth::Known(4),
            }))
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
}

#[cfg(test)]
mod value_pack_decl_tests {
    use super::*;

    fn value_pack() -> ParamDecl {
        ParamDecl::Value {
            id: ParamId::new("f", 0),
            name: "*vs".to_string(),
            ty: TyKind::ParamList(Ty::int()).into(),
            default: None,
            callable_default: None,
            infer_only: false,
            variadic: true,
            constraints: Vec::new(),
        }
    }

    struct Decls(Vec<ParamDecl>);

    impl fmt::Display for Decls {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write_param_decls(f, &self.0)
        }
    }

    #[test]
    fn a_value_pack_binds_the_raw_list_it_is_declared_with() {
        assert_eq!(
            value_pack().binder_meta(),
            MetaTy::ParamList(Box::new(MetaTy::value(Ty::int())))
        );
    }

    #[test]
    fn a_value_pack_binds_values_of_its_element_type() {
        assert_eq!(value_pack().value_element_ty(), Some(&Ty::int()));
    }

    #[test]
    fn a_value_pack_displays_its_element_type() {
        assert_eq!(Decls(vec![value_pack()]).to_string(), "[*vs: Int]");
    }
}

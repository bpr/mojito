//! Shared semantic type representation.
//!
//! This is the type lattice used by the checker, but it also needs to be visible
//! to compile-time values once comptime can carry type values. Keeping `Ty` out
//! of `checker.rs` lets [`CtValue`](crate::ct::CtValue) represent `Type(Box<Ty>)`
//! without making the checker the owner of all type-level facts.

use std::collections::HashMap;
use std::fmt;

use crate::ct::CtValue;
use crate::param_expr::{
    ParamBindings, ParamContext, ParamError, ParamExpr, ParamId, ParamKind, ParamRef,
};
use mojito_ast::ast::{ArgConvention, Dtype};

/// Descriptor type selected for a slice literal at the checked boundary.
///
/// Two-component literals can use the view-oriented contiguous descriptor;
/// literals with a second colon use the owning strided descriptor. `Slice` is
/// the general protocol fallback accepted by user-defined collections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SliceKind {
    Slice,
    ContiguousSlice,
    StridedSlice,
}

impl SliceKind {
    pub const fn type_name(self) -> &'static str {
        match self {
            Self::Slice => "Slice",
            Self::ContiguousSlice => "ContiguousSlice",
            Self::StridedSlice => "StridedSlice",
        }
    }
}

/// A loan-transfer effect inferred from a callable's body: an accepted store
/// into an outliving destination (`self` or a parameter) whose loan roots at
/// another parameter or `self`.
///
/// Call sites replay the effect against their actuals, installing the
/// caller-side loan the callee's store implies.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, Default, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DependentType {
    /// A type-valued parameter expression: a finite type selection
    /// (`ParamKind::Select`), or one element of a variadic pack that is still
    /// a parameter (`ParamKind::ListGet`).
    Parameter(ParamExpr),
}

impl DependentType {
    /// The type `expr` denotes: the contained type when it is a closed Type
    /// constant, and the symbolic wrapper otherwise.
    pub fn resolve(expr: ParamExpr) -> Ty {
        match expr.as_constant() {
            Some(CtValue::Type(ty)) => (**ty).clone(),
            _ => Ty::Dependent(Self::Parameter(expr)),
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
    pub fn map_types(&self, f: impl FnMut(&Ty) -> Ty) -> Ty {
        match self.selection() {
            Some((elements, index)) => ParamContext::detached()
                .select(elements.iter().map(f).collect(), index)
                .map_or_else(|_| Ty::Dependent(self.clone()), Self::resolve),
            None => Ty::Dependent(self.clone()),
        }
    }
}

/// The element type of a [`Ty::Simd`]: a concrete dtype, or a `DType`-valued
/// parameter expression while the enclosing declaration is still symbolic
/// (`Scalar[dt]` under source validation).
///
/// `Expr` is never a closed expression: [`simd_ty_from_slots`] folds one to
/// `Known`, so slot equality is type identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SimdDtype {
    Known(Dtype),
    Expr(ParamExpr),
}

impl SimdDtype {
    pub const fn known(&self) -> Option<Dtype> {
        match self {
            Self::Known(dtype) => Some(*dtype),
            Self::Expr(_) => None,
        }
    }

    pub const fn is_expr(&self) -> bool {
        matches!(self, Self::Expr(_))
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

/// The lane count of a [`Ty::Simd`]: a concrete width, or an `Int`-valued
/// parameter expression while the enclosing declaration is still symbolic
/// (`SIMD[dt, width]`, `SIMD[dt, 2 * n]`).
///
/// `Known(-1)` is the `SIMD[dt, _]` inference wildcard. `Expr` is never a
/// closed expression: [`simd_ty_from_slots`] folds one to `Known`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SimdWidth {
    Known(i64),
    Expr(ParamExpr),
}

impl SimdWidth {
    pub const fn known(&self) -> Option<i64> {
        match self {
            Self::Known(width) => Some(*width),
            Self::Expr(_) => None,
        }
    }

    pub const fn is_expr(&self) -> bool {
        matches!(self, Self::Expr(_))
    }
}

impl fmt::Display for SimdWidth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Known(width) => write!(f, "{width}"),
            Self::Expr(expr) => write!(f, "{expr}"),
        }
    }
}

/// A type in mojito's semantic lattice. Scalars mirror `ast::Type`; `Func` is
/// synthesized from a `def` signature or lowered from a function-type annotation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ty {
    Int,
    UInt,
    Bool,
    /// The compile-time string literal type (Mojo's `StringLiteral`). Its
    /// source spelling is concrete only as a parameter annotation; the
    /// nominal runtime `String` is the self-hosted stdlib struct.
    StringLiteral,
    Float64,
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
    /// A non-generic function. `params`/`names` describe the regular parameters;
    /// `required[i]` is true when regular parameter `i` has no default. The
    /// marker fields are indexes into this regular-parameter list.
    Func {
        /// Checked callable-environment contract. This is semantic type
        /// information even though the VM erases it at execution.
        environment: crate::origin::CallableEnvironment,
        params: Vec<Self>,
        names: Vec<String>,
        ret: Box<Self>,
        required: Vec<bool>,
        variadic: Option<Box<Self>>,
        /// Homogeneous element type collected by `**kwargs`, when present.
        kw_variadic: Option<Box<Self>>,
        positional_only: Option<usize>,
        keyword_only: Option<usize>,
        raises: bool,
        error: Option<Box<Self>>,
        /// The argument convention of each regular parameter.
        conventions: Vec<Option<ArgConvention>>,
        ref_params: Box<Vec<Option<crate::origin::RefSig>>>,
        ref_return: Option<Box<crate::origin::RefSig>>,
        /// Identity-transparent inferred transfer effects; see
        /// [`TransferSet`].
        transfers: TransferSet,
    },
    /// A generic function synthesized from a `def` with a `[params]` list.
    GenericFunc {
        environment: crate::origin::CallableEnvironment,
        decls: Vec<ParamDecl>,
        params: Vec<Self>,
        names: Vec<String>,
        ret: Box<Self>,
        required: Vec<bool>,
        variadic: Option<Box<Self>>,
        /// Homogeneous element type collected by `**kwargs`, when present.
        kw_variadic: Option<Box<Self>>,
        positional_only: Option<usize>,
        keyword_only: Option<usize>,
        raises: bool,
        error: Option<Box<Self>>,
        conventions: Vec<Option<ArgConvention>>,
        ref_params: Box<Vec<Option<crate::origin::RefSig>>>,
        ref_return: Option<Box<crate::origin::RefSig>>,
        /// Identity-transparent inferred transfer effects; see
        /// [`TransferSet`].
        transfers: TransferSet,
    },
    /// A source name that denotes multiple callable signatures. The checker
    /// resolves an overload set at each call site. The first implementation
    /// supports distinct call shapes/arity; keeping this as a first-class type
    /// leaves type-ranked overload resolution as a natural extension.
    Overload(Vec<Self>),
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
        callable_bound: Option<Box<Self>>,
    },
    /// A symbolic associated type lookup such as `C.Element` where `C` is an
    /// opaque type parameter. It may resolve to a concrete type once `C` is
    /// substituted at a generic use site. `args` is the parameter application of
    /// a parameterized associated type (`C.IteratorType[o]`); it is empty for a
    /// bare `C.Element`. The arguments are retained so the projection can be
    /// resolved concretely once the base is a conforming struct.
    Assoc {
        base: Box<Self>,
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
    Struct(String, Vec<TyArg>),
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
    ComptimeList(Box<Self>),
    /// Compiler-private `__RuntimeTuple[T1, ..., Tn]` storage. Public
    /// `Tuple[T1, ..., Tn]` values are nominal standard-library structs.
    Tuple(Vec<Self>),
    /// Internal checked ABI type for a compile-time-specialized heterogeneous
    /// runtime parameter pack. Unlike a source `Tuple[...]` used as the element
    /// type of an ordinary homogeneous `*args`, each entry describes one
    /// positional argument and the collector uses private tuple-shaped storage.
    /// This type cannot be written directly in Mojo source.
    RuntimePack(Vec<Self>),
    /// Internal checked ABI type for an ordinary homogeneous runtime variadic.
    /// Source `List[T]` is a nominal standard-library struct; a `*args: T`
    /// collector is compiler storage and must therefore not masquerade as that
    /// user-facing collection.
    VariadicPack(Box<Self>),
    /// The built-in tagged union `Variant[T1, ..., Tn]`.  The ordering is part
    /// of the type: it determines the runtime tag used by typed projection.
    Variant(Vec<Self>),
    /// The built-in `UnsafePointer[T, origin]`.  The VM erases the origin, but
    /// the checked and MIR types retain it for lifetime/aggregate validation.
    Pointer {
        element: Box<Self>,
        origin: crate::origin::PointerOrigin,
    },
    /// A reference value. Origins and permissions are checked statically; its
    /// runtime representation is introduced only after loan checking exists.
    Ref(crate::origin::RefTy),
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

pub const TSTRING_TYPE_NAME: &str = "TString";

pub const RANGE_TYPE_NAME: &str = "Range";

/// The nominal scalar range family mirroring current Mojo's three private
/// range structs, in `range(...)` arity order (1, 2, 3 arguments).
pub const SCALAR_RANGE_FAMILY: [&str; 3] =
    ["_ZeroStartingRange", "_SequentialRange", "_StridedRange"];

/// The floating-point strided range: upstream's `_StridedRange` float path,
/// which iterates by index through a fused multiply-add rather than by value.
pub const FLOAT_STRIDED_RANGE: &str = "_FloatStridedRange";

/// Decompose a checker-abstract scalar-range type — `Ty::Struct` naming a
/// [`SCALAR_RANGE_FAMILY`] member or [`FLOAT_STRIDED_RANGE`] (plain or
/// module-qualified) with one concrete dtype value argument.
///
/// This form exists only in the discovery round: the specialization fixpoint
/// rewrites every occurrence into a registered concrete struct before MIR
/// lowering.
pub fn scalar_range_parts(ty: &Ty) -> Option<(&'static str, mojito_ast::ast::Dtype)> {
    let Ty::Struct(name, arguments) = ty else {
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
/// The intrinsic `Ty::Variant` spelled from bundled standard-library sources
/// only (`var _storage: __VariantStorage[*Ts]`).
pub const VARIANT_STORAGE_TYPE_NAME: &str = "__VariantStorage";

/// Construct a nominal standard-library type from ordinary type arguments.
pub fn nominal_type(name: impl Into<String>, arguments: Vec<Ty>) -> Ty {
    Ty::Struct(name.into(), arguments.into_iter().map(TyArg::Ty).collect())
}

pub fn list_type(element: Ty) -> Ty {
    nominal_type(LIST_TYPE_NAME, vec![element])
}

pub fn array_type(element: Ty, length: i64) -> Ty {
    Ty::Struct(
        ARRAY_TYPE_NAME.into(),
        vec![TyArg::Ty(element), TyArg::Val(CtValue::Int(length))],
    )
}

pub fn array_parts(ty: &Ty) -> Option<(&Ty, i64)> {
    let Ty::Struct(_, arguments) = ty else {
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
pub fn array_element(ty: &Ty) -> Option<&Ty> {
    let Ty::Struct(name, arguments) = ty else {
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

/// The payload type of compiler-private inline uninit storage
/// (`__UninitStorage[T]`), including specialization-mangled and
/// backend-monomorphized (`…$mono$…`) instantiations.
///
/// Mono renames the struct while keeping its substituted argument list, so the
/// payload stays recoverable from the arguments.
pub fn uninit_storage_element(ty: &Ty) -> Option<&Ty> {
    let Ty::Struct(name, arguments) = ty else {
        return None;
    };
    let name = name.split("$mono").next().unwrap_or(name);
    if name != UNINIT_STORAGE_TYPE_NAME && !name.ends_with(&format!("${UNINIT_STORAGE_TYPE_NAME}"))
    {
        return None;
    }
    let [TyArg::Ty(element)] = arguments.as_slice() else {
        return None;
    };
    Some(element)
}

pub fn list_element(ty: &Ty) -> Option<&Ty> {
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

pub fn set_element(ty: &Ty) -> Option<&Ty> {
    let arguments = nominal_type_arguments(ty, SET_TYPE_NAME)?;
    let ([element] | [element, _]) = arguments.as_slice() else {
        return None;
    };
    Some(*element)
}

pub fn optional_element(ty: &Ty) -> Option<&Ty> {
    let arguments = nominal_type_arguments(ty, OPTIONAL_TYPE_NAME)?;
    let [element] = arguments.as_slice() else {
        return None;
    };
    Some(*element)
}

pub fn owned_pointer_element(ty: &Ty) -> Option<&Ty> {
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

pub fn dict_elements(ty: &Ty) -> Option<(&Ty, &Ty)> {
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
    match ty {
        Ty::Int | Ty::IntLiteral => "SIMD[DType.int, 1]".to_string(),
        Ty::UInt => "SIMD[DType.uint, 1]".to_string(),
        Ty::Float64 | Ty::FloatLiteral => "SIMD[DType.float64, 1]".to_string(),
        Ty::Bool => "Bool".to_string(),
        Ty::Simd { dtype, width } => format!("SIMD[{dtype}, {width}]"),
        Ty::StringLiteral => "StringLiteral".to_string(),
        Ty::None => "NoneType".to_string(),
        Ty::Struct(name, args) => {
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
        Ty::Variant(alternatives) => format!(
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
/// declares `*Ts`), and its starred spelling is its `Ty::Param` name.
pub fn pack_spread(elements: &[Ty]) -> Option<&Ty> {
    match elements {
        [pack @ Ty::Param { binder, .. }] if binder.name.starts_with('*') => Some(pack),
        _ => None,
    }
}

/// `ty` with every spread of the pack `pack` replaced by `elements`.
///
/// A spread is a whole argument list (see [`pack_spread`]), so this is the
/// one place a single type becomes several: `Tuple[*Ts]` at `Ts = [Int,
/// Bool]` is `Tuple[Int, Bool]`.
pub fn expand_pack_spread(ty: &Ty, pack: &str, elements: &[Ty]) -> Ty {
    let spreads = |list: &[Ty]| {
        matches!(pack_spread(list), Some(Ty::Param { binder, .. })
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
    match ty {
        Ty::Struct(name, arguments) => {
            let spread = pack_spread_argument(arguments)
                .is_some_and(|spread| spreads(std::slice::from_ref(spread)));
            let arguments = if spread {
                elements.iter().cloned().map(TyArg::Ty).collect()
            } else {
                map_tyargs(arguments, |ty| expand_pack_spread(ty, pack, elements))
            };
            Ty::Struct(name.clone(), arguments)
        }
        Ty::Tuple(list) => Ty::Tuple(expand(list)),
        Ty::RuntimePack(list) => Ty::RuntimePack(expand(list)),
        Ty::Variant(list) => Ty::Variant(expand(list)),
        Ty::Pointer { element, origin } => Ty::Pointer {
            element: Box::new(expand_pack_spread(element, pack, elements)),
            origin: origin.clone(),
        },
        Ty::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = Box::new(expand_pack_spread(&reference.referent, pack, elements));
            Ty::Ref(reference)
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

pub fn tuple_elements(ty: &Ty) -> Option<Vec<&Ty>> {
    let Ty::Struct(name, arguments) = ty else {
        return None;
    };
    // `$` cannot be written in a source identifier. Besides the ordinary and
    // historically module-qualified public names, accept the concrete symbols
    // emitted for variadic Tuple specializations. Their retained type arguments
    // are semantic metadata; the symbol itself is never decoded.
    if name != TUPLE_TYPE_NAME
        && !name.ends_with(&format!("${TUPLE_TYPE_NAME}"))
        && !name.starts_with(&format!("{TUPLE_TYPE_NAME}$"))
        && !name.contains(&format!("${TUPLE_TYPE_NAME}$"))
    {
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

pub fn tstring_type(elements: Vec<Ty>) -> Ty {
    nominal_type(TSTRING_TYPE_NAME, elements)
}

/// The interleaved element types of a lazy template string.
///
/// Both the public `TString` spelling and the concrete symbols emitted for its
/// variadic specializations are accepted — the same acceptance rule as
/// [`tuple_elements`].
pub fn tstring_elements(ty: &Ty) -> Option<Vec<&Ty>> {
    let Ty::Struct(name, arguments) = ty else {
        return None;
    };
    if name != TSTRING_TYPE_NAME
        && !name.ends_with(&format!("${TSTRING_TYPE_NAME}"))
        && !name.starts_with(&format!("{TSTRING_TYPE_NAME}$"))
        && !name.contains(&format!("${TSTRING_TYPE_NAME}$"))
    {
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

pub fn is_range_type(ty: &Ty) -> bool {
    nominal_type_arguments(ty, RANGE_TYPE_NAME).is_some_and(|arguments| arguments.is_empty())
}

/// Whether a `write_to`/`write_repr_to` parameter is the `Writable` protocol's writer.
///
/// The writer is `mut writer: Some[Writer]` or a `Writer`-bounded type
/// parameter. A method with any other parameter is an ordinary overload, and
/// the value displays through the reflective default instead.
pub fn is_writer_parameter(ty: &Ty) -> bool {
    matches!(ty, Ty::Param { bounds, .. } if bounds.iter().any(|bound| bound == "Writer"))
}

pub fn contains_infer(ty: &Ty) -> bool {
    mentions(ty, &|ty| matches!(ty, Ty::Infer))
}

/// Whether the compile-time `StringLiteral` occurs anywhere in `ty`.
///
/// Its runtime values are the literal representation rather than the nominal
/// `String` struct's, so an instantiation argument that mentions it never
/// selects a nominal-`String` instance.
pub fn contains_string_literal(ty: &Ty) -> bool {
    mentions(ty, &|ty| matches!(ty, Ty::StringLiteral))
}

/// Whether `predicate` holds for `ty` or any type nested in it (struct
/// arguments, elements, callable signatures).
pub fn mentions(ty: &Ty, predicate: &dyn Fn(&Ty) -> bool) -> bool {
    if predicate(ty) {
        return true;
    }
    let argument_mentions = |argument: &TyArg| match argument {
        TyArg::Ty(ty) => mentions(ty, predicate),
        TyArg::Val(CtValue::Expr(expr)) => expr_mentions(expr, predicate),
        TyArg::Val(_) | TyArg::Origin(_) => false,
    };
    match ty {
        Ty::Struct(_, arguments) => arguments.iter().any(argument_mentions),
        Ty::ComptimeList(element) | Ty::VariadicPack(element) | Ty::Pointer { element, .. } => {
            mentions(element, predicate)
        }
        Ty::Dependent(dependent) => expr_mentions(dependent.expr(), predicate),
        Ty::Tuple(elements) | Ty::RuntimePack(elements) | Ty::Variant(elements) => {
            elements.iter().any(|element| mentions(element, predicate))
        }
        Ty::Assoc { base, args, .. } => {
            mentions(base, predicate) || args.iter().any(argument_mentions)
        }
        Ty::Func {
            params,
            ret,
            variadic,
            kw_variadic,
            ..
        }
        | Ty::GenericFunc {
            params,
            ret,
            variadic,
            kw_variadic,
            ..
        } => {
            params.iter().any(|param| mentions(param, predicate))
                || mentions(ret, predicate)
                || variadic
                    .as_deref()
                    .is_some_and(|variadic| mentions(variadic, predicate))
                || kw_variadic
                    .as_deref()
                    .is_some_and(|variadic| mentions(variadic, predicate))
        }
        Ty::Overload(candidates) => candidates
            .iter()
            .any(|candidate| mentions(candidate, predicate)),
        _ => false,
    }
}

/// A declared compile-time parameter of a generic `struct`/`def`, classified
/// from `[name: X]` by whether `X` is a trait or a type.
///
/// Whether a type parameter's bound admits nullary construction (`H()`): such
/// parameters are reified at runtime as the bound struct's name so an erased
/// body can construct them.
pub fn constructible_type_parameter(declaration: &ParamDecl) -> bool {
    matches!(
        declaration,
        ParamDecl::Type { bounds, .. }
            if bounds.iter().any(|bound| matches!(bound.as_str(), "Hasher" | "Defaultable"))
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ParamDecl {
    /// A type parameter `T: Trait & ...`.
    Type {
        /// The binder's identity: its declaration and slot. Every `Ty::Param`
        /// naming this binder carries the same id.
        id: ParamId,
        name: String,
        bounds: Vec<String>,
        /// Checked anonymous callable-trait contract, when this parameter was
        /// declared with `F: def(...) -> ...`.
        callable_bound: Option<Box<Ty>>,
        default: Option<Box<Ty>>,
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
        ty: Box<Ty>,
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
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CallableDefault {
    Symbol(String),
    Parameter(String),
    If {
        condition: ParamExpr,
        then_value: Box<Self>,
        else_value: Box<Self>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConstraintOperand {
    Param(String),
    Value(CtValue),
    Type(Ty),
    /// `TypeList[Ts.values]().length` over a symbolic pack parameter,
    /// resolving to the bound pack's element count.
    PackLength(String),
    /// An arithmetic operand over value parameters (`n + 1` in `where n + 1 ==
    /// m`), in canonical form. It is retained symbolically and discharged by
    /// replacement at each application.
    Expr(ParamExpr),
}

/// The per-element predicate of a `TypeList` `any`/`all` proposition.
///
/// A builtin `IsTrivially*` spelling or a Bool-bodied predicate alias with one
/// type parameter, applied to each element of the bound pack.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GenericConstraint {
    /// A top-level `where (condition, "message")` clause. The message affects
    /// only the failed-specialization diagnostic; semantic operations recurse
    /// through the wrapped condition.
    WithMessage(Box<Self>, String),
    Conforms {
        param: String,
        trait_name: String,
    },
    ConformsPack {
        param: String,
        trait_name: String,
    },
    /// `TypeList[Ts.values]().any[P]()` / `.all[P]()` over a symbolic pack
    /// parameter: `P` holds for at least one / every element type.
    PackPredicate {
        param: String,
        predicate: PackPredicateRef,
        all: bool,
    },
    /// `TypeList[Ts.values]().contains[T]()`: the operand type equals some
    /// element of the bound pack.
    PackContains {
        param: String,
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

impl fmt::Display for GenericConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WithMessage(inner, message) => {
                write!(f, "({inner}, {message:?})")
            }
            Self::Conforms { param, trait_name } => {
                write!(f, "conforms_to({param}, {trait_name})")
            }
            Self::ConformsPack { param, trait_name } => {
                write!(f, "conforms_to({param}.values, {trait_name})")
            }
            Self::PackPredicate {
                param,
                predicate,
                all,
            } => {
                let reduction = if *all { "all" } else { "any" };
                let predicate = match predicate {
                    PackPredicateRef::Trivial(kind) => trivial_predicate_spelling(*kind),
                    PackPredicateRef::Alias(name) => name,
                };
                write!(f, "TypeList[{param}.values]().{reduction}[{predicate}]()")
            }
            Self::PackContains { param, element } => {
                write!(f, "TypeList[{param}.values]().contains[{element}]()")
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

    /// The reference a use of this binder carries: its identity with its
    /// spelling attached.
    pub fn binder(&self) -> ParamRef {
        ParamRef {
            id: self.id().clone(),
            name: std::sync::Arc::from(self.name()),
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
/// exactly like `Ty::Pointer` origins — a parameterized iterator's `origin`
/// argument distinguishes checked types without changing lowering.
#[derive(Debug, Clone, Eq)]
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
            Self::Ty(ty) => Some(CtValue::Type(Box::new(ty.clone()))),
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
        match self {
            Self::Int | Self::IntLiteral => write!(f, "Int"),
            Self::UInt => write!(f, "UInt"),
            Self::Bool => write!(f, "Bool"),
            Self::StringLiteral => write!(f, "StringLiteral"),
            Self::Float64 | Self::FloatLiteral => write!(f, "Float64"),
            Self::Infer => write!(f, "_"),
            Self::Dtype => write!(f, "DType"),
            Self::None => write!(f, "None"),
            Self::Never => write!(f, "Never"),
            Self::Func {
                environment,
                params,
                ret,
                raises,
                ..
            }
            | Self::GenericFunc {
                environment,
                params,
                ret,
                raises,
                ..
            } => {
                // A generic contract renders its binders and trailing `where`
                // constraints so a constrained-vs-unconstrained mismatch is
                // visible in diagnostics.
                if let Self::GenericFunc { decls, .. } = self {
                    write!(f, "def[")?;
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
                            ParamDecl::Value { name, ty, .. } => write!(f, "{name}: {ty}")?,
                        }
                    }
                    write!(f, "](")?;
                } else {
                    write!(f, "def(")?;
                }
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{p}")?;
                }
                write!(f, ")")?;
                match environment {
                    crate::origin::CallableEnvironment::Default => {}
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
                if let Self::GenericFunc { decls, .. } = self {
                    for decl in decls {
                        let (ParamDecl::Type { constraints, .. }
                        | ParamDecl::Value { constraints, .. }) = decl;
                        for constraint in constraints {
                            write!(f, " where {constraint}")?;
                        }
                    }
                }
                Ok(())
            }
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
            } => match dtype.scalar_alias() {
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
                        mutability: crate::origin::Mutability::Immutable,
                        ..
                    } => {
                        write!(f, ", ImmOrigin(origin#{})", id.0)?;
                    }
                    crate::origin::PointerOrigin::Param { id, .. } => {
                        write!(f, ", origin#{}", id.0)?;
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

fn nominal_type_arguments<'a>(ty: &'a Ty, expected: &str) -> Option<Vec<&'a Ty>> {
    let Ty::Struct(name, arguments) = ty else {
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
    match ty {
        Ty::Struct(name, arguments) => Ty::Struct(
            name.clone(),
            arguments
                .iter()
                .map(|argument| match argument {
                    TyArg::Ty(inner) => TyArg::Ty(recur(inner)),
                    TyArg::Val(value) => TyArg::Val(value.clone()),
                    TyArg::Origin(_) => TyArg::Origin(crate::origin::Origin::Unbound),
                })
                .collect(),
        ),
        Ty::Tuple(elements) => Ty::Tuple(elements.iter().map(recur).collect()),
        Ty::RuntimePack(elements) => Ty::RuntimePack(elements.iter().map(recur).collect()),
        Ty::Variant(alternatives) => Ty::Variant(alternatives.iter().map(recur).collect()),
        Ty::ComptimeList(element) => Ty::ComptimeList(Box::new(recur(element))),
        Ty::VariadicPack(element) => Ty::VariadicPack(Box::new(recur(element))),
        Ty::Pointer { element, origin } => Ty::Pointer {
            element: Box::new(recur(element)),
            origin: origin.clone(),
        },
        Ty::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = Box::new(recur(&reference.referent));
            Ty::Ref(reference)
        }
        other => other.clone(),
    }
}

/// The checker's value-coercion predicate, shared with MIR verification so the
/// verifier never re-derives conversion rules.
pub fn value_coerces(from: &Ty, to: &Ty) -> bool {
    coerces(from, to)
}

/// The (canonicalized) `Ty` for a SIMD of `dtype`/`width`.
///
/// A width-1 `int` is the native `Ty::Int` and a width-1 `float64` the native
/// `Ty::Float64` (Mojo unifies `Int`/`Float64` with their `SIMD[..., 1]`
/// spellings); everything else is a `Ty::Simd`.
///
/// Every phase that names a SIMD leaf type (checker annotations, the VM's
/// runtime leaf classification, the hasher clone names) agrees through this
/// one function.
pub const fn canonical_simd_ty(dtype: Dtype, width: i64) -> Ty {
    match (dtype, width) {
        (Dtype::Int, 1) => Ty::Int,
        (Dtype::Float64, 1) => Ty::Float64,
        _ => Ty::Simd {
            dtype: SimdDtype::Known(dtype),
            width: SimdWidth::Known(width),
        },
    }
}

/// The SIMD type of two slots that may still be symbolic.
///
/// A slot whose expression is a closed constant becomes `Known`, and two
/// known slots canonicalize through [`canonical_simd_ty`]. This is the only
/// constructor of a symbolic `Ty::Simd`, so `Expr` never holds a closed
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
        SimdWidth::Expr(expr) => match expr.as_constant() {
            Some(value) => SimdWidth::Known(
                crate::param_expr::fold::integer_value(value)
                    .and_then(|width| width.to_i64())
                    .ok_or_else(|| ParamError::TypeMismatch {
                        operation: "SIMD width".to_string(),
                        expected: "Int".to_string(),
                        found: value.to_string(),
                    })?,
            ),
            None => SimdWidth::Expr(expr),
        },
        known @ SimdWidth::Known(_) => known,
    };
    Ok(match (dtype, width) {
        (SimdDtype::Known(dtype), SimdWidth::Known(width)) => canonical_simd_ty(dtype, width),
        (dtype, width) => Ty::Simd { dtype, width },
    })
}

/// The type a Hashable builtin leaf contributes to its hasher as: a `DType`
/// hashes its code as a `UInt8` (upstream's `DType.__hash__`); every other
/// leaf hashes as itself.
pub fn hash_leaf_ty(ty: &Ty) -> Ty {
    match ty {
        Ty::Dtype => canonical_simd_ty(Dtype::UInt8, 1),
        other => other.clone(),
    }
}

/// The `(dtype, width)` shape of a SIMD-valued type — the native numeric
/// scalars are width-1 vectors of their dtype (`UInt` reports `uint64`, its
/// bit width) — or `None` for a non-SIMD type.
///
/// `Bool` is not a SIMD value (upstream's `Bool` is its own struct;
/// `Scalar[DType.bool]` is the vector).
pub const fn simd_shape(ty: &Ty) -> Option<(Dtype, i64)> {
    Some(match ty {
        Ty::Int | Ty::IntLiteral => (Dtype::Int, 1),
        Ty::UInt => (Dtype::UInt64, 1),
        Ty::Float64 | Ty::FloatLiteral => (Dtype::Float64, 1),
        Ty::Simd {
            dtype: SimdDtype::Known(dtype),
            width: SimdWidth::Known(width),
        } => (*dtype, *width),
        _ => return None,
    })
}

/// The slots of a SIMD-valued type, symbolic or not — [`simd_shape`] for a
/// declaration that is still a template.
pub fn simd_slots(ty: &Ty) -> Option<(SimdDtype, SimdWidth)> {
    match ty {
        Ty::Simd { dtype, width } => Some((dtype.clone(), width.clone())),
        _ => {
            simd_shape(ty).map(|(dtype, width)| (SimdDtype::Known(dtype), SimdWidth::Known(width)))
        }
    }
}

/// Whether `ty` is a width-one `Ty::Simd` (a scalar alias other than the
/// native `Int`/`Float64`), whatever its dtype slot.
pub const fn is_scalar_simd(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Simd {
            width: SimdWidth::Known(1),
            ..
        }
    )
}

/// The known dtype of a width-one `Ty::Simd`.
pub const fn scalar_simd_dtype(ty: &Ty) -> Option<Dtype> {
    match ty {
        Ty::Simd {
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
        SimdDtype::Expr(_) => Ty::Simd {
            dtype: dtype.clone(),
            width: SimdWidth::Known(1),
        },
    }
}

/// The runtime type an exact literal materializes to when no context selects
/// another (`IntLiteral` → `Int`, `FloatLiteral` → `Float64`), applied through
/// struct arguments, packs, and variants.
pub fn default_literal(ty: &Ty) -> Ty {
    match ty {
        Ty::IntLiteral => Ty::Int,
        Ty::FloatLiteral => Ty::Float64,
        Ty::Struct(name, arguments) => Ty::Struct(
            name.clone(),
            arguments
                .iter()
                .map(|argument| match argument {
                    TyArg::Ty(ty) => TyArg::Ty(default_literal(ty)),
                    TyArg::Val(value) => TyArg::Val(value.clone()),
                    TyArg::Origin(origin) => TyArg::Origin(origin.clone()),
                })
                .collect(),
        ),
        // Internal heterogeneous pack storage also materializes its elements.
        Ty::Tuple(elems) => Ty::Tuple(elems.iter().map(default_literal).collect()),
        Ty::VariadicPack(element) => Ty::VariadicPack(Box::new(default_literal(element))),
        Ty::RuntimePack(elems) => Ty::RuntimePack(elems.iter().map(default_literal).collect()),
        Ty::Variant(alternatives) => {
            Ty::Variant(alternatives.iter().map(default_literal).collect())
        }
        other => other.clone(),
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
    if *from == Ty::Never {
        return true;
    }
    if from == to {
        return true;
    }
    match (from, to) {
        (Ty::Struct(from, from_args), Ty::Struct(to, to_args))
            if matches!(from.as_str(), "ContiguousSlice" | "StridedSlice")
                && to == "Slice"
                && from_args.is_empty()
                && to_args.is_empty() =>
        {
            true
        }
        // The checked analogue of upstream's `@implicit StridedSlice(other:
        // Slice)`: a `Slice`-typed descriptor value selects the strided
        // (normalizing) overloads. A contiguous literal does not widen this
        // way, matching upstream where `[a:b]` never builds a `StridedSlice`.
        (Ty::Struct(from, from_args), Ty::Struct(to, to_args))
            if from == "Slice"
                && to == "StridedSlice"
                && from_args.is_empty()
                && to_args.is_empty() =>
        {
            true
        }
        // Public Tuple remains nominal, but its generated specialization symbol
        // deliberately differs from the canonical discovery-pass name. Compare
        // the retained semantic element arguments instead of requiring those
        // implementation symbols to match.
        (from, to) if tuple_elements(from).is_some() && tuple_elements(to).is_some() => {
            let from = tuple_elements(from).expect("guard established Tuple elements");
            let to = tuple_elements(to).expect("guard established Tuple elements");
            from.len() == to.len() && from.iter().zip(to).all(|(from, to)| coerces(from, to))
        }
        // The same public-vs-specialized bridge for the lazy TString.
        (from, to)
            if crate::types::tstring_elements(from).is_some()
                && crate::types::tstring_elements(to).is_some() =>
        {
            let from =
                crate::types::tstring_elements(from).expect("guard established TString elements");
            let to =
                crate::types::tstring_elements(to).expect("guard established TString elements");
            from.len() == to.len() && from.iter().zip(to).all(|(from, to)| coerces(from, to))
        }
        (Ty::Param { binder: a, .. }, Ty::Param { binder: b, .. }) => a == b,
        (Ty::Struct(an, aargs), Ty::Struct(bn, bargs)) => {
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
        (Ty::ComptimeList(a), Ty::ComptimeList(b)) => coerces(a, b),
        (
            Ty::Pointer {
                element: a,
                origin: ao,
            },
            Ty::Pointer {
                element: b,
                origin: bo,
            },
        ) => coerces(a, b) && ao.coerces_to(bo),
        (
            Ty::Func {
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
            },
            Ty::Func {
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
            },
        ) => {
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
                && match (from_error.as_deref(), to_error.as_deref()) {
                    (None, None) => true,
                    (None, Some(Ty::Never)) => true,
                    (None, Some(_)) => true,
                    (Some(from), Some(Ty::Error)) => from != &Ty::Never,
                    (Some(from), Some(to)) => from == to,
                    (Some(Ty::Never), None) => true,
                    (Some(_), None) => false,
                }
                && from_params.len() == to_params.len()
                && from_params
                    .iter()
                    .zip(to_params)
                    .all(|(from, to)| from == to)
                && from_ret == to_ret
        }
        (Ty::IntLiteral, Ty::Int | Ty::UInt | Ty::Float64 | Ty::FloatLiteral) => true,
        (Ty::FloatLiteral, Ty::Float64) => true,
        (
            literal,
            Ty::Simd {
                dtype,
                width: SimdWidth::Known(1),
            },
        ) if splats_to(literal, dtype) => true,
        (
            Ty::Simd {
                dtype: from_dtype,
                width: from_width,
            },
            Ty::Simd {
                dtype: to_dtype,
                width: SimdWidth::Known(-1),
            },
        ) => from_dtype == to_dtype && from_width.known().is_none_or(|width| width > 0),
        // A tuple coerces element-wise (same arity) — so a literal element
        // materializes: `(1, 2.0)` fits `Tuple[Float64, Float64]`.
        (Ty::Tuple(a), Ty::Tuple(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| coerces(x, y))
        }
        (Ty::Variant(a), Ty::Variant(b)) => {
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
pub fn splats_to(ty: &Ty, dtype: &SimdDtype) -> bool {
    match (ty, dtype) {
        (Ty::IntLiteral, SimdDtype::Known(dtype)) => *dtype != Dtype::Bool,
        (Ty::FloatLiteral, SimdDtype::Known(dtype)) => dtype.is_float(),
        (Ty::IntLiteral | Ty::FloatLiteral, SimdDtype::Expr(_)) => true,
        (Ty::Bool, SimdDtype::Known(dtype)) => *dtype == Dtype::Bool,
        (Ty::Int, SimdDtype::Known(dtype)) => *dtype == Dtype::Int,
        // `Float64` is `SIMD[DType.float64, 1]`, so it splats into a float64 vector.
        (Ty::Float64, SimdDtype::Known(dtype)) => *dtype == Dtype::Float64,
        (
            Ty::Simd {
                dtype: d,
                width: SimdWidth::Known(1),
            },
            dtype,
        ) => d == dtype,
        _ => false,
    }
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
            CallableEnvironment::Capturing(_),
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
pub const fn builtin_copy_is_value_read(ty: &Ty) -> bool {
    !matches!(
        ty,
        Ty::Struct(..)
            | Ty::Param { .. }
            | Ty::Assoc { .. }
            | Ty::Ref(_)
            | Ty::SelfType
            | Ty::Func { .. }
            | Ty::GenericFunc { .. }
            | Ty::Overload(_)
            | Ty::Error
    )
}

/// Recover the monomorphic or generic callable contract carried either directly
/// by a function type or indirectly by a callable-bounded type parameter.
pub fn callable_contract_ty(ty: &Ty) -> Option<&Ty> {
    match ty {
        Ty::Func { .. } | Ty::GenericFunc { .. } => Some(ty),
        Ty::Param {
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
    if matches!(actual, Ty::GenericFunc { .. }) || matches!(contract, Ty::GenericFunc { .. }) {
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

    let (
        Ty::Func {
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
        },
        Ty::Func {
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
        },
    ) = (actual, contract)
    else {
        return false;
    };

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
        && match (actual_error.as_deref(), contract_error.as_deref()) {
            (None | Some(Ty::Never), _) => true,
            (Some(_), None) => false,
            (Some(actual), Some(Ty::Error)) => actual != &Ty::Never,
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
    let Ty::GenericFunc {
        environment,
        decls,
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
    } = callable
    else {
        return None;
    };

    let mut signature = params.clone();
    let variadic_index = variadic.as_ref().map(|parameter| {
        let index = signature.len();
        signature.push((**parameter).clone());
        index
    });
    let kw_variadic_index = kw_variadic.as_ref().map(|parameter| {
        let index = signature.len();
        signature.push((**parameter).clone());
        index
    });
    let return_index = signature.len();
    signature.push((**ret).clone());
    let error_index = error.as_ref().map(|error| {
        let index = signature.len();
        signature.push((**error).clone());
        index
    });
    let (decls, signature) = canonical_generic_signature(decls, &signature);

    Some((
        decls,
        Ty::Func {
            environment: environment.clone(),
            params: signature[..params.len()].to_vec(),
            names: names.clone(),
            ret: Box::new(signature[return_index].clone()),
            required: required.clone(),
            variadic: variadic_index.map(|index| Box::new(signature[index].clone())),
            kw_variadic: kw_variadic_index.map(|index| Box::new(signature[index].clone())),
            positional_only: *positional_only,
            keyword_only: *keyword_only,
            raises: *raises,
            error: error_index.map(|index| Box::new(signature[index].clone())),
            conventions: conventions.clone(),
            ref_params: ref_params.clone(),
            ref_return: ref_return.clone(),
            transfers: transfers.clone(),
        },
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
    let mut binder_names: HashMap<String, String> = HashMap::new();
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
                name,
                bounds,
                callable_bound,
                default: _,
                infer_only: _,
                variadic,
                constraints,
            } => {
                let canonical_name = format!("${index}");
                let canonical_id = ParamId::new(CONTRACT_BINDER_OWNER, index);
                let canonical_callable_bound = callable_bound.as_ref().map(|bound| {
                    Box::new(bind_signature_slots(
                        &substitute(bound, &subst),
                        &value_slots,
                    ))
                });
                subst.insert(
                    id.clone(),
                    Ty::Param {
                        binder: ParamRef {
                            id: canonical_id.clone(),
                            name: canonical_name.as_str().into(),
                        },
                        bounds: bounds.clone(),
                        callable_bound: canonical_callable_bound.clone(),
                    },
                );
                binder_names.insert(
                    name.trim_start_matches('*').to_string(),
                    canonical_name.clone(),
                );
                ParamDecl::Type {
                    id: canonical_id,
                    name: canonical_name,
                    bounds: bounds.clone(),
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
                id: _,
                name,
                ty,
                default: _,
                callable_default: _,
                infer_only: _,
                variadic,
                constraints,
            } => {
                let canonical_name = format!("${index}");
                let canonical_ty = bind_signature_slots(&substitute(ty, &subst), &value_slots);
                value_slots.bind_name(
                    name,
                    ParamContext::detached().index_ref(
                        0,
                        u32::try_from(index).unwrap_or(u32::MAX),
                        crate::param_expr::MetaTy::value((**ty).clone()),
                    ),
                );
                binder_names.insert(
                    name.trim_start_matches('*').to_string(),
                    canonical_name.clone(),
                );
                ParamDecl::Value {
                    id: ParamId::new(CONTRACT_BINDER_OWNER, index),
                    name: canonical_name,
                    ty: Box::new(canonical_ty),
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
                        rename_constraint_parameters(
                            constraint,
                            &binder_names,
                            &subst,
                            &value_slots,
                        )
                    })
                    .collect();
            }
        }
    }
    (canonical_decls, canonical_params)
}

/// The substitution mapping a struct's type-parameter names to an instance's
/// type arguments (`[T] @ [Int]` ⟹ `{T: Int}`), for [`substitute`].
///
/// Value parameters and arguments are skipped: they never appear in a type.
/// Empty for a non-generic struct.
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

/// Replace every `Ty::Param` in `ty` with its solution from `subst` (leaving an
/// unsolved parameter untouched). Recurses into struct type arguments.
pub fn substitute(ty: &Ty, subst: &TySubst) -> Ty {
    match ty {
        Ty::Param {
            binder,
            bounds,
            callable_bound,
        } => subst.get(&binder.id).cloned().unwrap_or_else(|| Ty::Param {
            binder: binder.clone(),
            bounds: bounds.clone(),
            callable_bound: callable_bound
                .as_ref()
                .map(|bound| Box::new(substitute(bound, subst))),
        }),
        Ty::Struct(name, args) => {
            Ty::Struct(name.clone(), map_tyargs(args, |t| substitute(t, subst)))
        }
        Ty::Dependent(dependent) => {
            let mut bindings = ParamBindings::new();
            for (id, ty) in subst {
                bindings.bind_type(id.clone(), ty.clone());
            }
            ParamContext::detached()
                .replace(dependent.expr(), &bindings)
                .map_or_else(|_| ty.clone(), DependentType::resolve)
        }
        Ty::ComptimeList(elem) => Ty::ComptimeList(Box::new(substitute(elem, subst))),
        Ty::Tuple(elems) => Ty::Tuple(elems.iter().map(|t| substitute(t, subst)).collect()),
        Ty::RuntimePack(elems) => {
            Ty::RuntimePack(elems.iter().map(|t| substitute(t, subst)).collect())
        }
        Ty::VariadicPack(element) => Ty::VariadicPack(Box::new(substitute(element, subst))),
        Ty::Variant(alternatives) => Ty::Variant(
            alternatives
                .iter()
                .map(|ty| substitute(ty, subst))
                .collect(),
        ),
        Ty::Pointer { element, origin } => Ty::Pointer {
            element: Box::new(substitute(element, subst)),
            origin: origin.clone(),
        },
        Ty::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = Box::new(substitute(&reference.referent, subst));
            Ty::Ref(reference)
        }
        Ty::Assoc { base, name, args } => Ty::Assoc {
            base: Box::new(substitute(base, subst)),
            name: name.clone(),
            args: map_tyargs(args, |t| substitute(t, subst)),
        },
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
        } => Ty::Func {
            environment: environment.clone(),
            params: params.iter().map(|p| substitute(p, subst)).collect(),
            names: names.clone(),
            ret: Box::new(substitute(ret, subst)),
            required: required.clone(),
            variadic: variadic.as_ref().map(|v| Box::new(substitute(v, subst))),
            kw_variadic: kw_variadic.as_ref().map(|v| Box::new(substitute(v, subst))),
            positional_only: *positional_only,
            keyword_only: *keyword_only,
            raises: *raises,
            error: error
                .as_ref()
                .map(|error| Box::new(substitute(error, subst))),
            conventions: conventions.clone(),
            ref_params: ref_params.clone(),
            ref_return: ref_return.clone(),
            transfers: transfers.clone(),
        },
        Ty::GenericFunc {
            environment,
            decls,
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
                        callable_bound,
                        default,
                        infer_only,
                        variadic,
                        constraints,
                    } => ParamDecl::Type {
                        id: id.clone(),
                        name: name.clone(),
                        bounds: bounds.clone(),
                        callable_bound: callable_bound
                            .as_ref()
                            .map(|bound| Box::new(substitute(bound, &nested))),
                        default: default
                            .as_ref()
                            .map(|default| Box::new(substitute(default, &nested))),
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
                        ty: Box::new(substitute(ty, &nested)),
                        default: default.clone(),
                        callable_default: callable_default.clone(),
                        infer_only: *infer_only,
                        variadic: *variadic,
                        constraints: constraints.clone(),
                    },
                })
                .collect();
            Ty::GenericFunc {
                environment: environment.clone(),
                decls,
                params: params
                    .iter()
                    .map(|parameter| substitute(parameter, &nested))
                    .collect(),
                names: names.clone(),
                ret: Box::new(substitute(ret, &nested)),
                required: required.clone(),
                variadic: variadic
                    .as_ref()
                    .map(|parameter| Box::new(substitute(parameter, &nested))),
                kw_variadic: kw_variadic
                    .as_ref()
                    .map(|parameter| Box::new(substitute(parameter, &nested))),
                positional_only: *positional_only,
                keyword_only: *keyword_only,
                raises: *raises,
                error: error
                    .as_ref()
                    .map(|error| Box::new(substitute(error, &nested))),
                conventions: conventions.clone(),
                ref_params: ref_params.clone(),
                ref_return: ref_return.clone(),
                transfers: transfers.clone(),
            }
        }
        Ty::Overload(candidates) => Ty::Overload(
            candidates
                .iter()
                .map(|candidate| substitute(candidate, subst))
                .collect(),
        ),
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
}

/// Rebuild `ty` through `rewrite`. Origins, conventions, and transfer effects
/// pass through: they are checked decorations, not parameter positions.
pub fn rewrite_ty(ty: &Ty, rewrite: &mut dyn TyRewrite) -> Result<Ty, ParamError> {
    let all = |types: &[Ty], rewrite: &mut dyn TyRewrite| {
        types
            .iter()
            .map(|ty| rewrite_ty(ty, rewrite))
            .collect::<Result<Vec<_>, _>>()
    };
    let boxed = |ty: &Option<Box<Ty>>, rewrite: &mut dyn TyRewrite| {
        ty.as_deref()
            .map(|ty| rewrite_ty(ty, rewrite).map(Box::new))
            .transpose()
    };
    Ok(match ty {
        Ty::Param {
            binder,
            bounds,
            callable_bound,
        } => match rewrite.param(binder) {
            Some(replacement) => replacement,
            None => Ty::Param {
                binder: binder.clone(),
                bounds: bounds.clone(),
                callable_bound: boxed(callable_bound, rewrite)?,
            },
        },
        Ty::Struct(name, arguments) => {
            Ty::Struct(name.clone(), rewrite_tyargs(arguments, rewrite)?)
        }
        Ty::Dependent(dependent) => DependentType::resolve(rewrite.expr(dependent.expr())?),
        Ty::ComptimeList(element) => Ty::ComptimeList(Box::new(rewrite_ty(element, rewrite)?)),
        Ty::VariadicPack(element) => Ty::VariadicPack(Box::new(rewrite_ty(element, rewrite)?)),
        Ty::Tuple(elements) => Ty::Tuple(all(elements, rewrite)?),
        Ty::RuntimePack(elements) => Ty::RuntimePack(all(elements, rewrite)?),
        Ty::Variant(alternatives) => Ty::Variant(all(alternatives, rewrite)?),
        Ty::Overload(candidates) => Ty::Overload(all(candidates, rewrite)?),
        Ty::Pointer { element, origin } => Ty::Pointer {
            element: Box::new(rewrite_ty(element, rewrite)?),
            origin: origin.clone(),
        },
        Ty::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = Box::new(rewrite_ty(&reference.referent, rewrite)?);
            Ty::Ref(reference)
        }
        Ty::Assoc { base, name, args } => Ty::Assoc {
            base: Box::new(rewrite_ty(base, rewrite)?),
            name: name.clone(),
            args: rewrite_tyargs(args, rewrite)?,
        },
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
        } => Ty::Func {
            environment: environment.clone(),
            params: all(params, rewrite)?,
            names: names.clone(),
            ret: Box::new(rewrite_ty(ret, rewrite)?),
            required: required.clone(),
            variadic: boxed(variadic, rewrite)?,
            kw_variadic: boxed(kw_variadic, rewrite)?,
            positional_only: *positional_only,
            keyword_only: *keyword_only,
            raises: *raises,
            error: boxed(error, rewrite)?,
            conventions: conventions.clone(),
            ref_params: ref_params.clone(),
            ref_return: ref_return.clone(),
            transfers: transfers.clone(),
        },
        Ty::GenericFunc {
            environment,
            decls,
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
            rewrite.enter_signature(decls);
            let rebuilt = (|| {
                Ok(Ty::GenericFunc {
                    environment: environment.clone(),
                    decls: decls
                        .iter()
                        .map(|decl| rewrite_decl(decl, rewrite))
                        .collect::<Result<_, _>>()?,
                    params: all(params, rewrite)?,
                    names: names.clone(),
                    ret: Box::new(rewrite_ty(ret, rewrite)?),
                    required: required.clone(),
                    variadic: boxed(variadic, rewrite)?,
                    kw_variadic: boxed(kw_variadic, rewrite)?,
                    positional_only: *positional_only,
                    keyword_only: *keyword_only,
                    raises: *raises,
                    error: boxed(error, rewrite)?,
                    conventions: conventions.clone(),
                    ref_params: ref_params.clone(),
                    ref_return: ref_return.clone(),
                    transfers: transfers.clone(),
                })
            })();
            rewrite.exit_signature();
            rebuilt?
        }
        Ty::Simd { dtype, width } => {
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
        Ty::Int
        | Ty::UInt
        | Ty::Bool
        | Ty::StringLiteral
        | Ty::Float64
        | Ty::None
        | Ty::Never
        | Ty::IntLiteral
        | Ty::FloatLiteral
        | Ty::Infer
        | Ty::Dtype
        | Ty::SelfType
        | Ty::Error => ty.clone(),
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
        CtValue::Expr(expr) => rewrite.expr(expr)?.into_value(),
        CtValue::Type(ty) => CtValue::Type(Box::new(rewrite_ty(ty, rewrite)?)),
        CtValue::Reflected(ty) => CtValue::Reflected(Box::new(rewrite_ty(ty, rewrite)?)),
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
        CtValue::Int(_)
        | CtValue::UInt(_)
        | CtValue::Float(_)
        | CtValue::IntLiteral(_)
        | CtValue::FloatLiteral(_)
        | CtValue::Bool(_)
        | CtValue::Str(_)
        | CtValue::Dtype(_)
        | CtValue::Simd { .. }
        | CtValue::Deferred(_) => value.clone(),
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

/// Whether a deferred slot ([`CtValue::Deferred`]) occurs in a value argument
/// anywhere in `ty`.
pub fn mentions_deferred_value(ty: &Ty) -> bool {
    struct Finder(bool);
    impl TyRewrite for Finder {
        fn expr(&mut self, expr: &ParamExpr) -> Result<ParamExpr, ParamError> {
            Ok(expr.clone())
        }

        fn value(&mut self, value: &CtValue) {
            self.0 |= matches!(value, CtValue::Deferred(_));
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
            TyArg::Val(v) => TyArg::Val(v.clone()),
            // Origin substitution is threaded separately; pass origins through.
            TyArg::Origin(o) => TyArg::Origin(o.clone()),
        })
        .collect()
}

/// Alpha-rename the binder references inside one canonicalized constraint.
///
/// `param`-shaped fields rename through `binder_names` (falling back to the
/// pack-trimmed spelling), and embedded types canonicalize exactly like
/// signature types.
#[allow(clippy::implicit_hasher, reason = "TODO: generalize over BuildHasher")]
pub fn rename_constraint_parameters(
    constraint: &GenericConstraint,
    binder_names: &HashMap<String, String>,
    subst: &TySubst,
    value_slots: &ParamBindings,
) -> GenericConstraint {
    let rename = |name: &str| -> String {
        if let Some(canonical) = binder_names.get(name) {
            return canonical.clone();
        }
        let trimmed = name.trim_start_matches('*');
        if let Some(canonical) = binder_names.get(trimmed) {
            return canonical.clone();
        }
        name.to_string()
    };
    let operand = |operand: &crate::types::ConstraintOperand| -> crate::types::ConstraintOperand {
        use crate::types::ConstraintOperand;
        match operand {
            ConstraintOperand::Param(name) => ConstraintOperand::Param(rename(name)),
            ConstraintOperand::PackLength(name) => ConstraintOperand::PackLength(rename(name)),
            ConstraintOperand::Value(value) => ConstraintOperand::Value(value.clone()),
            ConstraintOperand::Expr(expr) => ConstraintOperand::Expr(
                ParamContext::detached()
                    .replace(expr, value_slots)
                    .unwrap_or_else(|_| expr.clone()),
            ),
            ConstraintOperand::Type(ty) => {
                ConstraintOperand::Type(bind_signature_slots(&substitute(ty, subst), value_slots))
            }
        }
    };
    let recurse = |inner: &GenericConstraint| {
        rename_constraint_parameters(inner, binder_names, subst, value_slots)
    };
    match constraint {
        GenericConstraint::WithMessage(inner, message) => {
            GenericConstraint::WithMessage(Box::new(recurse(inner)), message.clone())
        }
        GenericConstraint::Conforms { param, trait_name } => GenericConstraint::Conforms {
            param: rename(param),
            trait_name: trait_name.clone(),
        },
        GenericConstraint::ConformsPack { param, trait_name } => GenericConstraint::ConformsPack {
            param: rename(param),
            trait_name: trait_name.clone(),
        },
        GenericConstraint::PackPredicate {
            param,
            predicate,
            all,
        } => GenericConstraint::PackPredicate {
            param: rename(param),
            predicate: predicate.clone(),
            all: *all,
        },
        GenericConstraint::PackContains { param, element } => GenericConstraint::PackContains {
            param: rename(param),
            element: operand(element),
        },
        GenericConstraint::Trivial(kind, inner) => {
            GenericConstraint::Trivial(*kind, operand(inner))
        }
        GenericConstraint::Eq(a, b) => GenericConstraint::Eq(operand(a), operand(b)),
        GenericConstraint::Ne(a, b) => GenericConstraint::Ne(operand(a), operand(b)),
        GenericConstraint::Lt(a, b) => GenericConstraint::Lt(operand(a), operand(b)),
        GenericConstraint::Le(a, b) => GenericConstraint::Le(operand(a), operand(b)),
        GenericConstraint::Gt(a, b) => GenericConstraint::Gt(operand(a), operand(b)),
        GenericConstraint::Ge(a, b) => GenericConstraint::Ge(operand(a), operand(b)),
        GenericConstraint::And(a, b) => {
            GenericConstraint::And(Box::new(recurse(a)), Box::new(recurse(b)))
        }
        GenericConstraint::Or(a, b) => {
            GenericConstraint::Or(Box::new(recurse(a)), Box::new(recurse(b)))
        }
        GenericConstraint::Not(inner) => GenericConstraint::Not(Box::new(recurse(inner))),
        GenericConstraint::Bool(value) => GenericConstraint::Bool(*value),
    }
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

/// Whether `name` is the bundled `FileDescriptor` struct (`std.io`), the only
/// accepted `file=` argument of `print`.
///
/// Its checked identity is the linker's module-qualified spelling of
/// `std/io/file_descriptor.mojo`; the bare name is the prelude spelling and
/// shares the other stdlib name lists' shadowing caveat.
pub fn is_stdlib_file_descriptor_struct(name: &str) -> bool {
    name == "FileDescriptor" || name.ends_with("file$descriptor$FileDescriptor")
}

/// Identity of one checked declaration, stable across the checked program.
///
/// Defined here (below the checked handoff) so symbol mangling can spell
/// declaration-qualified names without depending on the handoff crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CheckedDeclId(pub u32);

/// Whether `ty` still mentions something only an instantiation can resolve.
///
/// A type parameter, an associated or dependent projection, `Self`, or an
/// unsolved inference variable, at any depth, makes it symbolic: such a type
/// has no runtime identity — no native layout, and nothing the VM can dispatch
/// on. Origins erase from the runtime ABI, so they never make a type symbolic.
pub fn is_symbolic(ty: &Ty) -> bool {
    match ty {
        Ty::Infer | Ty::Param { .. } | Ty::Assoc { .. } | Ty::Dependent(_) | Ty::SelfType => true,
        Ty::Struct(_, arguments) => arguments.iter().any(|argument| match argument {
            TyArg::Ty(ty) => is_symbolic(ty),
            TyArg::Val(value) => ct_value_is_symbolic(value),
            TyArg::Origin(_) => false,
        }),
        Ty::Func {
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        }
        | Ty::GenericFunc {
            params,
            ret,
            variadic,
            kw_variadic,
            error,
            ..
        } => {
            params.iter().any(is_symbolic)
                || is_symbolic(ret)
                || variadic.as_deref().is_some_and(is_symbolic)
                || kw_variadic.as_deref().is_some_and(is_symbolic)
                || error.as_deref().is_some_and(is_symbolic)
        }
        Ty::Overload(types) | Ty::Tuple(types) | Ty::RuntimePack(types) | Ty::Variant(types) => {
            types.iter().any(is_symbolic)
        }
        Ty::ComptimeList(element) | Ty::VariadicPack(element) | Ty::Pointer { element, .. } => {
            is_symbolic(element)
        }
        Ty::Ref(reference) => is_symbolic(&reference.referent),
        Ty::Simd { dtype, width } => dtype.is_expr() || width.is_expr(),
        Ty::Dtype
        | Ty::Int
        | Ty::UInt
        | Ty::Bool
        | Ty::StringLiteral
        | Ty::Float64
        | Ty::None
        | Ty::Never
        | Ty::IntLiteral
        | Ty::FloatLiteral
        | Ty::Error => false,
    }
}

/// Whether `ty` mentions a parameter that nothing inside it binds.
///
/// This is the *closedness* question, which is not [`is_symbolic`]'s: a
/// generic callable value binds the parameters its own signature mentions, so
/// `def[T](T) -> T` is closed — it is a compile-time type constant, and
/// `DependentType::resolve` unwraps it — even though a `Ty::Param` named `T`
/// occurs inside it. A binder nested below another's signature is left to
/// [`is_symbolic`]'s conservative answer.
pub fn has_free_parameters(ty: &Ty) -> bool {
    let Ty::GenericFunc { decls, .. } = ty else {
        return is_symbolic(ty);
    };
    let bound: Vec<&ParamId> = decls.iter().map(ParamDecl::id).collect();
    mentions(ty, &|inner| match inner {
        Ty::Param { binder, .. } => !bound.contains(&&binder.id),
        Ty::Infer | Ty::Assoc { .. } | Ty::Dependent(_) | Ty::SelfType => true,
        _ => false,
    })
}

/// The [`is_symbolic`] companion for compile-time values: a value parameter, or
/// any collection or type handle carrying one.
pub fn ct_value_is_symbolic(value: &CtValue) -> bool {
    match value {
        CtValue::Expr(_) | CtValue::Deferred(_) => true,
        CtValue::Tuple(values)
        | CtValue::List(values)
        | CtValue::Set {
            elements: values, ..
        } => values.iter().any(ct_value_is_symbolic),
        CtValue::Dict { entries, .. } => entries
            .iter()
            .any(|(key, value)| ct_value_is_symbolic(key) || ct_value_is_symbolic(value)),
        CtValue::Type(ty) | CtValue::Reflected(ty) => is_symbolic(ty),
        CtValue::Struct { fields, .. } => {
            fields.iter().any(|(_, value)| ct_value_is_symbolic(value))
        }
        CtValue::Int(_)
        | CtValue::UInt(_)
        | CtValue::Float(_)
        | CtValue::IntLiteral(_)
        | CtValue::FloatLiteral(_)
        | CtValue::Bool(_)
        | CtValue::Dtype(_)
        | CtValue::Simd { .. }
        | CtValue::Str(_) => false,
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

fn rewrite_decl(decl: &ParamDecl, rewrite: &mut dyn TyRewrite) -> Result<ParamDecl, ParamError> {
    let boxed = |ty: &Option<Box<Ty>>, rewrite: &mut dyn TyRewrite| {
        ty.as_deref()
            .map(|ty| rewrite_ty(ty, rewrite).map(Box::new))
            .transpose()
    };
    Ok(match decl {
        ParamDecl::Type {
            id,
            name,
            bounds,
            callable_bound,
            default,
            infer_only,
            variadic,
            constraints,
        } => ParamDecl::Type {
            id: id.clone(),
            name: name.clone(),
            bounds: bounds.clone(),
            callable_bound: boxed(callable_bound, rewrite)?,
            default: boxed(default, rewrite)?,
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
            ty: Box::new(rewrite_ty(ty, rewrite)?),
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

#[cfg(test)]
mod simd_slot_tests {
    use super::*;
    use crate::param_expr::{MetaTy, ParamId};

    fn dtype_binder(context: &ParamContext) -> ParamExpr {
        context.decl_ref(ParamId::new("f", 0), "dt", MetaTy::value(Ty::Dtype))
    }

    fn width_binder(context: &ParamContext) -> ParamExpr {
        context.decl_ref(ParamId::new("f", 1), "width", MetaTy::int())
    }

    #[test]
    fn closed_slots_fold_to_known_and_canonicalize() {
        let context = ParamContext::detached();
        let float64 = context
            .constant(CtValue::Dtype(Dtype::Float64))
            .expect("a constant");
        assert_eq!(
            simd_ty_from_slots(SimdDtype::Expr(float64), SimdWidth::Known(1)).expect("closed"),
            Ty::Float64
        );
        let one = context.constant(CtValue::Int(1)).expect("a constant");
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
        assert!(splats_to(&Ty::IntLiteral, &lane));
        assert!(splats_to(&Ty::FloatLiteral, &lane));
        for scalar in [
            Ty::Int,
            Ty::Float64,
            Ty::Bool,
            canonical_simd_ty(Dtype::Int32, 1),
        ] {
            assert!(!splats_to(&scalar, &lane), "{scalar}");
        }
        let same = Ty::Simd {
            dtype: lane.clone(),
            width: SimdWidth::Known(1),
        };
        assert!(splats_to(&same, &lane));
        assert!(coerces(&Ty::FloatLiteral, &same));
        assert!(!coerces(&same, &Ty::Float64));
        let vector = Ty::Simd {
            dtype: lane.clone(),
            width: SimdWidth::Expr(width_binder(&context)),
        };
        assert!(!coerces(&vector, &canonical_simd_ty(Dtype::Int32, 4)));
        assert!(coerces(
            &vector,
            &Ty::Simd {
                dtype: lane,
                width: SimdWidth::Known(-1),
            }
        ));
    }

    #[test]
    fn replacement_closes_symbolic_slots_through_the_canonical_form() {
        let context = ParamContext::detached();
        let scalar = Ty::Simd {
            dtype: SimdDtype::Expr(dtype_binder(&context)),
            width: SimdWidth::Expr(width_binder(&context)),
        };
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
            Ty::Float64
        );
    }
}

#[cfg(test)]
mod collection_representation_tests {
    use super::*;

    #[test]
    fn public_collection_helpers_construct_only_nominal_types() {
        let runtime_list = list_type(Ty::Int);
        for ty in [
            runtime_list.clone(),
            set_type(Ty::Int),
            dict_type(Ty::StringLiteral, Ty::Int),
            range_type(),
        ] {
            assert!(matches!(ty, Ty::Struct(..)), "got {ty:?}");
        }
        assert_ne!(runtime_list, Ty::ComptimeList(Box::new(Ty::Int)));
        assert_eq!(
            Ty::ComptimeList(Box::new(Ty::Int)).to_string(),
            "<comptime-list[Int]>"
        );
    }

    #[test]
    fn uninit_storage_element_recognizes_every_mangled_spelling() {
        let storage = |name: &str| Ty::Struct(name.to_string(), vec![TyArg::Ty(Ty::Int)]);
        for name in [
            "__UninitStorage",
            "mono_test$__UninitStorage",
            "__UninitStorage$mono$TInt",
            "mono_test$__UninitStorage$mono$TRecorder",
        ] {
            assert_eq!(
                uninit_storage_element(&storage(name)),
                Some(&Ty::Int),
                "{name}"
            );
        }
        assert_eq!(uninit_storage_element(&storage("Storageish")), None);
        assert_eq!(
            uninit_storage_element(&Ty::Struct("__UninitStorage".into(), vec![])),
            None
        );
    }
}

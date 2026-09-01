//! Shared semantic type representation.
//!
//! This is the type lattice used by the checker, but it also needs to be visible
//! to compile-time values once comptime can carry type values. Keeping `Ty` out
//! of `checker.rs` lets [`CtValue`](crate::ct::CtValue) represent `Type(Box<Ty>)`
//! without making the checker the owner of all type-level facts.

use std::fmt;

use crate::ast::{ArgConvention, Dtype};
use crate::ct::{CtExpr, CtValue};

/// Descriptor type selected for a slice literal at the checked boundary.
/// Two-component literals can use the view-oriented contiguous descriptor;
/// literals with a second colon use the owning strided descriptor. `Slice` is
/// the general protocol fallback accepted by user-defined collections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliceKind {
    Slice,
    ContiguousSlice,
    StridedSlice,
}

impl SliceKind {
    pub fn type_name(self) -> &'static str {
        match self {
            Self::Slice => "Slice",
            Self::ContiguousSlice => "ContiguousSlice",
            Self::StridedSlice => "StridedSlice",
        }
    }
}

/// A checked type expression whose final member is selected by compile-time
/// evaluation. Candidate types and the canonical [`CtExpr`] remain structural
/// semantic data; no phase has to encode or recover this operation from a
/// synthesized name.
///
/// The enum leaves room for future dependent projection forms without making
/// them special cases in the nominal type namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependentType {
    /// Index a finite, already-checked sequence of types.
    Indexed { elements: Vec<Ty>, index: CtExpr },
}

/// A type in mojito's semantic lattice. Scalars mirror `ast::Type`; `Func` is
/// synthesized from a `def` signature or lowered from a function-type annotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    Int,
    UInt,
    Bool,
    /// The compile-time string literal type (Mojo's `StringLiteral`). The
    /// nominal runtime `String` is the self-hosted stdlib struct; until the
    /// annotation takeover lands, source `String` annotations still resolve
    /// here.
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
        params: Vec<Ty>,
        names: Vec<String>,
        ret: Box<Ty>,
        required: Vec<bool>,
        variadic: Option<Box<Ty>>,
        /// Homogeneous element type collected by `**kwargs`, when present.
        kw_variadic: Option<Box<Ty>>,
        positional_only: Option<usize>,
        keyword_only: Option<usize>,
        raises: bool,
        error: Option<Box<Ty>>,
        /// The argument convention of each regular parameter.
        conventions: Vec<Option<ArgConvention>>,
        ref_params: Box<Vec<Option<crate::origin::RefSig>>>,
        ref_return: Option<Box<crate::origin::RefSig>>,
        /// Identity-transparent inferred transfer effects; see
        /// [`crate::checked::TransferSet`].
        transfers: crate::checked::TransferSet,
    },
    /// A generic function synthesized from a `def` with a `[params]` list.
    GenericFunc {
        environment: crate::origin::CallableEnvironment,
        decls: Vec<ParamDecl>,
        params: Vec<Ty>,
        names: Vec<String>,
        ret: Box<Ty>,
        required: Vec<bool>,
        variadic: Option<Box<Ty>>,
        /// Homogeneous element type collected by `**kwargs`, when present.
        kw_variadic: Option<Box<Ty>>,
        positional_only: Option<usize>,
        keyword_only: Option<usize>,
        raises: bool,
        error: Option<Box<Ty>>,
        conventions: Vec<Option<ArgConvention>>,
        ref_params: Box<Vec<Option<crate::origin::RefSig>>>,
        ref_return: Option<Box<crate::origin::RefSig>>,
        /// Identity-transparent inferred transfer effects; see
        /// [`crate::checked::TransferSet`].
        transfers: crate::checked::TransferSet,
    },
    /// A source name that denotes multiple callable signatures. The checker
    /// resolves an overload set at each call site. The first implementation
    /// supports distinct call shapes/arity; keeping this as a first-class type
    /// leaves type-ranked overload resolution as a natural extension.
    Overload(Vec<Ty>),
    /// A type parameter (`T`) inside a generic body, carrying its trait bounds.
    Param {
        name: String,
        bounds: Vec<String>,
        /// Anonymous callable-trait contract from a declaration such as
        /// `F: def(T) -> T`. Unlike an ordinary trait name, the full checked
        /// signature is needed both to validate specializations and to type
        /// calls through `F` inside the generic body.
        callable_bound: Option<Box<Ty>>,
    },
    /// A symbolic associated type lookup such as `C.Element` where `C` is an
    /// opaque type parameter. It may resolve to a concrete type once `C` is
    /// substituted at a generic use site. `args` is the parameter application of
    /// a parameterized associated type (`C.IteratorType[o]`); it is empty for a
    /// bare `C.Element`. The arguments are retained so the projection can be
    /// resolved concretely once the base is a conforming struct.
    Assoc {
        base: Box<Ty>,
        name: String,
        args: Vec<TyArg>,
    },
    /// Structured dependent type metadata. Generic declarations may retain it,
    /// but executable uses must substitute its index to a concrete type.
    Dependent(DependentType),
    /// `Self` inside a trait method requirement.
    SelfType,
    /// A nominal struct type, named, with its parameter arguments.
    Struct(String, Vec<TyArg>),
    /// A SIMD vector type `SIMD[DType.<dtype>, width]`.
    Simd {
        dtype: Dtype,
        width: i64,
    },
    /// The built-in `Error` type.
    Error,
    /// Compile-time-only list shape used while materializing `CtValue::List`.
    /// Checked executable List values use `Struct("List", ...)`.
    ComptimeList(Box<Ty>),
    /// Compiler-private `__RuntimeTuple[T1, ..., Tn]` storage. Public
    /// `Tuple[T1, ..., Tn]` values are nominal standard-library structs.
    Tuple(Vec<Ty>),
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
    VariadicPack(Box<Ty>),
    /// The built-in tagged union `Variant[T1, ..., Tn]`.  The ordering is part
    /// of the type: it determines the runtime tag used by typed projection.
    Variant(Vec<Ty>),
    /// The built-in `UnsafePointer[T, origin]`.  The VM erases the origin, but
    /// the checked and MIR types retain it for lifetime/aggregate validation.
    Pointer {
        element: Box<Ty>,
        origin: crate::origin::PointerOrigin,
    },
    /// A reference value. Origins and permissions are checked statically; its
    /// runtime representation is introduced only after loan checking exists.
    Ref(crate::origin::RefTy),
}

pub const ARRAY_TYPE_NAME: &str = "Array";

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

/// Decompose a checker-abstract scalar-range type — `Ty::Struct` naming a
/// [`SCALAR_RANGE_FAMILY`] member (plain or module-qualified) with one
/// concrete dtype value argument. This form exists only in the discovery
/// round: the specialization fixpoint rewrites every occurrence into a
/// registered concrete struct before MIR lowering.
pub fn scalar_range_parts(ty: &Ty) -> Option<(&'static str, crate::ast::Dtype)> {
    let Ty::Struct(name, arguments) = ty else {
        return None;
    };
    let family = SCALAR_RANGE_FAMILY
        .iter()
        .find(|family| *family == name || name.ends_with(&format!("${family}")))?;
    let [TyArg::Val(CtValue::Dtype(dtype))] = arguments.as_slice() else {
        return None;
    };
    Some((family, *dtype))
}

pub const OPTIONAL_TYPE_NAME: &str = "Optional";

/// Compiler-private inline possibly-uninitialized storage, the field type of
/// `MaybeUninit`. An unregistered nominal: resolvable only from bundled
/// standard-library sources, with every capability special-cased explicitly.
pub const UNINIT_STORAGE_TYPE_NAME: &str = "__UninitStorage";

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
/// whose `length` is still the symbolic `CtValue::Param`.
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
/// backend-monomorphized (`…$mono$…`) instantiations — mono renames the
/// struct while keeping its substituted argument list, so the payload stays
/// recoverable from the arguments.
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

/// The interleaved element types of a lazy template string, accepting both the
/// public `TString` spelling and the concrete symbols emitted for its variadic
/// specializations (the same acceptance rule as [`tuple_elements`]).
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

pub fn contains_infer(ty: &Ty) -> bool {
    match ty {
        Ty::Infer => true,
        Ty::Struct(_, arguments) => arguments.iter().any(|argument| match argument {
            TyArg::Ty(ty) => contains_infer(ty),
            TyArg::Val(_) | TyArg::Origin(_) => false,
        }),
        Ty::ComptimeList(element) | Ty::VariadicPack(element) | Ty::Pointer { element, .. } => {
            contains_infer(element)
        }
        Ty::Dependent(DependentType::Indexed { elements, .. }) => {
            elements.iter().any(contains_infer)
        }
        Ty::Tuple(elements) | Ty::RuntimePack(elements) | Ty::Variant(elements) => {
            elements.iter().any(contains_infer)
        }
        Ty::Assoc { base, args, .. } => {
            contains_infer(base)
                || args.iter().any(|argument| match argument {
                    TyArg::Ty(ty) => contains_infer(ty),
                    TyArg::Val(_) | TyArg::Origin(_) => false,
                })
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
            params.iter().any(contains_infer)
                || contains_infer(ret)
                || variadic.as_deref().is_some_and(contains_infer)
                || kw_variadic.as_deref().is_some_and(contains_infer)
        }
        Ty::Overload(candidates) => candidates.iter().any(contains_infer),
        _ => false,
    }
}

/// A declared compile-time parameter of a generic `struct`/`def`, classified
/// from `[name: X]` by whether `X` is a trait or a type.
/// Whether a type parameter's bound admits nullary construction (`H()`):
/// such parameters are reified at runtime as the bound struct's name so an
/// erased body can construct them.
pub fn constructible_type_parameter(declaration: &ParamDecl) -> bool {
    matches!(
        declaration,
        ParamDecl::Type { bounds, .. }
            if bounds.iter().any(|bound| matches!(bound.as_str(), "Hasher" | "Defaultable"))
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamDecl {
    /// A type parameter `T: Trait & ...`.
    Type {
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
        name: String,
        ty: Box<Ty>,
        default: Option<CtExpr>,
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallableDefault {
    Symbol(String),
    Parameter(String),
    If {
        condition: CtExpr,
        then_value: Box<CallableDefault>,
        else_value: Box<CallableDefault>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstraintOperand {
    Param(String),
    Value(CtValue),
    Type(Ty),
    /// `TypeList[Ts.values]().length` over a symbolic pack parameter,
    /// resolving to the bound pack's element count.
    PackLength(String),
}

/// The per-element predicate of a `TypeList` `any`/`all` proposition: a
/// builtin `IsTrivially*` spelling or a Bool-bodied predicate alias with one
/// type parameter, applied to each element of the bound pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackPredicateRef {
    Trivial(TrivialLifecycle),
    Alias(String),
}

/// The lifecycle facet queried by the
/// `IsTrivially{Movable,Copyable,Deinitable}[T]` comptime predicates: the type
/// conforms to `TrivialRegisterPassable`, or the base capability holds and the
/// corresponding lifecycle operation is compiler-generated with recursively
/// trivial fields (a bitwise move/copy or a no-op destructor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrivialLifecycle {
    Movable,
    Copyable,
    Deinitable,
}

/// Recognize an `IsTrivially*` comptime-predicate name. These are Bool-valued
/// predicates, not traits: they are valid in `where` clauses, conformance
/// conditions, and `comptime if`, but not as type-parameter bounds.
pub fn trivial_predicate_name(name: &str) -> Option<TrivialLifecycle> {
    match name {
        "IsTriviallyMovable" => Some(TrivialLifecycle::Movable),
        "IsTriviallyCopyable" => Some(TrivialLifecycle::Copyable),
        "IsTriviallyDeinitable" => Some(TrivialLifecycle::Deinitable),
        _ => None,
    }
}

/// The predicate spelling of a [`TrivialLifecycle`] facet — the inverse of
/// [`trivial_predicate_name`], used to record a `where IsTrivially*[T]` fact
/// as a body-side assumption and to look it up during capability queries.
pub fn trivial_predicate_spelling(kind: TrivialLifecycle) -> &'static str {
    match kind {
        TrivialLifecycle::Movable => "IsTriviallyMovable",
        TrivialLifecycle::Copyable => "IsTriviallyCopyable",
        TrivialLifecycle::Deinitable => "IsTriviallyDeinitable",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GenericConstraint {
    /// A top-level `where (condition, "message")` clause. The message affects
    /// only the failed-specialization diagnostic; semantic operations recurse
    /// through the wrapped condition.
    WithMessage(Box<GenericConstraint>, String),
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
    And(Box<GenericConstraint>, Box<GenericConstraint>),
    Or(Box<GenericConstraint>, Box<GenericConstraint>),
    Not(Box<GenericConstraint>),
    Bool(bool),
}

impl fmt::Display for GenericConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GenericConstraint::WithMessage(inner, message) => {
                write!(f, "({inner}, {message:?})")
            }
            GenericConstraint::Conforms { param, trait_name } => {
                write!(f, "conforms_to({param}, {trait_name})")
            }
            GenericConstraint::ConformsPack { param, trait_name } => {
                write!(f, "conforms_to({param}.values, {trait_name})")
            }
            GenericConstraint::PackPredicate {
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
            GenericConstraint::PackContains { param, element } => {
                write!(f, "TypeList[{param}.values]().contains[{element}]()")
            }
            GenericConstraint::Trivial(kind, operand) => {
                write!(f, "{}[{operand}]", trivial_predicate_spelling(*kind))
            }
            GenericConstraint::Eq(a, b) => write!(f, "{a} == {b}"),
            GenericConstraint::Ne(a, b) => write!(f, "{a} != {b}"),
            GenericConstraint::Lt(a, b) => write!(f, "{a} < {b}"),
            GenericConstraint::Le(a, b) => write!(f, "{a} <= {b}"),
            GenericConstraint::Gt(a, b) => write!(f, "{a} > {b}"),
            GenericConstraint::Ge(a, b) => write!(f, "{a} >= {b}"),
            GenericConstraint::And(a, b) => write!(f, "{a} and {b}"),
            GenericConstraint::Or(a, b) => write!(f, "{a} or {b}"),
            GenericConstraint::Not(inner) => write!(f, "not {inner}"),
            GenericConstraint::Bool(value) => {
                write!(f, "{}", if *value { "True" } else { "False" })
            }
        }
    }
}

impl fmt::Display for ConstraintOperand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConstraintOperand::Param(name) => write!(f, "{name}"),
            ConstraintOperand::Value(value) => write!(f, "{value}"),
            ConstraintOperand::Type(ty) => write!(f, "{ty}"),
            ConstraintOperand::PackLength(name) => {
                write!(f, "TypeList[{name}.values]().length")
            }
        }
    }
}

impl ParamDecl {
    pub fn name(&self) -> &str {
        match self {
            ParamDecl::Type { name, .. } | ParamDecl::Value { name, .. } => name,
        }
    }
}

/// One argument in a struct type's parameter list: a type, a compile-time value,
/// or an origin. Part of a struct type's identity, so `FixedBuffer[8] !=
/// FixedBuffer[9]`. Origins participate in checked identity but erase from the
/// runtime ABI, exactly like `Ty::Pointer` origins — a parameterized iterator's
/// `origin` argument distinguishes checked types without changing lowering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TyArg {
    Ty(Ty),
    Val(CtValue),
    Origin(crate::origin::Origin),
}

impl TyArg {
    /// The compile-time value this argument binds in a CTFE/elaboration
    /// scope. Origins erase from runtime state and bind no value.
    pub(crate) fn ct_value(&self) -> Option<CtValue> {
        match self {
            TyArg::Ty(ty) => Some(CtValue::Type(Box::new(ty.clone()))),
            TyArg::Val(value) => Some(value.clone()),
            TyArg::Origin(_) => None,
        }
    }
}

impl fmt::Display for TyArg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TyArg::Ty(t) => write!(f, "{}", t),
            TyArg::Val(v) => write!(f, "{}", v),
            TyArg::Origin(o) => write!(f, "{}", o),
        }
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Int | Ty::IntLiteral => write!(f, "Int"),
            Ty::UInt => write!(f, "UInt"),
            Ty::Bool => write!(f, "Bool"),
            Ty::StringLiteral => write!(f, "StringLiteral"),
            Ty::Float64 | Ty::FloatLiteral => write!(f, "Float64"),
            Ty::Infer => write!(f, "_"),
            Ty::Dtype => write!(f, "DType"),
            Ty::None => write!(f, "None"),
            Ty::Never => write!(f, "Never"),
            Ty::Func {
                environment,
                params,
                ret,
                raises,
                ..
            }
            | Ty::GenericFunc {
                environment,
                params,
                ret,
                raises,
                ..
            } => {
                // A generic contract renders its binders and trailing `where`
                // constraints so a constrained-vs-unconstrained mismatch is
                // visible in diagnostics.
                if let Ty::GenericFunc { decls, .. } = self {
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
                    write!(f, "{}", p)?;
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
                                write!(f, "origin_set#{}", id.0)?
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
                                            write!(f, "origin#{}", id.0)?
                                        }
                                        crate::origin::Origin::Place(place) => {
                                            write!(f, "origin@{}", place.root.0)?
                                        }
                                        crate::origin::Origin::SelfParam => {
                                            write!(f, "origin_of(self)")?
                                        }
                                        crate::origin::Origin::Static => write!(f, "static")?,
                                        crate::origin::Origin::Untracked { mutable: true } => {
                                            write!(f, "mut-untracked")?
                                        }
                                        crate::origin::Origin::Untracked { mutable: false } => {
                                            write!(f, "immut-untracked")?
                                        }
                                        crate::origin::Origin::Union(_) => {
                                            write!(f, "origin-union")?
                                        }
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
                write!(f, " -> {}", ret)?;
                if let Ty::GenericFunc { decls, .. } = self {
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
            Ty::Overload(candidates) => {
                write!(f, "overload(")?;
                for (i, candidate) in candidates.iter().enumerate() {
                    if i > 0 {
                        write!(f, " | ")?;
                    }
                    write!(f, "{}", candidate)?;
                }
                write!(f, ")")
            }
            Ty::Param { name, .. } => write!(f, "{}", name),
            Ty::Assoc { base, name, args } => {
                write!(f, "{}.{}", base, name)?;
                if !args.is_empty() {
                    write!(f, "[")?;
                    for (position, argument) in args.iter().enumerate() {
                        if position > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{}", argument)?;
                    }
                    write!(f, "]")?;
                }
                Ok(())
            }
            Ty::Dependent(DependentType::Indexed { elements, index }) => {
                write!(f, "type_sequence[")?;
                for (position, element) in elements.iter().enumerate() {
                    if position > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{element}")?;
                }
                write!(f, "][{index:?}]")
            }
            Ty::SelfType => write!(f, "Self"),
            Ty::Simd { dtype, width: 1 } => match dtype.scalar_alias() {
                Some(alias) => write!(f, "{}", alias),
                None => write!(f, "SIMD[DType.{}, 1]", dtype.name()),
            },
            Ty::Simd { dtype, width } => write!(f, "SIMD[DType.{}, {}]", dtype.name(), width),
            Ty::Error => write!(f, "Error"),
            Ty::Pointer { element, origin } => {
                write!(f, "Pointer[{element}")?;
                match origin {
                    crate::origin::PointerOrigin::Place { place, .. } => {
                        write!(f, ", origin@{}", place.root.0)?
                    }
                    crate::origin::PointerOrigin::Param { id, .. } => {
                        write!(f, ", origin#{}", id.0)?
                    }
                    crate::origin::PointerOrigin::SelfPlace { .. } => {
                        write!(f, ", origin_of(self)")?
                    }
                    crate::origin::PointerOrigin::Static => write!(f, ", ImmStaticOrigin")?,
                    crate::origin::PointerOrigin::Untracked { mutable: true } => {
                        write!(f, ", MutUntrackedOrigin")?
                    }
                    crate::origin::PointerOrigin::Untracked { mutable: false } => {
                        write!(f, ", ImmUntrackedOrigin")?
                    }
                    crate::origin::PointerOrigin::UnsafeAny { mutable: true } => {
                        write!(f, ", MutUnsafeAnyOrigin")?
                    }
                    crate::origin::PointerOrigin::UnsafeAny { mutable: false } => {
                        write!(f, ", ImmUnsafeAnyOrigin")?
                    }
                }
                write!(f, "]")
            }
            Ty::Ref(reference) => write!(f, "ref {}", reference.referent),
            Ty::ComptimeList(elem) => write!(f, "<comptime-list[{elem}]>"),
            Ty::Tuple(elems) => {
                write!(f, "Tuple[")?;
                for (i, t) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", t)?;
                }
                write!(f, "]")
            }
            Ty::RuntimePack(elems) => {
                write!(f, "$pack[")?;
                for (i, t) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{t}")?;
                }
                write!(f, "]")
            }
            Ty::VariadicPack(element) => write!(f, "$variadic[{element}]"),
            Ty::Variant(alternatives) => {
                write!(f, "Variant[")?;
                for (i, ty) in alternatives.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", ty)?;
                }
                write!(f, "]")
            }
            Ty::Struct(name, args) => {
                // The prelude-qualified nominal String prints its public
                // spelling rather than leaking the module-qualified symbol.
                if args.is_empty() && crate::symbol::is_stdlib_string_struct(name) {
                    return write!(f, "String");
                }
                write!(f, "{}", name)?;
                if !args.is_empty() {
                    write!(f, "[")?;
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{}", a)?;
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

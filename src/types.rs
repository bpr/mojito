//! Shared semantic type representation.
//!
//! This is the type lattice used by the checker, but it also needs to be visible
//! to compile-time values once comptime can carry type values. Keeping `Ty` out
//! of `checker.rs` lets [`CtValue`](crate::ct::CtValue) represent `Type(Box<Ty>)`
//! without making the checker the owner of all type-level facts.

use std::collections::HashMap;
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

/// A loan-transfer effect inferred from a callable's body: an accepted store
/// into an outliving destination (`self` or a parameter) whose loan roots at
/// another parameter or `self`. Call sites replay the effect against their
/// actuals, installing the caller-side loan the callee's store implies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferEffect {
    pub dest: crate::origin::SigOrigin,
    pub src: crate::origin::SigOrigin,
    /// Whether the loan roots at the source parameter's own (borrowed)
    /// storage — a `mut`/`ref` actual's place is loaned at the call — as
    /// opposed to loans merely carried by an owned value moving through.
    pub src_is_place: bool,
    pub mutable: bool,
}

/// Inferred transfer effects riding a checked function type, so a call
/// through a function-typed VALUE replays the effects of the `def` the value
/// came from. Transparent to type identity: two otherwise-equal function
/// types never differ by their inferred effects, and acceptance/coercion
/// must not consult them — a `def(...)` contract cannot spell effects (Mojo
/// has no such syntax), so soundness comes from call-site replay off the
/// value's type, never from acceptance filtering.
#[derive(Debug, Clone, Default, Eq)]
pub struct TransferSet(pub Vec<TransferEffect>);

impl TransferSet {
    /// Iterate the canonical transfer effects retained by a callable type.
    pub fn iter(&self) -> impl Iterator<Item = &TransferEffect> {
        self.0.iter()
    }
}

impl PartialEq for TransferSet {
    /// Always equal BY DESIGN: the set is metadata on the type, not part of
    /// its identity. See the type-level comment before relying on `==`.
    fn eq(&self, _other: &Self) -> bool {
        true
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
        /// [`TransferSet`].
        transfers: TransferSet,
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
        /// [`TransferSet`].
        transfers: TransferSet,
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
                if args.is_empty() && is_stdlib_string_struct(name) {
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

/// The checker's value-coercion predicate, shared with MIR verification so the
/// verifier never re-derives conversion rules.
pub fn value_coerces(from: &Ty, to: &Ty) -> bool {
    coerces(from, to)
}

/// Whether a value of type `from` can be used where `to` is required. Only the
/// literal types coerce (to the concrete numeric types, or `IntLiteral` up to
/// `FloatLiteral`); everything else must match exactly.
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
        (Ty::Param { name: a, .. }, Ty::Param { name: b, .. }) => a == b,
        (Ty::Struct(an, aargs), Ty::Struct(bn, bargs)) => {
            an == bn
                && aargs.len() == bargs.len()
                && aargs.iter().zip(bargs).all(|(a, b)| match (a, b) {
                    (TyArg::Ty(a), TyArg::Ty(b)) => coerces(a, b),
                    (TyArg::Val(a), TyArg::Val(b)) => a == b,
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
        ) => coerces(a, b) && ao == bo,
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
        (literal, Ty::Simd { dtype, width: 1 }) if splats_to(literal, *dtype) => true,
        (
            Ty::Simd {
                dtype: from_dtype,
                width: from_width,
            },
            Ty::Simd {
                dtype: to_dtype,
                width: -1,
            },
        ) => from_dtype == to_dtype && *from_width > 0,
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
/// argument, or the non-SIMD operand of an elementwise operator that splats). A
/// numeric literal fits any matching-kind lane; a same-dtype width-1 SIMD fits.
pub fn splats_to(ty: &Ty, dtype: Dtype) -> bool {
    match ty {
        Ty::IntLiteral => dtype != Dtype::Bool,
        Ty::FloatLiteral => dtype.is_float(),
        Ty::Bool => dtype == Dtype::Bool,
        Ty::Int => dtype == Dtype::Int,
        // `Float64` is `SIMD[DType.float64, 1]`, so it splats into a float64 vector.
        Ty::Float64 => dtype == Dtype::Float64,
        Ty::Simd { dtype: d, width: 1 } => *d == dtype,
        _ => false,
    }
}

/// The value-coercion policy for callable environments: current Mojo rejects
/// binding a capturing closure to an unqualified `def(...)` value position —
/// the contract must spell `capturing[...]` — while a thin function value
/// still binds. Comptime callable *bounds* stay on the permissive
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
/// set the VM can hash directly (`Int`/`UInt`/`Bool`/`String`/`Float64`). This
/// lets a user key struct combine `self.field.__hash__()` values.
/// Whether `Copyable.copy` on a value of this type has no callee: built-in
/// scalars, literals, tuples, packs, and variants copy by the ordinary value
/// read. Nominal, parametric, associated, and reference types resolve their
/// `copy` through declarations or trait dispatch instead.
pub fn builtin_copy_is_value_read(ty: &Ty) -> bool {
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
/// anonymous `def(...)` trait contract. This is intentionally directional:
/// non-raising/read-only implementations may fulfill raising/mutable contracts,
/// but not vice versa. Binder constraints are directional the other way
/// (upstream 2026-08): every `where` constraint the implementation declares
/// must be declared by the contract — otherwise calls through the contract
/// could violate the implementation's precondition — while an unconstrained
/// implementation may serve a constrained contract.
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
                    constraints.clear()
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
            (None, _) | (Some(Ty::Never), _) => true,
            (Some(_), None) => false,
            (Some(actual), Some(Ty::Error)) => actual != &Ty::Never,
            (Some(actual), Some(contract)) => actual == contract,
        }
}

/// Alpha-normalize a generic anonymous callable into its declaration list and
/// a monomorphic callable shape whose parameter occurrences use canonical
/// `$N` names.  Generic callable compatibility can then reuse the ordinary
/// directional callable-contract rules without making source binder spelling
/// part of the type identity.
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
    let mut subst = HashMap::new();
    let mut value_names = HashMap::new();
    let canonical_decls = decls
        .iter()
        .enumerate()
        .map(|(index, decl)| match decl {
            ParamDecl::Type {
                name,
                bounds,
                callable_bound,
                default: _,
                infer_only: _,
                variadic,
                constraints,
            } => {
                let canonical_name = format!("${index}");
                let canonical_callable_bound = callable_bound.as_ref().map(|bound| {
                    Box::new(rename_dependent_parameters(
                        &substitute(bound, &subst),
                        &value_names,
                    ))
                });
                subst.insert(
                    name.clone(),
                    Ty::Param {
                        name: canonical_name.clone(),
                        bounds: bounds.clone(),
                        callable_bound: canonical_callable_bound.clone(),
                    },
                );
                ParamDecl::Type {
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
                name,
                ty,
                default: _,
                callable_default: _,
                infer_only: _,
                variadic,
                constraints,
                ..
            } => {
                let canonical_name = format!("${index}");
                let canonical_ty =
                    rename_dependent_parameters(&substitute(ty, &subst), &value_names);
                value_names.insert(
                    name.trim_start_matches('*').to_string(),
                    canonical_name.clone(),
                );
                ParamDecl::Value {
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
        .map(|ty| rename_dependent_parameters(&substitute(ty, &subst), &value_names))
        .collect();
    // Second pass: alpha-rename binder references INSIDE the retained
    // constraints, so `def[w: Int](…) where w > 0` and `def[n: Int](…) where
    // n > 0` share one canonical identity. The maps are complete only after
    // the fold above (a clause on the last binder may reference any of them);
    // names the contract does not bind (an enclosing declaration's
    // parameters) stay as-is and correctly distinguish contracts.
    let mut binder_names: HashMap<String, String> = value_names.clone();
    for (name, ty) in &subst {
        if let Ty::Param {
            name: canonical, ..
        } = ty
        {
            binder_names.insert(name.clone(), canonical.clone());
        }
    }
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
                            &value_names,
                        )
                    })
                    .collect();
            }
        }
    }
    (canonical_decls, canonical_params)
}

/// Replace every `Ty::Param` in `ty` with its solution from `subst` (leaving an
/// unsolved parameter untouched). Recurses into struct type arguments.
pub fn substitute(ty: &Ty, subst: &HashMap<String, Ty>) -> Ty {
    match ty {
        Ty::Param {
            name,
            bounds,
            callable_bound,
        } => subst.get(name).cloned().unwrap_or_else(|| Ty::Param {
            name: name.clone(),
            bounds: bounds.clone(),
            callable_bound: callable_bound
                .as_ref()
                .map(|bound| Box::new(substitute(bound, subst))),
        }),
        Ty::Struct(name, args) => {
            Ty::Struct(name.clone(), map_tyargs(args, |t| substitute(t, subst)))
        }
        Ty::Dependent(crate::types::DependentType::Indexed { elements, index }) => {
            Ty::Dependent(crate::types::DependentType::Indexed {
                elements: elements.iter().map(|ty| substitute(ty, subst)).collect(),
                index: index.clone(),
            })
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
                nested.remove(declaration.name());
            }
            let decls = decls
                .iter()
                .map(|declaration| match declaration {
                    ParamDecl::Type {
                        name,
                        bounds,
                        callable_bound,
                        default,
                        infer_only,
                        variadic,
                        constraints,
                    } => ParamDecl::Type {
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
                        name,
                        ty,
                        default,
                        callable_default,
                        infer_only,
                        variadic,
                        constraints,
                    } => ParamDecl::Value {
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

/// Alpha-rename compile-time value binders referenced by structural dependent
/// types. Type-parameter substitution and value-parameter renaming are kept
/// separate: a value binder occurs inside [`CtExpr`], never as `Ty::Param`.
/// Nested generic callable declarations shadow an outer binder of the same
/// spelling, so only genuinely free references are renamed while descending.
pub fn rename_dependent_parameters(ty: &Ty, names: &HashMap<String, String>) -> Ty {
    match ty {
        Ty::Param {
            name,
            bounds,
            callable_bound,
        } => Ty::Param {
            name: name.clone(),
            bounds: bounds.clone(),
            callable_bound: callable_bound
                .as_ref()
                .map(|bound| Box::new(rename_dependent_parameters(bound, names))),
        },
        Ty::Struct(name, arguments) => Ty::Struct(
            name.clone(),
            map_tyargs(arguments, |ty| rename_dependent_parameters(ty, names)),
        ),
        Ty::Dependent(crate::types::DependentType::Indexed { elements, index }) => {
            Ty::Dependent(crate::types::DependentType::Indexed {
                elements: elements
                    .iter()
                    .map(|ty| rename_dependent_parameters(ty, names))
                    .collect(),
                index: index.rename_parameters(names),
            })
        }
        Ty::ComptimeList(element) => {
            Ty::ComptimeList(Box::new(rename_dependent_parameters(element, names)))
        }
        Ty::Tuple(elements) => Ty::Tuple(
            elements
                .iter()
                .map(|ty| rename_dependent_parameters(ty, names))
                .collect(),
        ),
        Ty::RuntimePack(elements) => Ty::RuntimePack(
            elements
                .iter()
                .map(|ty| rename_dependent_parameters(ty, names))
                .collect(),
        ),
        Ty::VariadicPack(element) => {
            Ty::VariadicPack(Box::new(rename_dependent_parameters(element, names)))
        }
        Ty::Variant(alternatives) => Ty::Variant(
            alternatives
                .iter()
                .map(|ty| rename_dependent_parameters(ty, names))
                .collect(),
        ),
        Ty::Pointer { element, origin } => Ty::Pointer {
            element: Box::new(rename_dependent_parameters(element, names)),
            origin: origin.clone(),
        },
        Ty::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = Box::new(rename_dependent_parameters(&reference.referent, names));
            Ty::Ref(reference)
        }
        Ty::Assoc { base, name, args } => Ty::Assoc {
            base: Box::new(rename_dependent_parameters(base, names)),
            name: name.clone(),
            args: map_tyargs(args, |t| rename_dependent_parameters(t, names)),
        },
        Ty::Func {
            environment,
            params,
            names: parameter_names,
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
            params: params
                .iter()
                .map(|ty| rename_dependent_parameters(ty, names))
                .collect(),
            names: parameter_names.clone(),
            ret: Box::new(rename_dependent_parameters(ret, names)),
            required: required.clone(),
            variadic: variadic
                .as_ref()
                .map(|ty| Box::new(rename_dependent_parameters(ty, names))),
            kw_variadic: kw_variadic
                .as_ref()
                .map(|ty| Box::new(rename_dependent_parameters(ty, names))),
            positional_only: *positional_only,
            keyword_only: *keyword_only,
            raises: *raises,
            error: error
                .as_ref()
                .map(|ty| Box::new(rename_dependent_parameters(ty, names))),
            conventions: conventions.clone(),
            ref_params: ref_params.clone(),
            ref_return: ref_return.clone(),
            transfers: transfers.clone(),
        },
        Ty::GenericFunc {
            environment,
            decls,
            params,
            names: parameter_names,
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
            let mut free_names = names.clone();
            for declaration in decls {
                free_names.remove(declaration.name().trim_start_matches('*'));
            }
            let decls = decls
                .iter()
                .map(|declaration| match declaration {
                    ParamDecl::Type {
                        name,
                        bounds,
                        callable_bound,
                        default,
                        infer_only,
                        variadic,
                        constraints,
                    } => ParamDecl::Type {
                        name: name.clone(),
                        bounds: bounds.clone(),
                        callable_bound: callable_bound
                            .as_ref()
                            .map(|bound| Box::new(rename_dependent_parameters(bound, &free_names))),
                        default: default.as_ref().map(|default| {
                            Box::new(rename_dependent_parameters(default, &free_names))
                        }),
                        infer_only: *infer_only,
                        variadic: *variadic,
                        constraints: constraints.clone(),
                    },
                    ParamDecl::Value {
                        name,
                        ty,
                        default,
                        callable_default,
                        infer_only,
                        variadic,
                        constraints,
                    } => ParamDecl::Value {
                        name: name.clone(),
                        ty: Box::new(rename_dependent_parameters(ty, &free_names)),
                        default: default
                            .as_ref()
                            .map(|value| value.rename_parameters(&free_names)),
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
                    .map(|ty| rename_dependent_parameters(ty, &free_names))
                    .collect(),
                names: parameter_names.clone(),
                ret: Box::new(rename_dependent_parameters(ret, &free_names)),
                required: required.clone(),
                variadic: variadic
                    .as_ref()
                    .map(|ty| Box::new(rename_dependent_parameters(ty, &free_names))),
                kw_variadic: kw_variadic
                    .as_ref()
                    .map(|ty| Box::new(rename_dependent_parameters(ty, &free_names))),
                positional_only: *positional_only,
                keyword_only: *keyword_only,
                raises: *raises,
                error: error
                    .as_ref()
                    .map(|ty| Box::new(rename_dependent_parameters(ty, &free_names))),
                conventions: conventions.clone(),
                ref_params: ref_params.clone(),
                ref_return: ref_return.clone(),
                transfers: transfers.clone(),
            }
        }
        Ty::Overload(candidates) => Ty::Overload(
            candidates
                .iter()
                .map(|ty| rename_dependent_parameters(ty, names))
                .collect(),
        ),
        _ => ty.clone(),
    }
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

/// Alpha-rename the binder references inside one canonicalized constraint:
/// `param`-shaped fields rename through `binder_names` (falling back to the
/// pack-trimmed spelling), and embedded types canonicalize exactly like
/// signature types.
pub fn rename_constraint_parameters(
    constraint: &GenericConstraint,
    binder_names: &HashMap<String, String>,
    subst: &HashMap<String, Ty>,
    value_names: &HashMap<String, String>,
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
            ConstraintOperand::Type(ty) => ConstraintOperand::Type(rename_dependent_parameters(
                &substitute(ty, subst),
                value_names,
            )),
        }
    };
    let recurse = |inner: &GenericConstraint| {
        rename_constraint_parameters(inner, binder_names, subst, value_names)
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

//! The abstract syntax tree for the implemented subset of Mojo.
//!
//! Kept separate from the parser so linking, compile-time elaboration, semantic
//! checking, lowering, and syntax-oriented tools do not depend on parser internals.
//!
//! **Spans.** Every [`Expr`] carries a source [`Span`] (`token::Span`), stamped by
//! the parser, so later stages (the MIR's `SpanTable`) can point diagnostics back
//! at source. The span is *metadata*: [`Expr`]'s `PartialEq` compares only the
//! `kind`, so AST-literal assertions in the tests stay span-agnostic.

use std::borrow::Cow;

use mojito_common::token::Span;

/// The compiler-internal base of a leading-dot
/// contextual member reference (`.red`); unspellable in source, resolved
/// against the expected type during checking and substituted during HIR syntax
/// renaming.
pub const CONTEXTUAL_SENTINEL: &str = "$contextual";

/// The derivation ordinal of a compiler-spelled value or type pack read.
///
/// The `ParameterList[values.values]()` or `TypeList[Ts.values]()`
/// construction the compiler spells in place of a pack read at run time
/// takes `SyntaxId::derived(read, this)`.
pub const PACK_READ_ORDINAL: u32 = u32::MAX;

/// The derivation ordinal of the `values.values` argument a spelled value
/// or type pack read passes to its list struct: an occurrence of its own,
/// apart from the construction the read's facts record.
pub const PACK_READ_RAW_LIST_ORDINAL: u32 = u32::MAX - 1;

/// The derivation ordinal of the pack operand of that `values.values`.
pub const PACK_READ_OPERAND_ORDINAL: u32 = u32::MAX - 2;

/// Whether a definition's decorators mark it as a parametric closure
/// (`@__parameter`; the pre-rename `@parameter` still warns-and-runs upstream,
/// so it stays accepted as a deprecation bridge).
///
/// The parser and the checker share this predicate: such a def takes no
/// capture list and captures its free variables implicitly.
pub fn is_parameter_closure(decorators: &[Decorator]) -> bool {
    decorators.iter().any(|decorator| {
        decorator.path.len() == 1
            && matches!(decorator.path[0].as_str(), "__parameter" | "parameter")
    })
}

/// Whether a declaration carries `@__unsafe_nested_origins_read_only`: the
/// callee promises to only read the origins its argument types carry, so a
/// call's exclusivity check counts them as immutable paths.
pub fn is_nested_origins_read_only(decorators: &[Decorator]) -> bool {
    decorators.iter().any(|decorator| {
        decorator.path.len() == 1 && decorator.path[0] == "__unsafe_nested_origins_read_only"
    })
}

/// Whether a declaration carries `@unavailable`: it declares a signature
/// whose every use is an error, and has no body to lower.
pub fn is_unavailable(decorators: &[Decorator]) -> bool {
    decorators
        .iter()
        .any(|decorator| decorator.path.len() == 1 && decorator.path[0] == "unavailable")
}

/// Whether a declaration carries a decorator that bears on its body or its
/// identity. `@__unsafe_nested_origins_read_only` does not: it states a
/// contract to the declaration's callers only.
pub fn has_body_decorator(decorators: &[Decorator]) -> bool {
    decorators
        .iter()
        .any(|decorator| !is_nested_origins_read_only(std::slice::from_ref(decorator)))
}

/// The trait names of a trait-position expression: one identifier, or a `&`
/// conjunction of them (`Copyable & Writable`, as `conforms_to` accepts).
/// `None` for any other shape.
pub fn trait_conjunction_names(expr: &Expr) -> Option<Vec<&str>> {
    match &expr.kind {
        ExprKind::Identifier(name) => Some(vec![name.as_str()]),
        ExprKind::Infix(InfixOp::BitAnd, left, right) => {
            let mut names = trait_conjunction_names(left)?;
            names.extend(trait_conjunction_names(right)?);
            Some(names)
        }
        _ => None,
    }
}

/// Canonicalize the whole-value destructor method name: upstream deprecated
/// `__del__` in favor of `__deinit__`.
///
/// Applied to method names at parse time so symbol mangling, overload maps,
/// and the VM's destructor lookup all see the canonical spelling.
pub fn canonical_destructor_name(name: &str) -> &str {
    match name {
        "__del__" => "__deinit__",
        other => other,
    }
}

/// The compile-time arguments a subscript's index spells: one per element
/// of a tuple index, none for the empty subscript `f[]`, else the index
/// itself.
pub fn subscript_param_args(index: &Expr) -> Vec<ParamArg> {
    match &index.kind {
        ExprKind::TupleLit(elements) => elements.iter().map(subscript_param_arg).collect(),
        ExprKind::EmptySubscript => Vec::new(),
        _ => vec![subscript_param_arg(index)],
    }
}

/// The compile-time arguments a multi-argument subscript spells, a keyword
/// argument (`plus_one[n=3]`) as a named one. `None` when one is a slice.
pub fn multi_subscript_param_args(args: &[SubscriptArg]) -> Option<Vec<ParamArg>> {
    args.iter()
        .map(|argument| match argument {
            SubscriptArg::Index(value) => Some(subscript_param_arg(value)),
            SubscriptArg::Keyword { name, value } => Some(ParamArg::Named {
                name: name.clone(),
                value: Box::new(subscript_param_arg(value)),
            }),
            SubscriptArg::Slice { .. } | SubscriptArg::KeywordSlice { .. } => None,
        })
        .collect()
}

/// One subscript index as a compile-time argument: a type the subscript
/// spells (`P.sz[Int]`) is a type argument.
pub fn subscript_param_arg(index: &Expr) -> ParamArg {
    match &index.kind {
        ExprKind::TypeValue(ty) => ParamArg::Type(ty.clone()),
        _ => ParamArg::Value(index.clone()),
    }
}

/// A type annotation.
///
/// Covers the scalar types plus nominal (`struct`) types, which may carry type
/// arguments (`Pair[Int]`), and references to a type parameter.
/// Function/closure and reference types are represented even though their
/// runtime semantics are not yet supported.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Type {
    Int,
    UInt,
    Bool,
    /// The compile-time string literal type, spelled `StringLiteral` in
    /// source. A bare `String` annotation parses as `Named("String")` and
    /// resolves to the prelude's nominal self-hosted struct.
    StringLiteral,
    /// The closed type of one string literal, `StringLiteral[value]` at the
    /// pin, whose value Mojito does not track. The elaborator spells a binder
    /// bound to an inferred literal's type with it (a pack element
    /// `"two"` binds), so the annotation it substitutes stays concrete where
    /// the open bare spelling would not. The parser never constructs it.
    ClosedStringLiteral,
    Float64,
    None,
    /// A name that resolves to a `struct` type, optionally applied to parameter
    /// arguments (`Point` is `Named("Point", [])`, `Pair[Int]` is
    /// `Named("Pair", [Type(Int)])`, `FixedBuffer[8]` is
    /// `Named("FixedBuffer", [Value(Int(8))])`). A bare `Named(name, [])` may also
    /// resolve to a type *parameter* in scope (a `T` inside a generic `def`); the
    /// checker decides. The checker validates the name and the argument kinds.
    Named(String, Vec<ParamArg>),
    /// `Self.T` — one of the enclosing struct's own type parameters, referenced
    /// from inside its body (Mojo spelling; a bare `T` is not in scope there).
    /// The checker also accepts it as a shorthand for a type-valued associated
    /// member when there is no struct parameter by that name. The qualified
    /// pack spread `*Self.Ts` keeps the leading `*` in the name
    /// (`SelfParam("*Ts")`); the elaborator's pack-qualification pass folds
    /// it onto the bare `Named("*Ts")` spread once a struct's members are
    /// known to qualify every use.
    SelfParam(String),
    /// `Base.Member` in type position — an associated type/comptime member lookup
    /// on a type parameter, `Self`, or a concrete type. `args` is the parameter
    /// application of a parameterized associated type
    /// (`Self.IteratorType[origin_of(self)]`); it is empty for a bare `C.Element`.
    Assoc {
        base: Box<Self>,
        name: String,
        args: Vec<ParamArg>,
    },
    /// Index a compile-time sequence of types while remaining in type position.
    ///
    /// This is deliberately a structured projection rather than a synthesized
    /// name: the base can itself be an associated projection and the index is a
    /// retained expression whose value may depend on a compile-time binder.
    /// For example, `values.element_types[index]` is represented as an indexed
    /// projection over `values.element_types`, not as the text
    /// `"values.element_types[index]"`.
    IndexedProjection {
        base: Box<Self>,
        index: Box<Expr>,
    },
    /// Bare `Self` — the enclosing struct type (in a struct method) or the
    /// conforming type (in a trait method requirement).
    SelfType,
    /// A **function type** annotation: `def(T1, mut T2, …) [thin | capturing[...]]
    /// [raises] [-> R]`.
    /// Parameters retain optional conventions, origins, and diagnostic names
    /// because those facts affect indirect-call ownership and reference results.
    /// `thin` marks a non-capturing function pointer (default is capturing).
    Func {
        /// Compile-time parameters declared by the callable contract.
        type_params: Vec<TypeParam>,
        params: Vec<FunctionTypeParam>,
        ret: Box<Self>,
        thin: bool,
        /// `None` when no explicit `capturing` effect was written,
        /// `Some([])` for bare `capturing`, and `Some(origins)` for
        /// `capturing[origins]`.
        capturing: Option<OriginSpec>,
        raises: bool,
        raises_type: Option<Box<Self>>,
        /// Trailing `where` clauses constraining the contract's own `def[...]`
        /// parameters. The clause binds to the innermost function type
        /// (upstream requires parenthesizing a function-type result to attach
        /// a declaration-level `where` after it). Compiled onto the anonymous
        /// contract's parameter declarations; rejected on plain annotations.
        where_clauses: Vec<Expr>,
    },
    /// A **reference type** `ref [origin] T` (Mojo's parametric-mutability
    /// reference — used in a `ref[origin]` return type). The origin specifier is
    /// parsed but **discarded** (origins are not modeled); `ty` is the referent
    /// type. Parsed; the checker flags a `ref` annotation as unsupported.
    Ref {
        referent: Box<Self>,
        origin: Option<OriginSpec>,
    },
    /// Upstream's `type_of(x)`: the type of a compile-time expression. The
    /// parser accepts any operand; the checker accepts a trait or a `&`
    /// composition of traits, whose type is the trait metatype
    /// (`type_of(AnyType)` types a trait-valued parameter).
    TypeOf(Box<Expr>),
    /// Upstream's generator type `__generator_type[params] Result`: the type
    /// of a parameterized compile-time declaration read unapplied, its
    /// parameter list and the type its application yields.
    Generator {
        params: Vec<TypeParam>,
        result: Box<Self>,
    },
    /// A compile-time value where the type grammar expects a type: a struct
    /// construction such as `TypeList[Trait=AnyType, Ts.values]()`. It is a
    /// type only as the base of an `IndexedProjection`, which reads the
    /// value's `__getitem_param__[index]`; alone, the checker rejects it.
    Value(Box<Expr>),
}

impl Type {
    /// The expression this type spelling denotes, the inverse of
    /// [`Expr::type_spelling`]: `TypeList.of` as the receiver of the
    /// qualified construction `TypeList.of[...]()`, or `Self.n` as the read
    /// of a member a bracket value slot spells as a type. `None` for a
    /// spelling no expression denotes.
    pub fn expression_spelling(&self, span: Span) -> Option<Expr> {
        let kind = match self {
            Self::Int => ExprKind::Identifier("Int".to_string()),
            Self::UInt => ExprKind::Identifier("UInt".to_string()),
            Self::Bool => ExprKind::Identifier("Bool".to_string()),
            Self::StringLiteral => ExprKind::Identifier("StringLiteral".to_string()),
            Self::Float64 => ExprKind::Identifier("Float64".to_string()),
            Self::SelfType => ExprKind::Identifier("Self".to_string()),
            Self::Named(name, args) if args.is_empty() => ExprKind::Identifier(name.clone()),
            Self::Named(name, args) => ExprKind::TypeApply {
                name: name.clone(),
                args: args.clone(),
            },
            Self::SelfParam(name) if !name.starts_with('*') => ExprKind::Member {
                object: Box::new(Expr::new(ExprKind::Identifier("Self".to_string()), span)),
                field: name.clone(),
            },
            Self::Assoc { base, name, args } if args.is_empty() => ExprKind::Member {
                object: Box::new(base.expression_spelling(span)?),
                field: name.clone(),
            },
            _ => return None,
        };
        Some(Expr::new(kind, span))
    }
}

/// One parameter in a source-level function/closure type.
///
/// Unlike [`FnParam`], it has no default; its regular or keyword-variadic role
/// and optional name are retained from spellings such as `def(mut writer:
/// Writer)` and `def(var **options: Int)`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FunctionTypeParam {
    pub name: Option<String>,
    pub kind: ParamKind,
    pub convention: Option<ArgConvention>,
    pub origin: Option<OriginSpec>,
    pub ty: Type,
}

/// Source syntax inside a `ref[...]` clause.
///
/// Expressions are deliberately retained verbatim: `origin_of` arguments are
/// never evaluated, and semantic resolution belongs to the checker where
/// bindings have stable identities.
pub type OriginSpec = Vec<Expr>;

/// Explicit name for [`Type`] when code handles parsed source annotations rather
/// than the checked semantic lattice in `types::Ty`. `Type` remains as a
/// compatibility name for the public AST API.
pub type SourceType = Type;

/// A compile-time **parameter** declared in a `[...]` list on a `struct` or
/// `def` header.
///
/// Syntactically uniform (`NAME: X`): it is a **type parameter** when `X` is
/// one or more trait names (`T: Copyable & Movable`) or a **value parameter**
/// when `X` is a concrete type (`n: Int`). The checker classifies by resolving
/// `bounds` — a single entry naming a type means a value parameter; trait
/// names mean a type parameter. The checker retains and validates the declared
/// value type; runtime materialization is governed by `CtValue::materialize`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TypeParam {
    /// The source name. A leading `*` is retained for a variadic type pack.
    pub name: String,
    /// The trait bound(s), or (for a value parameter) the single value type name.
    pub bounds: Vec<String>,
    /// Full source type for a syntactically unambiguous value parameter such as
    /// `items: List[Int]`. Scalar value parameters are classified semantically.
    pub value_type: Option<Type>,
    /// A retained callable right-hand side (`F: def(...)`). Whether it denotes a
    /// callable type constraint or a compile-time function-value parameter is a
    /// semantic decision; the parser preserves the complete function type.
    pub callable_bound: Option<Type>,
    /// `Some(expr)` for `Origin[mut=expr]`; `None` for type/value parameters.
    /// Upstream's `ImmOrigin` and `MutOrigin` are `comptime` aliases of
    /// `Origin[mut=False]` and `Origin[mut=True]`, so the parser records them
    /// as the `Origin` bound with a fixed `Bool` mutability.
    pub origin_mutability: Option<Expr>,
    /// Whether this parameter precedes the `//` infer-only marker.
    pub infer_only: bool,
    /// A retained compile-time default. Semantic classification and dependency
    /// checking happen after all earlier parameters have entered scope.
    pub default: Option<Expr>,
    /// Legacy storage retained during the typed-boundary migration. Current
    /// syntax rejects parameter-local `where`, so parsed declarations leave it
    /// empty and use the declaration's trailing predicate instead.
    pub constraints: Vec<Expr>,
}

impl TypeParam {
    /// Whether this parameter is semantic-only origin metadata: an `Origin` or
    /// `OriginSet` parameter, or the infer-only `Bool` binding one's
    /// mutability ([`Self::is_origin_mutability_binder`]). Such a parameter
    /// is inferred from places rather than occupying a binder slot.
    pub fn is_erased_origin_param(&self, siblings: &[Self]) -> bool {
        matches!(self.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet")
            || self.is_origin_mutability_binder(siblings)
    }

    /// This parameter's binder slot among `siblings`, its own list: its
    /// position once the erased origin parameters are skipped.
    pub fn binder_slot(&self, siblings: &[Self]) -> usize {
        siblings
            .iter()
            .take_while(|sibling| sibling.name != self.name)
            .filter(|sibling| !sibling.is_erased_origin_param(siblings))
            .count()
    }

    /// Whether this parameter is an infer-only `Bool` that exists to bind a
    /// sibling origin parameter's mutability (`iterable_mut: Bool, //,
    /// iterable_origin: Origin[mut=iterable_mut]`). Like the origin parameter
    /// it qualifies, such a binder is semantic-only: it is erased from runtime
    /// generic argument binding and inferred from the borrowed place instead
    /// of occupying a source-visible value-parameter slot.
    pub fn is_origin_mutability_binder(&self, siblings: &[Self]) -> bool {
        self.infer_only
            && matches!(self.bounds.as_slice(), [only] if only == "Bool")
            && self.value_type.is_none()
            && siblings.iter().any(|peer| {
                matches!(
                    &peer.origin_mutability,
                    Some(Expr {
                        kind: ExprKind::Identifier(name),
                        ..
                    }) if name == &self.name
                )
            })
    }
}

/// A parameter **argument** supplied in a `[...]` list at a use site, i.e. a
/// `Pair[Int]` / `FixedBuffer[8]`.
///
/// A type parameter takes a `Type`; a value parameter takes a comptime value
/// `Expr`. The two forms are distinguished by the parser where it can (a
/// leading type keyword / `Self` is a `Type`); a bare identifier or an
/// arithmetic expression parses as `Value`, and the checker reinterprets a
/// lone identifier as a type when the parameter is a type one.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ParamArg {
    Type(Type),
    Value(Expr),
    /// A keyword compile-time argument (`name=value`).
    Named {
        name: String,
        value: Box<Self>,
    },
}

/// A SIMD element type — the `<dt>` in `SIMD[DType.<dt>, width]`.
///
/// Mojito supports the signed and unsigned index integers, the fixed-width
/// integers up to 64 bits,
/// `float16`, `float32`, `float64`, and `bool`; upstream's other names are
/// [`UPSTREAM_ONLY_DTYPE_NAMES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Dtype {
    /// Platform-sized signed integer, used by `Int = Scalar[DType.int]`.
    Int,
    /// Platform-sized unsigned integer, used by `UInt = Scalar[DType.uint]`.
    UInt,
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

impl Dtype {
    /// The `DType.<name>` spelling → the dtype (e.g. `"int32"` → `Int32`).
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "int" => Self::Int,
            "uint" => Self::UInt,
            "int8" => Self::Int8,
            "int16" => Self::Int16,
            "int32" => Self::Int32,
            "int64" => Self::Int64,
            "uint8" => Self::UInt8,
            "uint16" => Self::UInt16,
            "uint32" => Self::UInt32,
            "uint64" => Self::UInt64,
            "float16" => Self::Float16,
            "float32" => Self::Float32,
            "float64" => Self::Float64,
            "bool" => Self::Bool,
            _ => return None,
        })
    }

    /// The `DType.<name>` spelling of this dtype.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Int => "int",
            Self::UInt => "uint",
            Self::Int8 => "int8",
            Self::Int16 => "int16",
            Self::Int32 => "int32",
            Self::Int64 => "int64",
            Self::UInt8 => "uint8",
            Self::UInt16 => "uint16",
            Self::UInt32 => "uint32",
            Self::UInt64 => "uint64",
            Self::Float16 => "float16",
            Self::Float32 => "float32",
            Self::Float64 => "float64",
            Self::Bool => "bool",
        }
    }

    /// The KGEN mnemonic of this dtype, which `!kgen.simd<N, …>` and
    /// `!kgen.scalar<…>` storage types spell (`si32`, `index`, `f64`).
    pub const fn kgen_mnemonic(self) -> &'static str {
        match self {
            Self::Int => "index",
            Self::UInt => "uindex",
            Self::Int8 => "si8",
            Self::Int16 => "si16",
            Self::Int32 => "si32",
            Self::Int64 => "si64",
            Self::UInt8 => "ui8",
            Self::UInt16 => "ui16",
            Self::UInt32 => "ui32",
            Self::UInt64 => "ui64",
            Self::Float16 => "f16",
            Self::Float32 => "f32",
            Self::Float64 => "f64",
            Self::Bool => "bool",
        }
    }

    /// The dtype a KGEN mnemonic names (`"si32"` → `Int32`), or `None`.
    pub fn from_kgen_mnemonic(mnemonic: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|dtype| dtype.kgen_mnemonic() == mnemonic)
    }

    /// The scalar-alias spelling (`Int32`, `Float16`, …) that means
    /// `SIMD[DType.<self>, 1]`, or `None` for `bool` (which has no alias).
    pub const fn scalar_alias(self) -> Option<&'static str> {
        Some(match self {
            Self::Int8 => "Int8",
            Self::Int16 => "Int16",
            Self::Int32 => "Int32",
            Self::Int64 => "Int64",
            Self::UInt8 => "UInt8",
            Self::UInt16 => "UInt16",
            Self::UInt32 => "UInt32",
            Self::UInt64 => "UInt64",
            Self::Float16 => "Float16",
            Self::Float32 => "Float32",
            // `Int`, `UInt`, and `Float64` name their scalars too, but
            // conversion routes them as the builtin numeric names, and `bool`
            // is spelled by `Bool`.
            Self::Int | Self::UInt | Self::Float64 | Self::Bool => return None,
        })
    }

    /// The type name a width-1 `SIMD` of this dtype spells its `repr` with
    /// (`Int32(4)`, `Float64(0.5)`), as upstream's `_scalar_repr_alias`
    /// answers it: every scalar alias, `Int`, `UInt`, and `Float64` included, and
    /// `None` for `bool`, whose `SIMD[DType.bool, 1]` keeps the verbose form.
    pub const fn repr_alias(self) -> Option<&'static str> {
        match self {
            Self::Int => Some("Int"),
            Self::UInt => Some("UInt"),
            Self::Float64 => Some("Float64"),
            _ => self.scalar_alias(),
        }
    }

    /// The dtype a scalar alias names (`"Int32"` → `Int32`), or `None`.
    pub fn from_scalar_alias(name: &str) -> Option<Self> {
        // Mojo's `Byte` is an exact public alias of `UInt8`, not a distinct
        // byte-string or character type.
        if name == "Byte" {
            return Some(Self::UInt8);
        }
        [
            Self::Int8,
            Self::Int16,
            Self::Int32,
            Self::Int64,
            Self::UInt8,
            Self::UInt16,
            Self::UInt32,
            Self::UInt64,
            Self::Float16,
            Self::Float32,
        ]
        .into_iter()
        .find(|d| d.scalar_alias() == Some(name))
    }

    /// The bit width of one lane (`bool` is one bit).
    pub const fn bit_width(self) -> u32 {
        match self {
            Self::Bool => 1,
            Self::Int8 | Self::UInt8 => 8,
            Self::Int16 | Self::UInt16 | Self::Float16 => 16,
            Self::Int32 | Self::UInt32 | Self::Float32 => 32,
            Self::Int | Self::UInt | Self::Int64 | Self::UInt64 | Self::Float64 => 64,
        }
    }

    /// Whether this dtype's lanes are floating-point.
    pub const fn is_float(self) -> bool {
        matches!(self, Self::Float16 | Self::Float32 | Self::Float64)
    }

    /// Whether this is a float lane narrower than `f64`: the VM and the
    /// native lowering compute it at double precision and round the result
    /// to the lane.
    pub const fn is_narrow_float(self) -> bool {
        matches!(self, Self::Float16 | Self::Float32)
    }

    /// Round a double-precision result to this float lane (`float16` and
    /// `float32` round, `float64` is exact).
    pub fn round_lane(self, value: f64) -> f64 {
        match self {
            Self::Float16 => mojito_common::float::round_f16(value),
            Self::Float32 => f64::from(value as f32),
            _ => value,
        }
    }

    /// An exact float literal correctly rounded to this float lane and
    /// widened back to `f64`, or `None` when it does not fit a double.
    pub fn float_literal_lane(self, literal: &mojito_common::literal::FloatLiteral) -> Option<f64> {
        match self {
            Self::Float16 => literal.to_f16().map(mojito_common::float::f16::to_f64),
            Self::Float32 => literal.to_f32().map(f64::from),
            _ => literal.to_f64(),
        }
    }

    /// Upstream's one-byte `!kgen.dtype` code, whose bits the predicates
    /// below test: `DTYPE_INTEGER_MASK` marks a sized integer,
    /// `DTYPE_FLOAT_MASK` a float, and `DTYPE_SIGNED_MASK` signedness.
    pub const fn code(self) -> u8 {
        match self {
            Self::Bool => 1,
            Self::Int => 3,
            Self::UInt => 4,
            Self::UInt8 => 134,
            Self::Int8 => 135,
            Self::UInt16 => 136,
            Self::Int16 => 137,
            Self::UInt32 => 138,
            Self::Int32 => 139,
            Self::UInt64 => 140,
            Self::Int64 => 141,
            Self::Float16 => 79,
            Self::Float32 => 81,
            Self::Float64 => 82,
        }
    }

    /// Every dtype, in declaration order.
    pub const ALL: [Self; 14] = [
        Self::Int,
        Self::UInt,
        Self::Int8,
        Self::Int16,
        Self::Int32,
        Self::Int64,
        Self::UInt8,
        Self::UInt16,
        Self::UInt32,
        Self::UInt64,
        Self::Float16,
        Self::Float32,
        Self::Float64,
        Self::Bool,
    ];

    /// `DType.is_floating_point()`.
    pub const fn is_floating_point(self) -> bool {
        self.matches_mask(DTYPE_FLOAT_MASK)
    }

    /// `DType.is_integral()`: an index integer or a sized integer.
    pub const fn is_integral(self) -> bool {
        matches!(self, Self::Int | Self::UInt) || self.matches_mask(DTYPE_INTEGER_MASK)
    }

    /// `DType.is_signed()`.
    pub const fn is_signed(self) -> bool {
        self.is_floating_point() || (self.is_integral() && self.matches_mask(DTYPE_SIGNED_MASK))
    }

    /// `DType.is_unsigned()`: the unsigned index integer, whose code carries
    /// neither mask bit, or an unsigned sized integer.
    pub const fn is_unsigned(self) -> bool {
        matches!(self, Self::UInt)
            || (self.matches_mask(DTYPE_INTEGER_MASK) && !self.matches_mask(DTYPE_SIGNED_MASK))
    }

    /// `DType.is_float8()`: none of the eight-bit float formats is a
    /// Mojito dtype.
    pub const fn is_float8(self) -> bool {
        false
    }

    /// `DType.is_half_float()`: `float16` (`bfloat16` is not a Mojito
    /// dtype).
    pub const fn is_half_float(self) -> bool {
        matches!(self, Self::Float16)
    }

    /// `DType.is_numeric()`: integral, or a float other than the
    /// non-arithmetic low-precision formats (none of which is a Mojito dtype).
    pub const fn is_numeric(self) -> bool {
        self.is_integral() || self.is_floating_point()
    }

    /// The zero-argument `Bool` predicate `DType.<method>()` names, if any.
    pub fn predicate(self, method: &str) -> Option<bool> {
        Some(match method {
            "is_integral" => self.is_integral(),
            "is_floating_point" => self.is_floating_point(),
            "is_signed" => self.is_signed(),
            "is_unsigned" => self.is_unsigned(),
            "is_numeric" => self.is_numeric(),
            "is_float8" => self.is_float8(),
            "is_half_float" => self.is_half_float(),
            _ => return None,
        })
    }

    /// The static `Int` query `DType.<method>[self]()` (one of
    /// [`DTYPE_FLOAT_QUERIES`]), or `None` when `self` is not a floating-point
    /// dtype, where upstream's `comptime assert` fails.
    pub fn float_query(self, method: &str) -> Option<i64> {
        let (bit_width, exponent_width, max_exponent) = match self {
            Self::Float16 => (16, 5, 16),
            Self::Float32 => (32, 8, 128),
            Self::Float64 => (64, 11, 1024),
            _ => return None,
        };
        match method {
            "mantissa_width" => Some(bit_width - exponent_width - 1),
            "max_exponent" => Some(max_exponent),
            "exponent_width" => Some(exponent_width),
            "exponent_bias" => Some(max_exponent - 1),
            _ => None,
        }
    }

    const fn matches_mask(self, mask: u8) -> bool {
        self.code() & mask != 0
    }
}

/// The `Bool` predicate methods a `DType` value answers.
pub const DTYPE_PREDICATES: [&str; 7] = [
    "is_integral",
    "is_floating_point",
    "is_signed",
    "is_unsigned",
    "is_numeric",
    "is_float8",
    "is_half_float",
];

/// The static `Int` queries `DType.<method>[dtype]()` answers for a
/// floating-point dtype.
pub const DTYPE_FLOAT_QUERIES: [&str; 4] = [
    "mantissa_width",
    "max_exponent",
    "exponent_width",
    "exponent_bias",
];

/// Upstream's `_mIsSigned` bit of a dtype code.
pub const DTYPE_SIGNED_MASK: u8 = 1;
/// Upstream's `_mIsFloat` bit of a dtype code.
pub const DTYPE_FLOAT_MASK: u8 = 64;
/// Upstream's `_mIsInteger` bit of a dtype code (sized integers only).
pub const DTYPE_INTEGER_MASK: u8 = 128;

/// `DType.<name>` spellings upstream defines that no Mojito dtype represents;
/// they reject as unsupported rather than as unknown attributes.
pub const UPSTREAM_ONLY_DTYPE_NAMES: [&str; 17] = [
    "bfloat16",
    "_uint1",
    "_uint2",
    "_uint4",
    "uint128",
    "int128",
    "uint256",
    "int256",
    "float4_e2m1fn",
    "float6_e2m3fn",
    "float6_e3m2fn",
    "float8_e8m0fnu",
    "float8_e3m4",
    "float8_e4m3fn",
    "float8_e4m3fnuz",
    "float8_e5m2",
    "float8_e5m2fnuz",
];

/// A typed struct field, e.g. `a: Int`. (Function parameters use `FnParam`.)
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Param {
    pub name: String,
    pub ty: Type,
}

/// A function/method parameter, e.g. `a: Int`, `b: Int = 2`, `*rest: Int`,
/// `var **opts: Int`, or `mut x: Int`.
///
/// Defaults, variadics, and the supported conventions participate in checking
/// and VM argument binding.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FnParam {
    pub name: String,
    pub ty: Type,
    /// A default value (`= expr`), for an optional argument. `None` if required.
    pub default: Option<Expr>,
    /// `*args` (variadic) / `**kwargs` (keyword variadic) / a plain parameter.
    pub kind: ParamKind,
    /// An argument convention (`imm`/`mut`/`var`/`out`); `None` = default.
    pub convention: Option<ArgConvention>,
    /// Retained source origin clause for a `ref[...]` convention.
    pub origin: Option<OriginSpec>,
}

/// A declared parameter as a call binds it and a lowering frames it: its
/// name, kind, convention, and default, whatever holds its type.
pub trait DeclaredParam {
    fn name(&self) -> &str;
    fn kind(&self) -> ParamKind;
    fn convention(&self) -> Option<ArgConvention>;
    /// Whether the parameter declares a default, which makes it optional.
    fn has_default(&self) -> bool;

    /// Whether this is a named `out` result: a callee-local slot the callable
    /// returns, which no caller passes.
    fn is_named_result(&self) -> bool {
        self.kind() == ParamKind::Regular && self.convention() == Some(ArgConvention::Out)
    }
}

impl<P: DeclaredParam + ?Sized> DeclaredParam for &P {
    fn name(&self) -> &str {
        (**self).name()
    }

    fn kind(&self) -> ParamKind {
        (**self).kind()
    }

    fn convention(&self) -> Option<ArgConvention> {
        (**self).convention()
    }

    fn has_default(&self) -> bool {
        (**self).has_default()
    }
}

impl DeclaredParam for FnParam {
    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> ParamKind {
        self.kind
    }

    fn convention(&self) -> Option<ArgConvention> {
        self.convention
    }

    fn has_default(&self) -> bool {
        self.default.is_some()
    }
}

/// A declared parameter that also holds its annotation as written, which a
/// symbol spells.
pub trait AnnotatedParam: DeclaredParam {
    fn annotation(&self) -> &Type;
}

impl<P: AnnotatedParam + ?Sized> AnnotatedParam for &P {
    fn annotation(&self) -> &Type {
        (**self).annotation()
    }
}

impl AnnotatedParam for FnParam {
    fn annotation(&self) -> &Type {
        &self.ty
    }
}

/// The named `out` result among `params`, when the callable declares one.
pub fn named_result(params: &[FnParam]) -> Option<&FnParam> {
    params
        .iter()
        .find(|param| DeclaredParam::is_named_result(*param))
}

/// Whether a `FnParam` is a plain parameter or a variadic (`*args`/`**kwargs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ParamKind {
    Regular,
    /// `*name: T` — a positional variadic parameter.
    Variadic,
    /// `var **name: T` — a consuming keyword-variadic parameter.
    KwVariadic,
}

/// An argument-passing convention on an ordinary parameter (Mojo's `imm`
/// (a.k.a.
///
/// borrowed, the default), `mut`, `var`, `out`, and `ref` — a
/// parametric-mutability reference). `imm`, `mut`, `var`, call-scoped `ref`,
/// and a single named `out` result on a function or a method are modeled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ArgConvention {
    /// `imm` — the immutable convention. The legacy `read` spelling is a hard
    /// error (removed upstream 2026-08) with a targeted migration diagnostic.
    Imm,
    Mut,
    Var,
    Out,
    Ref,
    /// `deinit` — the destructor/consuming-move convention: grants exclusive
    /// ownership and marks the value destroyed at function end. Current Mojo's
    /// `def __del__(deinit self)`.
    Deinit,
}

impl ArgConvention {
    /// Whether a parameter declared with `convention` is a written-back
    /// reference (`mut`/`ref`).
    pub const fn is_reference(convention: Option<Self>) -> bool {
        matches!(convention, Some(Self::Mut | Self::Ref))
    }
}

/// A keyword argument at a call site: `name=value`. Structural binding is owned
/// by `crate::call`; the checker and VM interpret the matched argument slot.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KwArg {
    pub name: String,
    pub value: Expr,
}

/// One argument inside a multi-dimensional subscript. Slice arguments remain
/// structured until checking selects their concrete descriptor type.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SubscriptArg {
    Index(Expr),
    /// `name=value` — a keyword subscript argument (`s[byte=i]`), dispatched
    /// to a keyword-only `__getitem__`/`__setitem__` parameter. Only value
    /// bases produce this form; named brackets over a type name remain
    /// compile-time parameter application.
    Keyword {
        name: String,
        value: Expr,
    },
    Slice {
        lower: Option<Box<Expr>>,
        upper: Option<Box<Expr>>,
        step: Option<Box<Expr>>,
    },
    /// `name=lower:upper[:step]` — a keyword slice argument (`s[byte=a:b]`),
    /// dispatched like [`SubscriptArg::Keyword`] with the value of the slice
    /// literal. Read-only, like every keyword subscript.
    KeywordSlice {
        name: String,
        lower: Option<Box<Expr>>,
        upper: Option<Box<Expr>>,
        step: Option<Box<Expr>>,
    },
}

/// Internal call-AST marker for current Mojo's moved keyword-dictionary
/// forwarding form, `**kwargs^`.
///
/// Keeping it in the keyword stream preserves source evaluation order while
/// the checker and VM distinguish it from a named keyword.
pub const FORWARDED_KWARGS_NAME: &str = "**";

impl KwArg {
    pub fn is_forwarded(&self) -> bool {
        self.name == FORWARDED_KWARGS_NAME
    }
}

/// A decorator `@name`, `@dotted.name`, or `@name(args)` preceding a `def` or
/// `struct` (or a struct method).
///
/// Parsed into the AST. `@fieldwise_init` on a struct sets
/// `Stmt::Struct.fieldwise_init`, and a callable's inline decorators set its
/// declaration's inline level (`mojito_checked::declaration::InlineLevel`);
/// the checker validates every decorator against the site it decorates.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Decorator {
    /// The dotted name parts, e.g. `["always_inline"]` or `["a", "b"]`.
    pub path: Vec<String>,
    /// Positional arguments in `@deco(args)`; empty if no `(...)`.
    pub args: Vec<Expr>,
    /// Keyword arguments in `@deco(k=v)`; empty if none.
    pub kwargs: Vec<KwArg>,
}

/// A `def` method inside a `struct`. `self` (the implicit first parameter)
/// is not stored in `params`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Method {
    pub name: String,
    pub type_params: Vec<TypeParam>,
    /// Whether the method has a `self` receiver. `false` for a `@staticmethod`,
    /// which uses the ordinary call ABI without an implicit receiver slot.
    pub has_self: bool,
    /// The receiver's argument convention: `None` = plain read-only `self`,
    /// `Some(Mut)` = `mut self` (writable, mutations persist). `out self` is the
    /// constructor convention, `deinit self` the destructor convention, and
    /// owned/read receivers participate in lifecycle and call checking.
    pub self_convention: Option<ArgConvention>,
    pub self_origin: Option<OriginSpec>,
    /// The receiver's annotation (`out self: W[2]`), `None` when `self` is
    /// unannotated. On `out self` it is the initializer's refined result
    /// type; on any other receiver the checker accepts only `Self`.
    pub self_type: Option<Type>,
    /// Decorators preceding the method (`@staticmethod`, …), validated by the
    /// checker.
    pub decorators: Vec<Decorator>,
    pub params: Vec<FnParam>,
    /// Index of a `/` (positional-only) marker in `params`, if present (parsed).
    pub positional_only: Option<usize>,
    /// Index of a bare `*` (keyword-only) marker in `params`, if present (parsed).
    pub keyword_only: Option<usize>,
    /// Whether the `raises` effect was declared.
    pub raises: bool,
    /// The declared typed error, or `None` for bare `raises`/non-raising.
    pub raises_type: Option<Type>,
    pub ret: Option<Type>,
    pub where_clauses: Vec<Expr>,
    pub body: Vec<Stmt>,
    /// Where the method came from, when not from source. The parser never
    /// sets it.
    pub provenance: MethodProvenance,
}

/// How a [`Method`] came to be declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum MethodProvenance {
    /// Written in source.
    #[default]
    Source,
    /// Materialized from a built-in trait's default (Copyable's `copy`,
    /// Hashable's `__hash__`) rather than written in source: its body
    /// serves every instance of a generic struct, which mints no clone of
    /// it.
    SynthesizedDefault,
}

/// A method in a `trait`: either a **requirement** (`def …:` with a `...`
/// body, which a conforming struct must supply) or a **default
/// implementation** (a real body).
///
/// Like `Method`, `self` is the implicit first parameter and is not stored in
/// `params`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TraitMethod {
    pub name: String,
    /// The method's decorators, outermost first, as a struct method's are.
    pub decorators: Vec<Decorator>,
    pub type_params: Vec<TypeParam>,
    /// The receiver's argument convention: `None` = plain read-only `self`,
    /// or one of Mojo's explicit receiver conventions (`mut self`, `var self`,
    /// `ref self`, ...). Like `Method`, `self` is implicit and not stored in
    /// `params`.
    pub self_convention: Option<ArgConvention>,
    pub self_origin: Option<OriginSpec>,
    /// The receiver's annotation, `None` when `self` is unannotated. A trait
    /// requirement accepts only `Self`.
    pub self_type: Option<Type>,
    pub params: Vec<FnParam>,
    pub positional_only: Option<usize>,
    pub keyword_only: Option<usize>,
    /// Whether the trait requirement/default declares a `raises` effect.
    pub raises: bool,
    /// The declared typed error, or `None` for bare `raises`/non-raising.
    pub raises_type: Option<Type>,
    pub ret: Option<Type>,
    pub where_clauses: Vec<Expr>,
    /// The method body. `None` for a pure requirement (`...`); `Some(body)` for a
    /// **default implementation**, which is the trait's member `Trait.method`,
    /// one function over the trait's `Self`.
    pub default_body: Option<Vec<Stmt>>,
}

/// A `comptime NAME: Type` **member requirement** inside a `trait` body.
///
/// It is a compile-time constant / associated alias a conforming struct
/// must define (`comptime count: Int`, `comptime EltType: Copyable`),
/// unless it declares a default (`comptime K: Int = 3`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TraitComptime {
    pub name: String,
    /// Parameter declarations of a parameterized associated type
    /// (`comptime IteratorType[iterable_mut: Bool, //, iterable_origin:
    /// Origin[mut=iterable_mut]]: Iterator`); empty for a monomorphic member.
    pub params: Vec<TypeParam>,
    pub ty: Type,
    /// Optional availability constraint, including the diagnostic tuple form.
    pub where_clauses: Vec<Expr>,
    /// The member a conformer that omits it takes, read with `Self` as
    /// the conformer.
    pub default: Option<Expr>,
}

/// One `case` of an `enum`: `case name` carries no payload, `case
/// name(T, U)` a payload of the written types (a `Tuple` of them when there
/// are several).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EnumCase {
    pub name: String,
    pub payload: Vec<Type>,
    /// The `case` declaration's source span, where each member it generates
    /// is located.
    pub span: Span,
}

/// A `comptime NAME = expr` associated compile-time fact inside a `struct` body.
/// These are declarations on the type, not runtime statements.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StructComptime {
    pub name: String,
    /// Parameter declarations when this defines a parameterized associated type
    /// (`comptime IteratorType[params] = ...`); empty for a monomorphic member.
    pub params: Vec<TypeParam>,
    /// Optional declared result/value type. Associated aliases use `AnyType` or
    /// a trait bound; value members commonly use a scalar type.
    pub ty: Option<Type>,
    /// Optional availability constraint, including the diagnostic tuple form.
    pub where_clauses: Vec<Expr>,
    pub value: Expr,
}

/// The names imported by a `from ... import ...` statement.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ImportNames {
    /// `import *`
    Wildcard,
    /// `import name [as alias], ...`
    Names(Vec<ImportName>),
}

/// One `name [as alias]` in a `from ... import` target list.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ImportName {
    pub name: String,
    pub alias: Option<String>,
}

/// One context manager in a `with` statement.
///
/// An expression whose value is a context manager, and an optional `as NAME`
/// binding for its `__enter__` result (`with open(p) as f`, or bindingless
/// `with lock()`).
///
/// A single `with` may carry several, comma-separated (`with a() as x, b() as
/// y:`). The `as` target is a plain `NAME` — the parenthesized and
/// tuple-target forms aren't in the Mojo docs, so (strict-subset) they aren't
/// parsed.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WithItem {
    pub context: Expr,
    pub var: Option<String>,
}

/// A statement node: its [`StmtKind`] plus the source [`Span`] it was parsed from.
/// Like [`Expr`], equality ignores the span (it is metadata), so AST-literal
/// assertions stay span-agnostic.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Stmt {
    pub kind: StmtKind,
    pub span: Span,
    /// Source module identity assigned by the linker. Parser-created statements
    /// use `None`; linked entry/import declarations carry their originating path.
    /// A package's encoding leaves it out: its importer stamps the path it
    /// reached the module by, as it stamps a module it parses.
    #[serde(with = "statement_module")]
    pub module: Option<String>,
    /// Concrete occurrence identity. It is deliberately ignored by structural
    /// AST equality, like source spans and module provenance. A decoded
    /// statement draws a fresh one, as a parsed one does.
    #[serde(with = "mojito_common::token::node_syntax")]
    pub syntax_id: mojito_common::token::SyntaxId,
}

impl Stmt {
    /// Construct a spanned statement.
    pub fn new(kind: StmtKind, span: Span) -> Self {
        Self {
            kind,
            span,
            module: None,
            syntax_id: mojito_common::token::SyntaxId::fresh(),
        }
    }

    pub fn source_span(&self) -> mojito_common::token::SourceSpan {
        mojito_common::token::SourceSpan::syntax(self.module.clone(), self.span, self.syntax_id)
    }
}

/// Span-agnostic structural equality: two statements are equal iff their kinds are.
impl PartialEq for Stmt {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

/// Wrap a bare [`StmtKind`] into a [`Stmt`] with a dummy span. Convenient for
/// building AST literals in tests (where the span is irrelevant).
impl From<StmtKind> for Stmt {
    fn from(kind: StmtKind) -> Self {
        Self::new(kind, mojito_common::token::DUMMY_SPAN)
    }
}

/// The source-level binding convention on a name a construct introduces: an
/// iteration target, a `case` pattern binding, or a destructuring leaf.
///
/// `for` statements, comprehension clauses, match patterns, and
/// destructuring targets share these spellings. The convention is
/// independent of whether the bound value is transferred with `^`: an
/// unadorned name is immutable, while `var` and contextual `ref` are explicit
/// conventions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BindingMode {
    /// `for item in iterable`, `case item:`.
    Immutable,
    /// `for var item in iterable`, `case var item:`, `var item` in a target.
    Var,
    /// `for ref item in iterable`, `case ref item:`, `ref item` in a target.
    Ref,
}

/// A `case` pattern: the recursive match grammar of `match`, `if case`, and
/// `while case`. Equality ignores the span and occurrence identity, as
/// [`Expr`]'s does.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MatchPattern {
    pub kind: MatchPatternKind,
    pub span: Span,
    /// Stamped as [`Stmt::module`] is.
    #[serde(with = "expression_source")]
    pub source: Option<String>,
    /// Concrete occurrence identity; ignored by structural equality.
    #[serde(with = "mojito_common::token::node_syntax")]
    pub syntax_id: mojito_common::token::SyntaxId,
}

impl MatchPattern {
    pub fn new(kind: MatchPatternKind, span: Span) -> Self {
        Self {
            kind,
            span,
            source: None,
            syntax_id: mojito_common::token::SyntaxId::fresh(),
        }
    }

    pub fn source_span(&self) -> mojito_common::token::SourceSpan {
        mojito_common::token::SourceSpan::syntax(self.source.clone(), self.span, self.syntax_id)
    }

    /// The names the pattern binds, in source order, with their conventions.
    /// Every alternative of an OR pattern is visited, so a name an OR binds
    /// appears once per alternative.
    pub fn bindings(&self) -> Vec<(&str, BindingMode)> {
        let mut out = Vec::new();
        self.collect_bindings(&mut out);
        out
    }

    /// The expressions the pattern embeds, in source order: literals, `==`
    /// operands, and call-shaped heads. They are the pattern's reads; its
    /// names are definitions, not expressions.
    pub fn expressions(&self) -> Vec<&Expr> {
        let mut out = Vec::new();
        self.collect_expressions(&mut out);
        out
    }

    pub fn expressions_mut(&mut self) -> Vec<&mut Expr> {
        let mut out = Vec::new();
        self.collect_expressions_mut(&mut out);
        out
    }

    fn collect_expressions<'a>(&'a self, out: &mut Vec<&'a Expr>) {
        match &self.kind {
            MatchPatternKind::Literal { value, .. }
            | MatchPatternKind::Equal { value, .. }
            | MatchPatternKind::Head(value) => out.push(value),
            MatchPatternKind::As { pattern, .. } => pattern.collect_expressions(out),
            MatchPatternKind::Tuple(items) | MatchPatternKind::Or(items) => {
                for item in items {
                    item.collect_expressions(out);
                }
            }
            MatchPatternKind::Sequence(items) => {
                for item in items {
                    item.pattern().collect_expressions(out);
                }
            }
            MatchPatternKind::Call { head, args, kwargs } => {
                out.push(head);
                for pattern in args {
                    pattern.collect_expressions(out);
                }
                for (_, pattern) in kwargs {
                    pattern.collect_expressions(out);
                }
            }
            MatchPatternKind::Fields(fields) => {
                for (_, pattern) in fields {
                    pattern.collect_expressions(out);
                }
            }
            MatchPatternKind::Wildcard | MatchPatternKind::Bind { .. } => {}
        }
    }

    fn collect_expressions_mut<'a>(&'a mut self, out: &mut Vec<&'a mut Expr>) {
        match &mut self.kind {
            MatchPatternKind::Literal { value, .. }
            | MatchPatternKind::Equal { value, .. }
            | MatchPatternKind::Head(value) => out.push(value),
            MatchPatternKind::As { pattern, .. } => pattern.collect_expressions_mut(out),
            MatchPatternKind::Tuple(items) | MatchPatternKind::Or(items) => {
                for item in items {
                    item.collect_expressions_mut(out);
                }
            }
            MatchPatternKind::Sequence(items) => {
                for item in items {
                    item.pattern_mut().collect_expressions_mut(out);
                }
            }
            MatchPatternKind::Call { head, args, kwargs } => {
                out.push(head);
                for pattern in args {
                    pattern.collect_expressions_mut(out);
                }
                for (_, pattern) in kwargs {
                    pattern.collect_expressions_mut(out);
                }
            }
            MatchPatternKind::Fields(fields) => {
                for (_, pattern) in fields {
                    pattern.collect_expressions_mut(out);
                }
            }
            MatchPatternKind::Wildcard | MatchPatternKind::Bind { .. } => {}
        }
    }

    fn collect_bindings<'a>(&'a self, out: &mut Vec<(&'a str, BindingMode)>) {
        match &self.kind {
            MatchPatternKind::Bind { name, mode } => out.push((name, *mode)),
            MatchPatternKind::As { pattern, name } => {
                pattern.collect_bindings(out);
                out.push((name, BindingMode::Immutable));
            }
            MatchPatternKind::Tuple(items) | MatchPatternKind::Or(items) => {
                for item in items {
                    item.collect_bindings(out);
                }
            }
            MatchPatternKind::Sequence(items) => {
                for item in items {
                    item.pattern().collect_bindings(out);
                }
            }
            MatchPatternKind::Call { args, kwargs, .. } => {
                for pattern in args {
                    pattern.collect_bindings(out);
                }
                for (_, pattern) in kwargs {
                    pattern.collect_bindings(out);
                }
            }
            MatchPatternKind::Fields(fields) => {
                for (_, pattern) in fields {
                    pattern.collect_bindings(out);
                }
            }
            MatchPatternKind::Wildcard
            | MatchPatternKind::Literal { .. }
            | MatchPatternKind::Equal { .. }
            | MatchPatternKind::Head(_) => {}
        }
    }
}

impl From<MatchPatternKind> for MatchPattern {
    fn from(kind: MatchPatternKind) -> Self {
        Self::new(kind, mojito_common::token::DUMMY_SPAN)
    }
}

impl PartialEq for MatchPattern {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

/// The forms of a [`MatchPattern`].
///
/// Call-shaped heads keep their syntax: the checker decides whether
/// `Head(...)` names a struct (keyword fields) or an enum case (one
/// positional payload), as upstream's `CallNode` does.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum MatchPatternKind {
    /// `_` — matches anything, binds nothing.
    Wildcard,
    /// `name`, `var name`, or `ref name` — matches anything and binds it.
    Bind { name: String, mode: BindingMode },
    /// A literal (`0`, `2.5`, `"s"`, `True`, `None`), compared through
    /// `__eq__`, with its source spelling (`0x4`, `'a'`, `True`) as
    /// upstream's literal node keeps it for coverage; an implicitly
    /// concatenated string and `None` have none.
    Literal {
        value: Expr,
        spelling: Option<String>,
    },
    /// `== expr` — an explicit equality test against any expression, with
    /// the operand's spelling when it is one literal token.
    Equal {
        value: Expr,
        spelling: Option<String>,
    },
    /// `(p, q)`, `p, q`, `()`, `(p,)` — a tuple of subpatterns.
    Tuple(Vec<MatchPattern>),
    /// `[p, *rest, q]` — a sequence of subpatterns with at most one star.
    Sequence(Vec<SequencePattern>),
    /// `Head(p, name=q)` — a call-shaped pattern: a struct pattern over
    /// keyword fields (`Point(x=0, y=yv)`) or an enum payload case
    /// (`Optional.Some(x)`, `.Some(x)`, whose head is a member chain, the
    /// leading-dot form rooted at the contextual sentinel).
    Call {
        head: Expr,
        args: Vec<MatchPattern>,
        kwargs: Vec<(String, MatchPattern)>,
    },
    /// `.name` or `Owner.name` — a member chain without arguments: an enum
    /// case without a payload.
    Head(Expr),
    /// `{name=p, ...}` — a contextual struct pattern over the subject's
    /// nominal type.
    Fields(Vec<(String, MatchPattern)>),
    /// `p | q` — alternatives tried in order.
    Or(Vec<MatchPattern>),
    /// `p as name` — `p` must match; `name` binds the whole value.
    As {
        pattern: Box<MatchPattern>,
        name: String,
    },
}

/// One element of a sequence pattern.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SequencePattern {
    Element(Box<MatchPattern>),
    /// `*p`: the closed pattern `p` matched against the slice between the
    /// fixed prefix and suffix.
    Star(Box<MatchPattern>),
}

impl SequencePattern {
    pub fn pattern(&self) -> &MatchPattern {
        match self {
            Self::Element(pattern) | Self::Star(pattern) => pattern,
        }
    }

    pub fn pattern_mut(&mut self) -> &mut MatchPattern {
        match self {
            Self::Element(pattern) | Self::Star(pattern) => pattern,
        }
    }
}

/// One `case pattern [if guard]: body` arm of a `match`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MatchCase {
    pub pattern: MatchPattern,
    pub guard: Option<Expr>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

/// An `if`/`elif`/`while` condition: a boolean expression, or a `case`
/// pattern test of a value.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Condition {
    /// `if expr:`
    Expr(Expr),
    /// `if case pattern = value:`
    Case {
        pattern: Box<MatchPattern>,
        value: Expr,
    },
}

impl Condition {
    /// The expression the condition evaluates: the boolean, or the matched
    /// value.
    pub const fn value(&self) -> &Expr {
        match self {
            Self::Expr(value) | Self::Case { value, .. } => value,
        }
    }

    pub const fn value_mut(&mut self) -> &mut Expr {
        match self {
            Self::Expr(value) | Self::Case { value, .. } => value,
        }
    }

    /// The pattern of a `case` condition.
    pub fn pattern(&self) -> Option<&MatchPattern> {
        match self {
            Self::Expr(_) => None,
            Self::Case { pattern, .. } => Some(pattern),
        }
    }

    pub fn pattern_mut(&mut self) -> Option<&mut MatchPattern> {
        match self {
            Self::Expr(_) => None,
            Self::Case { pattern, .. } => Some(pattern),
        }
    }
}

/// A destructuring target: where the pieces of an assigned value go.
///
/// Unlike a [`MatchPattern`], a leaf is a place to write or a name to
/// declare, never a test. Equality ignores the span and occurrence identity.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DestructureTarget {
    pub kind: DestructureTargetKind,
    pub span: Span,
    /// Stamped as [`Stmt::module`] is.
    #[serde(with = "expression_source")]
    pub source: Option<String>,
    /// Concrete occurrence identity; ignored by structural equality.
    #[serde(with = "mojito_common::token::node_syntax")]
    pub syntax_id: mojito_common::token::SyntaxId,
}

impl DestructureTarget {
    pub fn new(kind: DestructureTargetKind, span: Span) -> Self {
        Self {
            kind,
            span,
            source: None,
            syntax_id: mojito_common::token::SyntaxId::fresh(),
        }
    }

    pub fn source_span(&self) -> mojito_common::token::SourceSpan {
        mojito_common::token::SourceSpan::syntax(self.source.clone(), self.span, self.syntax_id)
    }

    /// The names the target declares (`var`/`ref` leaves), in source order.
    pub fn declared_names(&self) -> Vec<&str> {
        let mut out = Vec::new();
        self.collect_declared(&mut out);
        out
    }

    fn collect_declared<'a>(&'a self, out: &mut Vec<&'a str>) {
        match &self.kind {
            DestructureTargetKind::Declare { name, .. } => out.push(name),
            DestructureTargetKind::Tuple(items) => {
                for item in items {
                    item.collect_declared(out);
                }
            }
            DestructureTargetKind::Sequence(items) => {
                for item in items {
                    match item {
                        SequenceTarget::Element(target) | SequenceTarget::Star(target) => {
                            target.collect_declared(out);
                        }
                    }
                }
            }
            DestructureTargetKind::Discard | DestructureTargetKind::Place(_) => {}
        }
    }

    /// Whether any leaf is a `var`/`ref` declaration.
    pub fn declares(&self) -> bool {
        !self.declared_names().is_empty()
    }

    /// The name and mode of a target that is one `Declare` leaf, as a
    /// simple loop target (`for x in`, `for ref x in`) is.
    pub fn loop_binding(&self) -> Option<(&str, BindingMode)> {
        match &self.kind {
            DestructureTargetKind::Declare { name, mode } => Some((name, *mode)),
            _ => None,
        }
    }

    /// The place expressions the target writes, in source order.
    pub fn places(&self) -> Vec<&Expr> {
        let mut out = Vec::new();
        self.collect_places(&mut out);
        out
    }

    pub fn places_mut(&mut self) -> Vec<&mut Expr> {
        let mut out = Vec::new();
        self.collect_places_mut(&mut out);
        out
    }

    fn collect_places<'a>(&'a self, out: &mut Vec<&'a Expr>) {
        match &self.kind {
            DestructureTargetKind::Place(place) => out.push(place),
            DestructureTargetKind::Tuple(items) => {
                for item in items {
                    item.collect_places(out);
                }
            }
            DestructureTargetKind::Sequence(items) => {
                for item in items {
                    match item {
                        SequenceTarget::Element(target) | SequenceTarget::Star(target) => {
                            target.collect_places(out);
                        }
                    }
                }
            }
            DestructureTargetKind::Discard | DestructureTargetKind::Declare { .. } => {}
        }
    }

    fn collect_places_mut<'a>(&'a mut self, out: &mut Vec<&'a mut Expr>) {
        match &mut self.kind {
            DestructureTargetKind::Place(place) => out.push(place),
            DestructureTargetKind::Tuple(items) => {
                for item in items {
                    item.collect_places_mut(out);
                }
            }
            DestructureTargetKind::Sequence(items) => {
                for item in items {
                    match item {
                        SequenceTarget::Element(target) | SequenceTarget::Star(target) => {
                            target.collect_places_mut(out);
                        }
                    }
                }
            }
            DestructureTargetKind::Discard | DestructureTargetKind::Declare { .. } => {}
        }
    }
}

impl From<DestructureTargetKind> for DestructureTarget {
    fn from(kind: DestructureTargetKind) -> Self {
        Self::new(kind, mojito_common::token::DUMMY_SPAN)
    }
}

impl PartialEq for DestructureTarget {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

/// The forms of a [`DestructureTarget`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DestructureTargetKind {
    /// `_` — the piece is discarded (not read).
    Discard,
    /// `var name` or `ref name` — a new binding; a `var`/`ref` before a
    /// tuple or sequence target distributes to every leaf name in it. A
    /// bare name in a loop target is `Immutable`: an immutable binding to
    /// its piece (upstream's `Bind` pattern).
    Declare { name: String, mode: BindingMode },
    /// An existing name or a place expression (`a`, `p.x`, `xs[i]`), written.
    Place(Expr),
    /// `(t, u)`, `t, u`, `()` — the value's tuple elements, in order.
    Tuple(Vec<DestructureTarget>),
    /// `[t, *rest, u]` — the value's sequence elements, with at most one star.
    Sequence(Vec<SequenceTarget>),
}

/// One element of a sequence target.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SequenceTarget {
    Element(DestructureTarget),
    /// `*target`: the slice between the fixed prefix and suffix.
    Star(DestructureTarget),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[allow(clippy::large_enum_variant)]
pub enum StmtKind {
    /// `var name[: Type] = value` — declares (and initializes) a new variable.
    /// The annotation is optional: `ty: None` is an **inferred** `var` (the type
    /// comes from `value`, materializing a literal to its default kind).
    VarDecl {
        name: String,
        ty: Option<Type>,
        value: Expr,
    },
    /// `ref name = place` — a local reference binding. It aliases the source
    /// place without copying and participates in persistent loan analysis.
    RefDecl { name: String, value: Expr },
    /// `name = value` — re-assigns an already-declared variable (every `var` is
    /// mutable). Distinct from `VarDecl`; the value must keep the declared type.
    Assign { name: String, value: Expr },
    /// `target OP= value` — augmented assignment, where `target` is a `NAME` or a
    /// **place** (`Expr::Identifier`/`Member`/`Index`). Means `target = target OP
    /// value`, but the target (and any index within it) is evaluated once. `op` is
    /// one of `+ - * / // % **`.
    AugAssign {
        place: Expr,
        op: InfixOp,
        value: Expr,
    },
    /// `target = value` — **destructuring assignment** through a recursive
    /// [`DestructureTarget`]: `a, b = t`, `var a, b = t`,
    /// `(var a, (ref b, _)) = pair`, `var (x, y) = e`, `[first, *rest] =
    /// values`, nested places. The value is evaluated once and its pieces
    /// distributed to the target's leaves.
    Destructure {
        target: DestructureTarget,
        value: Expr,
    },
    /// `place = value` — assign through a **place expression** rooted at a
    /// mutable variable (or `mut self`): a field write `p.x = e`, a list-element
    /// write `xs[i] = e`, or any nesting (`p.items[i].x = e`). The place is a
    /// chain of `Expr::Member` / `Expr::Index` over an `Expr::Identifier` root.
    /// Mutation happens in place in the root variable's binding (value semantics
    /// preserved: only that binding changes).
    SetPlace { place: Expr, value: Expr },
    /// `[decorators] def name[type_params](params) [raises] -> ret: <body>`
    Def {
        name: String,
        /// Decorators preceding the function (`@always_inline`, …), validated by
        /// the checker.
        decorators: Vec<Decorator>,
        /// Type parameters (`[T: Trait]`); empty for a non-generic function.
        type_params: Vec<TypeParam>,
        params: Vec<FnParam>,
        /// Index of a `/` (positional-only) marker in `params`, if present (parsed).
        positional_only: Option<usize>,
        /// Index of a bare `*` (keyword-only) marker in `params`, if present (parsed).
        keyword_only: Option<usize>,
        /// Current Mojo closure captures. `None` is an ordinary nested function
        /// (which cannot capture); `Some` is the explicit `{ ... }` environment.
        /// The parser also accepts legacy `unified { ... }` as a Mojito-only
        /// compatibility spelling, but normalizes both forms here.
        captures: Option<CaptureList>,
        /// Whether the `raises` effect was declared. A following typed-error
        /// annotation is currently parsed but not retained.
        raises: bool,
        /// The typed error following `raises`, if present.
        raises_type: Option<Type>,
        ret: Option<Type>,
        where_clauses: Vec<Expr>,
        body: Vec<Stmt>,
    },
    /// `[decorators] struct Name[type_params](conforms): <fields, associated
    /// comptime facts, and methods>`
    Struct {
        name: String,
        /// Decorators preceding the struct (`@fieldwise_init`, …), validated by
        /// the checker; `@fieldwise_init` also sets `fieldwise_init`.
        decorators: Vec<Decorator>,
        /// Type parameters (`[T: Trait]`); empty for a non-generic struct.
        type_params: Vec<TypeParam>,
        /// Traits this struct declares conformance to (`struct S(A, B):`); empty
        /// if none. Nominal — a `[T: A]` bound is satisfied only if `A` is here.
        conforms: Vec<String>,
        /// The nominal closure trait (`def(...) -> ...`), when declared.
        callable_conformance: Option<Type>,
        /// Conditions attached to entries in `conforms`, represented as
        /// `(trait_name, comptime predicate)`. An absent entry is unconditional.
        conformance_conditions: Vec<(String, Expr)>,
        /// Trailing declaration constraint (`struct S[...] where ...:`).
        where_clauses: Vec<Expr>,
        fields: Vec<Param>,
        associated: Vec<StructComptime>,
        methods: Vec<Method>,
        /// Whether `@fieldwise_init` was present (so `Name(...)` constructs).
        fieldwise_init: bool,
        /// The cases of an `enum` declaration, in order; `None` for a
        /// `struct`. An enum is a struct whose members the compiler generates
        /// from these (`comptime::synthesize_enum_members`), as upstream's
        /// `StructDeclOp` carries `isEnum` and its case ops.
        enum_cases: Option<Vec<EnumCase>>,
    },
    /// `trait Name[(Super, …)]: <members>` — declares method (and `comptime`
    /// member) requirements that a conforming struct must implement. `refines` is
    /// the optional parenthesized list of **super-traits** (trait inheritance);
    /// `comptime_members` are `comptime NAME: Type` requirements. A method with
    /// a body is a default, which a conformer inherits through a forwarding
    /// wrapper. (Generic traits `trait T[U]:` are not valid current Mojo, so
    /// they are not parsed.)
    Trait {
        name: String,
        refines: Vec<String>,
        methods: Vec<TraitMethod>,
        comptime_members: Vec<TraitComptime>,
    },
    /// `comptime NAME = value` — a compile-time `Int` constant (Mojo removed
    /// `alias`; `comptime` replaces it). `value` must be a comptime `Int`
    /// expression; the constant is usable as a value-parameter argument and as an
    /// ordinary `Int` at runtime.
    Comptime {
        name: String,
        /// Retained for generic compile-time aliases. Ordinary constants leave
        /// this empty.
        type_params: Vec<TypeParam>,
        /// Optional declared result/value type.
        ty: Option<Type>,
        /// Optional availability constraint, including its diagnostic message.
        where_clauses: Vec<Expr>,
        value: Expr,
    },
    /// `comptime if cond: ... (elif cond: ...)* (else: ...)?` — a **compile-time
    /// conditional**. Compile-time elaboration selects one branch before semantic
    /// checking, so discarded branches do not need to type-check.
    ComptimeIf {
        branches: Vec<(Condition, Vec<Stmt>)>,
        orelse: Option<Vec<Stmt>>,
    },
    /// `comptime assert cond[, message]` — a **compile-time assertion**
    /// (upstream's `kgen.param.assert`). Allowed only in a function body; the
    /// elaborator judges it per reached instance, and for the rest of its
    /// block the asserted condition is known, as a `comptime if` arm's is.
    ComptimeAssert { cond: Expr, message: Option<Expr> },
    /// `comptime match subject: case ...` — a **compile-time match** over a
    /// parameter value or type. Same shape as `Match`; elaboration selects
    /// one case per instance after every case is checked.
    ComptimeMatch {
        subject: Expr,
        cases: Vec<MatchCase>,
    },
    /// `comptime for var in iter: <body>` — a **compile-time (unrolled) loop**
    /// (Mojo's modern spelling; the older `@parameter for` is deprecated). Same
    /// shape as `For`. Compile-time elaboration unrolls it under a fuel quota.
    ComptimeFor {
        var: String,
        iter: Expr,
        body: Vec<Stmt>,
    },
    /// `if cond: ... (elif cond: ...)* (else: ...)?`
    ///
    /// `branches` holds the `if` plus each `elif` as a `(condition, body)` pair,
    /// tried in order; `orelse` is the optional `else` body. A condition is a
    /// boolean expression or a `case pattern = value` test whose bindings
    /// the branch body sees.
    If {
        branches: Vec<(Condition, Vec<Stmt>)>,
        orelse: Option<Vec<Stmt>>,
    },
    /// `while cond: <body>`
    While {
        cond: Condition,
        body: Vec<Stmt>,
        orelse: Option<Vec<Stmt>>,
    },
    /// `match subject: case pattern [if guard]: <body> ...` — the subject is
    /// evaluated once and the cases tried in order; the first whose pattern
    /// matches and whose guard holds runs. A comma-separated subject forms a
    /// tuple. Cases may share the `match` indent or sit one level in.
    Match {
        subject: Expr,
        cases: Vec<MatchCase>,
    },
    /// `for target in iter: <body>` — `target` is a recursive
    /// [`DestructureTarget`] whose bare names bind immutably; after the check
    /// it is one `Declare` leaf, the user's name or the hidden binder whose
    /// destructuring the body starts with.
    For {
        target: DestructureTarget,
        iter: Expr,
        body: Vec<Stmt>,
        orelse: Option<Vec<Stmt>>,
    },
    /// `return` or `return expr`
    Return(Option<Expr>),
    /// `raise expr` — raise an error (an `Error` value, or a `String` shorthand).
    Raise(Expr),
    /// `import a.b.c [as alias]`. The linker resolves the source module/package
    /// and rewrites qualified member uses to module-unique linked symbols.
    Import {
        path: Vec<String>,
        alias: Option<String>,
    },
    /// `from [.]*module import <targets>`. `level` is the number of leading dots
    /// (0 = absolute; relative imports raise it); `path` is the dotted module
    /// name (possibly empty for `from . import x`). The linker resolves wildcard
    /// and selective imports, including member aliases and package re-exports.
    FromImport {
        level: usize,
        path: Vec<String>,
        names: ImportNames,
    },
    /// `with item (',' item)*: <body>` — a context-manager block, where each
    /// `item` is a `WithItem` (a context expression + optional `as NAME`). Parsed
    /// and grammar-documented; the checker rejects it until the
    /// `__enter__`/`__exit__` protocol is implemented.
    With {
        items: Vec<WithItem>,
        body: Vec<Stmt>,
    },
    /// `try: <body> [except [e]: ...] [else: ...] [finally: ...]`. At least one of
    /// `except`/`finally` is present. `except` optionally binds the error name.
    Try {
        body: Vec<Stmt>,
        except: Option<(Option<String>, Vec<Stmt>)>,
        orelse: Option<Vec<Stmt>>,
        finalbody: Option<Vec<Stmt>>,
    },
    /// A block compile-time elaboration kept that declares a binding — one
    /// unrolled copy of a `comptime for` body, or the arm a `comptime if`
    /// selected: a lexical scope with no runtime effect of its own, so each
    /// iteration's locals are its own and an arm's end with it. Elaboration
    /// emits it with the compile-time statement's identity; the parser never
    /// produces one.
    Scope(Vec<Stmt>),
    /// `pass`
    Pass,
    /// `break` — exit the innermost loop.
    Break,
    /// `continue` — skip to the next iteration of the innermost loop.
    Continue,
    /// A bare expression used for its side effects, e.g. `f(1)`.
    Expr(Expr),
}

/// The explicit environment of a nested closure.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CaptureList {
    pub entries: Vec<Capture>,
    /// A bare convention (`imm`, `mut`, `ref`, `var`, or `var^`) applies to
    /// otherwise-unlisted free variables. At most one default is permitted.
    pub default: Option<CaptureKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Capture {
    pub name: String,
    pub kind: CaptureKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CaptureKind {
    /// Immutable reference (`imm`; the legacy `read` spelling rejects).
    Imm,
    /// Mutable reference.
    Mut,
    /// Reference whose mutability is inherited from the captured origin.
    Ref,
    /// Owned copy (`var name`).
    Copy,
    /// Owned transfer (`var name^`, bare `name^`, or default `var^`).
    Move,
}

/// An expression node: its [`ExprKind`] plus the source [`Span`] it was parsed
/// from. Equality ignores the span (see the module note), so `a == b` iff their
/// kinds are structurally equal.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
    /// Stamped as [`Stmt::module`] is, and likewise left out of an encoding:
    /// a decoded expression is stamped with the module [`decoding_from`]
    /// names, if any.
    #[serde(with = "expression_source")]
    pub source: Option<String>,
    /// Concrete occurrence identity; ignored by structural AST equality. A
    /// decoded expression draws a fresh one.
    #[serde(with = "mojito_common::token::node_syntax")]
    pub syntax_id: mojito_common::token::SyntaxId,
}

impl Expr {
    /// Construct a spanned expression.
    pub fn new(kind: ExprKind, span: Span) -> Self {
        Self {
            kind,
            span,
            source: None,
            syntax_id: mojito_common::token::SyntaxId::fresh(),
        }
    }

    pub fn source_span(&self) -> mojito_common::token::SourceSpan {
        mojito_common::token::SourceSpan::syntax(self.source.clone(), self.span, self.syntax_id)
    }

    /// The name a bracket after an identifier applies and the compile-time
    /// operands it spells, upstream's `SubscriptNode` read before its base is
    /// classified: a parameter application (`S[Int]`), or a subscript with no
    /// slice operand (`twice[4]`, `pair[a=3, b=4]`, `total[]`). The view
    /// never says what the name denotes; each caller decides that.
    pub fn bracket_application(&self) -> Option<(&str, Cow<'_, [ParamArg]>)> {
        let (object, args) = match &self.kind {
            ExprKind::TypeApply { name, args } => return Some((name, Cow::Borrowed(args))),
            ExprKind::Index { object, index } => (object, subscript_param_args(index)),
            ExprKind::MultiIndex { object, args } => (object, multi_subscript_param_args(args)?),
            _ => return None,
        };
        match &object.kind {
            ExprKind::Identifier(name) => Some((name, Cow::Owned(args))),
            _ => None,
        }
    }

    /// The type a type-shaped expression spells: a capitalized name, `Self`,
    /// a parameter application (`S[Int]`, or the one-operand subscript
    /// `S[1]`), or a member chain rooted at one (`M.Inner`, `Self.T`). `None`
    /// for a value-shaped expression, whose lowercase root names a binding.
    pub fn type_spelling(&self) -> Option<Type> {
        match &self.kind {
            ExprKind::Identifier(name) if name == "Self" => Some(Type::SelfType),
            ExprKind::Identifier(name) if !name.chars().next()?.is_lowercase() => {
                Some(match name.as_str() {
                    "Int" => Type::Int,
                    "UInt" => Type::UInt,
                    "Bool" => Type::Bool,
                    "StringLiteral" => Type::StringLiteral,
                    "Float64" => Type::Float64,
                    _ => Type::Named(name.clone(), Vec::new()),
                })
            }
            ExprKind::TypeApply { name, args } => Some(Type::Named(name.clone(), args.clone())),
            ExprKind::Index { object, index }
                if let ExprKind::Identifier(name) = &object.kind
                    && !name.chars().next()?.is_lowercase() =>
            {
                Some(Type::Named(
                    name.clone(),
                    vec![ParamArg::Value((**index).clone())],
                ))
            }
            ExprKind::Member { object, field } => Some(match object.type_spelling()? {
                Type::SelfType => Type::SelfParam(field.clone()),
                base => Type::Assoc {
                    base: Box::new(base),
                    name: field.clone(),
                    args: Vec::new(),
                },
            }),
            _ => None,
        }
    }
}

/// Wrap a bare [`ExprKind`] into an [`Expr`] with a dummy span. Convenient for
/// synthesizing expressions and for building AST literals in tests (where the
/// span is irrelevant — [`Expr`]'s equality ignores it).
impl From<ExprKind> for Expr {
    fn from(kind: ExprKind) -> Self {
        Self::new(kind, mojito_common::token::DUMMY_SPAN)
    }
}

/// Span-agnostic structural equality: two expressions are equal iff their kinds
/// are equal, regardless of where in the source each was parsed.
impl PartialEq for Expr {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ExprKind {
    Int(mojito_common::literal::IntLiteral),
    Float(mojito_common::literal::FloatLiteral),
    Bool(bool),
    Str(String),
    None,
    /// Compiler-internal marker for `var name: Type` without an initializer.
    Uninitialized,
    /// Parser-internal marker for the empty subscript `p[]`: the index child
    /// of an `Index` node spelling a pointer dereference at offset 0. Distinct
    /// from `None` so a source `p[None]` cannot pose as a dereference.
    EmptySubscript,
    Identifier(String),
    /// `__is_run_in_comptime_interpreter`: a `Bool` that is `True` while the
    /// comptime interpreter runs the enclosing body and `False` in the
    /// compiled program — a run-time value, never folded before a backend
    /// reads it (upstream's `kgen.is_run_in_comptime_interpreter`).
    IsRunInComptimeInterpreter,
    /// A type used as a compile-time value, such as a function/closure signature.
    /// Parsed for source compatibility; semantic treatment as a first-class
    /// compile-time value is deferred.
    TypeValue(Type),
    /// A unary operator applied to an operand, e.g. `-x` or `not ok`.
    Prefix(PrefixOp, Box<Expr>),
    /// A binary operator applied to two operands, e.g. `a + b`.
    Infix(InfixOp, Box<Expr>, Box<Expr>),
    /// A call by name: `name[param_args](args)`. The callee is resolved in scope,
    /// so this covers `def`s, supported nested defs, built-ins, and struct
    /// construction. `param_args` are the explicit compile-time parameter
    /// arguments (`Pair[Int](...)`, `FixedBuffer[8](...)`); empty when omitted, in
    /// which case the checker infers the type parameters from `args`.
    /// `kwargs` are keyword arguments (`name=value`) and use the same structural
    /// matcher as positional/default/variadic arguments.
    Call {
        name: String,
        param_args: Vec<ParamArg>,
        args: Vec<Expr>,
        kwargs: Vec<KwArg>,
    },
    /// A call whose callee is an expression rather than a bare source name,
    /// including calls through fields and parameterized callable values.
    Invoke {
        callee: Box<Expr>,
        param_args: Vec<ParamArg>,
        args: Vec<Expr>,
        kwargs: Vec<KwArg>,
    },
    /// Field access on a struct value: `object.field`.
    Member {
        object: Box<Expr>,
        field: String,
    },
    /// A method call on a struct value: `object.method(args)`.
    MethodCall {
        object: Box<Expr>,
        method: String,
        args: Vec<Expr>,
        kwargs: Vec<KwArg>,
    },
    /// A subscript `object[index]` — supported for lists, tuples, SIMD, unsafe
    /// pointers, and user types implementing `__getitem__`.
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    /// A parameterized type in expression position — `Name[param_args]` — used as
    /// the receiver of a static method call, e.g. `UnsafePointer[T].alloc(n)`.
    /// Only meaningful there; a bare `TypeApply` is a type, not a runtime value.
    TypeApply {
        name: String,
        args: Vec<ParamArg>,
    },
    /// Positional call-argument expansion (`*values`). Heterogeneous pack
    /// specialization expands this into ordinary arguments before checking.
    Spread(Box<Expr>),
    /// The transfer sigil `expr^`: an ownership move checked by MIR dataflow,
    /// including field-sensitive partial moves.
    Transfer(Box<Expr>),
    /// A non-empty list literal `[a, b, …]` — a `List` whose element type is
    /// inferred from the elements.
    ListLit(Vec<Expr>),
    /// A brace-delimited set/dictionary literal. Entries retain an optional
    /// value (`key: value`). All entries must consistently be set elements or
    /// dictionary pairs; `{}` is the empty dictionary display.
    BraceLit(Vec<(Expr, Option<Expr>)>),
    /// A brace display with a keyword entry (`{1, kw=2}`): upstream's
    /// initializer list, which only the construction `T(1, kw=2)` of the
    /// type its context expects can mean. A brace without one stays a
    /// [`ExprKind::BraceLit`] until its context decides it.
    InitializerList {
        args: Vec<Expr>,
        kwargs: Vec<KwArg>,
    },
    /// A Python-style collection comprehension. Clauses are retained in source
    /// order because each iterable/filter can observe effects from all clauses
    /// to its left. `key` is present only for a dictionary comprehension; `value`
    /// is the list/set element or dictionary value.
    Comprehension {
        kind: CollectionKind,
        key: Option<Box<Expr>>,
        value: Box<Expr>,
        clauses: Vec<ComprehensionClause>,
    },
    /// A lambda expression `lambda [params] [(args)] [effects] [{captures}]
    /// [-> T]: expr`. The parser synthesizes the contained statement, which is
    /// always a `StmtKind::Def` with an unspellable hidden name
    /// (`$lambda$<start-offset>`), body `[Return(Some(<the body expression>))]`,
    /// and `captures` exactly as written (`None` ⇔ the list was omitted, which
    /// for a lambda means free variables imm-capture by default). The
    /// expression's value is that definition's function value, materialized at
    /// the expression's evaluation point.
    Lambda {
        def: Box<Stmt>,
    },
    /// A tuple literal `(a, b, …)` — a fixed-size, heterogeneous `Tuple`. `()` is
    /// the empty tuple and `(a,)` a 1-tuple; a plain `(e)` is grouping, not a tuple.
    TupleLit(Vec<Expr>),
    /// A named expression (walrus) `target := value`: stores the value
    /// through the lvalue `target`, as `target = value` does, and yields it.
    Named {
        target: Box<Expr>,
        value: Box<Expr>,
    },
    /// A conditional expression (ternary) `then_branch if cond else else_branch`.
    /// Checked and lowered with ordinary conditional control flow.
    IfExpr {
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
    /// A chained comparison `a < b < c …` (Python semantics: `a < b and b < c`,
    /// each operand evaluated once). `rest` is the `(op, operand)` sequence after
    /// `first`. A single comparison stays an `Infix`; this node is only built for a
    /// chain of length ≥ 2. Lowering evaluates every operand at most once and
    /// short-circuits the comparisons.
    Compare {
        first: Box<Expr>,
        rest: Vec<(InfixOp, Expr)>,
    },
    /// A slice subscript `object[lower:upper:step]`, each bound optional
    /// (`a[1:2]`, `a[:j]`, `a[::2]`). Supported for lists and strings.
    Slice {
        object: Box<Expr>,
        lower: Option<Box<Expr>>,
        upper: Option<Box<Expr>>,
        step: Option<Box<Expr>>,
    },
    /// Multiple subscript arguments, including mixed indices and slices:
    /// `matrix[row, 1:5:2]` → `matrix.__getitem__(row, <slice>)`.
    MultiIndex {
        object: Box<Expr>,
        args: Vec<SubscriptArg>,
    },
    /// A t-string `t"…{expr}…"` (or raw `rt"…"`, `raw = true`): alternating
    /// literal text and interpolated expressions. Each `{…}` is a fully parsed
    /// sub-expression. The checker currently rejects t-string semantics.
    TString {
        parts: Vec<TStringPart>,
        raw: bool,
    },
}

/// The result family selected by collection-display delimiters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CollectionKind {
    List,
    Set,
    Dict,
}

/// One left-to-right generator/filter clause in a collection comprehension.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ComprehensionClause {
    /// `for target in iter`, with `target` as in [`StmtKind::For`].
    /// `unpack` is the checker's desugar of a destructuring target, a
    /// `Destructure` of the hidden binder; the parser leaves it `None`.
    For {
        target: Box<DestructureTarget>,
        iter: Box<Expr>,
        unpack: Option<Box<Stmt>>,
    },
    If(Box<Expr>),
}

/// One piece of a t-string: literal text or an interpolated expression.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TStringPart {
    Literal(String),
    Expr(Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PrefixOp {
    /// Arithmetic negation, `-`.
    Neg,
    /// Logical negation, `not`.
    Not,
    /// Bitwise inversion, `~`.
    Invert,
}

impl PrefixOp {
    /// The operator as source spells it.
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Neg => "-",
            Self::Not => "not",
            Self::Invert => "~",
        }
    }

    /// The dunder a prefix operator dispatches to on a user `struct` operand
    /// (`-x` → `x.__neg__()`, `not x` → `not x.__bool__()`). Shared by the
    /// checker (typing) and the VM (runtime dispatch) so they agree.
    pub const fn dunder(self) -> &'static str {
        match self {
            Self::Neg => "__neg__",
            Self::Not => "__bool__",
            Self::Invert => "__invert__",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum InfixOp {
    Add,
    Sub,
    Mul,
    Div,
    FloorDiv,
    Mod,
    MatMul,
    Shl,
    Shr,
    BitAnd,
    BitOr,
    BitXor,
    Pow,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
    /// Membership: `x in container` → `Bool`.
    In,
    /// Non-membership: `x not in container` → `Bool`.
    NotIn,
    /// Identity: `x is y` → `Bool`, dispatching to `__is__` on the left
    /// operand (current Mojo: `Optional.__is__(self, other: NoneType)`).
    Is,
    /// Negated identity: `x is not y` → `Bool`, dispatching to `__isnot__`.
    IsNot,
}

impl InfixOp {
    /// Every binary operator.
    pub const ALL: [Self; 25] = [
        Self::Add,
        Self::Sub,
        Self::Mul,
        Self::Div,
        Self::FloorDiv,
        Self::Mod,
        Self::MatMul,
        Self::Shl,
        Self::Shr,
        Self::BitAnd,
        Self::BitOr,
        Self::BitXor,
        Self::Pow,
        Self::Eq,
        Self::Ne,
        Self::Lt,
        Self::Gt,
        Self::Le,
        Self::Ge,
        Self::And,
        Self::Or,
        Self::In,
        Self::NotIn,
        Self::Is,
        Self::IsNot,
    ];

    /// The operator as source spells it.
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
            Self::FloorDiv => "//",
            Self::Mod => "%",
            Self::MatMul => "@",
            Self::Shl => "<<",
            Self::Shr => ">>",
            Self::BitAnd => "&",
            Self::BitOr => "|",
            Self::BitXor => "^",
            Self::Pow => "**",
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Gt => ">",
            Self::Le => "<=",
            Self::Ge => ">=",
            Self::And => "and",
            Self::Or => "or",
            Self::In => "in",
            Self::NotIn => "not in",
            Self::Is => "is",
            Self::IsNot => "is not",
        }
    }

    /// The operator whose [`Self::dunder`] is `method`, or whose
    /// [`Self::reflected_dunder`] is when the flag is set.
    pub fn of_dunder(method: &str) -> Option<(Self, bool)> {
        Self::ALL.into_iter().find_map(|op| {
            if op.dunder() == Some(method) {
                Some((op, false))
            } else if op.reflected_dunder() == Some(method) {
                Some((op, true))
            } else {
                None
            }
        })
    }

    /// The dunder method a binary operator dispatches to on a user `struct` left
    /// operand (`a + b` → `a.__add__(b)`), or `None` for operators that don't
    /// dispatch this way (`and`/`or` short-circuit; `in`/`not in` dispatch to
    /// `__contains__` on the *right* operand — see the checker/VM). Shared by the
    /// checker (typing) and the VM (runtime dispatch) so they agree.
    pub const fn dunder(self) -> Option<&'static str> {
        Some(match self {
            Self::Add => "__add__",
            Self::Sub => "__sub__",
            Self::Mul => "__mul__",
            Self::Div => "__truediv__",
            Self::FloorDiv => "__floordiv__",
            Self::Mod => "__mod__",
            Self::MatMul => "__matmul__",
            Self::Shl => "__lshift__",
            Self::Shr => "__rshift__",
            Self::BitAnd => "__and__",
            Self::BitOr => "__or__",
            Self::BitXor => "__xor__",
            Self::Pow => "__pow__",
            Self::Eq => "__eq__",
            Self::Ne => "__ne__",
            Self::Lt => "__lt__",
            Self::Gt => "__gt__",
            Self::Le => "__le__",
            Self::Ge => "__ge__",
            Self::Is => "__is__",
            Self::IsNot => "__isnot__",
            Self::And | Self::Or | Self::In | Self::NotIn => return None,
        })
    }

    /// The reflected dunder the *right* operand answers when the left
    /// operand has no operator method for the pair (`1 + m` →
    /// `m.__radd__(1)`), or `None` for operators without a reflected form
    /// (comparisons, identity, membership, and the short-circuit operators).
    pub const fn reflected_dunder(self) -> Option<&'static str> {
        Some(match self {
            Self::Add => "__radd__",
            Self::Sub => "__rsub__",
            Self::Mul => "__rmul__",
            Self::Div => "__rtruediv__",
            Self::FloorDiv => "__rfloordiv__",
            Self::Mod => "__rmod__",
            Self::MatMul => "__rmatmul__",
            Self::Shl => "__rlshift__",
            Self::Shr => "__rrshift__",
            Self::BitAnd => "__rand__",
            Self::BitOr => "__ror__",
            Self::BitXor => "__rxor__",
            Self::Pow => "__rpow__",
            Self::Eq
            | Self::Ne
            | Self::Lt
            | Self::Gt
            | Self::Le
            | Self::Ge
            | Self::Is
            | Self::IsNot
            | Self::And
            | Self::Or
            | Self::In
            | Self::NotIn => return None,
        })
    }

    /// The in-place dunder an augmented assignment dispatches to on a user
    /// `struct` target (`a += b` → `a.__iadd__(b)`), or `None` for operators with
    /// no augmented form. The parser-accepted augmented operators (see
    /// `aug_assign_op`) have an in-place dunder;
    /// Mojo calls this dedicated method rather than the ordinary binary dunder.
    pub const fn inplace_dunder(self) -> Option<&'static str> {
        Some(match self {
            Self::Add => "__iadd__",
            Self::Sub => "__isub__",
            Self::Mul => "__imul__",
            Self::Div => "__itruediv__",
            Self::FloorDiv => "__ifloordiv__",
            Self::Mod => "__imod__",
            Self::Pow => "__ipow__",
            Self::BitAnd => "__iand__",
            Self::BitOr => "__ior__",
            Self::BitXor => "__ixor__",
            Self::Shl => "__ilshift__",
            Self::Shr => "__irshift__",
            _ => return None,
        })
    }
}

/// Collect the lambda expression nodes in `expr`, outermost first.
///
/// The walk does **not** descend into a collected lambda's own body — inner
/// lambdas belong to the inner definition's scope and are found when that body
/// is processed. Type positions are not walked: a lambda is a runtime
/// expression.
pub fn lambdas_in_expr<'a>(expr: &'a Expr, out: &mut Vec<&'a Expr>) {
    match &expr.kind {
        ExprKind::Lambda { .. } => out.push(expr),
        ExprKind::Prefix(_, value) | ExprKind::Transfer(value) | ExprKind::Spread(value) => {
            lambdas_in_expr(value, out);
        }
        ExprKind::Infix(_, left, right)
        | ExprKind::Named {
            target: left,
            value: right,
        }
        | ExprKind::Index {
            object: left,
            index: right,
        } => {
            lambdas_in_expr(left, out);
            lambdas_in_expr(right, out);
        }
        ExprKind::Call {
            param_args,
            args,
            kwargs,
            ..
        } => {
            lambdas_in_param_args(param_args, out);
            for arg in args {
                lambdas_in_expr(arg, out);
            }
            for arg in kwargs {
                lambdas_in_expr(&arg.value, out);
            }
        }
        ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } => {
            lambdas_in_expr(callee, out);
            lambdas_in_param_args(param_args, out);
            for arg in args {
                lambdas_in_expr(arg, out);
            }
            for arg in kwargs {
                lambdas_in_expr(&arg.value, out);
            }
        }
        ExprKind::Member { object, .. } => lambdas_in_expr(object, out),
        ExprKind::MethodCall {
            object,
            args,
            kwargs,
            ..
        } => {
            lambdas_in_expr(object, out);
            for arg in args {
                lambdas_in_expr(arg, out);
            }
            for arg in kwargs {
                lambdas_in_expr(&arg.value, out);
            }
        }
        ExprKind::TypeApply { args, .. } => lambdas_in_param_args(args, out),
        ExprKind::InitializerList { args, kwargs } => {
            for arg in args {
                lambdas_in_expr(arg, out);
            }
            for arg in kwargs {
                lambdas_in_expr(&arg.value, out);
            }
        }
        ExprKind::ListLit(items) | ExprKind::TupleLit(items) => {
            for item in items {
                lambdas_in_expr(item, out);
            }
        }
        ExprKind::BraceLit(entries) => {
            for (key, value) in entries {
                lambdas_in_expr(key, out);
                if let Some(value) = value {
                    lambdas_in_expr(value, out);
                }
            }
        }
        ExprKind::Comprehension {
            key,
            value,
            clauses,
            ..
        } => {
            for clause in clauses {
                match clause {
                    ComprehensionClause::For { target, iter, .. } => {
                        for place in target.places() {
                            lambdas_in_expr(place, out);
                        }
                        lambdas_in_expr(iter, out);
                    }
                    ComprehensionClause::If(cond) => lambdas_in_expr(cond, out),
                }
            }
            if let Some(key) = key {
                lambdas_in_expr(key, out);
            }
            lambdas_in_expr(value, out);
        }
        ExprKind::IfExpr {
            cond,
            then_branch,
            else_branch,
        } => {
            lambdas_in_expr(cond, out);
            lambdas_in_expr(then_branch, out);
            lambdas_in_expr(else_branch, out);
        }
        ExprKind::Compare { first, rest } => {
            lambdas_in_expr(first, out);
            for (_, value) in rest {
                lambdas_in_expr(value, out);
            }
        }
        ExprKind::Slice {
            object,
            lower,
            upper,
            step,
            ..
        } => {
            lambdas_in_expr(object, out);
            for value in [lower, upper, step].into_iter().flatten() {
                lambdas_in_expr(value, out);
            }
        }
        ExprKind::MultiIndex { object, args } => {
            lambdas_in_expr(object, out);
            for argument in args {
                match argument {
                    SubscriptArg::Index(value) | SubscriptArg::Keyword { value, .. } => {
                        lambdas_in_expr(value, out);
                    }
                    SubscriptArg::Slice {
                        lower, upper, step, ..
                    }
                    | SubscriptArg::KeywordSlice {
                        lower, upper, step, ..
                    } => {
                        for value in [lower, upper, step].into_iter().flatten() {
                            lambdas_in_expr(value, out);
                        }
                    }
                }
            }
        }
        ExprKind::TString { parts, .. } => {
            for part in parts {
                if let TStringPart::Expr(value) = part {
                    lambdas_in_expr(value, out);
                }
            }
        }
        ExprKind::TypeValue(_)
        | ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::None
        | ExprKind::Uninitialized
        | ExprKind::EmptySubscript
        | ExprKind::IsRunInComptimeInterpreter
        | ExprKind::Identifier(_) => {}
    }
}

/// Collect the lambda expression nodes in `stmt`'s **own** expressions,
/// outermost first.
///
/// Deliberately shallow: nested statement bodies are not walked, because every
/// caller already recurses over blocks and a lambda in an inner statement
/// belongs to that statement's visit.
pub fn lambdas_in_stmt<'a>(stmt: &'a Stmt, out: &mut Vec<&'a Expr>) {
    match &stmt.kind {
        StmtKind::VarDecl { value, .. }
        | StmtKind::RefDecl { value, .. }
        | StmtKind::Assign { value, .. }
        | StmtKind::Comptime { value, .. }
        | StmtKind::Return(Some(value))
        | StmtKind::Raise(value)
        | StmtKind::Expr(value) => lambdas_in_expr(value, out),
        StmtKind::AugAssign { place, value, .. } | StmtKind::SetPlace { place, value } => {
            lambdas_in_expr(place, out);
            lambdas_in_expr(value, out);
        }
        StmtKind::Destructure { target, value } => {
            for place in target.places() {
                lambdas_in_expr(place, out);
            }
            lambdas_in_expr(value, out);
        }
        StmtKind::If { branches, .. } | StmtKind::ComptimeIf { branches, .. } => {
            for (cond, _) in branches {
                lambdas_in_condition(cond, out);
            }
        }
        StmtKind::While { cond, .. } => lambdas_in_condition(cond, out),
        StmtKind::ComptimeAssert { cond, message } => {
            lambdas_in_expr(cond, out);
            if let Some(message) = message {
                lambdas_in_expr(message, out);
            }
        }
        StmtKind::For { target, iter, .. } => {
            for place in target.places() {
                lambdas_in_expr(place, out);
            }
            lambdas_in_expr(iter, out);
        }
        StmtKind::ComptimeFor { iter, .. } => lambdas_in_expr(iter, out),
        StmtKind::Match { subject, cases } | StmtKind::ComptimeMatch { subject, cases } => {
            lambdas_in_expr(subject, out);
            for case in cases {
                for value in case.pattern.expressions() {
                    lambdas_in_expr(value, out);
                }
                if let Some(guard) = &case.guard {
                    lambdas_in_expr(guard, out);
                }
            }
        }
        StmtKind::With { items, .. } => {
            for item in items {
                lambdas_in_expr(&item.context, out);
            }
        }
        StmtKind::Def { .. }
        | StmtKind::Struct { .. }
        | StmtKind::Trait { .. }
        | StmtKind::Try { .. }
        | StmtKind::Scope(_)
        | StmtKind::Return(None)
        | StmtKind::Import { .. }
        | StmtKind::FromImport { .. }
        | StmtKind::Pass
        | StmtKind::Break
        | StmtKind::Continue => {}
    }
}

fn lambdas_in_condition<'a>(condition: &'a Condition, out: &mut Vec<&'a Expr>) {
    if let Some(pattern) = condition.pattern() {
        for value in pattern.expressions() {
            lambdas_in_expr(value, out);
        }
    }
    lambdas_in_expr(condition.value(), out);
}

fn lambdas_in_param_args<'a>(args: &'a [ParamArg], out: &mut Vec<&'a Expr>) {
    for arg in args {
        match arg {
            ParamArg::Type(_) => {}
            ParamArg::Value(value) => lambdas_in_expr(value, out),
            ParamArg::Named { value, .. } => {
                lambdas_in_param_args(std::slice::from_ref(value.as_ref()), out);
            }
        }
    }
}

/// Ensure a unique identity for every statement and expression occurrence in a
/// final syntax tree.
///
/// This must run after cloning transformations (trait-default expansion and
/// compile-time specialization) and before semantic facts are recorded.
/// Already-unique parser IDs are retained so source-oriented public lookups
/// remain compatible with the input AST; a repeated ID is never trusted and
/// receives a deterministic final-tree identity here.
///
/// The returned [`SyntaxOrigins`] keeps the identity each re-keyed occurrence
/// had before: a clone's node carries the identity of the template node it
/// was copied from, which is the occurrence-level expansion trace.
#[allow(clippy::too_many_lines, reason = "TODO: split this pass")]
pub fn rekey_syntax(statements: &mut [Stmt]) -> SyntaxOrigins {
    struct Rekey {
        next: u64,
        used: std::collections::HashSet<mojito_common::token::SyntaxId>,
        lambda_names: std::collections::HashSet<String>,
        origins: SyntaxOrigins,
    }

    impl Rekey {
        fn id(
            &mut self,
            current: mojito_common::token::SyntaxId,
        ) -> mojito_common::token::SyntaxId {
            if self.used.insert(current) {
                return current;
            }
            loop {
                let id = mojito_common::token::SyntaxId(self.next);
                self.next += 1;
                if self.used.insert(id) {
                    self.origins.replaced.insert(id, current);
                    return id;
                }
            }
        }

        fn block(&mut self, statements: &mut [Stmt]) {
            for statement in statements {
                statement.syntax_id = self.id(statement.syntax_id);
                self.statement(&mut statement.kind);
            }
        }

        fn condition(&mut self, condition: &mut Condition) {
            if let Some(pattern) = condition.pattern_mut() {
                self.pattern(pattern);
            }
            self.expr(condition.value_mut());
        }

        fn pattern(&mut self, pattern: &mut MatchPattern) {
            pattern.syntax_id = self.id(pattern.syntax_id);
            match &mut pattern.kind {
                MatchPatternKind::Literal { value, .. }
                | MatchPatternKind::Equal { value, .. }
                | MatchPatternKind::Head(value) => self.expr(value),
                MatchPatternKind::As { pattern, .. } => self.pattern(pattern),
                MatchPatternKind::Tuple(items) | MatchPatternKind::Or(items) => {
                    for item in items {
                        self.pattern(item);
                    }
                }
                MatchPatternKind::Sequence(items) => {
                    for item in items {
                        self.pattern(item.pattern_mut());
                    }
                }
                MatchPatternKind::Call { head, args, kwargs } => {
                    self.expr(head);
                    for pattern in args {
                        self.pattern(pattern);
                    }
                    for (_, pattern) in kwargs {
                        self.pattern(pattern);
                    }
                }
                MatchPatternKind::Fields(fields) => {
                    for (_, pattern) in fields {
                        self.pattern(pattern);
                    }
                }
                MatchPatternKind::Wildcard | MatchPatternKind::Bind { .. } => {}
            }
        }

        fn target(&mut self, target: &mut DestructureTarget) {
            target.syntax_id = self.id(target.syntax_id);
            match &mut target.kind {
                DestructureTargetKind::Place(place) => self.expr(place),
                DestructureTargetKind::Tuple(items) => {
                    for item in items {
                        self.target(item);
                    }
                }
                DestructureTargetKind::Sequence(items) => {
                    for item in items {
                        match item {
                            SequenceTarget::Element(target) | SequenceTarget::Star(target) => {
                                self.target(target);
                            }
                        }
                    }
                }
                DestructureTargetKind::Discard | DestructureTargetKind::Declare { .. } => {}
            }
        }

        fn expr(&mut self, expr: &mut Expr) {
            expr.syntax_id = self.id(expr.syntax_id);
            match &mut expr.kind {
                ExprKind::Prefix(_, value)
                | ExprKind::Transfer(value)
                | ExprKind::Spread(value) => self.expr(value),
                ExprKind::Infix(_, left, right)
                | ExprKind::Index {
                    object: left,
                    index: right,
                } => {
                    self.expr(left);
                    self.expr(right);
                }
                ExprKind::Call {
                    param_args,
                    args,
                    kwargs,
                    ..
                } => {
                    self.param_args(param_args);
                    for arg in args {
                        self.expr(arg);
                    }
                    for arg in kwargs {
                        self.expr(&mut arg.value);
                    }
                }
                ExprKind::Invoke {
                    callee,
                    param_args,
                    args,
                    kwargs,
                } => {
                    self.expr(callee);
                    self.param_args(param_args);
                    for arg in args {
                        self.expr(arg);
                    }
                    for arg in kwargs {
                        self.expr(&mut arg.value);
                    }
                }
                ExprKind::Member { object, .. } => self.expr(object),
                ExprKind::MethodCall {
                    object,
                    args,
                    kwargs,
                    ..
                } => {
                    self.expr(object);
                    for arg in args {
                        self.expr(arg);
                    }
                    for arg in kwargs {
                        self.expr(&mut arg.value);
                    }
                }
                ExprKind::TypeApply { args, .. } => self.param_args(args),
                ExprKind::InitializerList { args, kwargs } => {
                    for arg in args {
                        self.expr(arg);
                    }
                    for arg in kwargs {
                        self.expr(&mut arg.value);
                    }
                }
                ExprKind::ListLit(values) | ExprKind::TupleLit(values) => {
                    for value in values {
                        self.expr(value);
                    }
                }
                ExprKind::BraceLit(entries) => {
                    for (key, value) in entries {
                        self.expr(key);
                        if let Some(value) = value {
                            self.expr(value);
                        }
                    }
                }
                ExprKind::Comprehension {
                    key,
                    value,
                    clauses,
                    ..
                } => {
                    if let Some(key) = key {
                        self.expr(key);
                    }
                    self.expr(value);
                    for clause in clauses {
                        match clause {
                            ComprehensionClause::For {
                                target,
                                iter,
                                unpack,
                            } => {
                                self.target(target);
                                self.expr(iter);
                                if let Some(unpack) = unpack {
                                    self.block(std::slice::from_mut(unpack));
                                }
                            }
                            ComprehensionClause::If(condition) => self.expr(condition),
                        }
                    }
                }
                ExprKind::Named { target, value } => {
                    self.expr(target);
                    self.expr(value);
                }
                ExprKind::IfExpr {
                    cond,
                    then_branch,
                    else_branch,
                } => {
                    self.expr(cond);
                    self.expr(then_branch);
                    self.expr(else_branch);
                }
                ExprKind::Compare { first, rest } => {
                    self.expr(first);
                    for (_, value) in rest {
                        self.expr(value);
                    }
                }
                ExprKind::Slice {
                    object,
                    lower,
                    upper,
                    step,
                    ..
                } => {
                    self.expr(object);
                    for value in [lower, upper, step].into_iter().flatten() {
                        self.expr(value);
                    }
                }
                ExprKind::MultiIndex { object, args } => {
                    self.expr(object);
                    for argument in args {
                        match argument {
                            SubscriptArg::Index(value) | SubscriptArg::Keyword { value, .. } => {
                                self.expr(value);
                            }
                            SubscriptArg::Slice {
                                lower, upper, step, ..
                            }
                            | SubscriptArg::KeywordSlice {
                                lower, upper, step, ..
                            } => {
                                for value in [lower, upper, step].into_iter().flatten() {
                                    self.expr(value);
                                }
                            }
                        }
                    }
                }
                ExprKind::TString { parts, .. } => {
                    for part in parts {
                        if let TStringPart::Expr(value) = part {
                            self.expr(value);
                        }
                    }
                }
                ExprKind::TypeValue(ty) => self.ty(ty),
                ExprKind::Lambda { def } => {
                    def.syntax_id = self.id(def.syntax_id);
                    // Cloning transformations (compile-time unrolling and
                    // specialization) duplicate a lambda's hidden definition
                    // name; the name is compiler-internal, so uniquify clones
                    // here alongside their syntax identities.
                    if let StmtKind::Def { name, .. } = &mut def.kind
                        && !self.lambda_names.insert(name.clone())
                    {
                        loop {
                            let unique = format!("{name}${}", self.next);
                            self.next += 1;
                            if self.lambda_names.insert(unique.clone()) {
                                *name = unique;
                                break;
                            }
                        }
                    }
                    self.statement(&mut def.kind);
                }
                ExprKind::Int(_)
                | ExprKind::Float(_)
                | ExprKind::Bool(_)
                | ExprKind::Str(_)
                | ExprKind::None
                | ExprKind::Uninitialized
                | ExprKind::EmptySubscript
                | ExprKind::IsRunInComptimeInterpreter
                | ExprKind::Identifier(_) => {}
            }
        }

        fn param_args(&mut self, args: &mut [ParamArg]) {
            for arg in args {
                match arg {
                    ParamArg::Type(ty) => self.ty(ty),
                    ParamArg::Value(value) => self.expr(value),
                    ParamArg::Named { value, .. } => {
                        self.param_args(std::slice::from_mut(value.as_mut()));
                    }
                }
            }
        }

        fn type_params(&mut self, params: &mut [TypeParam]) {
            for param in params {
                if let Some(ty) = &mut param.value_type {
                    self.ty(ty);
                }
                if let Some(ty) = &mut param.callable_bound {
                    self.ty(ty);
                }
                if let Some(value) = &mut param.origin_mutability {
                    self.expr(value);
                }
                if let Some(value) = &mut param.default {
                    self.expr(value);
                }
                for condition in &mut param.constraints {
                    self.expr(condition);
                }
            }
        }

        fn ty(&mut self, ty: &mut Type) {
            match ty {
                Type::Named(_, args) => self.param_args(args),
                Type::Assoc { base, args, .. } => {
                    self.ty(base);
                    self.param_args(args);
                }
                Type::IndexedProjection { base, index } => {
                    self.ty(base);
                    self.expr(index);
                }
                Type::Ref { referent, origin } => {
                    self.ty(referent);
                    if let Some(origins) = origin {
                        for expression in origins {
                            self.expr(expression);
                        }
                    }
                }
                Type::TypeOf(operand) | Type::Value(operand) => self.expr(operand),
                Type::Generator { params, result } => {
                    self.type_params(params);
                    self.ty(result);
                }
                Type::Func {
                    type_params,
                    params,
                    ret,
                    capturing,
                    raises_type,
                    where_clauses,
                    ..
                } => {
                    self.type_params(type_params);
                    for param in params {
                        self.ty(&mut param.ty);
                        if let Some(origins) = &mut param.origin {
                            for expression in origins {
                                self.expr(expression);
                            }
                        }
                    }
                    self.ty(ret);
                    if let Some(origins) = capturing {
                        for expression in origins {
                            self.expr(expression);
                        }
                    }
                    if let Some(error) = raises_type {
                        self.ty(error);
                    }
                    for clause in where_clauses {
                        self.expr(clause);
                    }
                }
                Type::Int
                | Type::UInt
                | Type::Bool
                | Type::StringLiteral
                | Type::ClosedStringLiteral
                | Type::Float64
                | Type::None
                | Type::SelfParam(_)
                | Type::SelfType => {}
            }
        }

        fn param(&mut self, param: &mut FnParam) {
            self.ty(&mut param.ty);
            if let Some(value) = &mut param.default {
                self.expr(value);
            }
        }

        fn decorators(&mut self, decorators: &mut [Decorator]) {
            for decorator in decorators {
                for arg in &mut decorator.args {
                    self.expr(arg);
                }
                for arg in &mut decorator.kwargs {
                    self.expr(&mut arg.value);
                }
            }
        }

        fn statement(&mut self, kind: &mut StmtKind) {
            match kind {
                StmtKind::VarDecl { ty, value, .. } => {
                    if let Some(ty) = ty {
                        self.ty(ty);
                    }
                    self.expr(value);
                }
                StmtKind::RefDecl { value, .. }
                | StmtKind::Assign { value, .. }
                | StmtKind::Raise(value)
                | StmtKind::Return(Some(value))
                | StmtKind::Expr(value) => self.expr(value),
                StmtKind::SetPlace { place, value } | StmtKind::AugAssign { place, value, .. } => {
                    self.expr(place);
                    self.expr(value);
                }
                StmtKind::Comptime {
                    type_params,
                    ty,
                    where_clauses,
                    value,
                    ..
                } => {
                    self.type_params(type_params);
                    if let Some(ty) = ty {
                        self.ty(ty);
                    }
                    for condition in where_clauses {
                        self.expr(condition);
                    }
                    self.expr(value);
                }
                StmtKind::Destructure { target, value } => {
                    self.target(target);
                    self.expr(value);
                }
                StmtKind::If { branches, orelse } | StmtKind::ComptimeIf { branches, orelse } => {
                    for (condition, block) in branches {
                        self.condition(condition);
                        self.block(block);
                    }
                    if let Some(block) = orelse {
                        self.block(block);
                    }
                }
                StmtKind::While { cond, body, orelse } => {
                    self.condition(cond);
                    self.block(body);
                    if let Some(body) = orelse {
                        self.block(body);
                    }
                }
                StmtKind::ComptimeAssert { cond, message } => {
                    self.expr(cond);
                    if let Some(message) = message {
                        self.expr(message);
                    }
                }
                StmtKind::Match { subject, cases } | StmtKind::ComptimeMatch { subject, cases } => {
                    self.expr(subject);
                    for case in cases {
                        self.pattern(&mut case.pattern);
                        if let Some(guard) = &mut case.guard {
                            self.expr(guard);
                        }
                        self.block(&mut case.body);
                    }
                }
                StmtKind::For {
                    target,
                    iter,
                    body,
                    orelse,
                } => {
                    self.target(target);
                    self.expr(iter);
                    self.block(body);
                    if let Some(body) = orelse {
                        self.block(body);
                    }
                }
                StmtKind::ComptimeFor { iter, body, .. } => {
                    self.expr(iter);
                    self.block(body);
                }
                StmtKind::Scope(body) => self.block(body),
                StmtKind::With { items, body } => {
                    for item in items {
                        self.expr(&mut item.context);
                    }
                    self.block(body);
                }
                StmtKind::Try {
                    body,
                    except,
                    orelse,
                    finalbody,
                } => {
                    self.block(body);
                    if let Some((_, block)) = except {
                        self.block(block);
                    }
                    if let Some(block) = orelse {
                        self.block(block);
                    }
                    if let Some(block) = finalbody {
                        self.block(block);
                    }
                }
                StmtKind::Def {
                    type_params,
                    params,
                    raises_type,
                    ret,
                    body,
                    decorators,
                    where_clauses,
                    ..
                } => {
                    self.type_params(type_params);
                    for param in params {
                        self.param(param);
                    }
                    if let Some(error) = raises_type {
                        self.ty(error);
                    }
                    if let Some(ret) = ret {
                        self.ty(ret);
                    }
                    self.decorators(decorators);
                    for condition in where_clauses {
                        self.expr(condition);
                    }
                    self.block(body);
                }
                StmtKind::Struct {
                    type_params,
                    fields,
                    associated,
                    methods,
                    decorators,
                    conformance_conditions,
                    callable_conformance,
                    where_clauses,
                    enum_cases,
                    ..
                } => {
                    self.type_params(type_params);
                    if let Some(callable) = callable_conformance {
                        self.ty(callable);
                    }
                    for condition in where_clauses {
                        self.expr(condition);
                    }
                    for field in fields {
                        self.ty(&mut field.ty);
                    }
                    for ty in enum_cases
                        .iter_mut()
                        .flatten()
                        .flat_map(|case| &mut case.payload)
                    {
                        self.ty(ty);
                    }
                    for member in associated {
                        self.type_params(&mut member.params);
                        if let Some(ty) = &mut member.ty {
                            self.ty(ty);
                        }
                        for condition in &mut member.where_clauses {
                            self.expr(condition);
                        }
                        self.expr(&mut member.value);
                    }
                    for (_, condition) in conformance_conditions {
                        self.expr(condition);
                    }
                    self.decorators(decorators);
                    for method in methods {
                        self.type_params(&mut method.type_params);
                        if let Some(origins) = &mut method.self_origin {
                            for origin in origins {
                                self.expr(origin);
                            }
                        }
                        for param in &mut method.params {
                            self.param(param);
                        }
                        for ty in [
                            &mut method.raises_type,
                            &mut method.ret,
                            &mut method.self_type,
                        ]
                        .into_iter()
                        .flatten()
                        {
                            self.ty(ty);
                        }
                        self.decorators(&mut method.decorators);
                        for condition in &mut method.where_clauses {
                            self.expr(condition);
                        }
                        self.block(&mut method.body);
                    }
                }
                StmtKind::Trait {
                    methods,
                    comptime_members,
                    ..
                } => {
                    for method in methods {
                        self.type_params(&mut method.type_params);
                        if let Some(origins) = &mut method.self_origin {
                            for origin in origins {
                                self.expr(origin);
                            }
                        }
                        for param in &mut method.params {
                            self.param(param);
                        }
                        for ty in [
                            &mut method.raises_type,
                            &mut method.ret,
                            &mut method.self_type,
                        ]
                        .into_iter()
                        .flatten()
                        {
                            self.ty(ty);
                        }
                        for condition in &mut method.where_clauses {
                            self.expr(condition);
                        }
                        self.decorators(&mut method.decorators);
                        if let Some(body) = &mut method.default_body {
                            self.block(body);
                        }
                    }
                    for member in comptime_members {
                        self.type_params(&mut member.params);
                        self.ty(&mut member.ty);
                        for condition in &mut member.where_clauses {
                            self.expr(condition);
                        }
                    }
                }
                StmtKind::Return(None)
                | StmtKind::Import { .. }
                | StmtKind::FromImport { .. }
                | StmtKind::Pass
                | StmtKind::Break
                | StmtKind::Continue => {}
            }
        }
    }

    let mut rekey = Rekey {
        next: 1,
        used: std::collections::HashSet::new(),
        lambda_names: std::collections::HashSet::new(),
        origins: SyntaxOrigins::default(),
    };
    rekey.block(statements);
    rekey.origins
}

/// The identity each occurrence [`rekey_syntax`] re-keyed had before it.
#[derive(Debug, Clone, Default)]
pub struct SyntaxOrigins {
    replaced:
        std::collections::HashMap<mojito_common::token::SyntaxId, mojito_common::token::SyntaxId>,
}

impl SyntaxOrigins {
    /// The identity `id` was copied with: itself unless it was re-keyed. A
    /// node synthesized from another ([`SyntaxId::derived`]) was copied with
    /// the identity synthesized at the same ordinal from its parent's origin.
    ///
    /// [`SyntaxId::derived`]: mojito_common::token::SyntaxId::derived
    pub fn origin(&self, id: mojito_common::token::SyntaxId) -> mojito_common::token::SyntaxId {
        if let Some(original) = self.replaced.get(&id) {
            return *original;
        }
        id.derivation().map_or(id, |(parent, ordinal)| {
            mojito_common::token::SyntaxId::derived(self.origin(parent), ordinal)
        })
    }
}

/// Decode with `decode`, stamping every expression it decodes with
/// `source`, the module the encoding was made from: a record whose
/// expressions sit inside other values, where no walk reaches them.
pub fn decoding_from<T>(source: &str, decode: impl FnOnce() -> T) -> T {
    let previous = DECODING_SOURCE.replace(Some(source.to_string()));
    let decoded = decode();
    DECODING_SOURCE.set(previous);
    decoded
}

/// Attach one linked source path to an entire AST subtree. Spans remain local
/// byte ranges; `source_span()` combines them with this provenance.
pub fn stamp_source(statements: &mut [Stmt], source: &str) {
    struct Stamp<'a>(&'a str);

    impl crate::visit::MutVisitor for Stamp<'_> {
        fn visit_stmt_mut(&mut self, statement: &mut Stmt) {
            statement.module = Some(self.0.to_string());
        }

        fn visit_expr_mut(&mut self, expr: &mut Expr) {
            expr.source = Some(self.0.to_string());
        }

        fn visit_pattern_mut(&mut self, pattern: &mut MatchPattern) {
            pattern.source = Some(self.0.to_string());
        }

        fn visit_target_mut(&mut self, target: &mut DestructureTarget) {
            target.source = Some(self.0.to_string());
        }
    }

    crate::visit::walk_block_mut(&mut Stamp(source), statements);
}

thread_local! {
    /// The module [`decoding_from`] is decoding.
    static DECODING_SOURCE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

fn decoded_source() -> Option<String> {
    DECODING_SOURCE.with_borrow(Clone::clone)
}

/// [`Stmt::module`] as its encoding carries it.
///
/// Inside an occurrence encoding
/// ([`mojito_common::token::encoding_occurrences`]) it is the module's file
/// index, and outside one nothing. Any other field naming a statement's
/// module encodes through it too.
pub mod statement_module {
    use serde::{Deserialize, Serialize};

    pub fn serialize<S: serde::Serializer>(
        module: &impl std::borrow::Borrow<Option<String>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        if mojito_common::token::occurrences_encode() {
            mojito_common::token::occurrence_file(module.borrow().as_deref())
                .map_err(serde::ser::Error::custom)?
                .serialize(serializer)
        } else {
            serializer.serialize_unit()
        }
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<String>, D::Error> {
        if mojito_common::token::occurrences_encode() {
            mojito_common::token::occurrence_source(u32::deserialize(deserializer)?)
                .map_err(serde::de::Error::custom)
        } else {
            <()>::deserialize(deserializer).map(|()| None)
        }
    }
}

/// [`Expr::source`] as its encoding carries it: as [`Stmt::module`]'s,
/// except that outside an occurrence encoding a decoded expression is
/// stamped with the module [`decoding_from`] names.
mod expression_source {
    use serde::Deserialize;

    pub fn serialize<S: serde::Serializer>(
        source: &impl std::borrow::Borrow<Option<String>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        super::statement_module::serialize(source, serializer)
    }

    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<String>, D::Error> {
        if mojito_common::token::occurrences_encode() {
            super::statement_module::deserialize(deserializer)
        } else {
            <()>::deserialize(deserializer).map(|()| super::decoded_source())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Dtype, Stmt, StmtKind, rekey_syntax};
    use mojito_common::token::SyntaxId;

    #[test]
    fn a_node_derived_from_a_rekeyed_copy_traces_to_the_original() {
        let original = Stmt::new(StmtKind::Pass, (0, 4));
        let mut statements = vec![original.clone(), original.clone()];
        let origins = rekey_syntax(&mut statements);
        let (kept, copy) = (statements[0].syntax_id, statements[1].syntax_id);
        assert_eq!(kept, original.syntax_id);
        assert_ne!(copy, kept);
        assert_eq!(
            origins.origin(SyntaxId::derived(copy, 2)),
            SyntaxId::derived(kept, 2)
        );
        assert_eq!(
            origins.origin(SyntaxId::derived(SyntaxId::derived(copy, 0), 1)),
            SyntaxId::derived(SyntaxId::derived(kept, 0), 1)
        );
    }

    #[test]
    fn dtype_predicates_follow_upstream_code_masks() {
        for dtype in Dtype::ALL {
            let expected = match dtype {
                Dtype::Bool => [false, false, false, false, false],
                Dtype::Int | Dtype::Int8 | Dtype::Int16 | Dtype::Int32 | Dtype::Int64 => {
                    [true, false, true, false, true]
                }
                Dtype::UInt | Dtype::UInt8 | Dtype::UInt16 | Dtype::UInt32 | Dtype::UInt64 => {
                    [true, false, false, true, true]
                }
                Dtype::Float16 | Dtype::Float32 | Dtype::Float64 => {
                    [false, true, true, false, true]
                }
            };
            let actual = [
                dtype.is_integral(),
                dtype.is_floating_point(),
                dtype.is_signed(),
                dtype.is_unsigned(),
                dtype.is_numeric(),
            ];
            assert_eq!(actual, expected, "{}", dtype.name());
            assert!(!dtype.is_float8(), "{}", dtype.name());
            assert_eq!(
                dtype.is_half_float(),
                dtype == Dtype::Float16,
                "{}",
                dtype.name()
            );
            assert_eq!(
                dtype.is_float(),
                dtype.is_floating_point(),
                "{}",
                dtype.name()
            );
        }
    }
}

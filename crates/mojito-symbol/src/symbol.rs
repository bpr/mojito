//! Canonical overload identity and lowered-symbol formatting.
//!
//! This module is the **single owner** of the `$ov$` signature-qualified name
//! scheme that overload resolution lowers to. The checker records a resolved
//! callee per call span, the MIR names each overloaded `def`/method, and the VM
//! looks both up — all three must agree on the exact spelling, so none of them
//! may assemble or inspect these strings directly. A new hand-built overload
//! symbol elsewhere in `src/` is a bug (`tests/symbol_test.rs` scans for it).
//!
//! An overload signature is represented as typed data (`SignatureKey`, a list of
//! `TypeKey`s) before it is ever formatted. A `TypeKey` can be built from two
//! worlds and **must produce the same spelling for the same source annotation**:
//!
//! - [`TypeKey::from_ast`] — the declared `ast::Type` (MIR/VM lowering names
//!   each overloaded definition from its parameter annotations).
//! - [`TypeKey::from_ty`] — the checker's resolved `types::Ty` (the checker
//!   records the selected callee from the winning signature's parameter types).
//!
//! Definition-side value arguments are folded with the same integer operations
//! as the checker before formatting, so `FixedBuffer[N]` and `FixedBuffer[2+6]`
//! name the same `FixedBuffer[8]` specialization selected at a call site.

use mojito_types::types::TyKind;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use mojito_ast::ast::{
    AnnotatedParam, ArgConvention, DeclaredParam, Expr, ExprKind, FnParam, Method, ParamArg,
    ParamKind, Stmt, StmtKind, Type, TypeParam,
};
use mojito_common::literal::{FloatLiteral, IntLiteral};
use mojito_types::ct::{CtLane, CtValue};
use mojito_types::param_expr::MetaTy;
use mojito_types::types::{FuncTy, Ty, TyArg, canonical_simd_ty, default_literal};
use sha2::{Digest, Sha256};

/// The module names a declaration's parameter annotations read, which its
/// lowered symbol spells as what they name: an integer constant as its
/// value, and a type alias as the type it names.
pub trait ModuleNames {
    /// The value of the module integer constant `name`.
    fn constant(&self, name: &str) -> Option<IntLiteral>;

    /// The source spelling of the type the module type alias `name` names.
    fn type_alias(&self, _name: &str) -> Option<Type> {
        None
    }
}

/// The [`ModuleNames`] of a scope that declares none.
pub struct NoModuleNames;

impl ModuleNames for NoModuleNames {
    fn constant(&self, _name: &str) -> Option<IntLiteral> {
        None
    }
}

/// The lowered symbols of `std._intrinsics._int_digits`/`_uint_digits`.
///
/// They are the Mojo bodies the native backend calls for an integer's
/// decimal text, where the VM runs `SIMD.write_to` (R1664). Each writes the digits (with a leading `-` for a negative `Int`)
/// into a caller-owned buffer of at least 21 bytes and returns how many it
/// wrote; there is no NUL terminator.
pub const INT_DIGITS_SYMBOL: &str = "__module$std$$intrinsics$_int_digits";
pub const UINT_DIGITS_SYMBOL: &str = "__module$std$$intrinsics$_uint_digits";

/// The buffer every `_int_digits`/`_uint_digits` caller must supply: 20
/// digits of `UInt.MAX` plus a sign, rounded up.
pub const DIGITS_BUFFER_BYTES: u64 = 32;

/// The receiver of a method symbol selected through a trait bound, which the
/// concrete type replaces at run time (`__trait_dispatch.copy$ov$`).
pub const TRAIT_DISPATCH: &str = "__trait_dispatch";

/// The method a writer receives for `x.write_to(writer)` on a value whose
/// `write_to` the host formats (a builtin scalar or literal).
///
/// It stands for the tail of upstream's `write_to` body, which writes the
/// formatted text through `writer.write_string` and never re-enters the
/// writer's own `write`; the `$` keeps any source method from naming it.
pub const WRITE_FORMATTED: &str = "$write_formatted";

/// The implicit-conversion target that splats a scalar across a vector.
///
/// It stands for upstream's `@implicit SIMD.__init__(Scalar[dtype])`, which
/// the builtin `SIMD` declares no constructor to name; MIR lowers it to a
/// one-element `MakeSimd`.
pub const SCALAR_SPLAT_CONVERSION: &str = "SIMD.__init__$splat";

/// The implicit-conversion target that converts a `SIMDLength` to a vector.
///
/// It stands for upstream's `@implicit SIMD.__init__(SIMDLength)`, which the
/// builtin `SIMD` declares no constructor to name; MIR lowers it as that
/// constructor's body: the length's `index` cast to `Int`, then to the
/// vector's dtype, then splat across its lanes.
pub const SIMD_LENGTH_CONVERSION: &str = "SIMD.__init__$simd_length";

/// The symbol of upstream's `@implicit SIMDLength.__init__(Int)`, which
/// spells the length a vector's `Int` width converts to.
pub const SIMD_LENGTH_FROM_INT: &str = "SIMDLength.__init__$ov$Int";

/// One declaration visible to phase-neutral runtime/backend dispatch.
#[derive(Debug, Clone, Copy)]
pub struct CallableCandidate<'a> {
    pub name: &'a str,
    pub n_params: usize,
}

/// One concrete argument in a backend-private function or struct instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InstanceArg {
    Ty(Ty),
    Value(CtValue),
}

/// What callable resolution asks of a symbol table, name by name, as
/// upstream finds an overload set by looking its name up.
pub trait CallableLookup {
    /// Whether a function is named exactly `name`.
    fn contains(&self, name: &str) -> bool;

    /// The `base$ov$…` overloads of `base`, with their runtime arities.
    fn overloads(&self, base: &str) -> impl Iterator<Item = CallableCandidate<'_>>;
}

/// Resolve a source callable name, preserving an exact declaration and using
/// arity only when precisely one overload is viable.
pub fn resolve_callable_symbol(table: &impl CallableLookup, name: &str, argc: usize) -> String {
    if table.contains(name) {
        return name.to_string();
    }
    let expected_params = if name.contains('.') { argc + 1 } else { argc };
    let mut matches = table
        .overloads(name)
        .filter(|candidate| candidate.n_params == expected_params);
    let Some(first) = matches.next() else {
        return name.to_string();
    };
    if matches.next().is_none() {
        return first.name.to_string();
    }
    // Several same-arity overloads: an unresolved dunder dispatch (a
    // `T: Equatable` body's `a == b` erased to the runtime receiver) is the
    // trait requirement's call, whose parameters are all `Self`; that
    // overload answers when it exists (`String.__eq__$ov$Self` beside
    // `String.__eq__$ov$StringSpan`).
    let self_shaped = format!("{name}{OV_SEP}{}", vec!["Self"; argc].join("$"));
    if table.contains(&self_shaped) {
        return self_shaped;
    }
    name.to_string()
}

/// Resolve a checker-selected own method against a concrete receiver's
/// template.
///
/// The selected symbol answers when the table holds it, else the
/// receiver's `{receiver}.{method}` by arity. A call through a bound is no
/// own selection: its member is the receiver's witness.
pub fn resolve_method_symbol(
    table: &impl CallableLookup,
    receiver_type: &str,
    method: &str,
    resolved: Option<&str>,
    argc: usize,
) -> String {
    if let Some(selected) = resolved.filter(|selected| table.contains(selected)) {
        return selected.to_string();
    }
    resolve_callable_symbol(table, &format!("{receiver_type}.{method}"), argc)
}

/// The entries of one member of a struct's member table, as callable
/// resolution looks them up.
///
/// Each is keyed by the method spelling it instantiates (`__eq__$ov$Self`)
/// and named `{struct}.{key}`, so the exact, arity, and `Self`-shaped rules
/// of [`resolve_callable_symbol`] select among a member's overloads as they
/// would among the struct's own methods.
pub struct MemberCandidates<'a> {
    owner: String,
    /// Each entry's `{struct}.{key}` name, symbol, and runtime arity.
    entries: Vec<(String, &'a str, usize)>,
}

impl<'a> MemberCandidates<'a> {
    /// The candidates of struct `owner`'s table entries, each a key, its
    /// instance's symbol, and that instance's runtime arity.
    pub fn new(owner: &str, entries: impl IntoIterator<Item = (&'a str, &'a str, usize)>) -> Self {
        Self {
            owner: owner.to_string(),
            entries: entries
                .into_iter()
                .map(|(key, symbol, n_params)| (format!("{owner}.{key}"), symbol, n_params))
                .collect(),
        }
    }

    /// The symbol of the instance a dispatch of `method` with `argc`
    /// arguments besides the receiver runs: the one entry under the key
    /// [`resolve_callable_symbol`] selects. `None` when no key is selected,
    /// or when several instances share it.
    pub fn resolve(&self, method: &str, argc: usize) -> Option<&'a str> {
        let selected = resolve_callable_symbol(self, &format!("{}.{method}", self.owner), argc);
        let mut instances = self
            .entries
            .iter()
            .filter(|(name, _, _)| *name == selected)
            .map(|(_, symbol, _)| *symbol);
        let symbol = instances.next()?;
        instances.next().is_none().then_some(symbol)
    }
}

impl CallableLookup for MemberCandidates<'_> {
    fn contains(&self, name: &str) -> bool {
        self.entries.iter().any(|(known, _, _)| known == name)
    }

    fn overloads(&self, base: &str) -> impl Iterator<Item = CallableCandidate<'_>> {
        self.entries
            .iter()
            .filter(move |(name, _, _)| is_overload_of(name, base))
            .map(|(name, _, n_params)| CallableCandidate {
                name,
                n_params: *n_params,
            })
    }
}

/// A type or value argument as a specialization clone spells it.
///
/// Literal types materialize (a string literal bound to a method's `T:
/// Movable` instantiates the `String` clone, which the call reaches through
/// the ordinary literal conversion), so two applications differing only in
/// literal-ness share one clone instead of minting two symbol-equivalent
/// overloads.
pub fn materialized_instantiation_argument(argument: &TyArg) -> TyArg {
    use mojito_types::types::erase_origin_arguments;
    match argument {
        TyArg::Ty(ty) if ty.is_string_literal() => {
            TyArg::Ty(TyKind::Struct(STDLIB_STRING_STRUCT.to_string(), Vec::new()).into())
        }
        TyArg::Ty(ty) => TyArg::Ty(erase_origin_arguments(&erase_capture_environment(
            &default_literal(ty),
        ))),
        // A checker-inferred type pack records its element types the same way.
        TyArg::Val(CtValue::Tuple(elements)) => TyArg::Val(CtValue::Tuple(
            elements
                .iter()
                .map(|element| match element {
                    CtValue::Type(ty) => CtValue::Type(erase_origin_arguments(ty)),
                    other => other.clone(),
                })
                .collect(),
        )),
        other => other.clone(),
    }
}

/// A method symbol split at its receiver separator (`Box` and
/// `__init__$y22:SIMD[DType.float32, 2]`): the last `.` outside brackets, so a
/// clone's baked type arguments (`DType.float32`) never split it.
pub fn split_method_symbol(symbol: &str) -> Option<(&str, &str)> {
    let mut depth = 0usize;
    let mut separator = None;
    for (index, ch) in symbol.char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            '.' if depth == 0 => separator = Some(index),
            _ => {}
        }
    }
    separator.map(|index| (&symbol[..index], &symbol[index + 1..]))
}

/// Deterministic MIR identity for a concrete generic instance. Origins have
/// already been erased from `arguments` and therefore cannot split ABI identity.
///
/// An argument spelling longer than [`INSTANCE_SPELLING_LIMIT`] is replaced
/// by its digest (`W$mono$H…`), so a nested instance — whose spelling holds
/// the symbol of the level below — stays bounded at every nesting depth.
pub fn instance_symbol(template: &str, arguments: &[InstanceArg]) -> String {
    let mut spelling = String::new();
    for argument in arguments {
        spelling.push('$');
        let raw = match argument {
            // Overload keys intentionally collapse StringLiteral and nominal
            // String, but generic instances cannot: their native layouts are
            // a 16-byte borrowed descriptor versus a 24-byte owning value.
            InstanceArg::Ty(ty) if ty.is_string_literal() => "TStringLiteral".to_string(),
            InstanceArg::Ty(ty) => format!("T{}", ty_raw_in(ty, None, KeyMode::Instance)),
            InstanceArg::Value(value) => format!("V{value}"),
        };
        spelling.push_str(&sanitize(&raw));
    }
    if spelling.len() > INSTANCE_SPELLING_LIMIT {
        // Every spelled argument opens with `$T` or `$V`, so the `$H` marker
        // keeps a digest apart from any spelling short enough to stay.
        let digest = Sha256::digest(spelling.as_bytes());
        spelling = format!("$H{}", hex_digest(&digest[..16]));
    }
    format!("{template}$mono{spelling}")
}

/// The concrete struct declaration whose member table a backend or pass
/// reads for a value of type `ty`.
///
/// Upstream's witness tables hang off the struct instance a type names: a
/// struct names its own, and a closed `SIMD` type the `SIMD` instance its
/// dtype and width apply, so a scalar alias and its `SIMD[dt, 1]` spelling
/// name one table. `None` for any other type, a symbolic `SIMD` included.
/// This is the only mapping from a type to that declaration.
pub fn member_table_name(ty: &Ty) -> Option<Cow<'_, str>> {
    match ty.kind() {
        TyKind::Struct(name, _) => Some(Cow::Borrowed(name)),
        kind @ TyKind::Simd { .. } => {
            let (dtype, width) = mojito_types::types::simd_shape(kind)?;
            let arguments =
                mojito_types::types::simd_struct_arguments(canonical_simd_ty(dtype, width).kind())?
                    .into_iter()
                    .map(|argument| match argument {
                        TyArg::Val(value) => Some(InstanceArg::Value(value)),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>()?;
            Some(Cow::Owned(instance_symbol(
                mojito_types::types::SIMD_TYPE_NAME,
                &arguments,
            )))
        }
        _ => None,
    }
}

/// The generator `template` names with its unused arguments removed, the
/// clone a direct call instantiates: upstream's `_REMOVED_ARG` suffix,
/// sanitized as an instance spelling is.
pub fn removed_argument_symbol(template: &str) -> String {
    format!("{template}$REMOVED$ARG")
}

/// The canonical mangled spelling of one parameter type. Only this module can
/// construct one, so every signature part obeys the same sanitization rules.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TypeKey(String);

impl TypeKey {
    /// Mangle a declared parameter annotation (the MIR/VM definition side).
    pub fn from_ast(ty: &Type) -> Self {
        Self(sanitize(&ast_raw(
            ty,
            &NoModuleNames,
            &HashMap::new(),
            None,
        )))
    }

    /// Mangle a declared parameter annotation in the scope of the
    /// declaration's own compile-time parameters, so an annotation naming one
    /// of them carries its bounds exactly as the checker's `TyKind::Param` does
    /// (`T$Copyable`). [`Self::from_ast`] spells such an annotation bare,
    /// which is right for a lowered symbol but does not compare against a
    /// checker-resolved type.
    pub fn from_ast_in_scope(ty: &Type, type_params: &[TypeParam]) -> Self {
        Self(sanitize(&ast_raw(
            ty,
            &NoModuleNames,
            &binder_spellings(type_params),
            None,
        )))
    }

    /// The mangled spelling. Two keys compare equal exactly when the
    /// declaration and call-resolution sides agree on the type, which is what
    /// makes a checker-recorded parameter type matchable against the
    /// declaration annotation it came from.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Mangle a checker-resolved type (the call-resolution side). Aligned with
    /// [`TypeKey::from_ast`]: a struct/parameter/`Self.T` type spells exactly as
    /// its annotation does, so checker-recorded callees name real MIR functions.
    pub fn from_ty(ty: &Ty) -> Self {
        Self(sanitize(&ty_raw(ty, None)))
    }

    /// Like [`TypeKey::from_ty`], but canonicalizes the enclosing struct type
    /// (however spelled) to `Self`. Use this for a method's own parameter types
    /// so a same-arity `self`-typed overload keys as `$ov$Self` — matching the
    /// declaration side, which mangles the bare `Self` annotation identically.
    pub fn from_ty_with_self(ty: &Ty, self_ty: &Ty) -> Self {
        Self(sanitize(&ty_raw(ty, Some(self_ty))))
    }
}

/// An overload signature as typed data: the ordered parameter type keys.
/// Format it only through [`function_symbol`]/[`method_symbol`] (or the
/// `lowered_*` helpers below).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SignatureKey {
    types: Vec<TypeKey>,
    /// The homogeneous keyword-variadic collector type, kept outside the
    /// positional sequence so `def f(x: T)` and `def f(var **x: T)` cannot
    /// collide after lowering.
    kw_variadic: Option<TypeKey>,
    /// Keyword-only parameter names. Overloads whose parameter types agree
    /// may still differ by keyword-only names (`s[byte=i]` vs
    /// `s[codepoint=i]`), so the names are part of callable identity; the
    /// suffix stays empty for signatures without keyword-only parameters,
    /// leaving every existing symbol unchanged.
    keywords: Vec<String>,
    /// Indexes of the regular parameters taken `var` (owned). Current Mojo
    /// overloads a free function on a read versus an owned parameter of the
    /// same type, so ownership is part of that callable's identity.
    owned: Vec<usize>,
    /// The explicit compile-time parameter names of an overload that takes
    /// no run-time parameter. Such overloads differ only in what a call
    /// binds in brackets (`bit_width_of[type: AnyType]()` beside
    /// `bit_width_of[dtype: DType]()`), so their binders are their identity;
    /// the suffix stays empty for every other signature.
    parameters: Vec<String>,
}

impl SignatureKey {
    /// The signature of a declared `def`/method parameter list.
    pub fn from_ast_params(params: &[FnParam]) -> Self {
        Self {
            types: params
                .iter()
                .filter(|parameter| parameter.kind != mojito_ast::ast::ParamKind::KwVariadic)
                .map(|parameter| TypeKey::from_ast(&parameter.ty))
                .collect(),
            kw_variadic: params
                .iter()
                .find(|parameter| parameter.kind == mojito_ast::ast::ParamKind::KwVariadic)
                .map(|parameter| TypeKey::from_ast(&parameter.ty)),
            keywords: Vec::new(),
            owned: Vec::new(),
            parameters: Vec::new(),
        }
    }

    /// The signature of a checker-resolved parameter-type list.
    pub fn from_tys<'a>(tys: impl IntoIterator<Item = &'a Ty>) -> Self {
        Self {
            types: tys.into_iter().map(TypeKey::from_ty).collect(),
            kw_variadic: None,
            keywords: Vec::new(),
            owned: Vec::new(),
            parameters: Vec::new(),
        }
    }

    /// Like [`SignatureKey::from_tys`], but canonicalizes the enclosing struct
    /// type to `Self` in every parameter (see [`TypeKey::from_ty_with_self`]).
    /// `self_ty` is `None` for callables without an enclosing struct (free
    /// functions and abstract trait dispatch, whose parameters already carry
    /// `TyKind::SelfType` and so spell `Self` regardless).
    pub fn from_tys_with_self<'a>(
        tys: impl IntoIterator<Item = &'a Ty>,
        self_ty: Option<&Ty>,
    ) -> Self {
        Self {
            types: tys
                .into_iter()
                .map(|ty| match self_ty {
                    Some(self_ty) => TypeKey::from_ty_with_self(ty, self_ty),
                    None => TypeKey::from_ty(ty),
                })
                .collect(),
            kw_variadic: None,
            keywords: Vec::new(),
            owned: Vec::new(),
            parameters: Vec::new(),
        }
    }

    /// Attach which regular parameters are owned (`var`) to a free function's
    /// identity, from the per-parameter conventions in declaration order. The
    /// suffix stays empty when no parameter is owned.
    #[must_use]
    pub fn with_owned_params(mut self, conventions: &[Option<ArgConvention>]) -> Self {
        self.owned = conventions
            .iter()
            .enumerate()
            .filter(|(_, convention)| matches!(convention, Some(ArgConvention::Var)))
            .map(|(index, _)| index)
            .collect();
        self
    }

    /// Attach the homogeneous keyword-variadic collector to callable identity.
    #[must_use]
    pub fn with_kw_variadic(mut self, ty: Option<&Ty>) -> Self {
        self.kw_variadic = ty.map(TypeKey::from_ty);
        self
    }

    /// Attach the keyword-only parameter names to this signature's identity.
    #[must_use]
    pub fn with_keyword_names(mut self, names: Vec<String>) -> Self {
        self.keywords = names;
        self
    }

    /// Attach the explicit compile-time parameter names `names` when this
    /// signature has no run-time parameter (see [`SignatureKey`]'s
    /// `parameters`).
    #[must_use]
    pub fn with_parameter_names<'a>(mut self, names: impl IntoIterator<Item = &'a str>) -> Self {
        if self.types.is_empty() && self.kw_variadic.is_none() {
            self.parameters = names.into_iter().map(str::to_string).collect();
        }
        self
    }

    fn suffix(&self) -> String {
        let parts = self
            .types
            .iter()
            .map(|k| k.0.as_str())
            .collect::<Vec<_>>()
            .join("$");
        let mut suffix = format!("{OV_SEP}{parts}");
        if let Some(keyword_variadic) = &self.kw_variadic {
            suffix.push_str("$kwv$");
            suffix.push_str(&keyword_variadic.0);
        }
        if !self.keywords.is_empty() {
            suffix.push_str("$kw$");
            suffix.push_str(&self.keywords.join("$"));
        }
        for index in &self.owned {
            suffix.push_str("$var");
            suffix.push_str(&index.to_string());
        }
        if !self.parameters.is_empty() {
            suffix.push_str("$ct$");
            suffix.push_str(&self.parameters.join("$"));
        }
        suffix
    }

    fn with_receiver(&self, convention: Option<ArgConvention>) -> Self {
        let receiver = match convention {
            None | Some(ArgConvention::Imm) => "SelfRead",
            Some(ArgConvention::Mut) => "SelfMut",
            Some(ArgConvention::Var) => "SelfVar",
            Some(ArgConvention::Out) => "SelfOut",
            Some(ArgConvention::Ref) => "SelfRef",
            Some(ArgConvention::Deinit) => "SelfDeinit",
        };
        let mut parts = vec![TypeKey(receiver.to_string())];
        parts.extend(self.types.iter().cloned());
        Self {
            types: parts,
            kw_variadic: self.kw_variadic.clone(),
            keywords: self.keywords.clone(),
            owned: self.owned.clone(),
            parameters: self.parameters.clone(),
        }
    }
}

/// A callable's `*args` collector as typed data: how many caller-visible
/// regular parameters precede it, and its element type's key.
///
/// A specialization request names one overload of a template by its regular
/// parameters' names and types, and a collector is spelled by neither: two
/// type-pack declarations can agree on both and differ only here, in the
/// collector's position (`f(a: Int, *rest: *Ts)` beside
/// `f(*rest: *Ts, a: Int)`) or in its pack's bounds. The collector's own name
/// and convention are not part of the key, since no call can select by them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VariadicKey {
    position: usize,
    element: TypeKey,
}

impl VariadicKey {
    /// The collector of a declared parameter list, keyed in the scope of the
    /// declaration's own compile-time parameters (see
    /// [`TypeKey::from_ast_in_scope`]). An `out` named result is not
    /// caller-visible and does not count toward the position.
    pub fn from_ast_params(params: &[FnParam], type_params: &[TypeParam]) -> Option<Self> {
        let index = params
            .iter()
            .position(|parameter| parameter.kind == mojito_ast::ast::ParamKind::Variadic)?;
        Some(Self {
            position: params[..index]
                .iter()
                .filter(|parameter| {
                    parameter.kind == mojito_ast::ast::ParamKind::Regular
                        && !matches!(parameter.convention, Some(ArgConvention::Out))
                })
                .count(),
            element: TypeKey::from_ast_in_scope(&params[index].ty, type_params),
        })
    }

    /// The collector of a checker-resolved callable type. Every regular
    /// parameter after a collector is keyword-only, so the callable's
    /// keyword-only index is the collector's position.
    pub fn from_callable(ty: &Ty) -> Option<Self> {
        let func = ty.callable()?;
        let element = func.variadic.as_ref()?;
        Some(Self {
            position: func.keyword_only.unwrap_or(func.params.len()),
            element: TypeKey::from_ty(element),
        })
    }

    /// How many caller-visible regular parameters precede the collector.
    pub const fn position(&self) -> usize {
        self.position
    }
}

/// The bundled stdlib's nominal `String` struct under its linked module
/// identity. The checker's construction routing and the VM's
/// literal-to-struct bridge both key on this exact declaration.
pub use mojito_types::types::{
    FORMAT_LITERAL_METHOD, STDLIB_FORMAT_UTILS_STRUCT, STDLIB_STRING_SPAN_STRUCT,
    STDLIB_STRING_STRUCT, is_stdlib_string_span_struct, is_stdlib_string_struct,
};

/// The lowered symbol of an overloaded free function: `pick$ov$Int`.
pub fn function_symbol(base: &str, sig: &SignatureKey) -> String {
    format!("{base}{}", sig.suffix())
}

/// The nominal String's `StringLiteral` constructor symbol — the wrap that
/// materializes a compile-time string result (`String("abc")`, `repr(x)`,
/// `input(...)`) as the nominal struct.
pub fn nominal_string_literal_ctor_symbol() -> String {
    format!("{STDLIB_STRING_STRUCT}.__init__$ov$StringLiteral")
}

/// The struct a `**kwargs` collector is, `StringDict[V]`, as upstream's.
pub const KEYWORD_COLLECTOR_STRUCT: &str = "StringDict";

/// The keyword collector's empty constructor, which a call building one names.
///
/// Upstream's call constructs the dictionary its callee's `**kwargs` takes
/// the same way.
pub fn keyword_collector_constructor_symbol() -> String {
    format!("{KEYWORD_COLLECTOR_STRUCT}.__init__")
}

/// The `_insert` a call building a keyword collector names per keyword.
///
/// It is the overload taking the keyword's literal key, as upstream's call
/// inserts each keyword under its `StringLiteral`.
pub fn keyword_collector_insert_symbol() -> String {
    format!("{KEYWORD_COLLECTOR_STRUCT}._insert$ov$StringLiteral$V$Movable")
}

/// Whether a constructor declaration is its struct's list-literal
/// initializer, the one a collection display (`[a, b]`) selects: variadic,
/// with a `__list_literal__` keyword.
pub fn is_list_literal_constructor(param_names: &[String], variadic: bool) -> bool {
    variadic && param_names.iter().any(|name| name == "__list_literal__")
}

/// Whether a constructor declaration is its struct's set-literal
/// initializer, the one a set display (`{a, b}`) selects: variadic, with a
/// `__set_literal__` keyword.
pub fn is_set_literal_constructor(param_names: &[String], variadic: bool) -> bool {
    variadic && param_names.iter().any(|name| name == "__set_literal__")
}

/// Whether a constructor declaration is its struct's dict-literal
/// initializer, the one a dictionary display (`{k: v}`) selects:
/// non-variadic, with a `__dict_literal__` keyword.
pub fn is_dict_literal_constructor(param_names: &[String], variadic: bool) -> bool {
    !variadic && param_names.iter().any(|name| name == "__dict_literal__")
}

/// Whether a constructor declaration is one a slice literal (`[a:b:c]`)
/// may call for a parameter of its struct's type: not variadic, with a
/// `__slice_literal__` keyword after the three operand parameters.
pub fn is_slice_literal_constructor(param_names: &[String], variadic: bool) -> bool {
    !variadic
        && param_names
            .iter()
            .position(|name| name == "__slice_literal__")
            .is_some_and(|marker| marker >= 3)
}

/// Whether `symbol` is a single-parameter constructor whose parameter is the
/// nominal `String` (`StringSpan.__init__$ov$String`): the shape of an
/// `@implicit` view constructor borrowing a String.
pub fn single_nominal_string_ctor(symbol: &str) -> bool {
    symbol.ends_with(".__init__$ov$String")
}

/// The lowered symbol of an overloaded struct method (including `__init__` and
/// the other lifecycle methods): `Box.value$ov$Int`.
pub fn method_symbol(type_name: &str, method: &str, sig: &SignatureKey) -> String {
    format!("{type_name}.{method}{}", sig.suffix())
}

/// Methods whose callable identity includes the receiver convention.
///
/// Current Mojo overloads them purely on the receiver (borrowed vs owned
/// `__iter__`, `ref` vs `deinit` `unsafe_assume_init`) with identical explicit
/// parameters, so the convention participates in registration and symbol
/// mangling. A `$`-suffixed spelling keeps its source method's rule.
pub fn receiver_overloaded_method(method: &str) -> bool {
    matches!(
        method.split('$').next().unwrap_or(method),
        "__iter__" | "__len__" | "unsafe_assume_init"
    )
}

/// Convention-qualified symbol for a receiver-overloaded method (see
/// [`receiver_overloaded_method`]): identical explicit parameters are
/// disambiguated by the receiver convention.
pub fn receiver_method_symbol(
    type_name: &str,
    method: &str,
    convention: Option<ArgConvention>,
    sig: &SignatureKey,
) -> String {
    method_symbol(type_name, method, &sig.with_receiver(convention))
}

/// Callable identity for a static overload sharing its explicit parameter
/// shape with an instance method (current Tuple's two `__len__` spellings).
pub fn static_method_symbol(type_name: &str, method: &str, sig: &SignatureKey) -> String {
    // Static methods have no source receiver convention. `SelfOut` is an
    // identity-only discriminator which cannot collide with a real static or
    // instance `__len__` declaration.
    method_symbol(
        type_name,
        method,
        &sig.with_receiver(Some(ArgConvention::Out)),
    )
}

/// Convention-qualified symbol for `__iter__` overloads.
///
/// Current Mojo permits borrowed and owned `__iter__` methods with identical
/// explicit parameters; receiver convention is therefore part of this method's
/// callable identity.
pub fn iterator_method_symbol(
    type_name: &str,
    convention: Option<ArgConvention>,
    sig: &SignatureKey,
) -> String {
    receiver_method_symbol(type_name, "__iter__", convention, sig)
}

/// Whether a selected method symbol dispatches through a trait bound: its
/// receiver is the abstract `__trait_dispatch`, and a concrete receiver's
/// witness table records the member answering it.
pub fn is_trait_dispatch_symbol(symbol: &str) -> bool {
    split_method_symbol(symbol).is_some_and(|(receiver, _)| receiver == TRAIT_DISPATCH)
}

/// Respell a method symbol under another receiver, preserving the method
/// and signature suffix.
///
/// It spells an instance's member names (`List.grow` as
/// `List$mono$TInt.grow`), and a `def(...)` contract's `__call__` on a
/// struct callee (R1466).
///
/// Keeping this parsing here preserves this module's ownership of the overload
/// encoding.
pub fn retarget_method_symbol(symbol: &str, type_name: &str) -> Option<String> {
    let (_, method_and_signature) = split_method_symbol(symbol)?;
    Some(format!("{type_name}.{method_and_signature}"))
}

/// Whether a resolved method symbol denotes the `Indexer` normalization hook.
/// Keep this knowledge beside the overload encoding so checked/MIR consumers do
/// not parse `$ov$` spellings independently.
pub fn is_index_normalization_symbol(symbol: &str) -> bool {
    split_method_symbol(symbol).is_some_and(|(_, method)| {
        method == "__mlir_index__"
            || method
                .strip_prefix("__mlir_index__")
                .is_some_and(|suffix| suffix.starts_with(OV_SEP))
    })
}

/// The overload set a lowered function declaration belongs to, by the
/// source name a lookup finds it under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverloadFamily<'a> {
    /// A free function `name`.
    Free(&'a str),
    /// A method or trait requirement `Type.method`.
    Method(&'a str),
}

/// The overload set the function lowered as `lowered_name` belongs to: the
/// name before its signature qualifier, a free function's or a
/// `Type.method`. A lifted function belongs to none.
pub fn overload_family(lowered_name: &str) -> Option<OverloadFamily<'_>> {
    if lifted_owner(lowered_name).is_some() {
        return None;
    }
    let base = lowered_name
        .split_once(OV_SEP)
        .map_or(lowered_name, |(base, _)| base);
    Some(if split_method_symbol(base).is_some() {
        OverloadFamily::Method(base)
    } else {
        OverloadFamily::Free(base)
    })
}

/// The overloaded declarations of a program, scanned from its top level: which
/// free-function names and `Type.method` names have more than one definition
/// (and at which arities).
///
/// A trait's requirements count under `Trait.method`, so an overloaded
/// requirement owns its binders under the symbol [`lowered_method_name`]
/// qualifies it to.
///
/// Definitions of non-overloaded names keep their plain source name, so
/// lowering consults this before qualifying anything. The scan also keeps the
/// parameter names of each synthesized `@fieldwise_init` constructor.
///
/// Its encoding is sorted, so equal sets encode the same bytes.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OverloadSets {
    #[serde(serialize_with = "sorted_arities")]
    functions: HashMap<String, HashSet<usize>>,
    #[serde(serialize_with = "sorted_names")]
    all_functions: HashSet<String>,
    #[serde(serialize_with = "sorted_arities")]
    methods: HashMap<String, HashSet<usize>>,
    #[serde(serialize_with = "sorted_map")]
    fieldwise_constructors: HashMap<String, Vec<String>>,
}

/// What lowering a program asks of its overload sets, name by name.
///
/// [`OverloadSets`] answers from its own maps; an importer's program
/// answers from its own sets first and from the declaration of its package
/// that owns the name on a miss, as upstream resolves a name's overload set
/// from the declarations a lookup finds.
pub trait OverloadLookup {
    /// Whether free function `name` is overloaded and defines arity `arity`.
    fn function_is_overloaded(&self, name: &str, arity: usize) -> bool;

    /// Whether `name` denotes any linked free-function declaration. MIR uses
    /// this to distinguish a function value from a non-local runtime name.
    fn is_function(&self, name: &str) -> bool;

    /// Whether method `source_name` (`Type.method`) is overloaded and defines
    /// arity `arity` (`self` excluded, matching the declared parameter list).
    fn method_is_overloaded(&self, source_name: &str, arity: usize) -> bool;

    /// The parameter names of struct `name`'s synthesized `@fieldwise_init`
    /// constructor (its field names, in order); `None` when the struct has no
    /// such constructor.
    fn fieldwise_parameters(&self, name: &str) -> Option<Cow<'_, [String]>>;
}

impl OverloadSets {
    pub fn scan(program: &[Stmt]) -> Self {
        let mut sets = Self::default();
        sets.scan_more(program);
        sets
    }

    /// Add the declarations of `program`, which declares none of the names
    /// already here: an importer's own statements over its package.
    pub fn scan_more(&mut self, program: &[Stmt]) {
        let mut functions: HashMap<String, Vec<usize>> = HashMap::new();
        let mut methods: HashMap<String, Vec<usize>> = HashMap::new();
        let fieldwise_constructors = &mut self.fieldwise_constructors;
        for stmt in program {
            match &stmt.kind {
                StmtKind::Def { name, params, .. } => {
                    functions
                        .entry(name.clone())
                        .or_default()
                        .push(params.len());
                }
                StmtKind::Struct {
                    name,
                    methods: ms,
                    fields,
                    fieldwise_init,
                    ..
                } => {
                    if *fieldwise_init
                        && !ms
                            .iter()
                            .any(|method| lifecycle_method_name(method) == "__init__")
                    {
                        fieldwise_constructors.insert(
                            name.clone(),
                            fields.iter().map(|field| field.name.clone()).collect(),
                        );
                    }
                    for method in ms {
                        let method_name = lifecycle_method_name(method);
                        methods
                            .entry(format!("{name}.{method_name}"))
                            .or_default()
                            .push(method.params.len());
                    }
                }
                StmtKind::Trait {
                    name, methods: ms, ..
                } => {
                    for method in ms {
                        methods
                            .entry(format!("{name}.{}", method.name))
                            .or_default()
                            .push(method.params.len());
                    }
                }
                _ => {}
            }
        }
        self.all_functions.extend(functions.keys().cloned());
        self.functions.extend(keep_overloaded(functions));
        self.methods.extend(keep_overloaded(methods));
    }

    /// Record every declaration of method `source_name` (`Type.method`) by
    /// its arity, the struct's spelled ones and those its check synthesizes
    /// alike: the name is overloaded when it is declared more than once.
    pub fn declare_method(&mut self, source_name: &str, arities: Vec<usize>) {
        if let Some((name, arities)) =
            keep_overloaded(HashMap::from([(source_name.to_string(), arities)]))
                .into_iter()
                .next()
        {
            self.methods.insert(name, arities);
        }
    }

    /// The sets the declarations `members` imply, each a function's
    /// overload family and source arity, beside the parameter names of each
    /// struct's synthesized fieldwise constructor: what [`Self::scan`] finds
    /// of a program.
    pub fn declared<'a>(
        members: impl IntoIterator<Item = (OverloadFamily<'a>, usize)>,
        fieldwise: impl IntoIterator<Item = (String, Vec<String>)>,
    ) -> Self {
        let mut functions: HashMap<String, Vec<usize>> = HashMap::new();
        let mut methods: HashMap<String, Vec<usize>> = HashMap::new();
        for (family, arity) in members {
            let (counts, name) = match family {
                OverloadFamily::Free(name) => (&mut functions, name),
                OverloadFamily::Method(name) => (&mut methods, name),
            };
            counts.entry(name.to_string()).or_default().push(arity);
        }
        Self {
            all_functions: functions.keys().cloned().collect(),
            functions: keep_overloaded(functions),
            methods: keep_overloaded(methods),
            fieldwise_constructors: fieldwise.into_iter().collect(),
        }
    }

    /// The first name, sorted, whose facts here differ from `other`'s;
    /// `None` when the two agree.
    pub fn differing_name(&self, other: &Self) -> Option<String> {
        fn differing<V: PartialEq>(
            left: &HashMap<String, V>,
            right: &HashMap<String, V>,
        ) -> impl Iterator<Item = String> {
            let names: std::collections::BTreeSet<&String> =
                left.keys().chain(right.keys()).collect();
            names
                .into_iter()
                .filter(|name| left.get(*name) != right.get(*name))
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
        }
        differing(&self.functions, &other.functions)
            .chain(
                self.all_functions
                    .symmetric_difference(&other.all_functions)
                    .cloned(),
            )
            .chain(differing(&self.methods, &other.methods))
            .chain(differing(
                &self.fieldwise_constructors,
                &other.fieldwise_constructors,
            ))
            .min()
    }
}

impl OverloadLookup for OverloadSets {
    fn function_is_overloaded(&self, name: &str, arity: usize) -> bool {
        self.functions
            .get(name)
            .is_some_and(|arities| arities.contains(&arity))
    }

    fn is_function(&self, name: &str) -> bool {
        self.all_functions.contains(name)
    }

    fn method_is_overloaded(&self, source_name: &str, arity: usize) -> bool {
        self.methods
            .get(source_name)
            .is_some_and(|arities| arities.contains(&arity))
    }

    fn fieldwise_parameters(&self, name: &str) -> Option<Cow<'_, [String]>> {
        self.fieldwise_constructors
            .get(name)
            .map(|names| Cow::Borrowed(names.as_slice()))
    }
}

/// The name a top-level `def` lowers to: signature-qualified when the name is
/// overloaded, the plain source name otherwise.
///
/// A module integer constant the signature names spells as the value
/// `constants` answers, its declaration's.
pub fn lowered_def_name<P: AnnotatedParam>(
    name: &str,
    type_params: &[TypeParam],
    params: &[P],
    sets: &impl OverloadLookup,
    constants: &dyn ModuleNames,
) -> String {
    if sets.function_is_overloaded(name, params.len()) {
        function_symbol(
            name,
            // A free function has no enclosing struct, so no `Self` to canonicalize.
            // The parameters after a `*args` collector are keyword-only, and
            // their names tell `f(a: Int, *rest: Int)` from
            // `f(*rest: Int, a: Int)`, whose type sequences agree.
            &signature_from_ast(params, type_params, constants, None)
                .with_keyword_names(keyword_only_names(params, None))
                .with_parameter_names(explicit_parameter_names(type_params))
                .with_owned_params(
                    &params
                        .iter()
                        .filter(|param| param.kind() == ParamKind::Regular)
                        .map(DeclaredParam::convention)
                        .collect::<Vec<_>>(),
                ),
        )
    } else {
        name.to_string()
    }
}

/// The signature facts of a struct method or trait requirement that its
/// lowered symbol spells.
#[derive(Debug)]
pub struct MethodShape<'a, P = FnParam> {
    /// The method's own compile-time parameters.
    pub type_params: &'a [TypeParam],
    pub params: &'a [P],
    pub keyword_only: Option<usize>,
    pub has_self: bool,
    pub self_convention: Option<ArgConvention>,
}

impl<P> Clone for MethodShape<'_, P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P> Copy for MethodShape<'_, P> {}

impl<'a> MethodShape<'a> {
    pub fn of(m: &'a Method) -> Self {
        Self {
            type_params: &m.type_params,
            params: &m.params,
            keyword_only: m.keyword_only,
            has_self: m.has_self,
            self_convention: m.self_convention,
        }
    }
}

/// The name a struct method lowers to, from its already-joined source name
/// (`Type.method`): signature-qualified when overloaded, unchanged otherwise.
///
/// The key spells every binder in scope — the struct's `type_params`, then
/// the method's own — with its bounds, exactly as the checker's `TyKind::Param`
/// spells a call's selected overload, so the symbol a call names is the one
/// MIR declares. A module integer constant spells as [`lowered_def_name`]
/// spells one.
pub fn lowered_method_name<P: AnnotatedParam>(
    source_name: &str,
    type_params: &[TypeParam],
    method: MethodShape<'_, P>,
    sets: &impl OverloadLookup,
    constants: &dyn ModuleNames,
) -> String {
    let MethodShape {
        type_params: method_type_params,
        params,
        keyword_only,
        has_self,
        self_convention,
    } = method;
    if sets.method_is_overloaded(source_name, params.len()) {
        // The enclosing struct's own instance type canonicalizes to `Self` in
        // the parameter keys, so a same-arity `self`-typed overload keys as
        // `$ov$Self` and matches the call side's canonicalized key.
        let self_spelling = split_method_symbol(source_name)
            .and_then(|(type_name, _)| self_struct_spelling(type_name, type_params));
        let scope: Vec<TypeParam> = type_params
            .iter()
            .chain(method_type_params)
            .cloned()
            .collect();
        let signature = signature_from_ast(params, &scope, constants, self_spelling.as_deref())
            .with_keyword_names(keyword_only_names(params, keyword_only))
            .with_parameter_names(explicit_parameter_names(method_type_params));
        match split_method_symbol(source_name)
            .filter(|(_, method)| receiver_overloaded_method(method))
        {
            Some((type_name, method)) => {
                if has_self {
                    receiver_method_symbol(type_name, method, self_convention, &signature)
                } else {
                    static_method_symbol(type_name, method, &signature)
                }
            }
            None => format!("{source_name}{}", signature.suffix()),
        }
    } else {
        source_name.to_string()
    }
}

/// The name a method is *registered and counted* under.
///
/// Current Mojo spells the copy constructor as an `__init__` overload with an
/// `out self, copy: Self` shape, which the whole pipeline models as
/// `__copyinit__`.
pub fn lifecycle_method_name(m: &Method) -> &str {
    lifecycle_name(LifecycleShape::of(m))
}

/// The signature facts of a struct method that decide whether it is a
/// lifecycle constructor.
#[derive(Debug)]
pub struct LifecycleShape<'a, P = FnParam> {
    pub name: &'a str,
    pub has_self: bool,
    pub self_convention: Option<ArgConvention>,
    pub positional_only: Option<usize>,
    pub keyword_only: Option<usize>,
    pub params: &'a [P],
    /// Whether the method declares a result type.
    pub returns: bool,
}

impl<P> Clone for LifecycleShape<'_, P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P> Copy for LifecycleShape<'_, P> {}

impl<'a> LifecycleShape<'a> {
    pub fn of(m: &'a Method) -> Self {
        Self {
            name: &m.name,
            has_self: m.has_self,
            self_convention: m.self_convention,
            positional_only: m.positional_only,
            keyword_only: m.keyword_only,
            params: &m.params,
            returns: m.ret.is_some(),
        }
    }
}

/// [`lifecycle_method_name`] over any method's declared shape.
pub fn lifecycle_name<P: AnnotatedParam>(shape: LifecycleShape<'_, P>) -> &str {
    if is_mojo_copy_constructor(&shape) {
        "__copyinit__"
    } else if is_mojo_move_constructor(&shape) {
        "__moveinit__"
    } else {
        shape.name
    }
}

/// The declaration that owns each struct method's own binders.
///
/// It is `Struct.method`, or for an overloaded method the symbol it lowers
/// to, so two overloads own distinct binders. The checker and the
/// elaborator both number a method's binders under this owner.
#[derive(Debug, Clone, Default)]
pub struct MethodBinderOwners {
    /// The lowered symbol of every overloaded method, keyed by its struct
    /// and beside the declaration it lowers: overloads differ in name or
    /// signature, never in both alike. Its signature qualifier is the one a
    /// call selecting that overload records.
    overloaded: HashMap<String, Vec<(OverloadDeclaration, String)>>,
}

impl MethodBinderOwners {
    pub fn scan(program: &[Stmt], sets: &impl OverloadLookup, constants: &dyn ModuleNames) -> Self {
        let overloaded = program
            .iter()
            .filter_map(|statement| match &statement.kind {
                StmtKind::Struct {
                    name,
                    type_params,
                    methods,
                    ..
                } => Some((name, type_params, methods)),
                _ => None,
            })
            .flat_map(|(name, type_params, methods)| {
                methods
                    .iter()
                    .filter(|m| specialization_template(&m.name).is_none())
                    .filter_map(move |m| {
                        let source = format!("{name}.{}", lifecycle_method_name(m));
                        let lowered = lowered_method_name(
                            &source,
                            type_params,
                            MethodShape::of(m),
                            sets,
                            constants,
                        );
                        (lowered != source)
                            .then(|| (name.clone(), (OverloadDeclaration::of(m), lowered)))
                    })
            })
            .fold(
                HashMap::<String, Vec<_>>::new(),
                |mut overloaded, (template, entry)| {
                    overloaded.entry(template).or_default().push(entry);
                    overloaded
                },
            );
        Self { overloaded }
    }

    /// The owner of `m`'s own binders on the struct `owner` names, a
    /// template or one of its specializations.
    pub fn owner(&self, owner: &str, m: &Method) -> String {
        let template = specialization_template(owner).unwrap_or(owner);
        self.lowered(template, m).map_or_else(
            || {
                format!(
                    "{template}.{}",
                    specialization_template(&m.name).unwrap_or(&m.name)
                )
            },
            str::to_string,
        )
    }
}

impl MethodBinderOwners {
    /// The signature qualifier a call selecting `m` records
    /// (`$ov$T$Copyable$Int`), when `m` is one of several overloads on the
    /// struct `owner` names.
    pub fn call_qualifier(&self, owner: &str, m: &Method) -> Option<&str> {
        self.lowered(owner, m)
            .or_else(|| self.lowered(specialization_template(owner)?, m))
            .and_then(overload_qualifier)
    }

    /// The lowered symbol of the overloaded method on `template` that `m`
    /// declares.
    fn lowered(&self, template: &str, m: &Method) -> Option<&str> {
        self.overloaded
            .get(template)?
            .iter()
            .find(|(declaration, _)| declaration.declares(m))
            .map(|(_, lowered)| lowered.as_str())
    }
}

/// The lifted name of a nested `def` (`inner` declared inside `outer`).
pub fn nested_lifted_name(outer: &str, inner: &str) -> String {
    format!("{outer}${inner}")
}

/// A same-spelled nested declaration's lifted name.
///
/// Declaration identity, not a source offset, provides the disambiguator so
/// cloned/synthesized syntax and ordinary block shadowing follow one stable
/// scheme.
pub fn nested_lifted_declaration_name(
    outer: &str,
    inner: &str,
    declaration: mojito_types::types::CheckedDeclId,
) -> String {
    format!("{outer}${inner}$decl{}", declaration.0)
}

/// The storage struct of the capture-list closure lifted as `lifted`, one
/// field per capture, as upstream's `ClosureEmitter` names `fn::__storage`.
pub fn closure_storage_name(lifted: &str) -> String {
    format!("{lifted}$__storage")
}

/// The method of its storage struct a closure's body is.
///
/// The closure is lifted as `lifted` and declared `source`; its storage is
/// [`closure_storage_name`], as upstream's `liftClosureIntoMethod` nests the
/// body in `fn::__storage`.
pub fn closure_method_name(lifted: &str, source: &str) -> String {
    format!("{}.{source}", closure_storage_name(lifted))
}

/// Whether struct `name` is a closure's storage ([`closure_storage_name`]).
pub fn is_closure_storage_name(name: &str) -> bool {
    name.ends_with("$__storage")
}

/// Whose a lifted compile-time function is, as its name says
/// ([`lifted_owner`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiftedOwner<'a> {
    /// A module constant's initializer: `$comptime$<constant>$module`.
    Constant(&'a str),
    /// An evaluated parameter default: `$default$<owner>$<parameter>`.
    Default { owner: &'a str, parameter: &'a str },
    /// A function the check lifts from the body of the function `owner`,
    /// numbered within it, the same in every compilation
    /// (`$comptime$<function>$c<k>`).
    Numbered(&'a str),
}

/// The lifted function of the module constant `constant`'s initializer.
pub fn lifted_constant_name(constant: &str) -> String {
    format!("$comptime${constant}$module")
}

/// The `index`th function the check lifts from the body of `function`: a
/// compile-time expression or a local display binding.
pub fn lifted_check_name(function: &str, index: usize) -> String {
    format!("$comptime${function}$c{index}")
}

/// The function an evaluated default of `owner`'s `parameter` lowers to.
pub fn lifted_default_name(owner: &str, parameter: &str) -> String {
    format!("$default${owner}${parameter}")
}

/// Whose the lifted function `name` is, or `None` when `name` names no
/// lifted function.
pub fn lifted_owner(name: &str) -> Option<LiftedOwner<'_>> {
    if let Some(rest) = name.strip_prefix("$default$") {
        let (owner, parameter) = rest.rsplit_once('$')?;
        return Some(LiftedOwner::Default { owner, parameter });
    }
    let (owner, tail) = name
        .strip_prefix("$comptime$")?
        .rsplit_once('$')
        .filter(|(owner, _)| !owner.is_empty())?;
    Some(match (owner, tail) {
        (constant, "module") => LiftedOwner::Constant(constant),
        (owner, _) => LiftedOwner::Numbered(owner),
    })
}

/// A deliberate **poison name** for an overloaded call the checker recorded no
/// target for (only reachable off the checked path).
///
/// It can never name a real function, so the VM reports it instead of guessing
/// among overloads.
pub fn unresolved_overload_marker(name: &str, argc: usize) -> String {
    format!("{name}#{argc}")
}

/// Whether `symbol` is a signature-qualified overload of source name `base`
/// (used by the VM's arity fallback to enumerate an overload set).
pub fn is_overload_of(symbol: &str, base: &str) -> bool {
    symbol
        .strip_prefix(base)
        .is_some_and(|rest| rest.starts_with(OV_SEP))
}

/// The signature qualifier of an overload symbol (`$ov$T$Int` in
/// `First.pick$ov$T$Int`), or `None` when `symbol` names no overload.
///
/// It names one declaration of an overloaded method whatever receiver
/// spelling the symbol carries, so a per-call clone request and the
/// template it selects compare on it.
pub fn overload_qualifier(symbol: &str) -> Option<&str> {
    symbol.find(OV_SEP).map(|start| &symbol[start..])
}

/// The lowered symbol of `type_name`'s `method` whose overload qualifier is
/// `qualifier` ([`overload_qualifier`]), or of its only declaration when
/// `qualifier` is `None`.
pub fn qualified_method_symbol(type_name: &str, method: &str, qualifier: Option<&str>) -> String {
    format!("{type_name}.{method}{}", qualifier.unwrap_or_default())
}

/// The method name and overload qualifier of `type_name`'s method whose
/// lowered symbol is `symbol` ([`qualified_method_symbol`]'s inverse), or
/// `None` when `symbol` is not one of `type_name`'s methods.
pub fn method_of_symbol<'s>(
    type_name: &str,
    symbol: &'s str,
) -> Option<(&'s str, Option<&'s str>)> {
    let rest = symbol.strip_prefix(type_name)?.strip_prefix('.')?;
    let qualifier = overload_qualifier(rest);
    let method = &rest[..rest.len() - qualifier.map_or(0, str::len)];
    (!method.is_empty() && !method.contains('.')).then_some((method, qualifier))
}

/// Whether an overload symbol's signature qualifier is exactly the `NoneType`
/// parameter spelling (`…$ov$None`).
///
/// The `@implicit` None-conversion constructor, which arity-based selection
/// must exclude when a bound can never be `None`.
pub fn is_none_overload(symbol: &str) -> bool {
    symbol.ends_with("$ov$None")
}

/// If `symbol` is a signature-qualified `__init__` overload (`Type.__init__$ov$…`),
/// the struct it constructs.
pub fn init_overload_struct(symbol: &str) -> Option<&str> {
    let (struct_name, rest) = symbol.rsplit_once(".__init__")?;
    rest.starts_with(OV_SEP).then_some(struct_name)
}

/// The struct a constructor symbol builds, and the lifecycle name it runs.
///
/// A bare `__init__` and its signature-qualified overload both answer here.
///
/// The copy and move constructors answer here too: a construction that names
/// one builds its receiver the same way.
pub fn lifecycle_constructor(symbol: &str) -> Option<(&str, &str)> {
    ["__init__", "__copyinit__", "__moveinit__"]
        .iter()
        .find_map(|lifecycle| {
            mojito_types::symbol_ref::member_owner(symbol, lifecycle)
                .map(|struct_name| (struct_name, *lifecycle))
        })
}

/// Whether the member `method` is an initializer: `__init__`, or the copy
/// or move constructor, each an `__init__` overload upstream, whose
/// declaration restates its struct's parameters ahead of its own.
pub fn is_initializer_member(method: &str) -> bool {
    matches!(method, "__init__" | "__copyinit__" | "__moveinit__")
}

/// Whether `symbol` names a lifecycle initializer, whose receiver is `out self`.
///
/// Recognizes `Type.__init__`, `Type.__copyinit__`, and `Type.__moveinit__`,
/// bare or with a `$`-mangled overload or specialization suffix, but not a
/// function lifted from one ([`lifted_owner`]), which has no receiver.
pub fn is_initializer_symbol(symbol: &str) -> bool {
    lifted_owner(symbol).is_none() && lifecycle_constructor(symbol).is_some()
}

/// Which identity a mangled type spelling serves: an overload key, or a
/// generic instance name, which spells a minted instance (`W$mono$TInt`) as
/// its own symbol.
#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyMode {
    Overload,
    Instance,
}

fn ty_raw(ty: &Ty, self_ty: Option<&Ty>) -> String {
    ty_raw_in(ty, self_ty, KeyMode::Overload)
}

#[allow(
    clippy::format_push_string,
    reason = "TODO: write! into the buffer instead"
)]
fn ty_raw_in(ty: &Ty, self_ty: Option<&Ty>, mode: KeyMode) -> String {
    // Canonicalize the enclosing struct type to `Self` however it is spelled
    // (`Self`, `Pair`, or `List[Self.T]` inside `List`). The declaration side
    // does the same (see `ast_raw`), so both agree on one key for a same-arity
    // overload's self-typed parameter and the checker-recorded callee names the
    // real MIR function. Origins erase from callable identity, so a `sep:
    // StringSpan` parameter inside `StringSpan` is `Self` whatever origin tail
    // the receiver carries.
    if self_ty.is_some_and(|self_ty| {
        self_ty == ty
            || mojito_types::types::erase_origin_arguments(self_ty)
                == mojito_types::types::erase_origin_arguments(ty)
    }) {
        return "Self".to_string();
    }
    match ty.kind() {
        // An `IntLiteral` (`FloatLiteral`) parameter is an overload of its
        // own beside an `Int` (`Float64`) one, as upstream's `IntLiteral[_]`
        // is; an instance argument materializes the literal.
        TyKind::IntLiteral if mode == KeyMode::Overload => "IntLiteral".to_string(),
        TyKind::FloatLiteral if mode == KeyMode::Overload => "FloatLiteral".to_string(),
        TyKind::INT | TyKind::IntLiteral => "Int".to_string(),
        TyKind::UINT => "UInt".to_string(),
        TyKind::FLOAT64 | TyKind::FloatLiteral => "Float64".to_string(),
        TyKind::Bool => "Bool".to_string(),
        TyKind::StringLiteral(None) => "StringLiteral".to_string(),
        // A literal whose type carries its value spells as the application
        // `StringLiteral[value]` it bridges.
        TyKind::StringLiteral(Some(value)) => format!(
            "{}$V{value}",
            encode_identifier(mojito_types::types::STRING_LITERAL_TYPE_NAME)
        ),
        TyKind::None => "None".to_string(),
        TyKind::ComptimeList(elem) => format!("__ComptimeList${}", ty_raw_in(elem, self_ty, mode)),
        TyKind::ParamList(elem) => format!("__ParamList${}", ty_raw_in(elem, self_ty, mode)),
        TyKind::AnyTrait(traits) => format!("__AnyTrait${}", traits.join("$")),
        TyKind::Trait(traits) => format!("__Trait${}", traits.join("$")),
        TyKind::TypeType => mojito_types::types::TYPE_OF_ALL_TYPES.to_string(),
        TyKind::KgenString => "__KgenString".to_string(),
        TyKind::KgenDeferred => "__KgenDeferred".to_string(),
        TyKind::KgenTarget => "__KgenTarget".to_string(),
        // A vector's storage keys as the vector it stores, marked apart.
        TyKind::KgenSimd { .. } => format!(
            "__KgenSimd${}",
            mojito_types::types::storage_simd(ty)
                .map_or_else(|| ty.to_string(), |vector| ty_raw_in(&vector, None, mode))
        ),
        TyKind::Tuple(elems) => format!(
            "Tuple${}",
            elems
                .iter()
                .map(|elem| ty_raw_in(elem, self_ty, mode))
                .collect::<Vec<_>>()
                .join("$")
        ),
        TyKind::RuntimePack(elems) => format!(
            "$pack${}",
            elems
                .iter()
                .map(|elem| ty_raw_in(elem, self_ty, mode))
                .collect::<Vec<_>>()
                .join("$")
        ),
        TyKind::VariadicPack(element) => format!("$variadic${}", ty_raw_in(element, self_ty, mode)),
        TyKind::Variant(alternatives) => format!(
            "Variant${}",
            alternatives
                .iter()
                .map(|alt| ty_raw_in(alt, self_ty, mode))
                .collect::<Vec<_>>()
                .join("$")
        ),
        // The nominal stdlib String keeps the historical `String` symbol
        // spelling while the compile-time literal spells `StringLiteral`, so
        // the two are distinct overload keys (as upstream: a literal argument
        // selects the exact `StringLiteral` overload beside a `String` one).
        TyKind::Struct(name, args) if args.is_empty() && is_stdlib_string_struct(name) => {
            "String".to_string()
        }
        // A minted generic instance (`W$mono$TInt`) spells as its own symbol:
        // that symbol already carries every argument, so re-encoding it and
        // appending the arguments again would make a nested instance's name
        // grow with its nesting depth.
        TyKind::Struct(name, _) if mode == KeyMode::Instance && is_instance_struct_symbol(name) => {
            name.clone()
        }
        // A struct type spells as its annotation does (`Point`, `Pair$Int`) —
        // no `Struct$` marker, so the MIR definition name matches.
        TyKind::Struct(name, args) => {
            let mut s = encode_identifier(name);
            for arg in args {
                match arg {
                    TyArg::Ty(t) => {
                        s.push('$');
                        s.push_str(&ty_raw_in(t, self_ty, mode));
                    }
                    TyArg::Val(v) => {
                        s.push('$');
                        s.push_str(&value_argument_raw(v, mode));
                    }
                    // The origin tail erases from the runtime ABI, exactly as
                    // the annotation side (`ast_raw`) erases origin arguments:
                    // origin-differing types share a specialization and a
                    // lowering, like `TyKind::Pointer` origins.
                    TyArg::Origin(_) => {}
                }
            }
            s
        }
        // A type parameter spells as the bare annotation `T` does.
        TyKind::Param {
            binder,
            bounds,
            callable_bound,
        } => {
            let mut result = encode_identifier(&binder.name);
            for bound in bounds {
                result.push('$');
                result.push_str(&encode_identifier(bound));
            }
            if let Some(callable) = callable_bound {
                result.push_str("$Callable$");
                result.push_str(&ty_raw_in(callable, self_ty, mode));
            }
            result
        }
        // Pointer origins affect checking/lifetimes but erase from the runtime
        // callable ABI, just like origin parameters on `ref` arguments. The
        // mangled spelling deliberately stays `UnsafePointer$` while the
        // user-facing name is `Pointer`: this string is internal callable
        // identity, and renaming it would churn every symbol golden for no
        // user-visible gain.
        TyKind::Pointer { element, .. } => {
            format!("UnsafePointer${}", ty_raw_in(element, self_ty, mode))
        }
        TyKind::KgenPointer(element) => {
            format!("KgenPointer${}", ty_raw_in(element, self_ty, mode))
        }
        TyKind::LitRef { element, .. } => {
            format!("LitRef${}", ty_raw_in(element, self_ty, mode))
        }
        // Application arguments participate in the mangled identity (so
        // `IteratorType[a]` and `IteratorType[b]` are distinct), except origins,
        // which erase from the runtime ABI like `TyKind::Pointer` origins above.
        TyKind::Assoc { base, name, args } => {
            let mut s = format!(
                "Assoc${}${}",
                ty_raw_in(base, self_ty, mode),
                encode_identifier(name)
            );
            for arg in args {
                s.push('$');
                match arg {
                    TyArg::Ty(t) => s.push_str(&ty_raw_in(t, self_ty, mode)),
                    TyArg::Val(v) => s.push_str(&value_argument_raw(v, mode)),
                    // Origins erase from the runtime ABI: every origin argument
                    // mangles to one marker so origin-differing types share a
                    // specialization and a lowering, like `TyKind::Pointer` origins.
                    TyArg::Origin(_) => s.push('O'),
                }
            }
            s
        }
        TyKind::SelfType => "Self".to_string(),
        other => other.to_string(),
    }
}

/// A value argument of a struct or associated type: an overload key spells
/// a bare reference to a value binder (`Array[Int, n]`, the auto-parameterized
/// `Array[T, _]`) as the declaration's annotation does, the binder followed by
/// its value type (`n$Int`), and any other value as `V` and its display.
fn value_argument_raw(value: &CtValue, mode: KeyMode) -> String {
    if mode == KeyMode::Overload
        && let CtValue::Expr(expression) = value
        && let Some(binder) = expression.as_decl_ref()
        && let MetaTy::Value(ty) = expression.meta()
    {
        return format!(
            "{}${}",
            encode_identifier(&binder.name),
            ty_raw_in(ty, None, mode)
        );
    }
    format!("V{value}")
}

/// The longest argument spelling [`instance_symbol`] keeps verbatim.
const INSTANCE_SPELLING_LIMIT: usize = 96;

/// Lowercase hexadecimal of `bytes`.
fn hex_digest(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

/// Whether `name` is a struct instance symbol [`instance_symbol`] minted.
/// No source identifier contains `$`, so the `$mono` marker cannot be
/// spelled by a user type.
fn is_instance_struct_symbol(name: &str) -> bool {
    name.contains("$mono")
}

/// Encode source-controlled identifier text injectively while leaving ordinary
/// ASCII identifiers unchanged. Structural `$` separators are added only after
/// this encoding, so stropped names such as `A-B` and `A_B` cannot collide.
#[allow(
    clippy::format_push_string,
    reason = "TODO: write! into the buffer instead"
)]
fn encode_identifier(name: &str) -> String {
    let mut encoded = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            encoded.push(ch);
        } else {
            encoded.push_str(&format!("$u{:X}$", ch as u32));
        }
    }
    encoded
}

fn ast_raw(
    ty: &Type,
    comptimes: &dyn ModuleNames,
    type_bounds: &HashMap<String, BinderSpelling>,
    self_spelling: Option<&str>,
) -> String {
    // A bracket operand parses as an expression; an application among a
    // type's arguments (`List[Pair[Int]]`) spells as the type it names.
    if let Type::Named(name, args) = ty
        && args
            .iter()
            .any(|arg| matches!(spelled_type_argument(arg), Cow::Owned(_)))
    {
        let args = args
            .iter()
            .map(|arg| spelled_type_argument(arg).into_owned())
            .collect();
        return ast_raw(
            &Type::Named(name.clone(), args),
            comptimes,
            type_bounds,
            self_spelling,
        );
    }
    let raw = match ty {
        // A module type alias spells as the type it names, as the checker's
        // resolved type does; a binder of the same name shadows it.
        Type::Named(name, args)
            if args.is_empty()
                && !type_bounds.contains_key(name)
                && let Some(aliased) = comptimes.type_alias(name) =>
        {
            return ast_raw(&aliased, comptimes, type_bounds, self_spelling);
        }
        // `__mlir_type[`!kgen.simd<`, length._mlir_value, `, `,
        // dtype._mlir_value, `>`]` spells as the checker's storage of the
        // vector `SIMD[dtype, length]` does.
        Type::Named(name, args)
            if name == "__mlir_type"
                && let Some(vector) = mlir_simd_annotation(args) =>
        {
            return format!(
                "__KgenSimd${}",
                ast_raw(&vector, comptimes, type_bounds, None)
            );
        }
        Type::Int => "Int".to_string(),
        Type::UInt => "UInt".to_string(),
        Type::Bool => "Bool".to_string(),
        Type::StringLiteral | Type::ClosedStringLiteral => "StringLiteral".to_string(),
        Type::Float64 => "Float64".to_string(),
        Type::None => "None".to_string(),
        Type::Named(name, args) if args.is_empty() && is_stdlib_string_struct(name) => {
            // Mirror `ty_raw`: the nominal String annotation mangles as the
            // stable `String` spelling.
            "String".to_string()
        }
        // Mirror `ty_raw`: the `NoneType` annotation resolves to `TyKind::None`,
        // which mangles as `None`.
        Type::Named(name, args) if args.is_empty() && name == "NoneType" => "None".to_string(),
        // A scalar alias (`Byte`, `UInt8`) mangles as the width-1 vector it
        // names, as `ty_raw` spells the resolved `TyKind::Simd`.
        Type::Named(name, args)
            if args.is_empty()
                && !type_bounds.contains_key(name)
                && let Some(dtype) = mojito_ast::ast::Dtype::from_scalar_alias(name) =>
        {
            simd_annotation_raw(dtype, 1)
        }
        // `Scalar[DType.x]` / `SIMD[DType.x, N]` annotations mangle as their
        // canonical checked spelling (`Scalar[DType.float32]` is the width-1
        // `Float32`), matching `ty_raw`'s display of the resolved `TyKind::Simd`.
        Type::Named(name, args)
            if name == "Scalar" && args.len() == 1 && param_arg_dtype(&args[0]).is_some() =>
        {
            let dtype = param_arg_dtype(&args[0]).expect("guard established a dtype");
            simd_annotation_raw(dtype, 1)
        }
        Type::Named(name, args)
            if name == "SIMD"
                && args.len() == 2
                && param_arg_dtype(&args[0]).is_some()
                && param_arg_width(&args[1], comptimes).is_some() =>
        {
            let dtype = param_arg_dtype(&args[0]).expect("guard established a dtype");
            let width = param_arg_width(&args[1], comptimes).expect("guard established a width");
            simd_annotation_raw(dtype, width)
        }
        // Mirror `ty_raw`: every pointer annotation (`UnsafePointer[T]`,
        // `Pointer[T, origin]`, the `ImmPointer`/`MutPointer` permission
        // aliases) mangles as `UnsafePointer$<element>` — the origin argument
        // erases from the runtime ABI, so a call site's `TyKind::Pointer` key and
        // the declaration agree.
        Type::Named(name, args)
            if matches!(
                name.as_str(),
                "Pointer" | "UnsafePointer" | "ImmPointer" | "MutPointer"
            ) && matches!(args.first(), Some(ParamArg::Type(_))) =>
        {
            let Some(ParamArg::Type(element)) = args.first() else {
                unreachable!("guard established a type argument");
            };
            format!(
                "UnsafePointer${}",
                ast_raw(element, comptimes, type_bounds, self_spelling)
            )
        }
        // A vector over a binder of the signature (`Scalar[dt]`,
        // `SIMD[dt, width]`) spells as the checker displays the symbolic
        // `TyKind::Simd`.
        Type::Named(name, args)
            if (name == "Scalar" && args.len() == 1 || name == "SIMD" && args.len() == 2)
                && args
                    .iter()
                    .any(|arg| simd_binder_slot(arg, type_bounds).is_some())
                && args.iter().all(|arg| {
                    simd_binder_slot(arg, type_bounds).is_some()
                        || param_arg_dtype(arg).is_some()
                        || param_arg_width(arg, comptimes).is_some()
                }) =>
        {
            let slots: Vec<String> = args
                .iter()
                .map(|arg| {
                    simd_binder_slot(arg, type_bounds)
                        .map(str::to_string)
                        .or_else(|| {
                            param_arg_dtype(arg).map(|dtype| format!("DType.{}", dtype.name()))
                        })
                        .or_else(|| param_arg_width(arg, comptimes).map(|width| width.to_string()))
                        .unwrap_or_default()
                })
                .collect();
            match slots.as_slice() {
                [dtype, width] if width == "1" => format!("Scalar[{dtype}]"),
                [dtype, width] => format!("SIMD[{dtype}, {width}]"),
                [dtype] => format!("Scalar[{dtype}]"),
                _ => unreachable!("guard established one or two slots"),
            }
        }
        // Mirror the checker's synthetic binder for an existential
        // `Some[Trait]` parameter, which `ty_raw` spells as its binder name
        // (`Some[Writer]`) followed by its one bound.
        Type::Named(name, args)
            if name == "Some" && args.len() == 1 && existential_trait(&args[0]).is_some() =>
        {
            let trait_name = existential_trait(&args[0]).expect("guard established a trait");
            format!(
                "{}${}",
                encode_identifier(&format!("Some[{trait_name}]")),
                encode_identifier(trait_name)
            )
        }
        // The runtime-pack collector of a specialized variadic (`*args: *Ts`
        // after `Ts` is bound) is the `$pack` annotation over its element
        // types; mirror `ty_raw`'s `TyKind::RuntimePack` spelling so the
        // declaration and its call sites agree.
        Type::Named(name, args) if name == "$pack" => format!(
            "$pack${}",
            args.iter()
                .map(|arg| match arg {
                    ParamArg::Type(t) => ast_raw(t, comptimes, type_bounds, self_spelling),
                    ParamArg::Value(v) => value_expr_raw(v, comptimes),
                    ParamArg::Named { .. } => String::new(),
                })
                .collect::<Vec<_>>()
                .join("$")
        ),
        Type::Named(name, args) => {
            let mut s = parameter_raw(name, type_bounds);
            for arg in args {
                // An origin argument — a placeholder (`_`/`...`), an
                // `origin_of(...)`/`ImmOrigin(...)`/`MutOrigin(...)` value, a
                // builtin origin name, or an `Origin`-bound parameter — erases
                // from the runtime ABI and from the checked spelling
                // (`ty_raw_in` emits nothing for the origin tail); erase it
                // from the declaration spelling too, so `Span[T, _]`,
                // `Span[T, origin_of(xs)]`, and `Span[T]` declare one symbol.
                if syntactic_origin_argument(arg, type_bounds) {
                    continue;
                }
                s.push('$');
                match arg {
                    ParamArg::Type(t) => {
                        s.push_str(&ast_raw(t, comptimes, type_bounds, self_spelling));
                    }
                    // The parser cannot tell a type parameter from a value
                    // in argument position (`List[T]` arrives as a value
                    // identifier); one naming a declared parameter of the
                    // signature spells as the checker's `TyKind::Param` does, so
                    // the declaration and its call sites agree.
                    ParamArg::Value(v)
                        if matches!(&v.kind, mojito_ast::ast::ExprKind::Identifier(name)
                            if type_bounds.contains_key(name)) =>
                    {
                        let mojito_ast::ast::ExprKind::Identifier(name) = &v.kind else {
                            unreachable!("guard matched an identifier");
                        };
                        s.push_str(&parameter_raw(name, type_bounds));
                    }
                    ParamArg::Value(v) => {
                        s.push('V');
                        s.push_str(&value_expr_raw(v, comptimes));
                    }
                    ParamArg::Named { name, value } => {
                        s.push_str(name);
                        s.push('=');
                        match &**value {
                            ParamArg::Type(t) => {
                                s.push_str(&ast_raw(t, comptimes, type_bounds, self_spelling));
                            }
                            ParamArg::Value(v) => s.push_str(&value_expr_raw(v, comptimes)),
                            ParamArg::Named { .. } => unreachable!(),
                        }
                    }
                }
            }
            s
        }
        // `Self.T` names the same parameter a bare `T` does inside the struct,
        // and the checker resolves both to the same `TyKind::Param` — spell them
        // identically so the two sides agree.
        Type::SelfParam(name) => parameter_raw(name, type_bounds),
        // An `` __mlir_type.`name` `` the subset reads spells as the type the
        // checker resolves it to (`mlir_type_named`), so the declaration and
        // its call sites agree.
        Type::Assoc { base, name, args }
            if args.is_empty()
                && matches!(base.as_ref(), Type::Named(base, base_args)
                    if base == "__mlir_type" && base_args.is_empty())
                && let Some(mlir) = mojito_types::types::mlir_type_named(name) =>
        {
            ty_raw(&mlir, None)
        }
        Type::Assoc { base, name, .. } => format!(
            "Assoc${}${}",
            ast_raw(base, comptimes, type_bounds, self_spelling),
            encode_identifier(name)
        ),
        Type::SelfType => "Self".to_string(),
        Type::Func { .. } => func_annotation_raw(ty),
        other => format!("{other:?}"),
    };
    // Canonicalize the enclosing struct type to `Self`, however it is spelled.
    // `self_spelling` is that struct's `ty_raw` mangling (e.g. `Pair`,
    // `List$T$AnyType`); a parameter that mangles the same denotes `Self`, and
    // the call side (`ty_raw` with `self_ty`) canonicalizes the equal resolved
    // type identically. A bare `Self` already produced `"Self"` above and never
    // equals a concrete `self_spelling`, so it passes through unchanged.
    match self_spelling {
        Some(spelling) if raw == spelling => "Self".to_string(),
        _ => raw,
    }
}

/// The `SIMD[dtype, length]` annotation whose storage a `__mlir_type[...]`
/// spelling of `!kgen.simd<length, dtype>` names, as the checker's
/// `mlir_applied_type` reads it; `None` for every other MLIR type spelling.
fn mlir_simd_annotation(args: &[ParamArg]) -> Option<Type> {
    let (text, operands) = mojito_types::types::mlir_type_spelling(args);
    let [length, dtype] = operands.as_slice() else {
        return None;
    };
    if text != "!kgen.simd<{},{}>" {
        return None;
    }
    let length = mojito_types::types::mlir_value_operand(length)?;
    let dtype = mojito_types::types::mlir_value_operand(dtype)?;
    Some(Type::Named("SIMD".to_string(), vec![dtype, length]))
}

/// Mirror `ty_raw`'s Display-based encoding of a resolved callable type
/// (`def(T) capturing[_] -> U`) for a source `def(...)` annotation, so a
/// callable-typed parameter produces the same overload suffix from the
/// declaration (AST) side and the call (checked `Ty`) side. Inner types render
/// as the checked Display does: bare parameter names without bounds and
/// bracketed argument lists. Annotations this cannot mirror (value arguments,
/// nominal names the linker qualifies) keep a stable source spelling that a
/// qualified call-site type will not match — no worse than the previous
/// Debug-format fallback, which embedded spans and never matched anything.
#[allow(
    clippy::format_push_string,
    reason = "TODO: write! into the buffer instead"
)]
/// A value operand that applies a type (`Pair[Int]`, `Box[1]`) as the type
/// argument it spells; any other argument unchanged. A bare name keeps its
/// value spelling.
fn spelled_type_argument(arg: &ParamArg) -> Cow<'_, ParamArg> {
    match arg {
        ParamArg::Value(value)
            if matches!(
                value.kind,
                ExprKind::TypeApply { .. } | ExprKind::Index { .. }
            ) =>
        {
            value.type_spelling().map_or(Cow::Borrowed(arg), |spelled| {
                Cow::Owned(ParamArg::Type(spelled))
            })
        }
        _ => Cow::Borrowed(arg),
    }
}

fn func_annotation_raw(ty: &Type) -> String {
    let Type::Func {
        type_params,
        params,
        ret,
        thin,
        capturing,
        raises,
        ..
    } = ty
    else {
        unreachable!("func_annotation_raw takes a Type::Func");
    };
    let mut s = String::from("def(");
    for (index, param) in params.iter().enumerate() {
        if index > 0 {
            s.push_str(", ");
        }
        s.push_str(&annotation_display(&param.ty));
    }
    s.push(')');
    if *thin {
        s.push_str(" thin");
    } else if let Some(spec) = capturing {
        s.push_str(" capturing[");
        match spec.as_slice() {
            [] => {}
            [
                Expr {
                    kind: ExprKind::Identifier(name),
                    ..
                },
            ] if name == "_" => s.push('_'),
            [
                Expr {
                    kind: ExprKind::Identifier(name),
                    ..
                },
            ] => match type_params.iter().position(|parameter| {
                parameter.name == *name && parameter.bounds.as_slice() == ["OriginSet"]
            }) {
                Some(index) => {
                    let _ = write!(s, "origin_set#{index}");
                }
                None => s.push_str(name),
            },
            _ => s.push_str("..."),
        }
        s.push(']');
    }
    if *raises {
        s.push_str(" raises");
    }
    s.push_str(" -> ");
    s.push_str(&annotation_display(ret));
    s
}

/// Render one annotation as the checked `Ty` Display would, for use inside
/// [`func_annotation_raw`] only: parameters print bare (no bounds), nominal
/// arguments print bracketed and comma-separated.
#[allow(
    clippy::format_push_string,
    reason = "TODO: write! into the buffer instead"
)]
fn annotation_display(ty: &Type) -> String {
    match ty {
        Type::Int => "Int".to_string(),
        Type::UInt => "UInt".to_string(),
        Type::Bool => "Bool".to_string(),
        Type::Float64 => "Float64".to_string(),
        Type::StringLiteral | Type::ClosedStringLiteral => "StringLiteral".to_string(),
        Type::None => "None".to_string(),
        Type::Named(name, args) if args.is_empty() && is_stdlib_string_struct(name) => {
            "String".to_string()
        }
        Type::Named(name, args) if args.is_empty() && name == "NoneType" => "None".to_string(),
        Type::Named(name, args) => {
            let mut s = name.clone();
            if !args.is_empty() {
                s.push('[');
                for (index, argument) in args.iter().enumerate() {
                    if index > 0 {
                        s.push_str(", ");
                    }
                    match argument {
                        ParamArg::Type(t) => s.push_str(&annotation_display(t)),
                        other => s.push_str(&format!("{other:?}")),
                    }
                }
                s.push(']');
            }
            s
        }
        Type::SelfParam(name) => name.clone(),
        Type::SelfType => "Self".to_string(),
        func @ Type::Func { .. } => func_annotation_raw(func),
        other => format!("{other:?}"),
    }
}

/// The dtype named by a `DType.<dt>` annotation argument, if that is what it is.
fn param_arg_dtype(argument: &ParamArg) -> Option<mojito_ast::ast::Dtype> {
    let ParamArg::Value(value) = argument else {
        return None;
    };
    expr_dtype(value)
}

/// The dtype a `DType.<dt>` expression names, if that is what it is.
fn expr_dtype(expr: &Expr) -> Option<mojito_ast::ast::Dtype> {
    let ExprKind::Member { object, field } = &expr.kind else {
        return None;
    };
    matches!(&object.kind, ExprKind::Identifier(name) if name == "DType")
        .then(|| mojito_ast::ast::Dtype::from_name(field))
        .flatten()
}

/// A comptime-evaluable SIMD width annotation argument.
fn param_arg_width(argument: &ParamArg, comptimes: &dyn ModuleNames) -> Option<i64> {
    let ParamArg::Value(expr) = argument else {
        return None;
    };
    eval_comptime_int(expr, comptimes)?.to_i64()
}

/// The canonical checked spelling of a scalar/SIMD annotation: width-1
/// `int`/`float64` canonicalize to the native scalars, everything else uses
/// the `TyKind::Simd` display (the scalar alias where one exists).
fn simd_annotation_raw(dtype: mojito_ast::ast::Dtype, width: i64) -> String {
    canonical_simd_ty(dtype, width).to_string()
}

/// The exact value of a signature's integer argument, folded as the check
/// folds it (`fold_infix` over `IntLiteral`s) over the module constants'
/// values the check's fold gave them; `None` for anything the check would
/// not fold to an integer.
fn eval_comptime_int(expr: &Expr, comptimes: &dyn ModuleNames) -> Option<IntLiteral> {
    use mojito_ast::ast::PrefixOp;
    use mojito_types::param_expr::fold::fold_infix;
    match &expr.kind {
        ExprKind::Int(value) => Some(value.clone()),
        ExprKind::Identifier(name) => comptimes.constant(name),
        ExprKind::Prefix(PrefixOp::Neg, value) => Some(eval_comptime_int(value, comptimes)?.neg()),
        ExprKind::Prefix(PrefixOp::Pos, value) => eval_comptime_int(value, comptimes),
        ExprKind::Infix(op, left, right) => {
            let (left, right) = (
                eval_comptime_int(left, comptimes)?,
                eval_comptime_int(right, comptimes)?,
            );
            match fold_infix(*op, &CtValue::IntLiteral(left), &CtValue::IntLiteral(right)) {
                Ok(CtValue::IntLiteral(value)) => Some(value),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The separator that marks a signature-qualified overload symbol.
///
/// `pick$ov$Int`, `Box.__init__$ov$String`. The native monomorphizer reads it
/// to tell such an overload from a per-instantiation clone.
pub const OV_SEP: &str = "$ov$";

fn sanitize(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '$' })
        .collect()
}

/// A callable-typed instantiation argument with its capture environment
/// erased. A callable-bounded parameter contributes nothing to a clone's
/// name, and a concrete capture set names per-check owners, which would make
/// the same instantiation look new on every discovery round.
fn erase_capture_environment(ty: &Ty) -> Ty {
    use mojito_types::origin::CallableEnvironment;
    let mut erased = ty.clone();
    if matches!(
        erased.as_func(),
        Some(FuncTy {
            environment: CallableEnvironment::Capturing(_),
            ..
        })
    ) {
        erased.update(|kind| {
            if let Some(func) = kind.as_func_mut() {
                func.environment = CallableEnvironment::Default;
            }
        });
    }
    erased
}

/// The trait an existential `Some[Trait]` annotation argument names, in its
/// canonical spelling.
fn existential_trait(arg: &ParamArg) -> Option<&str> {
    match arg {
        ParamArg::Type(Type::Named(trait_name, trait_args)) if trait_args.is_empty() => {
            Some(trait_name)
        }
        ParamArg::Value(Expr {
            kind: ExprKind::Identifier(trait_name),
            ..
        }) => Some(trait_name),
        _ => None,
    }
}

/// The binder of the signature a `Scalar`/`SIMD` annotation argument names,
/// bare (`n`) or through `Self` (`Self.n`), which name one parameter.
fn simd_binder_slot<'a>(
    arg: &'a ParamArg,
    type_bounds: &HashMap<String, BinderSpelling>,
) -> Option<&'a str> {
    let name = match arg {
        ParamArg::Value(Expr {
            kind: ExprKind::Identifier(name),
            ..
        })
        | ParamArg::Type(Type::SelfParam(name)) => name,
        ParamArg::Value(Expr {
            kind: ExprKind::Member { object, field },
            ..
        }) if matches!(&object.kind, ExprKind::Identifier(base) if base == "Self") => field,
        _ => return None,
    };
    type_bounds.contains_key(name).then_some(name.as_str())
}

/// A declared compile-time parameter as its annotation spells in an overload
/// key: its trait bounds, and a callable bound (`F: def(Int) -> Int`) as the
/// checker's `TyKind::Param` spells its `callable_bound`.
struct BinderSpelling {
    bounds: Vec<String>,
    callable: Option<String>,
}

impl BinderSpelling {
    fn of(param: &TypeParam) -> Self {
        Self {
            bounds: param
                .bounds
                .iter()
                .filter(|bound| bound.as_str() != "<function type>")
                .cloned()
                .collect(),
            callable: param
                .callable_bound
                .as_ref()
                .filter(|callable| matches!(callable, Type::Func { .. }))
                .map(func_annotation_raw),
        }
    }
}

fn binder_spellings(type_params: &[TypeParam]) -> HashMap<String, BinderSpelling> {
    type_params
        .iter()
        .map(|param| (param.name.clone(), BinderSpelling::of(param)))
        .collect()
}

fn parameter_raw(name: &str, type_bounds: &HashMap<String, BinderSpelling>) -> String {
    let mut result = encode_identifier(name);
    if let Some(binder) = type_bounds.get(name) {
        for bound in &binder.bounds {
            result.push('$');
            result.push_str(&encode_identifier(bound));
        }
        if let Some(callable) = &binder.callable {
            result.push_str("$Callable$");
            result.push_str(callable);
        }
    }
    result
}

/// Whether an annotation argument spells an origin: the `_`/`...` placeholder,
/// an `origin_of(...)`/`ImmOrigin(...)`/`MutOrigin(...)` value, a builtin
/// origin name, or a parameter the signature binds by `Origin`/`OriginSet`.
fn syntactic_origin_argument(
    arg: &ParamArg,
    type_bounds: &HashMap<String, BinderSpelling>,
) -> bool {
    let is_origin_binder = |name: &str| {
        matches!(
            type_bounds.get(name).map(|binder| binder.bounds.as_slice()),
            Some([only]) if only == "Origin" || only == "OriginSet"
        )
    };
    let value = match arg {
        ParamArg::Value(value) => value,
        // A struct's own origin binder, spelled `Self.origin`.
        ParamArg::Type(Type::SelfParam(name)) => return is_origin_binder(name),
        _ => return false,
    };
    match &value.kind {
        ExprKind::Identifier(name) => {
            matches!(
                name.as_str(),
                "_" | "..."
                    | "ImmStaticOrigin"
                    | "ImmUntrackedOrigin"
                    | "ImmUnsafeAnyOrigin"
                    | "MutUnsafeAnyOrigin"
                    | "MutUntrackedOrigin"
            ) || is_origin_binder(name)
        }
        ExprKind::Call { name, .. } => {
            matches!(name.as_str(), "origin_of" | "ImmOrigin" | "MutOrigin")
        }
        _ => false,
    }
}

/// The mangled spelling of a compile-time value argument in an annotation
/// (`FixedBuffer[8]` → `8`, `_ZeroStartingRange[DType.int]` → `DType.int`,
/// as a checked `CtValue` displays). A non-literal expression degrades to a
/// stable placeholder — good enough because the name only needs to be
/// deterministic.
fn value_expr_raw(expr: &Expr, comptimes: &dyn ModuleNames) -> String {
    if let Some(value) = eval_comptime_int(expr, comptimes) {
        return value.to_string();
    }
    if let Some(dtype) = expr_dtype(expr) {
        return format!("DType.{}", dtype.name());
    }
    match &expr.kind {
        ExprKind::Bool(b) => b.to_string(),
        ExprKind::Str(s) => encode_identifier(s),
        ExprKind::Identifier(name) => encode_identifier(name),
        _ => "expr".to_string(),
    }
}

fn sorted_map<V: serde::Serialize, S: serde::Serializer>(
    map: &HashMap<String, V>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serde::Serialize::serialize(
        &map.iter().collect::<std::collections::BTreeMap<_, _>>(),
        serializer,
    )
}

fn sorted_arities<S: serde::Serializer>(
    map: &HashMap<String, HashSet<usize>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serde::Serialize::serialize(
        &map.iter()
            .map(|(name, arities)| {
                (
                    name,
                    arities.iter().collect::<std::collections::BTreeSet<_>>(),
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>(),
        serializer,
    )
}

fn sorted_names<S: serde::Serializer>(
    names: &HashSet<String>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serde::Serialize::serialize(
        &names.iter().collect::<std::collections::BTreeSet<_>>(),
        serializer,
    )
}

fn keep_overloaded(counts: HashMap<String, Vec<usize>>) -> HashMap<String, HashSet<usize>> {
    counts
        .into_iter()
        .filter_map(|(name, arities)| {
            if arities.len() > 1 {
                Some((name, arities.into_iter().collect()))
            } else {
                None
            }
        })
        .collect()
}

fn signature_from_ast<P: AnnotatedParam>(
    params: &[P],
    type_params: &[TypeParam],
    comptimes: &dyn ModuleNames,
    self_spelling: Option<&str>,
) -> SignatureKey {
    let type_bounds = binder_spellings(type_params);
    let key = |param: &P| {
        TypeKey(sanitize(&ast_raw(
            denoted_annotation(params, param.annotation()),
            comptimes,
            &type_bounds,
            self_spelling,
        )))
    };
    SignatureKey {
        types: params
            .iter()
            .filter(|param| param.kind() != ParamKind::KwVariadic)
            .map(key)
            .collect(),
        kw_variadic: params
            .iter()
            .find(|param| param.kind() == ParamKind::KwVariadic)
            .map(key),
        keywords: Vec::new(),
        owned: Vec::new(),
        parameters: Vec::new(),
    }
}

/// The annotation a parameter's type denotes: `type_of(x)` over another
/// parameter `x` of the same list is `x`'s own annotation, as the call side
/// keys the type `x` resolved to rather than the `type_of` spelling.
fn denoted_annotation<'a, P: AnnotatedParam>(params: &'a [P], annotation: &'a Type) -> &'a Type {
    let mut annotation = annotation;
    for _ in params {
        let Type::TypeOf(operand) = annotation else {
            break;
        };
        let ExprKind::Identifier(name) = &operand.kind else {
            break;
        };
        let Some(named) = params.iter().find(|param| param.name() == name) else {
            break;
        };
        annotation = named.annotation();
    }
    annotation
}

/// The `ty_raw` mangling of a struct's own instance type, reconstructed from its
/// name and type parameters — the declaration-side spelling of `Self`. Mirrors
/// `ty_raw(TyKind::Struct(name, decls.map(param_as_arg)))`: the struct name followed
/// by each type parameter (`Pair`, `List$T$AnyType`). Origin parameters and
/// the infer-only `Bool` binders of their mutability are erased exactly as the
/// checker erases them from the struct's declarations, so a view struct such
/// as `StringSpan[mut: Bool, //, origin: Origin[mut=mut]]` spells `StringSpan`
/// and its bare-annotation parameters (`sep: StringSpan`) canonicalize to
/// `Self` on both sides. Returns `None` when the struct carries other value
/// or callable parameters (whose argument mangling this cheap reconstruction
/// does not reproduce), leaving those rare `Self`-typed overloads at their
/// prior behavior rather than risking a mismatched key.
fn self_struct_spelling(type_name: &str, type_params: &[TypeParam]) -> Option<String> {
    // The nominal String struct already mangles its own type as the stable
    // `String` spelling on both sides (see `ty_raw`/`ast_raw`), so its overload
    // keys are not asymmetric and must not be rewritten to `Self` — every
    // String key would churn for no gain.
    if is_stdlib_string_struct(type_name) {
        return None;
    }
    let retained: Vec<&TypeParam> = type_params
        .iter()
        .filter(|param| {
            !matches!(param.bounds.as_slice(), [only] if only == "Origin" || only == "OriginSet")
                && !param.is_origin_mutability_binder(type_params)
        })
        .collect();
    let all_type_params = retained.iter().all(|param| {
        param.value_type.is_none()
            && param.callable_bound.is_none()
            && param.origin_mutability.is_none()
            && !param.name.starts_with('*')
    });
    if !all_type_params {
        return None;
    }
    let type_bounds: HashMap<String, BinderSpelling> = retained
        .iter()
        .map(|param| (param.name.clone(), BinderSpelling::of(param)))
        .collect();
    let mut spelling = encode_identifier(type_name);
    for param in retained {
        spelling.push('$');
        spelling.push_str(&parameter_raw(&param.name, &type_bounds));
    }
    Some(spelling)
}

/// The names of the keyword-only parameters, part of overload identity.
/// Parameters after a `*` marker or a variadic collector are keyword-only;
/// the collectors themselves are not named parameters.
/// The names a call may bind in brackets: every compile-time parameter after
/// the `//` infer-only marker, a pack's without its `*`.
fn explicit_parameter_names(type_params: &[TypeParam]) -> impl Iterator<Item = &str> {
    type_params
        .iter()
        .filter(|param| !param.infer_only)
        .map(|param| param.name.trim_start_matches('*'))
}

fn keyword_only_names<P: DeclaredParam>(params: &[P], keyword_only: Option<usize>) -> Vec<String> {
    let variadic = params
        .iter()
        .position(|param| param.kind() == ParamKind::Variadic);
    let boundary = match (keyword_only, variadic) {
        (Some(marker), Some(variadic)) => marker.min(variadic + 1),
        (Some(marker), None) => marker,
        (None, Some(variadic)) => variadic + 1,
        (None, None) => return Vec::new(),
    };
    params
        .iter()
        .skip(boundary)
        .filter(|param| !matches!(param.kind(), ParamKind::Variadic | ParamKind::KwVariadic))
        .map(|param| param.name().to_string())
        .collect()
}

fn is_mojo_move_constructor<P: AnnotatedParam>(m: &LifecycleShape<'_, P>) -> bool {
    matches!(m.name, "__init__" | "__moveinit__")
        && m.has_self
        && matches!(m.self_convention, Some(ArgConvention::Out))
        && m.positional_only.is_none()
        && m.keyword_only == Some(0)
        && m.params.len() == 1
        && m.params[0].name() == "move"
        && !m.params[0].has_default()
        && m.params[0].kind() == ParamKind::Regular
        // Current Mojo requires the consuming `deinit move: Self` convention;
        // the bare `move: Self` shape is rejected by the checker.
        && matches!(m.params[0].convention(), Some(ArgConvention::Deinit))
        && matches!(m.params[0].annotation(), Type::SelfType)
        && !m.returns
}

fn is_mojo_copy_constructor<P: AnnotatedParam>(m: &LifecycleShape<'_, P>) -> bool {
    matches!(m.name, "__init__" | "__copyinit__")
        && m.has_self
        && matches!(m.self_convention, Some(ArgConvention::Out))
        && m.positional_only.is_none()
        && m.keyword_only == Some(0)
        && m.params.len() == 1
        && m.params[0].name() == "copy"
        && !m.params[0].has_default()
        && m.params[0].kind() == ParamKind::Regular
        && m.params[0].convention().is_none()
        && matches!(m.params[0].annotation(), Type::SelfType)
        && !m.returns
}

/// A signature-qualified abstract `__call__` symbol for an indirect callable
/// contract.
///
/// The VM retargets this symbol to a nominal receiver's runtime type; ordinary
/// function and closure values ignore it. Keeping the signature here avoids
/// falling back to arity when a callable struct overloads `__call__` on
/// parameter type.
pub fn callable_contract_target(ty: &Ty) -> Option<String> {
    let contract = mojito_types::types::callable_contract_ty(ty)?;
    let FuncTy {
        params,
        variadic,
        kw_variadic,
        ..
    } = contract.as_func()?;
    let signature_types = params.iter().chain(variadic.iter());
    let signature = SignatureKey::from_tys(signature_types).with_kw_variadic(kw_variadic.as_ref());
    Some(method_symbol(TRAIT_DISPATCH, "__call__", &signature))
}

/// The specialization-key spelling of a type: a caller place a clone binder
/// stands for spells as the first clone binder, at every depth.
pub fn canonical_specialization_type(ty: &Ty) -> Ty {
    let respell = |inner: &Ty| canonical_specialization_type(inner);
    match ty.kind() {
        TyKind::Struct(name, args) => ty.reusing(TyKind::Struct(
            name.clone(),
            mojito_types::types::map_tyargs(args, respell),
        )),
        TyKind::ComptimeList(element) => TyKind::ComptimeList(respell(element)).into(),
        TyKind::ParamList(element) => TyKind::ParamList(respell(element)).into(),
        TyKind::Tuple(elements) => TyKind::Tuple(elements.iter().map(respell).collect()).into(),
        TyKind::RuntimePack(elements) => {
            TyKind::RuntimePack(elements.iter().map(respell).collect()).into()
        }
        TyKind::VariadicPack(element) => TyKind::VariadicPack(respell(element)).into(),
        TyKind::Variant(alternatives) => {
            TyKind::Variant(alternatives.iter().map(respell).collect()).into()
        }
        // A caller place a clone binder stands for spells as the first
        // clone binder (`Elab::clone_binding`): one clone serves every place
        // of the shape, as an erased struct origin tail does.
        TyKind::Pointer { element, origin } => TyKind::Pointer {
            element: respell(element),
            origin: origin.clone_bindable_place().map_or_else(
                || origin.clone(),
                |projection| {
                    projection.binder_origin(mojito_types::origin::OriginParamId(u32::MAX))
                },
            ),
        }
        .into(),
        TyKind::KgenPointer(element) => TyKind::KgenPointer(respell(element)).into(),
        TyKind::LitRef { element, origin } => TyKind::LitRef {
            element: respell(element),
            origin: origin.clone_bindable_place().map_or_else(
                || origin.clone(),
                |projection| {
                    projection.binder_origin(mojito_types::origin::OriginParamId(u32::MAX))
                },
            ),
        }
        .into(),
        TyKind::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = respell(&reference.referent);
            TyKind::Ref(reference).into()
        }
        _ => ty.clone(),
    }
}

/// Current Mojo's unqualified spelling of a checked type, with minted value
/// specializations spelled through their baked arguments.
///
/// A minted specialization spells `AHasher[[0, 0, 0, 0] : SIMD[DType.uint64,
/// 4]]` for the clone `mangle` named, at every nesting level
/// (`Dict[String, SIMD[DType.int, 1], AHasher[...]]`); every other type
/// spells as `unqualified_type_name`.
pub fn unqualified_instance_name(ty: &Ty) -> String {
    use mojito_types::types::{unqualified_type_name, unqualified_value_argument};

    let spell_value = |value: &CtValue| match value {
        CtValue::Type(ty) => unqualified_instance_name(ty),
        other => unqualified_value_argument(other),
    };
    match ty.kind() {
        TyKind::Struct(name, arguments) => {
            if let Some(elements) = mojito_types::types::tuple_elements(ty) {
                let elements = elements
                    .into_iter()
                    .map(unqualified_instance_name)
                    .collect::<Vec<_>>()
                    .join(", ");
                return format!("Tuple[{elements}]");
            }
            // An elaborated instance (`AHasher$mono$…`) keeps its arguments
            // and spells its template's name.
            let name = mojito_types::types::struct_template(name);
            let (base, baked) = match demangle_specialization(name) {
                Some((template, values)) => (
                    unqualified_type_name(&Ty::new(TyKind::Struct(
                        template.to_string(),
                        Vec::new(),
                    ))),
                    values,
                ),
                None => (
                    unqualified_type_name(&Ty::new(TyKind::Struct(name.to_string(), Vec::new()))),
                    Vec::new(),
                ),
            };
            let mut spelled: Vec<String> = baked.iter().map(spell_value).collect();
            spelled.extend(arguments.iter().map(|argument| {
                match argument {
                    TyArg::Ty(ty) => unqualified_instance_name(ty),
                    // A bound type pack (`TypeNames[Int, String]`) spells its
                    // element types.
                    TyArg::Val(CtValue::Tuple(values))
                        if values.iter().all(|value| matches!(value, CtValue::Type(_))) =>
                    {
                        values
                            .iter()
                            .map(spell_value)
                            .collect::<Vec<_>>()
                            .join(", ")
                    }
                    TyArg::Val(value) => spell_value(value),
                    other @ TyArg::Origin(_) => other.to_string(),
                }
            }));
            if spelled.is_empty() {
                base
            } else {
                format!("{base}[{}]", spelled.join(", "))
            }
        }
        TyKind::Variant(alternatives) => format!(
            "Variant[{}]",
            alternatives
                .iter()
                .map(unqualified_instance_name)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => unqualified_type_name(ty),
    }
}

/// Upstream's `#kgen.get_type_name` and `#kgen.get_base_type_name`: the
/// spelling `reflect[T].name()` and `reflect[T].base_name()` answer for a
/// closed type.
///
/// A name is qualified by its declaring module (`probe.Point`,
/// `std.collections.optional.Optional[…]`), which `module_of` answers for a
/// struct's linked name, except that `SIMD`, `Bool`, `String`, and `List`
/// spell bare unless `qualified_builtins`. Each argument spells inside the
/// brackets: a type in the same form, a scalar value `value : Type` (`3 :
/// SIMD[DType.int, 1]`), a `Bool` or `DType` bare, an origin `{}`. A
/// base name is the base type's declared name alone (`List` for
/// `List[Int]`).
pub fn reflected_type_name(
    ty: &Ty,
    form: mojito_types::param_expr::TypeNameForm,
    module_of: &dyn Fn(&str) -> Option<String>,
) -> String {
    match form {
        mojito_types::param_expr::TypeNameForm::Name { qualified_builtins } => TypeNameSpeller {
            qualified_builtins,
            module_of,
        }
        .name(ty),
        mojito_types::param_expr::TypeNameForm::Base => reflected_base_name(ty),
    }
}

/// The inverse of [`mangle`] for a self-delimiting value suffix.
///
/// The template name and the baked values, or `None` when the symbol carries
/// no specialization suffix or a type or reflected key (a type spelling
/// cannot be rebuilt into a `Ty` here). [`specialization_template`] answers for
/// those too, when the name alone is what is wanted.
///
/// A module-qualified template (`__module$$ahash$AHasher$v…`) keeps its
/// qualification.
pub fn demangle_specialization(symbol: &str) -> Option<(&str, Vec<CtValue>)> {
    let (template, values) = split_specialization(symbol)?;
    values
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .map(|values| (template, values))
}

/// The template `symbol` specializes, whatever its key bakes.
///
/// [`demangle_specialization`] answers only when it can rebuild every baked
/// value, so a type-keyed clone (`List.__hash__$y3:Int`) is invisible to it.
/// A binder belongs to the template rather than to one of its clones, and
/// that identity needs the name alone.
pub fn specialization_template(symbol: &str) -> Option<&str> {
    split_specialization(symbol).map(|(template, _)| template)
}

/// How many values `symbol` bakes, whatever they are; `None` when it carries
/// no specialization suffix.
pub fn specialization_arity(symbol: &str) -> Option<usize> {
    split_specialization(symbol).map(|(_, values)| values.len())
}

/// The template name and one entry per baked value, `None` where a value is
/// well formed but not rebuildable here.
fn split_specialization(symbol: &str) -> Option<(&str, Vec<Option<CtValue>>)> {
    let mut cursor = 0;
    while let Some(offset) = symbol[cursor..].find('$') {
        let split = cursor + offset;
        let suffix = &symbol[split..];
        if let Some(values) = decode_specialization_suffix(suffix)
            && !values.is_empty()
            && split > 0
        {
            return Some((&symbol[..split], values));
        }
        cursor = split + 1;
    }
    None
}

fn decode_specialization_suffix(suffix: &str) -> Option<Vec<Option<CtValue>>> {
    let bytes = suffix.as_bytes();
    let mut position = 0;
    let mut values = Vec::new();
    while position < bytes.len() {
        if bytes[position] != b'$' {
            return None;
        }
        position += 1;
        let (value, next) = decode_specialization_value(suffix, position)?;
        values.push(value);
        position = next;
    }
    Some(values)
}

/// Decode one encoded value at `position`, returning it and the position past
/// its terminator.
///
/// The value is `None` where the encoding is well formed but names something
/// this crate cannot rebuild: a type or reflected key renders a `Ty`, and no
/// parser here reads one back. A container holding such a
/// value is `None` too, and still reports where it ends — which is all
/// [`specialization_template`] needs.
fn decode_specialization_value(text: &str, position: usize) -> Option<(Option<CtValue>, usize)> {
    let bytes = text.as_bytes();
    let code = *bytes.get(position)?;
    let body = position + 1;
    let until = |end: u8| -> Option<(&str, usize)> {
        let length = text[body..].find(end as char)?;
        Some((&text[body..body + length], body + length + 1))
    };
    Some(match code {
        b'i' => {
            let (digits, next) = until(b';')?;
            (Some(CtValue::Int(digits.parse().ok()?)), next)
        }
        b'u' => {
            let (digits, next) = until(b';')?;
            (Some(CtValue::UInt(digits.parse().ok()?)), next)
        }
        b'x' => {
            let (digits, next) = until(b';')?;
            (Some(CtValue::Index(digits.parse().ok()?)), next)
        }
        b'b' => {
            let (digit, next) = until(b';')?;
            (Some(CtValue::Bool(digit == "1")), next)
        }
        b'n' => (Some(CtValue::None), until(b';')?.1),
        b'f' => {
            let (hex, next) = until(b';')?;
            (
                Some(CtValue::Float(u64::from_str_radix(hex, 16).ok()?)),
                next,
            )
        }
        b'd' => {
            let (name, next) = until(b';')?;
            (
                Some(CtValue::Dtype(mojito_ast::ast::Dtype::from_name(name)?)),
                next,
            )
        }
        // The length-prefixed codes, all `{code}{length}:{spelling}`. A
        // string or a literal spelling reads back exactly; a type or a
        // reflected type renders a `Ty`, so the walk reports where the value
        // ends and leaves rebuilding to a phase that can parse one.
        b'y' | b'r' | b'I' | b'F' | b's' | b'k' | b'a' | b'K' | b'A' => {
            let (digits, after) = until(b':')?;
            let length: usize = digits.parse().ok()?;
            let end = after.checked_add(length)?;
            let spelling = text.get(after..end)?;
            let value = match code {
                b's' => Some(CtValue::Str(spelling.to_string())),
                b'k' => Some(CtValue::KgenString(spelling.to_string())),
                b'a' => Some(CtValue::MlirAttr(spelling.to_string())),
                b'K' => spelling
                    .strip_prefix("#kgen.target<")
                    .and_then(|body| body.strip_suffix('>'))
                    .and_then(mojito_types::ct::KgenTarget::parse)
                    .map(|target| CtValue::KgenTarget(Box::new(target))),
                b'I' => IntLiteral::parse_radix(spelling, 10).map(CtValue::IntLiteral),
                b'F' => FloatLiteral::parse_exact(spelling).map(CtValue::FloatLiteral),
                _ => None,
            };
            (value, end)
        }
        b'v' => {
            let (dtype, after_dtype) = until(b':')?;
            let dtype = mojito_ast::ast::Dtype::from_name(dtype)?;
            let width_end = text[after_dtype..].find(';')?;
            let width: usize = text[after_dtype..after_dtype + width_end].parse().ok()?;
            let mut cursor = after_dtype + width_end + 1;
            if bytes.get(cursor) != Some(&b'[') {
                return None;
            }
            cursor += 1;
            let mut lanes = Vec::with_capacity(width);
            for _ in 0..width {
                let (lane, next) = decode_specialization_value(text, cursor)?;
                lanes.push(match lane {
                    Some(CtValue::Int(value)) => CtLane::Int(i128::from(value)),
                    Some(CtValue::Float(bits)) => CtLane::Float(bits),
                    Some(CtValue::Bool(value)) => CtLane::Bool(value),
                    _ => return None,
                });
                cursor = next;
            }
            if bytes.get(cursor) != Some(&b']') {
                return None;
            }
            (Some(CtValue::Simd { dtype, lanes }), cursor + 1)
        }
        b'S' => {
            let (length, after_length) = until(b':')?;
            let length: usize = length.parse().ok()?;
            let name = text.get(after_length..after_length + length)?;
            let mut cursor = after_length + length;
            if bytes.get(cursor) != Some(&b'{') {
                return None;
            }
            cursor += 1;
            let mut fields = Vec::new();
            while bytes.get(cursor) != Some(&b'}') {
                let (value, next) = decode_specialization_value(text, cursor)?;
                fields.push(value.map(|value| (String::new(), value)));
                cursor = next;
            }
            (
                fields
                    .into_iter()
                    .collect::<Option<Vec<_>>>()
                    .map(|fields| CtValue::Struct {
                        name: name.to_string(),
                        fields,
                    }),
                cursor + 1,
            )
        }
        b't' | b'l' => {
            let (count, after_count) = until(b'[')?;
            let count: usize = count.parse().ok()?;
            let mut cursor = after_count;
            let mut items = Vec::with_capacity(count);
            for _ in 0..count {
                let (value, next) = decode_specialization_value(text, cursor)?;
                items.push(value);
                cursor = next;
            }
            if bytes.get(cursor) != Some(&b']') {
                return None;
            }
            let value = items.into_iter().collect::<Option<Vec<_>>>().map(|items| {
                if code == b't' {
                    CtValue::Tuple(items)
                } else {
                    CtValue::List(items)
                }
            });
            (value, cursor + 1)
        }
        b'm' | b'h' => {
            let (count, after_count) = until(b'[')?;
            let count: usize = count.parse().ok()?;
            let mut cursor = after_count;
            let value = if code == b'm' {
                let mut entries = Vec::with_capacity(count);
                for _ in 0..count {
                    let (key, next) = decode_specialization_value(text, cursor)?;
                    let (value, next) = decode_specialization_value(text, next)?;
                    entries.push(key.zip(value));
                    cursor = next;
                }
                entries
                    .into_iter()
                    .collect::<Option<Vec<_>>>()
                    .map(|entries| CtValue::Dict {
                        spelling: None,
                        entries,
                    })
            } else {
                let mut elements = Vec::with_capacity(count);
                for _ in 0..count {
                    let (value, next) = decode_specialization_value(text, cursor)?;
                    elements.push(value);
                    cursor = next;
                }
                elements
                    .into_iter()
                    .collect::<Option<Vec<_>>>()
                    .map(|elements| CtValue::Set {
                        spelling: None,
                        elements,
                    })
            };
            if bytes.get(cursor) != Some(&b']') {
                return None;
            }
            (value, cursor + 1)
        }
        _ => return None,
    })
}

/// A specialization value that is not a constant: a residual parameter
/// expression or a deferred slot somewhere in the key's input. No symbolic
/// spelling enters a key, so the request is refused instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonConstantSpecialization {
    pub template: String,
    pub value: String,
}

impl std::fmt::Display for NonConstantSpecialization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cannot specialize '{}': '{}' is not a compile-time constant",
            self.template, self.value
        )
    }
}

impl std::error::Error for NonConstantSpecialization {}

/// One part of a specialization key.
///
/// A part is a closed compile-time value, or the marker an erased origin
/// argument contributes to an owner key. The marker is key vocabulary, not a
/// parameter declaration.
#[derive(Debug, Clone, Copy)]
pub enum SpecializationKeyPart<'a> {
    Value(&'a CtValue),
    ErasedOrigin,
}

/// The specialized name for `orig` at value arguments `vals` — e.g. `f$0`, `f$1`.
///
/// `$` cannot appear in a source identifier, so a specialization never collides
/// with a user-written name. Every value must be recursively closed.
pub fn mangle(orig: &str, vals: &[CtValue]) -> Result<String, NonConstantSpecialization> {
    let parts: Vec<_> = vals.iter().map(SpecializationKeyPart::Value).collect();
    mangle_parts(orig, &parts)
}

/// [`mangle`] over explicit key parts. The complete input is validated before
/// any of the key is written.
pub fn mangle_parts(
    orig: &str,
    parts: &[SpecializationKeyPart<'_>],
) -> Result<String, NonConstantSpecialization> {
    if let Some(open) = parts.iter().find_map(|part| match part {
        SpecializationKeyPart::Value(value) if !specialization_value_is_closed(value) => {
            Some(*value)
        }
        _ => None,
    }) {
        return Err(NonConstantSpecialization {
            template: orig.to_string(),
            value: open.to_string(),
        });
    }
    let mut s = orig.to_string();
    for part in parts {
        s.push('$');
        match part {
            SpecializationKeyPart::Value(value) => encode_specialization_value(value, &mut s),
            SpecializationKeyPart::ErasedOrigin => s.push_str("p6:origin"),
        }
    }
    Ok(s)
}

/// Whether no residual parameter expression or deferred slot occurs in `value`.
///
/// The search covers an aggregate child, a struct field, and a value argument
/// of a type payload. A type parameter inside a type payload keeps its
/// spelling; a callable type is closed relative to its own binders.
pub fn specialization_value_is_closed(value: &CtValue) -> bool {
    match value {
        CtValue::Expr(_) | CtValue::Deferred(_) | CtValue::Marker(_) => false,
        CtValue::Tuple(values)
        | CtValue::List(values)
        | CtValue::Set {
            elements: values, ..
        } => values.iter().all(specialization_value_is_closed),
        CtValue::Dict { entries, .. } => entries.iter().all(|(key, value)| {
            specialization_value_is_closed(key) && specialization_value_is_closed(value)
        }),
        CtValue::Struct { fields, .. } => fields
            .iter()
            .all(|(_, value)| specialization_value_is_closed(value)),
        CtValue::Pointer { memory, .. } => memory
            .iter()
            .flatten()
            .flatten()
            .all(specialization_value_is_closed),
        CtValue::StoreToMem(referent) => specialization_value_is_closed(referent),
        CtValue::PopArray { element, elements } => {
            specialization_value_is_closed(&CtValue::Type(element.clone()))
                && elements.iter().all(specialization_value_is_closed)
        }
        CtValue::Variant { payload, .. } => payload
            .as_deref()
            .is_none_or(specialization_value_is_closed),
        CtValue::Type(ty) => {
            let mut open = std::collections::HashSet::new();
            mojito_types::types::referenced_parameters(ty, &mut open);
            open.is_empty() && !mojito_types::types::mentions_deferred_value(ty)
        }
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
        | CtValue::Simd { .. }
        | CtValue::Trait(_)
        | CtValue::KgenString(_)
        | CtValue::MlirAttr(_)
        | CtValue::KgenTarget(_)
        | CtValue::Generator(_)
        | CtValue::RemovedArg => true,
    }
}

#[allow(
    clippy::format_push_string,
    reason = "TODO: write! into the buffer instead"
)]
fn encode_specialization_value(value: &CtValue, out: &mut String) {
    match value {
        CtValue::Pointer { memory, offset } => {
            use std::fmt::Write;
            let _ = write!(out, "p{offset}:");
            if let Some(slots) = memory {
                let _ = write!(out, "{}[", slots.len());
                for slot in slots {
                    if let Some(value) = slot {
                        encode_specialization_value(value, out);
                    } else {
                        out.push('_');
                    }
                }
                out.push(']');
            } else {
                out.push('d');
            }
        }
        CtValue::StoreToMem(referent) => {
            out.push('M');
            encode_specialization_value(referent, out);
        }
        // The case index and its payload; the alternatives are the type's.
        CtValue::Variant { index, payload, .. } => {
            out.push_str(&format!("e{index}:"));
            if let Some(payload) = payload {
                encode_specialization_value(payload, out);
            }
        }
        CtValue::Int(value) => out.push_str(&format!("i{value};")),
        CtValue::UInt(value) => out.push_str(&format!("u{value};")),
        CtValue::Index(value) => out.push_str(&format!("x{value};")),
        CtValue::Dtype(dtype) => out.push_str(&format!("d{};", dtype.name())),
        CtValue::Simd { dtype, lanes } => {
            out.push_str(&format!("v{}:{};[", dtype.name(), lanes.len()));
            for lane in lanes {
                match lane {
                    CtLane::Int(value) => out.push_str(&format!("i{value};")),
                    CtLane::Float(bits) => out.push_str(&format!("f{bits:016x};")),
                    CtLane::Bool(value) => out.push_str(if *value { "b1;" } else { "b0;" }),
                }
            }
            out.push(']');
        }
        CtValue::Struct { name, fields } => {
            out.push_str(&format!("S{}:{name}{{", name.len()));
            for (_, value) in fields {
                encode_specialization_value(value, out);
            }
            out.push('}');
        }
        CtValue::Float(bits) => out.push_str(&format!("f{bits:016x};")),
        CtValue::IntLiteral(value) => {
            let rendered = value.to_string();
            out.push_str(&format!("I{}:{rendered}", rendered.len()));
        }
        CtValue::FloatLiteral(value) => {
            let rendered = value.to_string();
            out.push_str(&format!("F{}:{rendered}", rendered.len()));
        }
        CtValue::Bool(value) => out.push_str(if *value { "b1;" } else { "b0;" }),
        CtValue::None => out.push_str("n;"),
        CtValue::Str(value) => out.push_str(&format!("s{}:{value}", value.len())),
        CtValue::KgenString(value) => out.push_str(&format!("k{}:{value}", value.len())),
        CtValue::MlirAttr(value) => out.push_str(&format!("a{}:{value}", value.len())),
        CtValue::KgenTarget(target) => {
            let rendered = target.to_string();
            out.push_str(&format!("K{}:{rendered}", rendered.len()));
        }
        // The attribute and its element type, `#pop.array<…> : T`.
        CtValue::PopArray { element, .. } => {
            let rendered = format!("{value} : {}", canonical_specialization_type(element));
            out.push_str(&format!("A{}:{rendered}", rendered.len()));
        }
        CtValue::Tuple(values) => {
            out.push_str(&format!("t{}[", values.len()));
            for value in values {
                encode_specialization_value(value, out);
            }
            out.push(']');
        }
        CtValue::List(values) => {
            out.push_str(&format!("l{}[", values.len()));
            for value in values {
                encode_specialization_value(value, out);
            }
            out.push(']');
        }
        // A display's entries in order; the explicit spelling is not part of
        // the value identity.
        CtValue::Dict { entries, .. } => {
            out.push_str(&format!("m{}[", entries.len()));
            for (key, value) in entries {
                encode_specialization_value(key, out);
                encode_specialization_value(value, out);
            }
            out.push(']');
        }
        CtValue::Set { elements, .. } => {
            out.push_str(&format!("h{}[", elements.len()));
            for value in elements {
                encode_specialization_value(value, out);
            }
            out.push(']');
        }
        CtValue::Type(ty) => {
            let rendered = canonical_specialization_type(ty).to_string();
            out.push_str(&format!("y{}:{rendered}", rendered.len()));
        }
        CtValue::Trait(traits) => {
            let rendered = traits.join("&");
            out.push_str(&format!("T{}:{rendered}", rendered.len()));
        }
        CtValue::RemovedArg => out.push('R'),
        CtValue::Generator(_) => {
            let rendered = value.to_string();
            out.push_str(&format!("g{}:{rendered}", rendered.len()));
        }
        // `mangle_parts` validated the whole key before encoding began.
        CtValue::Expr(_) | CtValue::Deferred(_) | CtValue::Marker(_) => {}
    }
}

/// What tells one overloaded method from another: its name and the
/// signature facts its lowered symbol spells.
#[derive(Debug, Clone)]
struct OverloadDeclaration {
    name: String,
    type_params: Vec<TypeParam>,
    params: Vec<FnParam>,
    keyword_only: Option<usize>,
    has_self: bool,
    self_convention: Option<ArgConvention>,
}

impl OverloadDeclaration {
    fn of(m: &Method) -> Self {
        Self {
            name: m.name.clone(),
            type_params: m.type_params.clone(),
            params: m.params.clone(),
            keyword_only: m.keyword_only,
            has_self: m.has_self,
            self_convention: m.self_convention,
        }
    }

    fn declares(&self, m: &Method) -> bool {
        self.name == m.name
            && self.has_self == m.has_self
            && self.self_convention == m.self_convention
            && self.keyword_only == m.keyword_only
            && self.params == m.params
            && self.type_params == m.type_params
    }
}

/// Upstream's `#kgen.get_type_name` printer over one form
/// ([`reflected_type_name`]).
struct TypeNameSpeller<'a> {
    qualified_builtins: bool,
    module_of: &'a dyn Fn(&str) -> Option<String>,
}

impl TypeNameSpeller<'_> {
    fn name(&self, ty: &Ty) -> String {
        match ty.kind() {
            TyKind::Simd { dtype, width } => self.simd(dtype, width),
            TyKind::IntLiteral => self.simd(&"DType.int", &1),
            TyKind::FloatLiteral => self.simd(&"DType.float64", &1),
            TyKind::Bool => self.qualify("std.builtin.bool", "Bool"),
            TyKind::None => self.qualify("std.builtin.none", "NoneType"),
            TyKind::Dtype => self.qualify("std.builtin.dtype", "DType"),
            TyKind::Error => self.qualify("std.builtin.error", "Error"),
            TyKind::StringLiteral(_) => self.qualify("std.builtin.string_literal", "StringLiteral"),
            // A variadic type pack spells as upstream's printer shows one.
            TyKind::Variant(_) => self.applied("std.utils.variant", "Variant", &Self::pack()),
            TyKind::Struct(..) if mojito_types::types::tuple_elements(ty).is_some() => {
                let module = (self.module_of)(mojito_types::types::TUPLE_TYPE_NAME);
                self.applied(&module.unwrap_or_default(), "Tuple", &Self::pack())
            }
            TyKind::Struct(name, arguments) => {
                let template = mojito_types::types::struct_template(name);
                let (template, baked) =
                    demangle_specialization(template).unwrap_or((template, Vec::new()));
                let mut spelled: Vec<String> =
                    baked.iter().map(|value| self.value(value)).collect();
                spelled.extend(arguments.iter().map(|argument| match argument {
                    TyArg::Ty(ty) => self.name(ty),
                    TyArg::Val(value) => self.value(value),
                    TyArg::Origin(_) => "{}".to_string(),
                }));
                self.applied(
                    &(self.module_of)(template).unwrap_or_default(),
                    declared_struct_name(template),
                    &spelled,
                )
            }
            _ => ty.to_string(),
        }
    }

    /// A parameter value inside a type's brackets: a scalar `value : Type`,
    /// a `Bool` or `DType` bare, a struct value its fields (`{3 :
    /// SIMD[DType.int, 1], True}`), a one-field struct its field alone (`4`
    /// for a `SIMDLength`).
    fn value(&self, value: &CtValue) -> String {
        match value {
            CtValue::Bool(value) => (if *value { "True" } else { "False" }).to_string(),
            CtValue::Int(_) | CtValue::IntLiteral(_) => {
                format!("{value} : {}", self.simd(&"DType.int", &1))
            }
            CtValue::UInt(lane) => format!("{lane} : {}", self.simd(&"DType.uint", &1)),
            CtValue::Float(bits) => format!(
                "{:?} : {}",
                f64::from_bits(*bits),
                self.simd(&"DType.float64", &1)
            ),
            CtValue::FloatLiteral(_) => format!("{value} : {}", self.simd(&"DType.float64", &1)),
            CtValue::Simd { dtype, lanes } => {
                let shown = value.to_string();
                let elements = shown.split(" : ").next().unwrap_or(&shown);
                let elements = match lanes.as_slice() {
                    [_] => elements.trim_start_matches('[').trim_end_matches(']'),
                    _ => elements,
                };
                let lane = format!("DType.{}", dtype.name());
                format!("{elements} : {}", self.simd(&lane, &lanes.len()))
            }
            CtValue::Dtype(_) => self.dtype(value),
            CtValue::Type(ty) => self.name(ty),
            CtValue::Struct { fields, .. } => match fields.as_slice() {
                [(_, field)] => self.value(field),
                fields => format!(
                    "{{{}}}",
                    fields
                        .iter()
                        .map(|(_, field)| self.value(field))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            },
            other => other.to_string(),
        }
    }

    fn applied(&self, module: &str, base: &str, arguments: &[String]) -> String {
        let name = self.qualify(module, base);
        if arguments.is_empty() {
            name
        } else {
            format!("{name}[{}]", arguments.join(", "))
        }
    }

    /// `base` qualified by `module`, a library builtin bare unless the
    /// form qualifies builtins.
    fn qualify(&self, module: &str, base: &str) -> String {
        let builtin = matches!(base, "SIMD" | "Bool" | "String" | "List")
            && (module == "std" || module.starts_with("std."));
        if builtin && !self.qualified_builtins || module.is_empty() {
            base.to_string()
        } else {
            format!("{module}.{base}")
        }
    }

    fn simd(&self, dtype: &dyn std::fmt::Display, width: &dyn std::fmt::Display) -> String {
        format!(
            "{}[{}, {width}]",
            self.qualify("std.simd", "SIMD"),
            self.dtype(dtype)
        )
    }

    fn dtype(&self, dtype: &dyn std::fmt::Display) -> String {
        if self.qualified_builtins {
            format!("std.builtin.dtype.{dtype}")
        } else {
            dtype.to_string()
        }
    }

    fn pack() -> Vec<String> {
        vec!["<unprintable>".to_string(), "{}".to_string()]
    }
}

/// `reflect[T].base_name()`'s spelling of `ty`: its base type's declared
/// name, without module or parameters.
fn reflected_base_name(ty: &Ty) -> String {
    match ty.kind() {
        TyKind::Simd { .. } | TyKind::IntLiteral | TyKind::FloatLiteral => "SIMD",
        TyKind::Bool => "Bool",
        TyKind::None => "NoneType",
        TyKind::Dtype => "DType",
        TyKind::Error => "Error",
        TyKind::StringLiteral(_) => "StringLiteral",
        TyKind::Variant(_) => "Variant",
        TyKind::Struct(..) if mojito_types::types::tuple_elements(ty).is_some() => "Tuple",
        TyKind::Struct(name, _) => {
            let template = mojito_types::types::struct_template(name);
            declared_struct_name(specialization_template(template).unwrap_or(template))
        }
        _ => return ty.to_string(),
    }
    .to_string()
}

/// The name a struct's declaration spells, its linked name less any module
/// qualification (`AHasher` of `__module$std$hash$$ahash$AHasher`).
fn declared_struct_name(linked: &str) -> &str {
    linked
        .strip_prefix("__module$")
        .map_or(linked, |rest| rest.rsplit('$').next().unwrap_or(rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_method_symbol_splits_back_into_its_method_and_qualifier() {
        for (method, qualifier) in [
            ("pop", None),
            ("pop", Some("$ov$")),
            ("__init__", Some("$ov$dtype$DType.float32")),
        ] {
            let symbol = qualified_method_symbol("List", method, qualifier);
            assert_eq!(method_of_symbol("List", &symbol), Some((method, qualifier)));
        }
        assert_eq!(method_of_symbol("Lis", "List.pop"), None);
        assert_eq!(method_of_symbol("Dict", "List.pop$ov$"), None);
        assert_eq!(method_of_symbol("List", "List."), None);
    }

    #[test]
    fn a_function_the_check_lifts_is_owned_by_its_declaration() {
        let name = lifted_check_name("S.g$ov$Int", 0);
        assert_eq!(name, "$comptime$S.g$ov$Int$c0");
        assert_eq!(
            lifted_owner(&name),
            Some(LiftedOwner::Numbered("S.g$ov$Int"))
        );
        assert_eq!(lifted_owner("$comptime$$7"), None);
    }

    #[test]
    fn a_constructor_symbol_names_the_struct_it_builds() {
        assert_eq!(
            lifecycle_constructor("E2.__init__$ov$Int$Int"),
            Some(("E2", "__init__"))
        );
        assert_eq!(
            lifecycle_constructor("Box.__copyinit__"),
            Some(("Box", "__copyinit__"))
        );
        assert_eq!(lifecycle_constructor("Box.__init__x"), None);
        assert_eq!(lifecycle_constructor("Box.pop"), None);
    }

    #[test]
    fn a_value_binder_argument_keys_as_its_declaration_spells_it() {
        use mojito_ast::ast::{Expr, ExprKind};
        use mojito_common::token::Span;
        use mojito_types::param_expr::{ParamContext, ParamId};
        let binder = TypeParam {
            name: "k".to_string(),
            bounds: vec!["Int".to_string()],
            value_type: None,
            callable_bound: None,
            origin_mutability: None,
            infer_only: false,
            default: None,
            constraints: Vec::new(),
        };
        let annotation = Type::Named(
            "Array".to_string(),
            vec![
                ParamArg::Type(Type::Int),
                ParamArg::Value(Expr::new(
                    ExprKind::Identifier("k".to_string()),
                    Span::default(),
                )),
            ],
        );
        let reference =
            ParamContext::detached().decl_ref(ParamId::new("f", 0), "k", MetaTy::value(Ty::int()));
        let checked = Ty::new(TyKind::Struct(
            "Array".to_string(),
            vec![TyArg::Ty(Ty::int()), TyArg::Val(CtValue::Expr(reference))],
        ));
        let declared = TypeKey::from_ast_in_scope(&annotation, std::slice::from_ref(&binder));
        assert_eq!(declared.as_str(), "Array$Int$k$Int");
        assert_eq!(TypeKey::from_ty(&checked), declared);
    }

    #[test]
    fn a_function_lifted_from_an_initializer_is_no_initializer() {
        assert!(is_initializer_symbol("Box.__init__"));
        assert!(is_initializer_symbol("Box.__init__$ov$Int"));
        assert!(!is_initializer_symbol(&lifted_default_name(
            "Box.__init__",
            "scale"
        )));
        assert!(!is_initializer_symbol(&lifted_check_name(
            "Box.__init__",
            0
        )));
    }
}

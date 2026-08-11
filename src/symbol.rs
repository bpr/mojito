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

use std::collections::{HashMap, HashSet};

use crate::ast::{
    ArgConvention, Expr, ExprKind, FnParam, Method, ParamArg, ParamKind, Stmt, StmtKind, Type,
    TypeParam,
};
use crate::types::{Ty, TyArg};

/// The canonical mangled spelling of one parameter type. Only this module can
/// construct one, so every signature part obeys the same sanitization rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeKey(String);

impl TypeKey {
    /// Mangle a declared parameter annotation (the MIR/VM definition side).
    pub fn from_ast(ty: &Type) -> TypeKey {
        TypeKey(sanitize(&ast_raw(ty, &HashMap::new(), &HashMap::new())))
    }

    /// Mangle a checker-resolved type (the call-resolution side). Aligned with
    /// [`TypeKey::from_ast`]: a struct/parameter/`Self.T` type spells exactly as
    /// its annotation does, so checker-recorded callees name real MIR functions.
    pub fn from_ty(ty: &Ty) -> TypeKey {
        TypeKey(sanitize(&ty_raw(ty)))
    }
}

/// An overload signature as typed data: the ordered parameter type keys.
/// Format it only through [`function_symbol`]/[`method_symbol`] (or the
/// `lowered_*` helpers below).
#[derive(Debug, Clone, PartialEq, Eq)]
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
}

impl SignatureKey {
    /// The signature of a declared `def`/method parameter list.
    pub fn from_ast_params(params: &[FnParam]) -> SignatureKey {
        SignatureKey {
            types: params
                .iter()
                .filter(|parameter| parameter.kind != crate::ast::ParamKind::KwVariadic)
                .map(|parameter| TypeKey::from_ast(&parameter.ty))
                .collect(),
            kw_variadic: params
                .iter()
                .find(|parameter| parameter.kind == crate::ast::ParamKind::KwVariadic)
                .map(|parameter| TypeKey::from_ast(&parameter.ty)),
            keywords: Vec::new(),
        }
    }

    /// The signature of a checker-resolved parameter-type list.
    pub fn from_tys<'a>(tys: impl IntoIterator<Item = &'a Ty>) -> SignatureKey {
        SignatureKey {
            types: tys.into_iter().map(TypeKey::from_ty).collect(),
            kw_variadic: None,
            keywords: Vec::new(),
        }
    }

    /// Attach the homogeneous keyword-variadic collector to callable identity.
    pub fn with_kw_variadic(mut self, ty: Option<&Ty>) -> SignatureKey {
        self.kw_variadic = ty.map(TypeKey::from_ty);
        self
    }

    /// Attach the keyword-only parameter names to this signature's identity.
    pub fn with_keyword_names(mut self, names: Vec<String>) -> SignatureKey {
        self.keywords = names;
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
        suffix
    }

    fn with_receiver(&self, convention: Option<ArgConvention>) -> SignatureKey {
        let receiver = match convention {
            None | Some(ArgConvention::Read) => "SelfRead",
            Some(ArgConvention::Mut) => "SelfMut",
            Some(ArgConvention::Var) => "SelfVar",
            Some(ArgConvention::Out) => "SelfOut",
            Some(ArgConvention::Ref) => "SelfRef",
            Some(ArgConvention::Deinit) => "SelfDeinit",
        };
        let mut parts = vec![TypeKey(receiver.to_string())];
        parts.extend(self.types.iter().cloned());
        SignatureKey {
            types: parts,
            kw_variadic: self.kw_variadic.clone(),
            keywords: self.keywords.clone(),
        }
    }
}

/// The lowered symbol of an overloaded free function: `pick$ov$Int`.
/// The bundled stdlib's nominal `String` struct under its linked module
/// identity. The checker's construction routing and the VM's
/// literal-to-struct bridge both key on this exact declaration.
pub(crate) const STDLIB_STRING_STRUCT: &str = "__module$std$string$String";

/// Whether `name` is the bundled nominal `String` struct — the linked
/// qualified identity, or the bare name in unlinked/focused contexts.
pub(crate) fn is_stdlib_string_struct(name: &str) -> bool {
    name == "String" || name == STDLIB_STRING_STRUCT
}

pub fn function_symbol(base: &str, sig: &SignatureKey) -> String {
    format!("{base}{}", sig.suffix())
}

/// The lowered symbol of an overloaded struct method (including `__init__` and
/// the other lifecycle methods): `Box.value$ov$Int`.
pub fn method_symbol(type_name: &str, method: &str, sig: &SignatureKey) -> String {
    format!("{type_name}.{method}{}", sig.suffix())
}

/// Convention-qualified symbol for `__iter__` overloads. Current Mojo permits
/// borrowed and owned `__iter__` methods with identical explicit parameters;
/// receiver convention is therefore part of this method's callable identity.
pub fn iterator_method_symbol(
    type_name: &str,
    convention: Option<ArgConvention>,
    sig: &SignatureKey,
) -> String {
    method_symbol(type_name, "__iter__", &sig.with_receiver(convention))
}

/// Convention-qualified abstract `__iter__` symbol for a bounded generic
/// receiver. The checker records this exact symbol in the iteration protocol;
/// the VM only retargets its receiver prefix once the erased generic value's
/// nominal runtime type is known.
pub fn iterator_dispatch_symbol(convention: ArgConvention) -> String {
    iterator_method_symbol(
        "__trait_dispatch",
        Some(convention),
        &SignatureKey {
            types: Vec::new(),
            kw_variadic: None,
            keywords: Vec::new(),
        },
    )
}

/// The sibling borrowed-receiver spelling of an abstract `__iter__` dispatch
/// symbol. A borrowed conformer may declare `self` (Read) or `ref self` (Ref),
/// while the checker pins one spelling in the iteration protocol; runtime
/// retargeting probes the sibling before giving up. Owned (`var self`) dispatch
/// has no sibling.
pub fn borrowed_iterator_dispatch_alternate(symbol: &str) -> Option<String> {
    let read = iterator_dispatch_symbol(ArgConvention::Read);
    let reference = iterator_dispatch_symbol(ArgConvention::Ref);
    if symbol == read {
        Some(reference)
    } else if symbol == reference {
        Some(read)
    } else {
        None
    }
}

/// Retarget a checker-selected method symbol from an abstract receiver (for
/// example `__trait_dispatch.pick$ov$Int`) to the concrete runtime type while
/// preserving the exact selected method/signature suffix. Keeping this parsing
/// here preserves this module's ownership of the overload encoding.
pub fn retarget_method_symbol(symbol: &str, type_name: &str) -> Option<String> {
    let (_, method_and_signature) = symbol.rsplit_once('.')?;
    Some(format!("{type_name}.{method_and_signature}"))
}

/// Whether a resolved method symbol denotes the `Indexer` normalization hook.
/// Keep this knowledge beside the overload encoding so checked/MIR consumers do
/// not parse `$ov$` spellings independently.
pub fn is_index_normalization_symbol(symbol: &str) -> bool {
    symbol.rsplit_once('.').is_some_and(|(_, method)| {
        method == "__mlir_index__"
            || method
                .strip_prefix("__mlir_index__")
                .is_some_and(|suffix| suffix.starts_with(OV_SEP))
    })
}

/// The overloaded declarations of a program, scanned from its top level: which
/// free-function names and `Type.method` names have more than one definition
/// (and at which arities). Definitions of non-overloaded names keep their plain
/// source name, so lowering consults this before qualifying anything.
#[derive(Debug, Default, Clone)]
pub struct OverloadSets {
    functions: HashMap<String, HashSet<usize>>,
    all_functions: HashSet<String>,
    methods: HashMap<String, HashSet<usize>>,
    comptimes: HashMap<String, i64>,
}

impl OverloadSets {
    pub fn scan(program: &[Stmt]) -> OverloadSets {
        let mut functions: HashMap<String, Vec<usize>> = HashMap::new();
        let mut methods: HashMap<String, Vec<usize>> = HashMap::new();
        let mut comptimes = HashMap::new();
        for stmt in program {
            match &stmt.kind {
                StmtKind::Comptime { name, value, .. } => {
                    if let Some(value) = eval_comptime_int(value, &comptimes) {
                        comptimes.insert(name.clone(), value);
                    }
                }
                StmtKind::Def { name, params, .. } => {
                    functions
                        .entry(name.clone())
                        .or_default()
                        .push(params.len());
                }
                StmtKind::Struct {
                    name, methods: ms, ..
                } => {
                    for method in ms {
                        let method_name = lifecycle_method_name(method);
                        methods
                            .entry(format!("{name}.{method_name}"))
                            .or_default()
                            .push(method.params.len());
                    }
                }
                _ => {}
            }
        }
        OverloadSets {
            all_functions: functions.keys().cloned().collect(),
            functions: keep_overloaded(functions),
            methods: keep_overloaded(methods),
            comptimes,
        }
    }

    /// Whether free function `name` is overloaded and defines arity `arity`.
    pub fn function_is_overloaded(&self, name: &str, arity: usize) -> bool {
        self.functions
            .get(name)
            .is_some_and(|arities| arities.contains(&arity))
    }

    /// Whether `name` denotes any linked free-function declaration. MIR uses
    /// this to distinguish a function value from a non-local runtime name.
    pub fn is_function(&self, name: &str) -> bool {
        self.all_functions.contains(name)
    }

    /// Whether method `source_name` (`Type.method`) is overloaded and defines
    /// arity `arity` (`self` excluded, matching the declared parameter list).
    pub fn method_is_overloaded(&self, source_name: &str, arity: usize) -> bool {
        self.methods
            .get(source_name)
            .is_some_and(|arities| arities.contains(&arity))
    }
}

/// The name a top-level `def` lowers to: signature-qualified when the name is
/// overloaded, the plain source name otherwise.
pub fn lowered_def_name(
    name: &str,
    type_params: &[TypeParam],
    params: &[FnParam],
    sets: &OverloadSets,
) -> String {
    if sets.function_is_overloaded(name, params.len()) {
        function_symbol(
            name,
            &signature_from_ast(params, type_params, &sets.comptimes),
        )
    } else {
        name.to_string()
    }
}

/// The name a struct method lowers to, from its already-joined source name
/// (`Type.method`): signature-qualified when overloaded, unchanged otherwise.
pub fn lowered_method_name(
    source_name: &str,
    type_params: &[TypeParam],
    params: &[FnParam],
    keyword_only: Option<usize>,
    self_convention: Option<ArgConvention>,
    sets: &OverloadSets,
) -> String {
    if sets.method_is_overloaded(source_name, params.len()) {
        let signature = signature_from_ast(params, type_params, &sets.comptimes)
            .with_keyword_names(keyword_only_names(params, keyword_only));
        if let Some(type_name) = source_name.strip_suffix(".__iter__") {
            iterator_method_symbol(type_name, self_convention, &signature)
        } else {
            format!("{source_name}{}", signature.suffix())
        }
    } else {
        source_name.to_string()
    }
}

/// The name a method is *registered and counted* under: current Mojo spells the
/// copy constructor as an `__init__` overload with an `out self, copy: Self`
/// shape, which the whole pipeline models as `__copyinit__`.
pub fn lifecycle_method_name(m: &Method) -> &str {
    if is_mojo_copy_constructor(m) {
        "__copyinit__"
    } else if is_mojo_move_constructor(m) {
        "__moveinit__"
    } else {
        &m.name
    }
}

/// The lifted name of a nested `def` (`inner` declared inside `outer`).
pub fn nested_lifted_name(outer: &str, inner: &str) -> String {
    format!("{outer}${inner}")
}

/// A same-spelled nested declaration's lifted name. Declaration identity, not
/// a source offset, provides the disambiguator so cloned/synthesized syntax and
/// ordinary block shadowing follow one stable scheme.
pub fn nested_lifted_declaration_name(
    outer: &str,
    inner: &str,
    declaration: crate::CheckedDeclId,
) -> String {
    format!("{outer}${inner}$decl{}", declaration.0)
}

/// A deliberate **poison name** for an overloaded call the checker recorded no
/// target for (only reachable off the checked path): it can never name a real
/// function, so the VM reports it instead of guessing among overloads.
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

/// If `symbol` is a signature-qualified `__init__` overload (`Type.__init__$ov$…`),
/// the struct it constructs.
pub fn init_overload_struct(symbol: &str) -> Option<&str> {
    let (struct_name, rest) = symbol.rsplit_once(".__init__")?;
    rest.starts_with(OV_SEP).then_some(struct_name)
}

fn ty_raw(ty: &Ty) -> String {
    match ty {
        Ty::Int | Ty::IntLiteral => "Int".to_string(),
        Ty::UInt => "UInt".to_string(),
        Ty::Float64 | Ty::FloatLiteral => "Float64".to_string(),
        Ty::Bool => "Bool".to_string(),
        Ty::StringLiteral => "String".to_string(),
        Ty::None => "None".to_string(),
        Ty::ComptimeList(elem) => format!("__ComptimeList${}", ty_raw(elem)),
        Ty::Tuple(elems) => format!(
            "Tuple${}",
            elems.iter().map(ty_raw).collect::<Vec<_>>().join("$")
        ),
        Ty::RuntimePack(elems) => format!(
            "$pack${}",
            elems.iter().map(ty_raw).collect::<Vec<_>>().join("$")
        ),
        Ty::VariadicPack(element) => format!("$variadic${}", ty_raw(element)),
        Ty::Variant(alternatives) => format!(
            "Variant${}",
            alternatives
                .iter()
                .map(ty_raw)
                .collect::<Vec<_>>()
                .join("$")
        ),
        // The nominal stdlib String keeps the historical `String` symbol
        // spelling, so overload identities do not churn across the
        // StringLiteral/String type split. Consequence: an overload set
        // differing only in StringLiteral-vs-String collides and is rejected.
        Ty::Struct(name, args) if args.is_empty() && is_stdlib_string_struct(name) => {
            "String".to_string()
        }
        // A struct type spells as its annotation does (`Point`, `Pair$Int`) —
        // no `Struct$` marker, so the MIR definition name matches.
        Ty::Struct(name, args) => {
            let mut s = encode_identifier(name);
            for arg in args {
                s.push('$');
                match arg {
                    TyArg::Ty(t) => s.push_str(&ty_raw(t)),
                    TyArg::Val(v) => s.push_str(&format!("V{v}")),
                    // Origins erase from the runtime ABI: every origin argument
                    // mangles to one marker so origin-differing types share a
                    // specialization and a lowering, like `Ty::Pointer` origins.
                    TyArg::Origin(_) => s.push('O'),
                }
            }
            s
        }
        // A type parameter spells as the bare annotation `T` does.
        Ty::Param {
            name,
            bounds,
            callable_bound,
        } => {
            let mut result = encode_identifier(name);
            for bound in bounds {
                result.push('$');
                result.push_str(&encode_identifier(bound));
            }
            if let Some(callable) = callable_bound {
                result.push_str("$Callable$");
                result.push_str(&ty_raw(callable));
            }
            result
        }
        // Pointer origins affect checking/lifetimes but erase from the runtime
        // callable ABI, just like origin parameters on `ref` arguments.
        Ty::Pointer { element, .. } => format!("UnsafePointer${}", ty_raw(element)),
        // Application arguments participate in the mangled identity (so
        // `IteratorType[a]` and `IteratorType[b]` are distinct), except origins,
        // which erase from the runtime ABI like `Ty::Pointer` origins above.
        Ty::Assoc { base, name, args } => {
            let mut s = format!("Assoc${}${}", ty_raw(base), encode_identifier(name));
            for arg in args {
                s.push('$');
                match arg {
                    TyArg::Ty(t) => s.push_str(&ty_raw(t)),
                    TyArg::Val(v) => s.push_str(&format!("V{v}")),
                    // Origins erase from the runtime ABI: every origin argument
                    // mangles to one marker so origin-differing types share a
                    // specialization and a lowering, like `Ty::Pointer` origins.
                    TyArg::Origin(_) => s.push('O'),
                }
            }
            s
        }
        Ty::SelfType => "Self".to_string(),
        other => other.to_string(),
    }
}

/// Encode source-controlled identifier text injectively while leaving ordinary
/// ASCII identifiers unchanged. Structural `$` separators are added only after
/// this encoding, so stropped names such as `A-B` and `A_B` cannot collide.
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
    comptimes: &HashMap<String, i64>,
    type_bounds: &HashMap<String, Vec<String>>,
) -> String {
    match ty {
        Type::Int => "Int".to_string(),
        Type::UInt => "UInt".to_string(),
        Type::Bool => "Bool".to_string(),
        Type::StringLiteral => "String".to_string(),
        Type::Float64 => "Float64".to_string(),
        Type::None => "None".to_string(),
        Type::Named(name, args) if args.is_empty() && is_stdlib_string_struct(name) => {
            // Mirror `ty_raw`: the nominal String annotation mangles as the
            // stable `String` spelling.
            "String".to_string()
        }
        // Mirror `ty_raw`: the `NoneType` annotation resolves to `Ty::None`,
        // which mangles as `None`.
        Type::Named(name, args) if args.is_empty() && name == "NoneType" => "None".to_string(),
        // `Scalar[DType.x]` / `SIMD[DType.x, N]` annotations mangle as their
        // canonical checked spelling (`Scalar[DType.float32]` is the width-1
        // `Float32`), matching `ty_raw`'s display of the resolved `Ty::Simd`.
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
        Type::Named(name, args) => {
            let mut s = parameter_raw(name, type_bounds);
            for arg in args {
                s.push('$');
                match arg {
                    ParamArg::Type(t) => s.push_str(&ast_raw(t, comptimes, type_bounds)),
                    ParamArg::Value(v) => {
                        s.push('V');
                        s.push_str(&value_expr_raw(v, comptimes));
                    }
                    ParamArg::Named { name, value } => {
                        s.push_str(name);
                        s.push('=');
                        match &**value {
                            ParamArg::Type(t) => s.push_str(&ast_raw(t, comptimes, type_bounds)),
                            ParamArg::Value(v) => s.push_str(&value_expr_raw(v, comptimes)),
                            ParamArg::Named { .. } => unreachable!(),
                        }
                    }
                }
            }
            s
        }
        // `Self.T` names the same parameter a bare `T` does inside the struct,
        // and the checker resolves both to the same `Ty::Param` — spell them
        // identically so the two sides agree.
        Type::SelfParam(name) => parameter_raw(name, type_bounds),
        Type::Assoc { base, name, .. } => format!(
            "Assoc${}${}",
            ast_raw(base, comptimes, type_bounds),
            encode_identifier(name)
        ),
        Type::SelfType => "Self".to_string(),
        Type::MaterializedCallable(key) => key.clone(),
        other => format!("{other:?}"),
    }
}

/// The dtype named by a `DType.<dt>` annotation argument, if that is what it is.
fn param_arg_dtype(argument: &ParamArg) -> Option<crate::ast::Dtype> {
    let ParamArg::Value(Expr {
        kind: ExprKind::Member { object, field },
        ..
    }) = argument
    else {
        return None;
    };
    matches!(&object.kind, ExprKind::Identifier(name) if name == "DType")
        .then(|| crate::ast::Dtype::from_name(field))
        .flatten()
}

/// A comptime-evaluable SIMD width annotation argument.
fn param_arg_width(argument: &ParamArg, comptimes: &HashMap<String, i64>) -> Option<i64> {
    let ParamArg::Value(expr) = argument else {
        return None;
    };
    eval_comptime_int(expr, comptimes)
}

/// The canonical checked spelling of a scalar/SIMD annotation: width-1
/// `int`/`float64` canonicalize to the native scalars, everything else uses
/// the `Ty::Simd` display (the scalar alias where one exists).
fn simd_annotation_raw(dtype: crate::ast::Dtype, width: i64) -> String {
    match (dtype, width) {
        (crate::ast::Dtype::Int, 1) => "Int".to_string(),
        (crate::ast::Dtype::Float64, 1) => "Float64".to_string(),
        _ => Ty::Simd { dtype, width }.to_string(),
    }
}

fn eval_comptime_int(expr: &Expr, comptimes: &HashMap<String, i64>) -> Option<i64> {
    use crate::ast::{InfixOp, PrefixOp};
    match &expr.kind {
        ExprKind::Int(value) => value.to_i64(),
        ExprKind::Identifier(name) => comptimes.get(name).copied(),
        ExprKind::Prefix(PrefixOp::Neg, value) => {
            eval_comptime_int(value, comptimes)?.checked_neg()
        }
        ExprKind::Infix(op, left, right) => {
            let (left, right) = (
                eval_comptime_int(left, comptimes)?,
                eval_comptime_int(right, comptimes)?,
            );
            match op {
                InfixOp::Add => left.checked_add(right),
                InfixOp::Sub => left.checked_sub(right),
                InfixOp::Mul => left.checked_mul(right),
                InfixOp::FloorDiv if right != 0 => Some(left.div_euclid(right)),
                InfixOp::Mod if right != 0 => Some(left.rem_euclid(right)),
                InfixOp::Pow if right >= 0 => left.checked_pow(right as u32),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The separator that marks a signature-qualified overload symbol:
/// `pick$ov$Int`, `Box.__init__$ov$String`. Never referenced outside this
/// module.
const OV_SEP: &str = "$ov$";

fn sanitize(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '$' })
        .collect()
}

fn parameter_raw(name: &str, type_bounds: &HashMap<String, Vec<String>>) -> String {
    let mut result = encode_identifier(name);
    if let Some(bounds) = type_bounds.get(name) {
        for bound in bounds {
            result.push('$');
            result.push_str(&encode_identifier(bound));
        }
    }
    result
}

/// The mangled spelling of a compile-time value argument in an annotation
/// (`FixedBuffer[8]` → `8`). A non-literal expression degrades to a stable
/// placeholder — good enough because the name only needs to be deterministic.
fn value_expr_raw(expr: &Expr, comptimes: &HashMap<String, i64>) -> String {
    if let Some(value) = eval_comptime_int(expr, comptimes) {
        return value.to_string();
    }
    match &expr.kind {
        ExprKind::Bool(b) => b.to_string(),
        ExprKind::Str(s) => encode_identifier(s),
        ExprKind::Identifier(name) => encode_identifier(name),
        _ => "expr".to_string(),
    }
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

fn signature_from_ast(
    params: &[FnParam],
    type_params: &[TypeParam],
    comptimes: &HashMap<String, i64>,
) -> SignatureKey {
    let type_bounds = type_params
        .iter()
        .map(|param| (param.name.clone(), param.bounds.clone()))
        .collect();
    SignatureKey {
        types: params
            .iter()
            .filter(|param| param.kind != crate::ast::ParamKind::KwVariadic)
            .map(|param| TypeKey(sanitize(&ast_raw(&param.ty, comptimes, &type_bounds))))
            .collect(),
        kw_variadic: params
            .iter()
            .find(|param| param.kind == crate::ast::ParamKind::KwVariadic)
            .map(|param| TypeKey(sanitize(&ast_raw(&param.ty, comptimes, &type_bounds)))),
        keywords: Vec::new(),
    }
}

/// The names of the keyword-only parameters, part of overload identity.
/// Parameters after a `*` marker or a variadic collector are keyword-only;
/// the collectors themselves are not named parameters.
fn keyword_only_names(params: &[FnParam], keyword_only: Option<usize>) -> Vec<String> {
    let variadic = params
        .iter()
        .position(|param| param.kind == crate::ast::ParamKind::Variadic);
    let boundary = match (keyword_only, variadic) {
        (Some(marker), Some(variadic)) => marker.min(variadic + 1),
        (Some(marker), None) => marker,
        (None, Some(variadic)) => variadic + 1,
        (None, None) => return Vec::new(),
    };
    params
        .iter()
        .skip(boundary)
        .filter(|param| {
            !matches!(
                param.kind,
                crate::ast::ParamKind::Variadic | crate::ast::ParamKind::KwVariadic
            )
        })
        .map(|param| param.name.clone())
        .collect()
}

fn is_mojo_move_constructor(m: &Method) -> bool {
    m.name == "__init__"
        && m.has_self
        && matches!(m.self_convention, Some(ArgConvention::Out))
        && m.positional_only.is_none()
        && m.keyword_only == Some(0)
        && m.params.len() == 1
        && m.params[0].name == "move"
        && m.params[0].default.is_none()
        && m.params[0].kind == ParamKind::Regular
        // Current Mojo requires the consuming `deinit move: Self` convention;
        // the bare `move: Self` shape is rejected by the checker.
        && matches!(m.params[0].convention, Some(ArgConvention::Deinit))
        && matches!(m.params[0].ty, Type::SelfType)
        && m.ret.is_none()
}

fn is_mojo_copy_constructor(m: &Method) -> bool {
    m.name == "__init__"
        && m.has_self
        && matches!(m.self_convention, Some(ArgConvention::Out))
        && m.positional_only.is_none()
        && m.keyword_only == Some(0)
        && m.params.len() == 1
        && m.params[0].name == "copy"
        && m.params[0].default.is_none()
        && m.params[0].kind == ParamKind::Regular
        && m.params[0].convention.is_none()
        && matches!(m.params[0].ty, Type::SelfType)
        && m.ret.is_none()
}

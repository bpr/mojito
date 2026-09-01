//! Typing and capability rules for compiler-known builtin types and operations.

use super::*;

pub(super) fn default_literal(ty: &Ty) -> Ty {
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

/// Whether `ty` is a non-numeric scalar value type — what `==`/`!=` compare once
/// the numeric cases (handled by `common_numeric`) are out of the way.
pub(super) fn is_scalar(ty: &Ty) -> bool {
    matches!(ty, Ty::Bool | Ty::StringLiteral | Ty::None)
}

/// Whether an opaque type parameter carries a bound that promises equality.
/// Built-in bounds are intentionally shallow today, but `T: Equatable` should at
/// least let generic library code type-check `T == T`. `Comparable` refines
/// equality (roadmap milestone 4), so it counts too; `Hashable` deliberately does **not**
/// (a hash-backed key bounds `K: Hashable & Equatable` when it needs both).
pub(super) fn has_equality_bound(ty: &Ty) -> bool {
    match ty {
        Ty::Param { bounds, .. } => bounds.iter().any(|b| {
            matches!(
                b.as_str(),
                "Equatable" | "Comparable" | "EqualityComparable"
            )
        }),
        _ => false,
    }
}

pub(super) fn has_equality_bound_or_concrete(checker: &Checker, ty: &Ty) -> bool {
    match ty {
        Ty::Struct(name, _) => checker
            .structs
            .get(name)
            .is_some_and(|s| s.conforms.iter().any(|c| c == "Equatable")),
        Ty::Variant(alternatives) => alternatives
            .iter()
            .all(|alternative| has_equality_bound_or_concrete(checker, alternative)),
        _ => has_equality_bound(ty) || is_scalar(ty) || is_numeric_like(ty),
    }
}

/// Whether an opaque type parameter carries a bound that promises an ordering
/// (`<`/`<=`/`>`/`>=`). Only `Comparable` grants this — a plain `T: Equatable`
/// permits `==`/`!=` but *not* ordering (see `has_equality_bound`). In current
/// Mojo `Comparable` also implies equality, which `has_equality_bound` reflects.
pub(super) fn has_order_bound(ty: &Ty) -> bool {
    match ty {
        Ty::Param { bounds, .. } => bounds.iter().any(|b| b.as_str() == "Comparable"),
        _ => false,
    }
}

/// Whether an opaque type parameter carries a bound that promises a length, so
/// `len(x)` is well-typed on it. `Sized` (`__len__(self) -> Int`) and
/// `SizedRaising` (`__len__(self) raises -> Int`) both do — mojito's effect
/// analysis is deferred, so the two are not distinguished at the call site; a
/// plain `T: AnyType` grants no length.
pub(super) fn has_len_bound(ty: &Ty) -> bool {
    match ty {
        Ty::Param { bounds, .. } => bounds
            .iter()
            .any(|b| matches!(b.as_str(), "Sized" | "SizedRaising")),
        _ => false,
    }
}

/// Whether `ty` is an opaque type parameter carrying the named trait `bound`.
/// The numeric-operation traits (roadmap milestone 7 — `Absable`/`Roundable`/`Powable`/
/// `Intable`/`Floatable`/`Boolable`/`DivModable`) gate a corresponding built-in
/// or operator on an opaque `T` this way: the concrete type's implementation
/// runs after type erasure.
pub(super) fn param_has_bound(ty: &Ty, bound: &str) -> bool {
    matches!(ty, Ty::Param { bounds, .. } if bounds.iter().any(|b| b == bound))
}

pub(super) fn builtin_trait_operation(trait_name: &str) -> Option<&'static str> {
    match trait_name {
        "Hashable" => Some("__hash__(self, mut hasher: Some[Hasher]) -> None"),
        "Absable" => Some("__abs__() -> Self"),
        "Roundable" => Some("__round__() -> Self"),
        "Powable" => Some("__pow__(Self) -> Self"),
        "Intable" => Some("__int__() -> Int"),
        "Floatable" => Some("__float__() -> Float64"),
        "Boolable" => Some("__bool__() -> Bool"),
        "DivModable" => Some("__divmod__(Self) -> Tuple[Self, Self]"),
        // Binary-operator traits: each names the dunder its operator dispatches
        // through. `__truediv__` returns `Float64` (mojito's `/` is always
        // floating); the rest return `Self`.
        "Addable" => Some("__add__(Self) -> Self"),
        "Subtractable" => Some("__sub__(Self) -> Self"),
        "Multipliable" => Some("__mul__(Self) -> Self"),
        "Divisible" => Some("__truediv__(Self) -> Float64"),
        "FloorDivisible" => Some("__floordiv__(Self) -> Self"),
        "Modable" => Some("__mod__(Self) -> Self"),
        "ShiftLeftable" => Some("__lshift__(Self) -> Self"),
        "ShiftRightable" => Some("__rshift__(Self) -> Self"),
        "Andable" => Some("__and__(Self) -> Self"),
        "Orable" => Some("__or__(Self) -> Self"),
        "Xorable" => Some("__xor__(Self) -> Self"),
        "Negatable" => Some("__neg__() -> Self"),
        _ => None,
    }
}

/// The operation trait a binary operator dispatches through, for both builtin
/// scalars and user structs. `None` for operators that do not route this way:
/// `and`/`or` (Bool short-circuit), `in`/`not in` (`__contains__` on the right
/// operand), and `@`/matmul (no scalar meaning — struct dunder only).
pub(super) fn infix_operation_trait(op: crate::ast::InfixOp) -> Option<&'static str> {
    use crate::ast::InfixOp::*;
    Some(match op {
        Add => "Addable",
        Sub => "Subtractable",
        Mul => "Multipliable",
        Div => "Divisible",
        FloorDiv => "FloorDivisible",
        Mod => "Modable",
        Pow => "Powable",
        Shl => "ShiftLeftable",
        Shr => "ShiftRightable",
        BitAnd => "Andable",
        BitOr => "Orable",
        BitXor => "Xorable",
        Lt | Gt | Le | Ge => "Comparable",
        Eq | Ne => "Equatable",
        MatMul | And | Or | In | NotIn => return None,
    })
}

/// The operation trait a prefix operator dispatches through.
pub(super) fn prefix_operation_trait(op: crate::ast::PrefixOp) -> &'static str {
    match op {
        crate::ast::PrefixOp::Neg => "Negatable",
        crate::ast::PrefixOp::Not => "Boolable",
    }
}

/// Integer-kind scalars — the operands of bitwise and shift operators
/// (`IntLiteral` materializes to `Int`).
pub(super) fn is_integer_like(ty: &Ty) -> bool {
    matches!(default_literal(ty), Ty::Int | Ty::UInt)
}

/// Signed numeric scalars — the operands of arithmetic negation (`-x`); `UInt`
/// is excluded, matching the existing `infer_prefix` rule.
pub(super) fn is_signed_numeric_like(ty: &Ty) -> bool {
    matches!(default_literal(ty), Ty::Int | Ty::Float64)
}

/// The trait bounds that supply a numeric-rounding dunder (`method`/`argc`),
/// used by the self-hosted `math` module (roadmap milestone 7). `__floor__`/`__ceil__`/
/// `__trunc__` are nullary (`Floorable`/`Ceilable`/`Truncable`); `__ceildiv__`
/// is unary and granted by `CeilDivable` or its raising sibling
/// `CeilDivableRaising` (mojito's deferred effect model does not distinguish
/// them). A bound satisfies the dunder if it is any of the returned names.
pub(super) fn math_dunder_bound(method: &str, argc: usize) -> &'static [&'static str] {
    match (method, argc) {
        ("__floor__", 0) => &["Floorable"],
        ("__ceil__", 0) => &["Ceilable"],
        ("__trunc__", 0) => &["Truncable"],
        ("__ceildiv__", 1) => &["CeilDivable", "CeilDivableRaising"],
        _ => &[],
    }
}

/// Whether a *concrete* built-in type has an intrinsic `__hash__` — the scalar
/// set the VM can hash directly (`Int`/`UInt`/`Bool`/`String`/`Float64`). This
/// lets a user key struct combine `self.field.__hash__()` values.
/// Whether `Copyable.copy` on a value of this type has no callee: built-in
/// scalars, literals, tuples, packs, and variants copy by the ordinary value
/// read. Nominal, parametric, associated, and reference types resolve their
/// `copy` through declarations or trait dispatch instead.
pub(crate) fn builtin_copy_is_value_read(ty: &Ty) -> bool {
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

pub(super) fn builtin_hashable_ty(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Int | Ty::UInt | Ty::Bool | Ty::StringLiteral | Ty::Float64 | Ty::Simd { width: 1, .. }
    )
}

pub(super) fn is_numeric_like(ty: &Ty) -> bool {
    is_numeric(&default_literal(ty))
}

/// Enforce that a builtin-driven dunder (`__len__`/`__str__`/`__contains__`)
/// returns its Mojo-mandated type, so `len`/`String`/`in` on a user struct stay
/// well-typed.
pub(super) fn require_dunder_ret(ret: Ty, expected: &Ty, name: &str) -> Result<Ty, TypeError> {
    if ret == *expected {
        Ok(ret)
    } else {
        Err(TypeError::TypeMismatch {
            expected: expected.to_string(),
            found: ret.to_string(),
            context: format!("return type of '{name}'"),
        })
    }
}

/// Whether list elements of type `ty` can be compared for equality (needed by
/// `List.remove`/`count`/`index`) — the same scalar set `==`/`!=` accept.
pub(super) fn is_list_equatable(ty: &Ty) -> bool {
    is_numeric(ty)
        || matches!(ty, Ty::Bool | Ty::StringLiteral | Ty::None)
        || has_equality_bound(ty)
}

/// Whether every element in a tuple supports equality. Tuples recurse so nested
/// tuple comparisons and membership stay structural without making `List`
/// equality part of this compiler-known subset.
pub(super) fn tuple_elements_equatable(elements: &[Ty]) -> bool {
    elements.iter().all(|ty| match ty {
        Ty::Tuple(nested) => tuple_elements_equatable(nested),
        other => is_list_equatable(other),
    })
}

/// Tuple ordering is lexicographic. Every element must be comparable, and each
/// pair in the common prefix must have a compatible comparison operation.
pub(super) fn tuple_order_compatible(left: &[Ty], right: &[Ty]) -> bool {
    tuple_elements_comparable(left)
        && tuple_elements_comparable(right)
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| tuple_order_pair_compatible(left, right))
}

/// Whether a value of type `ty` can be `print`ed (has a user-facing display).
/// Functions, ranges, and opaque type parameters are not printable.
pub(super) fn is_printable(ty: &Ty) -> bool {
    match ty {
        Ty::Int
        | Ty::UInt
        | Ty::Bool
        | Ty::StringLiteral
        | Ty::Float64
        | Ty::None
        | Ty::IntLiteral
        | Ty::FloatLiteral
        | Ty::Struct(_, _)
        | Ty::Simd { .. }
        | Ty::Error
        | Ty::ComptimeList(_) => true,
        // A tuple prints if every element prints.
        Ty::Tuple(elems) => elems.iter().all(is_printable),
        Ty::Variant(alternatives) => alternatives.iter().all(is_printable),
        _ => false,
    }
}

/// Whether `ty` is a numeric type (concrete or literal).
pub(super) fn is_numeric(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Int | Ty::UInt | Ty::Float64 | Ty::IntLiteral | Ty::FloatLiteral
    )
}

/// Whether a value of type `from` can be used where `to` is required. Only the
/// literal types coerce (to the concrete numeric types, or `IntLiteral` up to
/// `FloatLiteral`); everything else must match exactly.
pub(super) fn coerces(from: &Ty, to: &Ty) -> bool {
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

/// The value-coercion policy for callable environments: current Mojo rejects
/// binding a capturing closure to an unqualified `def(...)` value position —
/// the contract must spell `capturing[...]` — while a thin function value
/// still binds. Comptime callable *bounds* stay on the permissive
/// `callable_environment_coerces` below.
pub(crate) fn callable_environment_value_coerces(
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

/// True when `from` fails to bind to `to` solely because a capturing
/// environment meets an unqualified `def(...)` value contract — the shape that
/// deserves the "spell `capturing[...]`" migration hint.
pub(super) fn callable_mismatch_is_environment_only(from: &Ty, to: &Ty) -> bool {
    let (
        Ty::Func {
            environment: from_environment,
            ..
        },
        Ty::Func {
            environment: to_environment,
            ..
        },
    ) = (from, to)
    else {
        return false;
    };
    if callable_environment_value_coerces(from_environment, to_environment)
        || !callable_environment_coerces(from_environment, to_environment)
    {
        return false;
    }
    let mut aligned = from.clone();
    if let Ty::Func { environment, .. } = &mut aligned {
        *environment = to_environment.clone();
    }
    coerces(&aligned, to)
}

pub(crate) fn callable_environment_coerces(
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

/// The common type of two list elements: numeric elements unify like operands
/// (widening literals); otherwise the two must be equal.
pub(super) fn common_elem(a: &Ty, b: &Ty) -> Option<Ty> {
    if is_numeric(a) && is_numeric(b) {
        common_numeric(a, b)
    } else if a == b {
        Some(a.clone())
    } else {
        None
    }
}

/// The common type of two numeric operands, coercing literals as needed, or
/// `None` if they can't be unified (e.g. two different concrete types).
/// The common type of a ternary's two branches: unify numerics (widening
/// literals), else an exact match or a one-way literal coercion. `None` if the
/// branches are incompatible.
pub(super) fn common_branch_ty(a: &Ty, b: &Ty) -> Option<Ty> {
    if let Some(c) = common_numeric(a, b) {
        return Some(c);
    }
    if a == b {
        Some(a.clone())
    } else if coerces(a, b) {
        Some(b.clone())
    } else if coerces(b, a) {
        Some(a.clone())
    } else {
        None
    }
}

pub(super) fn common_numeric(a: &Ty, b: &Ty) -> Option<Ty> {
    if !is_numeric(a) || !is_numeric(b) {
        return None;
    }
    if a == b {
        Some(a.clone())
    } else if coerces(a, b) {
        Some(b.clone())
    } else if coerces(b, a) {
        Some(a.clone())
    } else {
        None
    }
}

/// Type inference for compiler-known builtin free functions
/// (`print`, `len`, `range`, conversions, `divmod`, …). Moved from `checker.rs`.
impl Checker {
    /// Type `print(...)`. Intrinsic scalars have builtin writing; nominal values,
    /// including public collections, opt into current `Writable`. During tuple
    /// specialization discovery an as-yet-unmaterialized nominal shape is checked
    /// element-wise; executable values always cross the concrete struct boundary.
    pub(super) fn infer_print(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        for (i, arg) in args.iter().enumerate() {
            let ty = self.infer(arg)?;
            let runtime_ty = default_literal(&ty);
            if runtime_ty != ty {
                self.record_literal_materializations(arg, &ty, &runtime_ty)?;
            }
            if let Ty::Struct(name, _) = &ty
                && !self.structs.contains_key(name)
                && (list_element(&ty).is_some_and(is_printable)
                    || set_element(&ty).is_some_and(is_printable)
                    || dict_elements(&ty)
                        .is_some_and(|(key, value)| is_printable(key) && is_printable(value))
                    || tuple_elements(&ty)
                        .is_some_and(|elements| elements.into_iter().all(is_printable)))
            {
                continue;
            }
            if matches!(ty, Ty::Struct(..) | Ty::Variant(_)) {
                if self.conforms_to(&ty, "Writable") {
                    continue;
                }
                return Err(TypeError::TypeMismatch {
                    expected: "Writable".to_string(),
                    found: ty.to_string(),
                    context: format!("argument {} to 'print'", i + 1),
                });
            }
            if matches!(ty, Ty::Param { .. }) && self.conforms_to(&ty, "Writable") {
                continue;
            }
            if !is_printable(&ty) {
                return Err(TypeError::TypeMismatch {
                    expected: "a printable value".to_string(),
                    found: ty.to_string(),
                    context: format!("argument {} to 'print'", i + 1),
                });
            }
        }
        Ok(Ty::None)
    }

    /// Type the built-in `input(prompt)`: the prompt is a compile-time or
    /// nominal `String`, and the line read from standard input materializes
    /// as the nominal `String`.
    pub(super) fn infer_input(&self, span: SourceSpan, args: &[Expr]) -> Result<Ty, TypeError> {
        let tys = self.builtin_args("input", 1, args)?;
        let nominal_prompt = matches!(&tys[0], Ty::Struct(name, args)
            if args.is_empty() && crate::symbol::is_stdlib_string_struct(name));
        if tys[0] == Ty::StringLiteral || nominal_prompt {
            self.nominal_string_wrap(span)
        } else {
            Err(TypeError::TypeMismatch {
                expected: "String".to_string(),
                found: tys[0].to_string(),
                context: "argument to 'input'".to_string(),
            })
        }
    }

    /// Require a built-in call to have exactly `n` arguments, and return the
    /// inferred type of each.
    pub(super) fn builtin_args(
        &self,
        name: &str,
        n: usize,
        args: &[Expr],
    ) -> Result<Vec<Ty>, TypeError> {
        if args.len() != n {
            return Err(TypeError::ArityMismatch {
                name: name.to_string(),
                expected: n,
                got: args.len(),
            });
        }
        args.iter().map(|a| self.infer(a)).collect()
    }

    /// Type `String(x)`: stringify a numeric, `Bool`, or `String` value.
    pub(super) fn infer_stringify(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        let tys = self.builtin_args("String", 1, args)?;
        if is_numeric(&tys[0]) || tys[0] == Ty::Bool || tys[0] == Ty::StringLiteral {
            let runtime_ty = default_literal(&tys[0]);
            if runtime_ty != tys[0] {
                self.record_literal_materializations(&args[0], &tys[0], &runtime_ty)?;
            }
            return Ok(Ty::StringLiteral);
        }
        if self.conforms_to(&tys[0], "Writable") {
            // Like `print`, nominal String conversion formats through a
            // borrowed `Writable` receiver and must retain its caller storage
            // until that synchronous formatter returns.
            self.call_place_uses
                .borrow_mut()
                .insert(args[0].source_span());
            return Ok(Ty::StringLiteral);
        }
        Err(TypeError::TypeMismatch {
            expected: "Writable".to_string(),
            found: tys[0].to_string(),
            context: "argument to 'String'".to_string(),
        })
    }

    /// Type `abs(x)`: a numeric argument, returning the same numeric type.
    pub(super) fn infer_abs(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        let tys = self.builtin_args("abs", 1, args)?;
        // A numeric value, or an opaque `T: Absable` — `abs` returns the same type
        // (`__abs__(self) -> Self`); the concrete impl runs after type erasure.
        if is_numeric(&tys[0]) || param_has_bound(&tys[0], "Absable") {
            Ok(tys[0].clone())
        } else if let Some(result) = self.struct_dunder(&tys[0], "__abs__", &[]) {
            // A concrete struct routes through `__abs__(self) -> Self`.
            result
        } else {
            Err(TypeError::TypeMismatch {
                expected: "a numeric value".to_string(),
                found: tys[0].to_string(),
                context: "argument to 'abs'".to_string(),
            })
        }
    }

    /// Type `min(a, b)` / `max(a, b)`: two numeric arguments unified like an
    /// operator (no concrete-type mixing), returning their common type.
    pub(super) fn infer_min_max(&self, name: &str, args: &[Expr]) -> Result<Ty, TypeError> {
        let tys = self.builtin_args(name, 2, args)?;
        common_numeric(&tys[0], &tys[1]).ok_or_else(|| TypeError::BadOperator {
            op: name.to_string(),
            operands: format!("{} and {}", tys[0], tys[1]),
        })
    }

    /// Type `round(x)`: a `Float64` argument returning `Float64`, or an opaque
    /// `T: Roundable` returning the same type (`__round__(self) -> Self`; the
    /// concrete impl runs after type erasure).
    pub(super) fn infer_round(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        let tys = self.builtin_args("round", 1, args)?;
        if matches!(tys[0], Ty::Float64 | Ty::FloatLiteral) {
            Ok(Ty::Float64)
        } else if param_has_bound(&tys[0], "Roundable") {
            Ok(tys[0].clone())
        } else if let Some(result) = self.struct_dunder(&tys[0], "__round__", &[]) {
            // A concrete struct routes through `__round__(self) -> Self`.
            result
        } else {
            Err(TypeError::TypeMismatch {
                expected: "Float64".to_string(),
                found: tys[0].to_string(),
                context: "argument to 'round'".to_string(),
            })
        }
    }

    pub(super) fn len_result_for_type(&self, ty: &Ty) -> Result<Option<Ty>, TypeError> {
        if let Ty::Dependent(DependentType::Indexed { elements, .. }) = ty {
            for element in elements {
                match self.len_result_for_type(element)? {
                    Some(Ty::Int) => {}
                    _ => return Ok(None),
                }
            }
            return Ok(Some(Ty::Int));
        }
        if matches!(
            ty,
            Ty::StringLiteral
                | Ty::ComptimeList(_)
                | Ty::Tuple(_)
                | Ty::RuntimePack(_)
                | Ty::VariadicPack(_)
        ) {
            return Ok(Some(Ty::Int));
        }
        if let Ty::Struct(name, _) = ty
            && !self.structs.contains_key(name)
            && (list_element(ty).is_some()
                || set_element(ty).is_some()
                || dict_elements(ty).is_some()
                || tuple_elements(ty).is_some()
                || crate::types::is_range_type(ty)
                || crate::types::scalar_range_parts(ty).is_some())
        {
            return Ok(Some(Ty::Int));
        }
        // `len(c)` on a user struct dispatches to `c.__len__()` (`Sized`), which
        // must return `Int`.
        if let Some(result) = self.struct_dunder(ty, "__len__", &[]) {
            return result
                .and_then(|ret| require_dunder_ret(ret, &Ty::Int, "__len__"))
                .map(Some);
        }
        // `len(x)` on an opaque type parameter is permitted when its bound
        // promises a length (`T: Sized`) — the concrete type's `__len__` runs at
        // runtime after type erasure.
        if has_len_bound(ty) {
            return Ok(Some(Ty::Int));
        }
        Ok(None)
    }

    /// Type `len(x)`: every possible type of a dependent input must fulfill the
    /// same `Sized`/`__len__ -> Int` contract.
    pub(super) fn infer_len(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        let tys = self.builtin_args("len", 1, args)?;
        if let Some(result) = self.len_result_for_type(&tys[0])? {
            return Ok(result);
        }
        Err(TypeError::TypeMismatch {
            expected: "String, List, or Tuple".to_string(),
            found: tys[0].to_string(),
            context: "argument to 'len'".to_string(),
        })
    }

    /// Type the built-in `range(stop)` / `range(start, stop)` /
    /// `range(start, stop, step)`. All arguments must be `Int`; the result is a
    /// `range`. A zero `step` is valid and produces an empty sequence.
    pub(super) fn infer_range(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        if args.is_empty() {
            return Err(TypeError::ArityMismatch {
                name: "range".to_string(),
                expected: 1,
                got: 0,
            });
        }
        if args.len() > 3 {
            return Err(TypeError::ArityMismatch {
                name: "range".to_string(),
                expected: 3,
                got: args.len(),
            });
        }
        for (i, arg) in args.iter().enumerate() {
            let arg_ty = self.infer(arg)?;
            if !coerces(&arg_ty, &Ty::Int) {
                return Err(TypeError::TypeMismatch {
                    expected: "Int".to_string(),
                    found: arg_ty.to_string(),
                    context: format!("argument {} to 'range'", i + 1),
                });
            }
            self.record_literal_materializations(arg, &arg_ty, &Ty::Int)?;
        }
        Ok(range_type())
    }

    /// Scalar `range` inference for the dtype-inferred family. Upstream's
    /// `range[dtype: DType, //](...)` overloads are infer-only — there is no
    /// explicit-argument spelling — so the linked Int overload set cannot
    /// host them as source defs. Called only after ordinary overload
    /// selection found no match; returns `None` when no argument names a
    /// concrete non-Int scalar, so the ordinary no-match error stands. On
    /// success the result is the abstract family type plus a recorded
    /// instantiation, and the specialization fixpoint rewrites the call into
    /// the generated concrete struct's constructor.
    pub(super) fn infer_scalar_range(
        &self,
        span: &crate::token::SourceSpan,
        args: &[Expr],
    ) -> Result<Option<Ty>, TypeError> {
        if args.is_empty() || args.len() > 3 {
            return Ok(None);
        }
        let mut tys = Vec::with_capacity(args.len());
        for arg in args {
            tys.push(self.infer(arg)?);
        }
        let mut dtype: Option<Dtype> = None;
        let mut triggered = false;
        for ty in &tys {
            let this = match ty {
                Ty::Simd { dtype, width: 1 } => {
                    triggered = true;
                    Some(*dtype)
                }
                Ty::Float64 => {
                    triggered = true;
                    Some(Dtype::Float64)
                }
                Ty::Int => Some(Dtype::Int),
                Ty::IntLiteral | Ty::FloatLiteral => None,
                // Not a scalar-range shape at all; the ordinary
                // no-matching-overload diagnostic stands.
                _ => return Ok(None),
            };
            if let (Some(current), Some(this)) = (&dtype, this)
                && *current != this
            {
                return Err(TypeError::TypeMismatch {
                    expected: format!("Scalar[DType.{}]", current.name()),
                    found: ty.to_string(),
                    context: "range arguments must share one dtype".to_string(),
                });
            }
            dtype = dtype.or(this);
        }
        if !triggered {
            return Ok(None);
        }
        let dtype = dtype.expect("a triggering argument carries a concrete dtype");
        if dtype == Dtype::Bool {
            return Err(TypeError::Unsupported(
                "range requires a numeric dtype".to_string(),
            ));
        }
        if dtype.is_float() {
            return Err(TypeError::Unsupported(if args.len() == 3 {
                "float strided ranges are not supported; use an integral dtype".to_string()
            } else {
                "a floating-point range requires an explicit step; use range(start, end, step)"
                    .to_string()
            }));
        }
        let lane = simd_ty(dtype, 1);
        for (ty, arg) in tys.iter().zip(args) {
            if !splats_to(ty, dtype) {
                return Err(TypeError::TypeMismatch {
                    expected: lane.to_string(),
                    found: ty.to_string(),
                    context: "range argument".to_string(),
                });
            }
            self.record_literal_materializations(arg, ty, &lane)?;
        }
        let family = crate::types::SCALAR_RANGE_FAMILY[args.len() - 1];
        let arguments = vec![TyArg::Val(CtValue::Dtype(dtype))];
        self.generic_instantiations.borrow_mut().insert(
            span.clone(),
            crate::checked::GenericInstantiation {
                callee: family.to_string(),
                arguments: arguments.clone(),
            },
        );
        Ok(Some(Ty::Struct(family.to_string(), arguments)))
    }

    /// Type a conversion built-in `Int(x)` / `UInt(x)` / `Float64(x)` / `Bool(x)`:
    /// exactly one argument of a numeric or `Bool` type, producing `target`. An
    /// opaque type parameter is also accepted when its bound promises the
    /// conversion — `Int(x)` on `T: Intable`, `Float64(x)` on `T: Floatable`,
    /// `Bool(x)` on `T: Boolable` (`__int__`/`__float__`/`__bool__` run after
    /// type erasure).
    pub(super) fn infer_conversion(&self, target: Ty, args: &[Expr]) -> Result<Ty, TypeError> {
        if args.len() != 1 {
            return Err(TypeError::ArityMismatch {
                name: target.to_string(),
                expected: 1,
                got: args.len(),
            });
        }
        let arg_ty = self.infer(&args[0])?;
        // A concrete value routes through its conversion dunder
        // (`Int(x)` → `x.__int__() -> Int`, `Float64`/`Bool` likewise); the
        // same protocol an opaque `T: Intable/Floatable/Boolable` uses.
        let conversion = match target {
            Ty::Int => Some(("__int__", Ty::Int)),
            Ty::Float64 => Some(("__float__", Ty::Float64)),
            Ty::Bool => Some(("__bool__", Ty::Bool)),
            _ => None,
        };
        if let Some((dunder, expected)) = &conversion
            && let Some(result) = self.struct_dunder(&arg_ty, dunder, &[])
        {
            require_dunder_ret(result?, expected, dunder)?;
            return Ok(target);
        }
        let bounded = match target {
            Ty::Int => param_has_bound(&arg_ty, "Intable"),
            Ty::Float64 => param_has_bound(&arg_ty, "Floatable"),
            Ty::Bool => param_has_bound(&arg_ty, "Boolable"),
            _ => false,
        };
        // A width-1 SIMD value is a scalar alias (`UInt8`, `Byte`, ...);
        // Mojo's scalar conversions accept it.
        let simd_scalar = matches!(&arg_ty, Ty::Simd { width: 1, .. });
        if !(is_numeric(&arg_ty) || arg_ty == Ty::Bool || bounded || simd_scalar) {
            return Err(TypeError::TypeMismatch {
                expected: "a numeric or Bool value".to_string(),
                found: arg_ty.to_string(),
                context: format!("argument to '{}'", target),
            });
        }
        Ok(target)
    }

    /// Type the prelude built-in `divmod(a, b)` (`DivModable`) → `Tuple[T, T]`:
    /// two numeric arguments of a common type (like an operator), or two equal
    /// opaque type parameters bounded by `DivModable`.
    pub(super) fn infer_divmod(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        let tys = self.builtin_args("divmod", 2, args)?;
        if let Some(common) = common_numeric(&tys[0], &tys[1]) {
            return Ok(self.public_tuple_type(vec![common.clone(), common]));
        }
        if tys[0] == tys[1] && param_has_bound(&tys[0], "DivModable") {
            return Ok(self.public_tuple_type(vec![tys[0].clone(), tys[0].clone()]));
        }
        Err(TypeError::BadOperator {
            op: "divmod".to_string(),
            operands: format!("{} and {}", tys[0], tys[1]),
        })
    }
}

fn tuple_elements_comparable(elements: &[Ty]) -> bool {
    elements.iter().all(tuple_element_comparable)
}

fn tuple_element_comparable(ty: &Ty) -> bool {
    match ty {
        Ty::Tuple(nested) => tuple_elements_comparable(nested),
        Ty::StringLiteral => true,
        other => is_numeric(other) || has_order_bound(other),
    }
}

fn tuple_order_pair_compatible(left: &Ty, right: &Ty) -> bool {
    if common_numeric(left, right).is_some() {
        return true;
    }
    match (left, right) {
        (Ty::StringLiteral, Ty::StringLiteral) => true,
        (Ty::Tuple(left), Ty::Tuple(right)) => tuple_order_compatible(left, right),
        _ => left == right && has_order_bound(left),
    }
}

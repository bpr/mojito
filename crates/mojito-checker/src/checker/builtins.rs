//! Typing and capability rules for compiler-known builtin types and operations.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::FuncTy;
use mojito_types::types::TyKind;
pub(super) use mojito_types::types::is_nominal_string;
pub use mojito_types::types::{
    builtin_copy_is_value_read, callable_environment_coerces, callable_environment_value_coerces,
    coerces,
};

pub(super) use mojito_types::conformance::{
    builtin_hashable_ty, is_numeric, is_numeric_like, is_scalar, simd_valued_ty,
};
pub(super) use mojito_types::types::default_literal;

/// Upstream's `print` rejection: its variadic `*values` pack element does not
/// conform to `Writable`.
pub(super) fn not_writable(ty: &Ty) -> TypeError {
    TypeError::BadCall {
        func: "print".to_string(),
        reason: format!(
            "an element of 'values' with type '{ty}' does not conform to trait 'Writable'; \
             either prove the conformance with 'conforms_to', or add conformance"
        ),
    }
}

/// The failed `comptime assert` of upstream's `@implicit
/// SIMD.__init__(FloatLiteral)` at the integer `target` a float literal
/// constructs (`Int(3.9)`, `Int8(3.9)`, `SIMD[DType.int, 4](3.9)`).
pub(super) fn float_literal_conversion_error(target: &impl std::fmt::Display) -> TypeError {
    TypeError::BadCall {
        func: target.to_string(),
        reason: format!(
            "constraint failed: {}",
            mojito_types::types::FLOAT_LITERAL_CONVERSION_MESSAGE
        ),
    }
}

/// Whether `print` accepts a value of type `ty` for its keyword `name`, with
/// the type it expects there: `sep`/`end` take a string, `flush` a `Bool`,
/// and `file` the stdlib `RawFileDescriptor`. `None` for any other keyword.
pub(super) fn print_keyword(name: &str, ty: &Ty) -> Option<(bool, &'static str)> {
    match name {
        "sep" | "end" => Some((
            *ty == TyKind::StringLiteral
                || matches!(ty.kind(), TyKind::Struct(name, targs)
                    if (targs.is_empty() && mojito_symbol::symbol::is_stdlib_string_struct(name))
                        || mojito_types::types::is_stdlib_string_span_struct(name)),
            "String",
        )),
        "flush" => Some((*ty == TyKind::Bool, "Bool")),
        "file" => Some((
            matches!(ty.kind(), TyKind::Struct(name, _)
                if mojito_types::types::is_stdlib_file_descriptor_struct(name)),
            "RawFileDescriptor",
        )),
        _ => None,
    }
}

/// The conversion dunder and the bound an opaque argument needs for the
/// conversion built-in `target` (`Int(x)` reads `__int__` and needs
/// `Intable`, `Float64(x)` `__float__` and `Floatable`, `Bool(x)`
/// `__bool__` and `Boolable`).
pub(super) fn conversion_protocol(target: &Ty) -> Option<(&'static str, &'static str)> {
    match target.kind() {
        TyKind::INT => Some(("__int__", "Intable")),
        TyKind::FLOAT64 => Some(("__float__", "Floatable")),
        TyKind::Bool => Some(("__bool__", "Boolable")),
        _ => None,
    }
}

pub(super) fn builtin_trait_operation(trait_name: &str) -> Option<&'static str> {
    match trait_name {
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
/// operand), `is`/`is not` (identity — `__is__`/`__isnot__` struct dunders
/// only, no scalar meaning), and `@`/matmul (struct dunder only).
pub(super) const fn infix_operation_trait(op: mojito_ast::ast::InfixOp) -> Option<&'static str> {
    use mojito_ast::ast::InfixOp::{
        Add, And, BitAnd, BitOr, BitXor, Div, Eq, FloorDiv, Ge, Gt, In, Is, IsNot, Le, Lt, MatMul,
        Mod, Mul, Ne, NotIn, Or, Pow, Shl, Shr, Sub,
    };
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
        MatMul | And | Or | In | NotIn | Is | IsNot => return None,
    })
}

/// The operation trait a prefix operator dispatches through.
pub(super) const fn prefix_operation_trait(op: mojito_ast::ast::PrefixOp) -> &'static str {
    match op {
        mojito_ast::ast::PrefixOp::Neg => "Negatable",
        mojito_ast::ast::PrefixOp::Not => "Boolable",
        mojito_ast::ast::PrefixOp::Invert => "Invertable",
    }
}

/// `SIMD.to_bits`'s default target: the unsigned dtype of the source lane's
/// width (a `bool` lane reads through `uint8`, as upstream casts it first).
pub(super) const fn unsigned_dtype_of_width(bits: u32) -> Dtype {
    match bits {
        16 => Dtype::UInt16,
        32 => Dtype::UInt32,
        64 => Dtype::UInt64,
        _ => Dtype::UInt8,
    }
}

/// [`unsigned_dtype_of_width`] over a symbolic source lane `source`: a `cond`
/// chain testing the lane's identity against each dtype of a width, which
/// folds to the closed target once an instance binds the lane, as upstream's
/// `_unsigned_integral_type_of[dtype]()` default does.
pub(super) fn symbolic_unsigned_dtype_of(
    context: &ParamContext,
    source: &ParamExpr,
) -> Result<ParamExpr, ParamError> {
    let dtype = |dtype: Dtype| context.constant(CtValue::Dtype(dtype));
    let mut target = dtype(Dtype::UInt64)?;
    for unsigned in [Dtype::UInt32, Dtype::UInt16, Dtype::UInt8] {
        let tests = Dtype::ALL
            .into_iter()
            .filter(|lane| unsigned_dtype_of_width(lane.bit_width()) == unsigned)
            .map(|lane| Ok(context.identical(source, &dtype(lane)?)))
            .collect::<Result<Vec<_>, ParamError>>()?;
        let same_width = context.op(ParamOp::BoolOr, &tests)?;
        target = context.op(ParamOp::Cond, &[same_width, dtype(unsigned)?, target])?;
    }
    Ok(target)
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

/// Whether a value of type `ty` can be `print`ed (has a user-facing display).
/// Functions, ranges, and opaque type parameters are not printable.
pub(super) fn is_printable(ty: &Ty) -> bool {
    match ty.kind() {
        TyKind::INT
        | TyKind::UINT
        | TyKind::Bool
        | TyKind::StringLiteral
        | TyKind::FLOAT64
        | TyKind::None
        | TyKind::IntLiteral
        | TyKind::FloatLiteral
        | TyKind::Struct(_, _)
        | TyKind::Simd { .. }
        | TyKind::Dtype
        | TyKind::Error
        | TyKind::ComptimeList(_) => true,
        // A tuple prints if every element prints.
        TyKind::Tuple(elems) => elems.iter().all(is_printable),
        _ => false,
    }
}

/// Whether a value of `from`, a closure's storage, binds `to` as the
/// closure of its body: `to` is a callable value of another environment
/// (a run-time `def(...) capturing[...]` contract).
pub(super) fn closure_converts_to_callable(from: &Ty, to: &Ty) -> bool {
    let storage = |ty: &Ty| {
        matches!(
            ty.as_func().map(|func| &func.environment),
            Some(mojito_types::origin::CallableEnvironment::Storage(_))
        )
    };
    storage(from) && to.as_func().is_some() && !storage(to)
}

/// True when `from` fails to bind to `to` solely because a capturing
/// environment meets an unqualified `def(...)` value contract — the shape that
/// deserves the "spell `capturing[...]`" migration hint.
pub(super) fn callable_mismatch_is_environment_only(from: &Ty, to: &Ty) -> bool {
    let (
        Some(FuncTy {
            environment: from_environment,
            ..
        }),
        Some(FuncTy {
            environment: to_environment,
            ..
        }),
    ) = (from.as_func(), to.as_func())
    else {
        return false;
    };
    if callable_environment_value_coerces(from_environment, to_environment)
        || !callable_environment_coerces(from_environment, to_environment)
    {
        return false;
    }
    let mut aligned = from.clone();
    aligned.update(|kind| {
        if let Some(FuncTy { environment, .. }) = kind.as_func_mut() {
            *environment = to_environment.clone();
        }
    });
    coerces(&aligned, to)
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

/// The common element of a display's two elements ([`common_elem`]), where a
/// string literal beside the nominal `String` joins it, and two aggregates
/// that differ only in which elements are still literals join their
/// materialized form (`[(1, 2), (3, n)]` is a list of `Tuple[Int, Int]`).
pub(super) fn common_display_element(a: &Ty, b: &Ty) -> Option<Ty> {
    match (a.kind(), b.kind()) {
        (TyKind::StringLiteral, _) if is_nominal_string(b) => Some(b.clone()),
        (_, TyKind::StringLiteral) if is_nominal_string(a) => Some(a.clone()),
        (TyKind::Struct(..), TyKind::Struct(..)) if a != b => {
            let materialized = mojito_types::types::default_literal(a);
            (materialized == mojito_types::types::default_literal(b)).then_some(materialized)
        }
        _ => common_elem(a, b),
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
    /// Whether `ty` is an opaque type parameter conforming to the named trait
    /// `bound`, by a bound that is or refines it, or an assumed conformance.
    /// The numeric-operation traits (`Absable`/`Roundable`/`Powable`/
    /// `Intable`/`Floatable`/`Boolable`/`DivModable`) gate a corresponding
    /// built-in or operator on an opaque `T` this way: the concrete type's
    /// implementation runs after type erasure.
    pub(super) fn param_has_bound(&self, ty: &Ty, bound: &str) -> bool {
        matches!(ty.kind(), TyKind::Param { .. }) && self.conforms_to(ty, bound)
    }

    /// Whether the type parameter `ty` is `Equatable` by its bounds or an
    /// assumed conformance, so `==` and `!=` call its requirements.
    pub(super) fn equatable_bound(&self, ty: &Ty) -> bool {
        matches!(ty.kind(), TyKind::Param { .. }) && self.conforms_to(ty, "Equatable")
    }

    /// Whether list elements of type `ty` can be compared for equality
    /// (needed by `List.remove`/`count`/`index`) — the same scalar set
    /// `==`/`!=` accept.
    pub(super) fn is_list_equatable(&self, ty: &Ty) -> bool {
        is_numeric(ty)
            || matches!(
                ty.kind(),
                TyKind::Bool | TyKind::StringLiteral | TyKind::None | TyKind::Dtype
            )
            || self.equatable_bound(ty)
    }

    /// Whether every element in a tuple supports equality. Tuples recurse so
    /// nested tuple comparisons and membership stay structural without making
    /// `List` equality part of this compiler-known subset.
    pub(super) fn tuple_elements_equatable(&self, elements: &[Ty]) -> bool {
        elements.iter().all(|ty| match ty.kind() {
            TyKind::Tuple(nested) => self.tuple_elements_equatable(nested),
            _ => self.is_list_equatable(ty),
        })
    }

    /// Type `print(...)`. Intrinsic scalars have builtin writing; nominal values,
    /// including public collections, opt into current `Writable`.
    /// Type the built-in `print(*args, sep=, end=, flush=, file=)`: every
    /// positional argument is printable; `sep`/`end` are strings, `flush` a
    /// `Bool`, and `file` the stdlib `RawFileDescriptor`.
    pub(super) fn infer_print(
        &self,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        for keyword in kwargs {
            let ty = self.infer(&keyword.value)?;
            self.borrow_reference_result_argument(&keyword.value);
            self.borrow_nominal_place_argument(&keyword.value, &ty);
            self.borrow_storage_element_argument(&keyword.value);
            let (accepted, expected) =
                print_keyword(&keyword.name, &ty).ok_or_else(|| TypeError::BadCall {
                    func: "print".to_string(),
                    reason: format!("unexpected keyword argument '{}'", keyword.name),
                })?;
            if !accepted {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: ty.to_string(),
                    context: format!("'{}' argument to 'print'", keyword.name),
                });
            }
        }
        let forwarded_pack = self.forwarded_pack_argument("print", args, true)?;
        for (position, arg) in args.iter().enumerate() {
            // A forwarded pack prints element by element, so its bound must
            // give every element `Writable`.
            if let Some((spread, forwarded)) = &forwarded_pack
                && *spread == position
            {
                if !self.conforms_to(&forwarded.pack, "Writable") {
                    return Err(not_writable(&forwarded.pack));
                }
                continue;
            }
            let ty = self.infer(arg)?;
            self.borrow_reference_result_argument(arg);
            self.borrow_nominal_place_argument(arg, &ty);
            self.borrow_parameter_place_argument(arg, &ty);
            self.borrow_storage_element_argument(arg);
            // `print` writes through `Writable`, taking no ownership: it
            // reads a temporary the caller destroys once it returns.
            self.record_read_temporary_argument(arg);
            let runtime_ty = default_literal(&ty);
            if runtime_ty != ty {
                self.record_literal_materializations(arg, &ty, &runtime_ty)?;
            }
            if !self.printable_argument(&ty) {
                return Err(not_writable(&ty));
            }
        }
        Ok(Ty::none())
    }

    /// Whether a value of type `ty` may be handed to `print` or a `Writer`: a
    /// printable built-in, a built-in aggregate of printable elements, or a
    /// struct or parameter that is `Writable`.
    pub(super) fn printable_argument(&self, ty: &Ty) -> bool {
        if let TyKind::Struct(name, _) = ty.kind()
            && !self.structs.contains_key(name)
            && (list_element(ty).is_some_and(is_printable)
                || set_element(ty).is_some_and(is_printable)
                || dict_elements(ty)
                    .is_some_and(|(key, value)| is_printable(key) && is_printable(value))
                || tuple_elements(ty)
                    .is_some_and(|elements| elements.into_iter().all(is_printable)))
        {
            return true;
        }
        if matches!(
            ty.kind(),
            TyKind::Struct(..) | TyKind::Param { .. } | TyKind::Assoc { .. }
        ) || self.opaque_element(ty).is_some()
        {
            return self.conforms_to(ty, "Writable");
        }
        is_printable(ty)
    }

    /// Type the built-in `input(prompt)`: the prompt is a compile-time or
    /// nominal `String`, the line read from standard input materializes as
    /// the nominal `String`, and the call raises `Error` at end of input.
    pub(super) fn infer_input(&self, span: SourceSpan, args: &[Expr]) -> Result<Ty, TypeError> {
        let tys = self.builtin_args("input", 1, args)?;
        let nominal_prompt = matches!((tys[0]).kind(), TyKind::Struct(name, args)
            if args.is_empty() && mojito_symbol::symbol::is_stdlib_string_struct(name));
        if tys[0] == TyKind::StringLiteral || nominal_prompt {
            self.record_call_effect(span, Ty::error());
            self.require_error("call to raising function 'input'", Ty::error())?;
            Ok(self.builtin_string_result())
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
        // A builtin reads an argument's capabilities off its bounds, which an
        // element of an unbound pack has through its view.
        args.iter()
            .map(|a| {
                let ty = self.infer(a)?;
                self.borrow_reference_result_argument(a);
                Ok(self.opaque_element(&ty).unwrap_or(ty))
            })
            .collect()
    }

    /// A builtin reads its arguments; one that is a reference result
    /// (`len(v[List[Int]])` on the self-hosted `Variant`'s projection) is
    /// read through the retained handle, not copied out.
    pub(super) fn borrow_reference_result_argument(&self, argument: &Expr) {
        if self.infer_reference_value(argument).is_some() {
            self.record_at(argument.source_span())
                .borrowed_read_call_place = true;
        }
    }

    /// A read-only builtin (`len`, `print`) over a named nominal place reads
    /// it in place, as a `read` parameter would, rather than taking the
    /// lifecycle copy: `len(xs)` never copies the list (and never reads
    /// slots an `unsafe_uninit_length` construction left unwritten), and
    /// `print(e)` never copies the `Error`, upstream's library struct.
    pub(super) fn borrow_nominal_place_argument(&self, argument: &Expr, ty: &Ty) {
        let nominal = match ty.kind() {
            TyKind::Struct(name, _) => self.structs.contains_key(name),
            TyKind::Error => true,
            _ => false,
        };
        if matches!(argument.kind, ExprKind::Identifier(_)) && nominal {
            self.record_at(argument.source_span())
                .borrowed_read_call_place = true;
        }
    }

    /// A read dunder reached through a bound reads a named operand of a
    /// parameter type or a pack element (`Ts[i]`) where it lies, as
    /// [`Self::borrow_nominal_place_argument`] reads a struct operand: the
    /// instance's dunder takes it by `read`. An element an accessor returns
    /// by reference (`bag.storage[i]`) is read through that reference
    /// likewise.
    pub(super) fn borrow_parameter_place_argument(&self, argument: &Expr, ty: &Ty) {
        let named = matches!(argument.kind, ExprKind::Identifier(_));
        let element = matches!(argument.kind, ExprKind::Index { .. })
            && self.infer_reference_value(argument).is_some();
        if (named || element) && matches!(ty.kind(), TyKind::Param { .. } | TyKind::Dependent(_)) {
            self.record_at(argument.source_span())
                .borrowed_read_call_place = true;
        }
    }

    /// `len` over a named tuple or pack reads the storage where it lies, as
    /// upstream's `len[T: Sized](value: T)` takes it by `read`: taking the
    /// length copies no element.
    pub(super) fn borrow_storage_place_argument(&self, argument: &Expr, ty: &Ty) {
        if matches!(argument.kind, ExprKind::Identifier(_))
            && matches!(
                ty.kind(),
                TyKind::Tuple(_) | TyKind::RuntimePack(_) | TyKind::VariadicPack(_)
            )
        {
            self.record_at(argument.source_span())
                .borrowed_read_call_place = true;
        }
    }

    /// A read-only builtin (`print`) over an element of a named tuple or
    /// pack reads the element where it lies, as it reads a field.
    pub(super) fn borrow_storage_element_argument(&self, argument: &Expr) {
        let ExprKind::Index { object, .. } = &argument.kind else {
            return;
        };
        let storage = matches!(object.kind, ExprKind::Identifier(_))
            && matches!(
                (self
                    .node_facts
                    .borrow()
                    .get(&object.source_span())
                    .and_then(|facts| facts.ty.as_ref()))
                .map(Ty::kind),
                Some(TyKind::Tuple(_) | TyKind::RuntimePack(_) | TyKind::VariadicPack(_))
            );
        if storage {
            self.record_at(argument.source_span())
                .borrowed_read_call_place = true;
        }
    }

    /// Type `String(x)`: stringify a numeric, `Bool`, or `String` value.
    pub(super) fn infer_stringify(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        let tys = self.builtin_args("String", 1, args)?;
        if is_numeric(&tys[0]) || tys[0] == TyKind::Bool || tys[0] == TyKind::StringLiteral {
            let runtime_ty = default_literal(&tys[0]);
            if runtime_ty != tys[0] {
                self.record_literal_materializations(&args[0], &tys[0], &runtime_ty)?;
            }
            return Ok(Ty::string_literal());
        }
        if self.conforms_to(&tys[0], "Writable") {
            // Like `print`, nominal String conversion formats through a
            // borrowed `Writable` receiver and must retain its caller storage
            // until that synchronous formatter returns.
            self.record_at(args[0].source_span()).call_place_use = true;
            return Ok(Ty::string_literal());
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
        if is_numeric(&tys[0]) || self.param_has_bound(&tys[0], "Absable") {
            Ok(tys[0].clone())
        } else if let Some(result) = self.struct_operation(&tys[0], "Absable", "__abs__", &[]) {
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
        if matches!((tys[0]).kind(), TyKind::FLOAT64 | TyKind::FloatLiteral) {
            Ok(Ty::float64())
        } else if self.param_has_bound(&tys[0], "Roundable") {
            Ok(tys[0].clone())
        } else if let Some(result) = self.struct_operation(&tys[0], "Roundable", "__round__", &[]) {
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
        if let TyKind::Dependent(dependent) = ty.kind()
            && let Some((elements, _)) = dependent.selection()
        {
            for element in elements {
                match self.len_result_for_type(element)?.as_ref().map(Ty::kind) {
                    Some(TyKind::INT) => {}
                    _ => return Ok(None),
                }
            }
            return Ok(Some(Ty::int()));
        }
        // `len(c)` on a struct dispatches to `c.__len__()` (`Sized`), which
        // must return `Int`: a vector's or scalar's is `SIMD`'s, and a literal
        // argument materializes to one first.
        if let Some(result) = self.struct_dunder(&default_literal(ty), "__len__", &[]) {
            return result
                .and_then(|ret| require_dunder_ret(ret, &Ty::int(), "__len__"))
                .map(Some);
        }
        // A pack's storage answers `Sized` where it stands.
        if mojito_types::conformance::modelled_declares(
            &default_literal(ty),
            "Sized",
            &mut |_, _| true,
        ) {
            return Ok(Some(Ty::int()));
        }
        if let TyKind::Struct(name, _) = ty.kind()
            && !self.structs.contains_key(name)
            && (list_element(ty).is_some()
                || set_element(ty).is_some()
                || dict_elements(ty).is_some()
                || tuple_elements(ty).is_some()
                || mojito_types::types::is_range_type(ty)
                || mojito_types::types::scalar_range_parts(ty).is_some())
        {
            return Ok(Some(Ty::int()));
        }
        // `len(x)` on an opaque type parameter is permitted when it conforms
        // to either of upstream's `len` overload bounds, `Sized` or
        // `SizedRaising` — the concrete type's `__len__` runs after
        // elaboration.
        if matches!(ty.kind(), TyKind::Param { .. })
            && (self.conforms_to(ty, "Sized") || self.conforms_to(ty, "SizedRaising"))
        {
            return Ok(Some(Ty::int()));
        }
        Ok(None)
    }

    /// Type `len(x)` for a program linked without the prelude, as a phase
    /// test links, where every possible type of a dependent input must
    /// fulfill the same `Sized`/`__len__ -> Int` contract the library `len`
    /// asks.
    pub(super) fn infer_len(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        let tys = self.builtin_args("len", 1, args)?;
        self.borrow_nominal_place_argument(&args[0], &tys[0]);
        self.borrow_storage_place_argument(&args[0], &tys[0]);
        if let Some(result) = self.len_result_for_type(&tys[0])? {
            return Ok(result);
        }
        Err(TypeError::NoMatchingFunction {
            name: "len".to_string(),
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
            if !coerces(&arg_ty, &Ty::int()) {
                return Err(TypeError::TypeMismatch {
                    expected: "Int".to_string(),
                    found: arg_ty.to_string(),
                    context: format!("argument {} to 'range'", i + 1),
                });
            }
            self.record_literal_materializations(arg, &arg_ty, &Ty::int())?;
        }
        Ok(range_type())
    }

    /// Scalar `range` inference for the dtype-inferred family. Upstream's
    /// `range[dtype: DType, //](...)` overloads are infer-only — there is no
    /// explicit-argument spelling — so the linked Int overload set cannot
    /// host them as source defs. Called only after ordinary overload
    /// selection found no match; returns `None` when no argument names a
    /// concrete non-Int scalar, so the ordinary no-match error stands. On
    /// success the call is spelled as the construction of the linked range
    /// struct at the dtype (`SemanticAdjustment::SpelledConstruction`), typed
    /// as any construction is, and the checked arena hands HIR that
    /// construction in the call's place.
    pub(super) fn infer_scalar_range(
        &self,
        span: &mojito_common::token::SourceSpan,
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
            let this = match ty.kind() {
                TyKind::FLOAT64 => {
                    triggered = true;
                    Some(Dtype::Float64)
                }
                TyKind::INT => Some(Dtype::Int),
                TyKind::UINT => Some(Dtype::UInt),
                TyKind::IntLiteral | TyKind::FloatLiteral => None,
                // Not a scalar-range shape at all (a symbolic lane included);
                // the ordinary no-matching-overload diagnostic stands.
                _ => match scalar_simd_dtype(ty) {
                    Some(dtype) => {
                        triggered = true;
                        Some(dtype)
                    }
                    None => return Ok(None),
                },
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
        if dtype.is_float() && args.len() != 3 {
            return Err(TypeError::Unsupported(
                "a floating-point range requires an explicit step; use range(start, end, step)"
                    .to_string(),
            ));
        }
        let lane = simd_ty(dtype, 1);
        for ty in &tys {
            if !splats_to(ty, &SimdDtype::Known(dtype)) {
                return Err(TypeError::TypeMismatch {
                    expected: lane.to_string(),
                    found: ty.to_string(),
                    context: "range argument".to_string(),
                });
            }
        }
        let family = if dtype.is_float() {
            mojito_types::types::FLOAT_STRIDED_RANGE
        } else {
            mojito_types::types::SCALAR_RANGE_FAMILY[args.len() - 1]
        };
        // The pin's overloads return the family struct at the inferred
        // dtype: the call is spelled as that struct's construction, under
        // the linked (module-qualified) name the checker registered.
        let linked = self
            .structs
            .keys()
            .filter(|name| *name == family || name.ends_with(&format!("${family}")))
            .min()
            .map(str::to_string)
            .ok_or_else(|| {
                TypeError::InvariantViolation(format!(
                    "scalar range family '{family}' is not linked"
                ))
            })?;
        let syntax = span.syntax.ok_or_else(|| {
            TypeError::InvariantViolation("scalar range call has no syntax identity".to_string())
        })?;
        let sugar = Expr {
            kind: ExprKind::Call {
                name: "range".to_string(),
                param_args: Vec::new(),
                args: args.to_vec(),
                kwargs: Vec::new(),
            },
            span: span.span,
            source: span.source.clone(),
            syntax_id: syntax,
        };
        let dtype_argument = CtValue::Dtype(dtype)
            .materialize(span.span)
            .ok_or_else(|| {
                TypeError::InvariantViolation(format!(
                    "range dtype '{}' has no spelling",
                    dtype.name()
                ))
            })?;
        let construction = super::initializer_list::derived_construction(
            &sugar,
            ExprKind::Call {
                name: linked,
                param_args: vec![mojito_ast::ast::ParamArg::Value(dtype_argument)],
                args: args.to_vec(),
                kwargs: Vec::new(),
            },
        );
        let ty = self.infer(&construction)?;
        self.record_spelled_construction(&sugar, construction, &ty);
        Ok(Some(ty))
    }

    /// Whether a `SIMDLength` argument constructs the vector `target`
    /// (`Int(w)`, `Scalar[dt](w)`, `SIMD[dt, n](w)`): that is upstream's
    /// `@implicit SIMD.__init__(SIMDLength)` called by name, so the call is
    /// its argument's conversion.
    pub(super) fn simd_length_construction(
        &self,
        span: &SourceSpan,
        target: &Ty,
        arg: &Expr,
        arg_ty: &Ty,
    ) -> Result<bool, TypeError> {
        if !mojito_types::types::is_simd_length(arg_ty)
            || !self.record_implicit_conversion(arg, arg_ty, target)?
        {
            return Ok(false);
        }
        self.record_at(span.clone()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::ConvertedArgument,
        ));
        Ok(true)
    }

    /// Type a conversion built-in `Int(x)` / `UInt(x)` / `Float64(x)` / `Bool(x)`:
    /// exactly one argument of a numeric or `Bool` type, producing `target`,
    /// recorded as the `ScalarCast` upstream's inlined `pop.cast` is. An
    /// opaque type parameter is also accepted when its bound promises the
    /// conversion — `Int(x)` on `T: Intable`, `Float64(x)` on `T: Floatable`,
    /// `Bool(x)` on `T: Boolable` (`__int__`/`__float__`/`__bool__` run after
    /// type erasure).
    pub(super) fn infer_conversion(
        &self,
        span: &SourceSpan,
        target: Ty,
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        // The scalar built-ins are `Defaultable`: the nullary form is zero.
        if args.is_empty() {
            return Ok(target);
        }
        if args.len() != 1 {
            return Err(TypeError::ArityMismatch {
                name: target.to_string(),
                expected: 1,
                got: args.len(),
            });
        }
        let arg_ty = self.infer(&args[0])?;
        let arg_ty = self.opaque_element(&arg_ty).unwrap_or(arg_ty);
        if self.simd_length_construction(span, &target, &args[0], &arg_ty)? {
            return Ok(target);
        }
        // A literal argument is converted where the call is built, as
        // upstream's builtin-inline `SIMD.__init__(IntLiteral)` is
        // `pop.int_literal_convert`.
        if matches!(
            (arg_ty.kind(), target.kind()),
            (
                TyKind::IntLiteral,
                TyKind::INT | TyKind::UINT | TyKind::FLOAT64
            ) | (TyKind::FloatLiteral, TyKind::FLOAT64)
        ) {
            self.record_literal_materializations(&args[0], &arg_ty, &target)?;
            self.record_at(span.clone()).adjustment = Some(Box::new(
                mojito_checked::checked::SemanticAdjustment::ConvertedArgument,
            ));
            return Ok(target);
        }
        // Upstream's exact-type `@implicit SIMD.__init__(FloatLiteral)`
        // outranks the generic `Intable` overload, and its `comptime assert`
        // fails for an integer dtype. A generic `Int(x)` instantiated at a
        // `FloatLiteral` still binds `Intable` inside its body.
        if *arg_ty == TyKind::FloatLiteral && matches!(target.kind(), TyKind::INT | TyKind::UINT) {
            return Err(float_literal_conversion_error(&target));
        }
        // A scalar operand is cast; `Int(pointer)` is the address conversion
        // (upstream `Pointer.__int__`), a null pointer converting to 0, which
        // is how the libc-facing stdlib tests an optional result.
        let cast = mojito_types::ct::ScalarConversion::to(&target)
            .filter(|conversion| conversion.casts_from(&arg_ty));
        if let Some(cast) = cast {
            self.record_at(span.clone()).adjustment = Some(Box::new(
                mojito_checked::checked::SemanticAdjustment::ScalarCast(cast),
            ));
            return Ok(target);
        }
        // A concrete value routes through its conversion dunder
        // (`Int(x)` → `x.__int__() -> Int`, `Float64`/`Bool` likewise); the
        // same protocol an opaque `T: Intable/Floatable/Boolable` uses.
        let conversion = conversion_protocol(&target);
        if let Some((dunder, bound)) = conversion
            && let Some(result) = self.struct_dunder(&arg_ty, dunder, &[])
        {
            require_dunder_ret(result?, &target, dunder)?;
            // Upstream's overload is `__init__[T: Boolable]` (or `Intable`,
            // `Floatable`), so the struct must declare the trait; a raising
            // dunder answers to the `*Raising` sibling, not modelled here.
            let raises = self
                .struct_dunder_signature_for(&arg_ty, dunder, &[])
                .is_some_and(|(_, sig, _)| sig.raises);
            if !raises && !self.conforms_to(&arg_ty, bound) {
                return Err(TypeError::TraitNotSatisfied {
                    param: "T".to_string(),
                    ty: arg_ty.to_string(),
                    trait_name: bound.to_string(),
                    reason: None,
                });
            }
            self.borrow_nominal_place_argument(&args[0], &arg_ty);
            self.record_read_temporary_argument(&args[0]);
            // A raising conversion dunder (`String.__int__() raises`) makes
            // the conversion a raising call.
            if let Some((_, sig, _)) = self.struct_dunder_signature_for(&arg_ty, dunder, &[])
                && sig.raises
            {
                let error = sig.error.clone().unwrap_or_else(Ty::error);
                self.record_call_effect(span.clone(), error.clone());
                self.require_error(format!("call to raising conversion '{target}'"), error)?;
            }
            return Ok(target);
        }
        if !conversion.is_some_and(|(_, bound)| self.param_has_bound(&arg_ty, bound)) {
            return Err(TypeError::TypeMismatch {
                expected: "a numeric or Bool value".to_string(),
                found: arg_ty.to_string(),
                context: format!("argument to '{target}'"),
            });
        }
        self.borrow_parameter_place_argument(&args[0], &arg_ty);
        self.record_read_temporary_argument(&args[0]);
        Ok(target)
    }

    /// Type the prelude built-in `divmod(a, b)` (`DivModable`) → `Tuple[T, T]`:
    /// two numeric arguments of a common type (like an operator), or two equal
    /// opaque type parameters bounded by `DivModable`.
    pub(super) fn infer_divmod(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        let tys = self.builtin_args("divmod", 2, args)?;
        if let Some(common) = common_numeric(&tys[0], &tys[1]) {
            return Ok(mojito_types::types::tuple_type(vec![
                common.clone(),
                common,
            ]));
        }
        if tys[0] == tys[1] && self.param_has_bound(&tys[0], &super::math_trait("DivModable")) {
            return Ok(mojito_types::types::tuple_type(vec![
                tys[0].clone(),
                tys[0].clone(),
            ]));
        }
        // A concrete struct routes through `__divmod__(self, Self)`.
        if let Some(result) = self.struct_operation(
            &tys[0],
            &super::math_trait("DivModable"),
            "__divmod__",
            &[&tys[1]],
        ) {
            return result;
        }
        Err(TypeError::BadOperator {
            op: "divmod".to_string(),
            operands: format!("{} and {}", tys[0], tys[1]),
        })
    }

    /// The result of the `std.math` operation `method` on a concrete struct
    /// `ty`, which upstream's library function reaches only through its
    /// `trait_name` bound. `None` for a type that is no struct.
    fn struct_operation(
        &self,
        ty: &Ty,
        trait_name: &str,
        method: &str,
        args: &[&Ty],
    ) -> Option<Result<Ty, TypeError>> {
        if !matches!(ty.kind(), TyKind::Struct(..)) {
            return None;
        }
        if !self.conforms_to(ty, trait_name) {
            return Some(Err(TypeError::TraitNotSatisfied {
                param: "T".to_string(),
                ty: ty.to_string(),
                trait_name: trait_name.to_string(),
                reason: self.trait_failure_reason(ty, trait_name),
            }));
        }
        self.struct_dunder(ty, method, args)
    }
}

//! Typing and capability rules for compiler-known builtin types and operations.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::types::TyKind;
pub use mojito_types::types::{builtin_copy_is_value_read, callable_environment_coerces, coerces};

pub(super) use mojito_types::conformance::{
    builtin_hashable_ty, is_numeric, is_scalar, simd_valued_ty,
};
pub(super) use mojito_types::types::default_literal;

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
        kind if kind.is_simd() => true,
        TyKind::None
        | TyKind::Struct(_, _)
        | TyKind::Dtype
        | TyKind::Error
        | TyKind::ComptimeList(_) => true,
        // A tuple prints if every element prints.
        TyKind::Tuple(elems) => elems.iter().all(is_printable),
        _ => false,
    }
}

/// The common type of two numeric operands, coercing literals as needed, or
/// `None` if they can't be unified (e.g. two different concrete types).
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
/// (`print`, `len`, `range`, conversions, …). Moved from `checker.rs`.
impl Checker {
    /// Whether `ty` is an opaque type parameter conforming to the named trait
    /// `bound`, by a bound that is or refines it, or an assumed conformance.
    /// The numeric-operation traits (`Absable`/`Roundable`/
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
            || ty.is_bool()
            || ty.is_string_literal()
            || matches!(ty.kind(), TyKind::None | TyKind::Dtype)
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

    /// Type the receiver of an inverted write (`x.write_to(writer)` over a
    /// receiver with no `write_to` body): a read of a `Writable` value.
    pub(super) fn infer_written_operand(&self, operand: &Expr) -> Result<(), TypeError> {
        let ty = self.infer(operand)?;
        self.bind_read_builtin_argument(operand, &ty);
        self.borrow_storage_element_argument(operand);
        let runtime_ty = default_literal(&ty);
        if runtime_ty != ty {
            self.record_literal_materializations(operand, &ty, &runtime_ty)?;
        }
        if !self.printable_argument(&ty) {
            return Err(TypeError::TraitNotSatisfied {
                param: "Self".to_string(),
                ty: ty.to_string(),
                trait_name: "Writable".to_string(),
                reason: None,
            });
        }
        Ok(())
    }

    /// Whether a value of type `ty` may be handed to a `Writer`: a
    /// printable built-in, a built-in aggregate of printable elements, or a
    /// struct or parameter that is `Writable`.
    pub(super) fn printable_argument(&self, ty: &Ty) -> bool {
        if let Some((name, _)) = ty.kind().as_non_simd_struct()
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
        if ty.is_non_simd_struct()
            || matches!(ty.kind(), TyKind::Param { .. } | TyKind::Assoc { .. })
            || self.opaque_element(ty).is_some()
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
        self.bind_read_builtin_argument(&args[0], &tys[0]);
        let nominal_prompt = (tys[0])
            .kind()
            .as_non_simd_struct()
            .is_some_and(|(name, args)| {
                args.is_empty() && mojito_symbol::symbol::is_stdlib_string_struct(name)
            });
        if tys[0].is_string_literal() || nominal_prompt {
            self.record_call_effect(span, Ty::error());
            self.require_error("call to raising function 'input'", Ty::error())?;
            self.builtin_string_result("input")
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

    /// Bind `argument`, of type `ty`, as the `read` parameter of upstream's
    /// library function a builtin stands for (`print`, `input`, `abort`,
    /// `Error`) binds it: a named place or a reference result is read where
    /// it lies, and an owned temporary is read in its call-result slot,
    /// which the caller destroys once the call returns.
    pub(super) fn bind_read_builtin_argument(&self, argument: &Expr, ty: &Ty) {
        self.borrow_reference_result_argument(argument);
        self.borrow_nominal_place_argument(argument, ty);
        self.borrow_parameter_place_argument(argument, ty);
        self.record_read_temporary_argument(argument);
    }

    /// A read-only builtin (`len`, `print`) over a named nominal place reads
    /// it in place, as a `read` parameter would, rather than taking the
    /// lifecycle copy: `len(xs)` never copies the list (and never reads
    /// slots an `unsafe_uninit_length` construction left unwritten), and
    /// `print(e)` never copies the `Error`, upstream's library struct.
    pub(super) fn borrow_nominal_place_argument(&self, argument: &Expr, ty: &Ty) {
        let nominal = match ty.kind() {
            struct_ty if let Some((name, _)) = struct_ty.as_non_simd_struct() => {
                self.structs.contains_key(name)
            }
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

    /// Type `abs(x)`: a numeric argument, returning the same numeric type.
    /// A call reaches this rule only where no `abs` binding exists — a
    /// module loaded while the prelude bootstraps, or a program linked
    /// without the prelude; elsewhere it calls `std.math.abs`.
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
    /// operator (no concrete-type mixing), returning their common type. A
    /// call reaches this rule only where no `max`/`min` binding exists — a
    /// module loaded while the prelude bootstraps, or a program linked
    /// without the prelude; elsewhere it calls `std.math`'s overloads.
    /// Two literals of different values materialize, to `Float64` beside
    /// a float and else `Int`, the result of the `std.math` overload the
    /// rule stands in for.
    pub(super) fn infer_min_max(&self, name: &str, args: &[Expr]) -> Result<Ty, TypeError> {
        let tys = self.builtin_args(name, 2, args)?;
        if tys[0] != tys[1] && tys.iter().all(|ty| ty.kind().is_numeric_literal()) {
            let target = if tys.iter().any(Ty::is_float_literal) {
                Ty::float64()
            } else {
                Ty::int()
            };
            for (arg, ty) in args.iter().zip(&tys) {
                self.record_literal_materializations(arg, ty, &target)?;
            }
            return Ok(target);
        }
        let result = common_numeric(&tys[0], &tys[1]).ok_or_else(|| TypeError::BadOperator {
            op: name.to_string(),
            operands: format!("{} and {}", tys[0], tys[1]),
        })?;
        for (arg, ty) in args.iter().zip(&tys) {
            if ty.kind().is_numeric_literal() && *ty != result {
                self.record_literal_materializations(arg, ty, &result)?;
            }
        }
        Ok(result)
    }

    pub(super) fn len_result_for_type(&self, ty: &Ty) -> Result<Option<Ty>, TypeError> {
        if let TyKind::Dependent(dependent) = ty.kind()
            && let Some((elements, _)) = dependent.selection()
        {
            for element in elements {
                match self.len_result_for_type(element)?.as_ref().map(Ty::kind) {
                    Some(kind) if kind.is_int() => {}
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
        if let Some((name, _)) = ty.kind().as_non_simd_struct()
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
                kind if kind.is_float64() => {
                    triggered = true;
                    Some(Dtype::Float64)
                }
                kind if kind.is_int() => Some(Dtype::Int),
                kind if kind.is_uint() => Some(Dtype::UInt),
                kind if kind.is_numeric_literal() => None,
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
        if !ty.kind().is_non_simd_struct() {
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

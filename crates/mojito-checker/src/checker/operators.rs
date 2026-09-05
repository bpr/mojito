//! Operator, SIMD-construction, and pointer-construction type inference
//! for the checker. Extracted from `checker.rs`; see `docs/symbol-map.md`.

use super::*;

impl Checker {
    pub(super) fn infer_prefix(&self, op: PrefixOp, operand: &Expr) -> Result<Ty, TypeError> {
        let t = self.infer(operand)?;
        match (op, &t) {
            // Negation preserves the (possibly literal) numeric type, except UInt.
            (PrefixOp::Neg, Ty::Int | Ty::Float64 | Ty::IntLiteral | Ty::FloatLiteral) => {
                return Ok(t);
            }
            // Elementwise negation on numeric SIMD lanes preserves the type;
            // a bool mask does not negate.
            (PrefixOp::Neg, Ty::Simd { dtype, width }) if *dtype != Dtype::Bool => {
                return Ok(simd_ty(*dtype, *width));
            }
            (PrefixOp::Not, Ty::Bool) => return Ok(Ty::Bool),
            // Bitwise inversion keeps an integer (or `Bool`) type; float
            // operands and masks have no inversion.
            (PrefixOp::Invert, Ty::Int | Ty::UInt | Ty::IntLiteral | Ty::Bool) => {
                return Ok(t);
            }
            (PrefixOp::Invert, Ty::Simd { dtype, width }) if !dtype.is_float() => {
                return Ok(simd_ty(*dtype, *width));
            }
            _ => {}
        }
        // An opaque type parameter bounded by the prefix operator's trait
        // dispatches after erasure (`-x` needs `Negatable`, `not x` needs
        // `Boolable`); the concrete impl runs on the erased type.
        if param_has_bound(&t, prefix_operation_trait(op)) {
            return Ok(match op {
                PrefixOp::Neg | PrefixOp::Invert => t,
                PrefixOp::Not => Ty::Bool,
            });
        }
        // A user struct routes through the operator's dunder (`-x` →
        // `x.__neg__() -> Self`, `not x` → `not x.__bool__() -> Bool`).
        if let Some(result) = self.struct_dunder(&t, op.dunder(), &[]) {
            let ret = result?;
            return match op {
                PrefixOp::Neg | PrefixOp::Invert => Ok(ret),
                PrefixOp::Not => require_dunder_ret(ret, &Ty::Bool, "__bool__"),
            };
        }
        Err(TypeError::BadOperator {
            op: prefix_symbol(op).to_string(),
            operands: t.to_string(),
        })
    }

    /// Whether every element of a public Tuple supports `==`/`!=`: scalars
    /// and bounded parameters as before, nested tuples recursively, and user
    /// structs with `__eq__` (the specialization's `__eq__`/`__ne__` run at
    /// the operator).
    pub(in crate::checker) fn tuple_elements_equatable_nominal(&self, elements: &[Ty]) -> bool {
        elements.iter().all(|element| match element {
            Ty::Tuple(nested) => self.tuple_elements_equatable_nominal(nested),
            other => has_equality_bound_or_concrete(self, other),
        })
    }

    pub(super) fn infer_infix(
        &self,
        span: Option<SourceSpan>,
        op: InfixOp,
        left: &Expr,
        right: &Expr,
    ) -> Result<Ty, TypeError> {
        let lt = self.infer(left)?;
        let rt = self.infer(right)?;
        use InfixOp::*;

        // Membership `in` / `not in` — the right operand is a container.
        if matches!(op, In | NotIn) {
            return self.infer_membership(span, op, left, right, &lt, &rt);
        }
        // SIMD operators are elementwise (handled before the scalar-numeric path).
        if matches!(lt, Ty::Simd { .. }) || matches!(rt, Ty::Simd { .. }) {
            return self.infer_simd_infix(op, &lt, &rt);
        }
        // Arithmetic and identity comparison reason about allocation layout,
        // which an origin-bearing pointer to a single checked value does not
        // have; Mojo leaves such use undefined, so Mojito rejects it early.
        let origin_bearing =
            |ty: &Ty| matches!(ty, Ty::Pointer { origin, .. } if origin.as_origin().is_some());
        if matches!(op, Add | Sub | Eq | Ne)
            && (origin_bearing(&lt) || origin_bearing(&rt))
            && (matches!(lt, Ty::Pointer { .. }) || matches!(rt, Ty::Pointer { .. }))
        {
            return Err(TypeError::Unsupported(
                "pointer arithmetic and comparison are not supported on an \
                 origin-bearing Pointer"
                    .to_string(),
            ));
        }
        if let Ty::Pointer { element, .. } = &lt {
            match (op, &rt) {
                (Add | Sub, Ty::Int | Ty::IntLiteral) => return Ok(lt.clone()),
                (Sub, Ty::Pointer { element: other, .. }) if element == other => {
                    return Ok(Ty::Int);
                }
                (Eq | Ne, Ty::Pointer { element: other, .. }) if element == other => {
                    return Ok(Ty::Bool);
                }
                _ => {}
            }
        }

        // Tuple comparisons are structural. Equality accepts independently
        // equatable element packs (different element types simply compare
        // unequal); ordering requires a lexicographically compatible prefix.
        if let (Ty::Tuple(left), Ty::Tuple(right)) = (&lt, &rt) {
            // Current Tuple comparison methods take `other: Self`: different
            // arities or element packs are not comparable merely because the VM
            // could walk both vectors. Literal element coercion may still make
            // the two tuple types the same contextual `Self`.
            let same_self = coerces(&lt, &rt) || coerces(&rt, &lt);
            let supported = match op {
                Eq | Ne => {
                    same_self
                        && self.tuple_elements_equatable_nominal(left)
                        && self.tuple_elements_equatable_nominal(right)
                }
                Lt | Gt | Le | Ge => same_self && tuple_order_compatible(left, right),
                _ => false,
            };
            if supported {
                return Ok(Ty::Bool);
            }
        }
        if matches!((&lt, &rt), (Ty::Struct(left, _), Ty::Struct(right, _))
            if !self.structs.contains_key(left) && !self.structs.contains_key(right))
            && let (Some(left), Some(right)) = (tuple_elements(&lt), tuple_elements(&rt))
        {
            let left = left.into_iter().cloned().collect::<Vec<_>>();
            let right = right.into_iter().cloned().collect::<Vec<_>>();
            let same_self = coerces(&lt, &rt) || coerces(&rt, &lt);
            let supported = match op {
                Eq | Ne => {
                    same_self
                        && self.tuple_elements_equatable_nominal(&left)
                        && self.tuple_elements_equatable_nominal(&right)
                }
                Lt | Gt | Le | Ge => same_self && tuple_order_compatible(&left, &right),
                _ => false,
            };
            if supported {
                return Ok(Ty::Bool);
            }
        }
        // `Slice` is Equatable (upstream `builtin_slice.mojo`); its
        // `ContiguousSlice`/`StridedSlice` sub-kinds are not, so only exact
        // `Slice` operands compare. The VM compares the three bounds.
        if matches!(op, Eq | Ne)
            && matches!((&lt, &rt), (Ty::Struct(left, left_args), Ty::Struct(right, right_args))
                if left == "Slice" && right == "Slice" && left_args.is_empty() && right_args.is_empty())
        {
            return Ok(Ty::Bool);
        }
        // Two equal opaque type parameters bounded by an arithmetic, bitwise,
        // or shift operation trait dispatch after erasure
        // (`def f[T: Addable](a: T, b: T) -> T: return a + b`). Comparison,
        // equality, and `**` params are handled in the result match below via
        // their (refinement-aware) bound helpers.
        if lt == rt
            && matches!(
                op,
                Add | Sub | Mul | FloorDiv | Mod | Div | Shl | Shr | BitAnd | BitOr | BitXor
            )
            && let Some(trait_name) = infix_operation_trait(op)
            && param_has_bound(&lt, trait_name)
        {
            return Ok(if matches!(op, Div) {
                Ty::Float64
            } else {
                lt.clone()
            });
        }

        // `common` is the unified numeric type when both operands are numeric
        // (literals coerced as needed), else None.
        let common = common_numeric(&lt, &rt);
        if let Some(target) = common.as_ref()
            && matches!(
                target,
                Ty::Int | Ty::UInt | Ty::Float64 | Ty::Simd { width: 1, .. }
            )
        {
            self.record_literal_materializations(left, &lt, target)?;
            self.record_literal_materializations(right, &rt, target)?;
        }
        // Integer powers of exact literals stay exact. A fractional exponent
        // is not rational in general, so this is the semantic boundary where
        // both operands become Float64 and runtime `powf` takes over.
        if matches!(op, Pow) && matches!(common.as_ref(), Some(Ty::FloatLiteral)) {
            let exponent_is_integer = match self.exact_literal_value(right) {
                Some(CtValue::IntLiteral(_)) => true,
                Some(CtValue::FloatLiteral(value)) => value.to_int_if_whole().is_some(),
                _ => false,
            };
            if !exponent_is_integer {
                self.record_literal_materializations(left, &lt, &Ty::Float64)?;
                self.record_literal_materializations(right, &rt, &Ty::Float64)?;
                return Ok(Ty::Float64);
            }
        }
        let result = match op {
            // Short-circuiting boolean logic requires `Bool` operands.
            And | Or if lt == Ty::Bool && rt == Ty::Bool => Some(Ty::Bool),
            // `+` concatenates String, or adds numbers (result = common type).
            Add if lt == Ty::StringLiteral && rt == Ty::StringLiteral => Some(Ty::StringLiteral),
            // `**` between equal opaque type parameters bounded by `Powable`
            // (`__pow__(self, Self) -> Self`); the concrete impl runs after
            // erasure. Checked before the numeric arm since a `Param` isn't
            // numeric (so `common` is None here).
            Pow if lt == rt && param_has_bound(&lt, "Powable") => Some(lt.clone()),
            // Arithmetic that preserves the operand type.
            Add | Sub | Mul | FloorDiv | Mod | Pow => common,
            Shl | Shr | BitAnd | BitOr | BitXor
                if common
                    .as_ref()
                    .is_some_and(|ty| matches!(ty, Ty::Int | Ty::UInt | Ty::IntLiteral)) =>
            {
                common
            }
            // Literal division stays exact until a runtime context chooses
            // Float64; concrete operands perform fixed-width runtime division.
            Div if common.is_some() => Some(
                if matches!(common.as_ref(), Some(Ty::IntLiteral | Ty::FloatLiteral)) {
                    Ty::FloatLiteral
                } else {
                    Ty::Float64
                },
            ),
            // Ordering between numbers, or between equal opaque type parameters
            // whose bound promises an ordering (`T: Comparable`).
            Lt | Gt | Le | Ge
                if common.is_some()
                    || (lt == rt
                        && (has_order_bound(&lt)
                            || self.has_assumed_conformance(&lt, "Comparable"))) =>
            {
                Some(Ty::Bool)
            }
            // Equality: between numbers (any common type), or equal non-numeric
            // scalars (Bool/String/None).
            Eq | Ne
                if common.is_some()
                    || (lt == rt
                        && (is_scalar(&lt)
                            || has_equality_bound(&lt)
                            || self.has_assumed_conformance(&lt, "Equatable")
                            || self.has_assumed_conformance(&lt, "Comparable"))) =>
            {
                Some(Ty::Bool)
            }
            _ => None,
        };
        if let Some(ty) = result {
            return Ok(ty);
        }
        // Mixed literal/nominal String operands normalize onto the nominal
        // struct: the literal side converts through the `@implicit` literal
        // constructor and the operator dispatches the struct's dunder, so
        // `s + "x"`, `"x" + s`, and mixed comparisons all run library code.
        let nominal_string = |ty: &Ty| {
            matches!(ty, Ty::Struct(name, args)
                if args.is_empty()
                    && mojito_symbol::symbol::is_stdlib_string_struct(name)
                    && self.structs.contains_key(name))
        };
        let mixed_string = if nominal_string(&lt) && rt == Ty::StringLiteral {
            Some((right, lt.clone()))
        } else if nominal_string(&rt) && lt == Ty::StringLiteral {
            Some((left, rt.clone()))
        } else {
            None
        };
        if let Some((literal_operand, nominal_ty)) = mixed_string
            && let Some(dunder) = op.dunder()
            && self.record_implicit_conversion(literal_operand, &Ty::StringLiteral, &nominal_ty)?
            && let Some(r) = self.struct_dunder(&nominal_ty, dunder, &[&nominal_ty])
        {
            return r;
        }
        // Operator overloading: `a OP b` on a user struct dispatches to the left
        // operand's dunder method (`a.__add__(b)`, `a.__eq__(b)`, …). Among
        // same-arity overloads the operand type selects (first by value
        // coercion, then through an `@implicit` conversion of the right
        // operand, as a call argument would convert); an overloaded dunder
        // records the exact lowered symbol so `BinOp.resolved` names it.
        // `a != b` on a struct declaring `__eq__` without `__ne__` is
        // Equatable's default `__ne__` (`not (a == b)`): the operator
        // dispatches `__eq__` and MIR negates the result.
        // (`struct_dunder_signature_for` falls back to the first same-arity
        // declaration for diagnostics, so acceptance is re-checked here: a
        // `__ne__` overload set that accepts no operand of this type still
        // defers to `__eq__`.)
        let dunder_accepts = |dunder: &str| {
            self.struct_dunder_signature_for(&lt, dunder, &[&rt])
                .is_some_and(|(info, sig, targs)| {
                    self.value_coerces(&rt, &substitute_at(&sig.params[0], &info.decls, targs))
                })
        };
        let negated_equality = op == Ne && !dunder_accepts("__ne__") && dunder_accepts("__eq__");
        let dunder = if negated_equality {
            Some("__eq__")
        } else {
            op.dunder()
        };
        if let Some(dunder) = dunder
            && let Some((info, sig, targs)) = self.struct_dunder_signature_for(&lt, dunder, &[&rt])
        {
            if negated_equality && let Some(span) = span.clone() {
                self.operation_adjustments.borrow_mut().insert(
                    span,
                    mojito_checked::checked::SemanticAdjustment::NegatedEquality,
                );
            }
            let Ty::Struct(sname, _) = &lt else {
                unreachable!("dunder signatures resolve on struct receivers")
            };
            let overloaded = info.methods.get(dunder).is_some_and(|sigs| sigs.len() > 1);
            let mut operand_ty = rt.clone();
            let mut selected = sig;
            let param = substitute_at(&sig.params[0], &info.decls, targs);
            if !self.value_coerces(&rt, &param) {
                let same_arity = info
                    .methods
                    .get(dunder)
                    .into_iter()
                    .flatten()
                    .filter(|sig| sig.params.len() == 1);
                for candidate in same_arity {
                    let param = substitute_at(&candidate.params[0], &info.decls, targs);
                    if self.implicit_conversion_target(&rt, &param)?.is_some() {
                        self.record_implicit_conversion(right, &rt, &param)?;
                        operand_ty = param;
                        selected = candidate;
                        break;
                    }
                }
            }
            // The dunder's availability clause (`__eq__ ... where
            // conforms_to(Self.T, Equatable)`) is judged against the
            // receiver's arguments, as a method call judges it; a failing
            // clause leaves the operator undefined for these operands.
            let environment: HashMap<String, TyArg> = info
                .decls
                .iter()
                .map(|decl| decl.name().to_string())
                .zip(targs.iter().cloned())
                .collect();
            if self
                .method_constraint_result(selected, &environment)
                .is_err()
            {
                return Err(TypeError::BadOperator {
                    op: infix_symbol(op).to_string(),
                    operands: format!("{} and {}", lt, rt),
                });
            }
            if matches!(
                selected.conventions.first().copied().flatten(),
                Some(ArgConvention::Var | ArgConvention::Deinit)
            ) {
                self.check_consuming_as(
                    right,
                    &rt,
                    &format!("operand of '{}'", infix_symbol(op)),
                    super::traits::ConsumeKind::Move,
                )?;
            }
            if overloaded && let Some(span) = span.clone() {
                self.overload_targets.borrow_mut().insert(
                    span,
                    method_lowered_name(
                        sname,
                        dunder,
                        selected,
                        self.self_instance_ty(sname).as_ref(),
                    ),
                );
            }
            return self
                .struct_dunder(&lt, dunder, &[&operand_ty])
                .expect("dunder signature was resolved");
        }
        // The right operand's reflected dunder (`1 + m` → `m.__radd__(1)`)
        // answers when the left operand has no operator method for the
        // pair: MIR swaps the operands and calls the recorded symbol.
        if let Some(span) = span
            && let Some(reflected) = op.reflected_dunder()
            && let Some((info, sig, targs)) =
                self.struct_dunder_signature_for(&rt, reflected, &[&lt])
            && let Ty::Struct(rname, _) = &rt
            && self.value_coerces(&lt, &substitute_at(&sig.params[0], &info.decls, targs))
        {
            let environment: HashMap<String, TyArg> = info
                .decls
                .iter()
                .map(|decl| decl.name().to_string())
                .zip(targs.iter().cloned())
                .collect();
            if self.method_constraint_result(sig, &environment).is_ok() {
                // The symbol the backends dispatch: the overload key only
                // when the reflected dunder is overloaded.
                let target = if info
                    .methods
                    .get(reflected)
                    .is_some_and(|sigs| sigs.len() > 1)
                {
                    method_lowered_name(
                        rname,
                        reflected,
                        sig,
                        self.self_instance_ty(rname).as_ref(),
                    )
                } else {
                    format!("{rname}.{reflected}")
                };
                self.overload_targets
                    .borrow_mut()
                    .insert(span.clone(), target);
                self.operation_adjustments.borrow_mut().insert(
                    span,
                    mojito_checked::checked::SemanticAdjustment::ReflectedOperator,
                );
                return self
                    .struct_dunder(&rt, reflected, &[&lt])
                    .expect("reflected dunder signature was resolved");
            }
        }
        Err(TypeError::BadOperator {
            op: infix_symbol(op).to_string(),
            operands: format!("{} and {}", lt, rt),
        })
    }

    /// Type a membership test `x in c` / `x not in c` → `Bool`. The container is
    /// a `List[T]`, heterogeneous `Tuple`, or `String` (substring test).
    pub(super) fn infer_membership(
        &self,
        span: Option<SourceSpan>,
        op: InfixOp,
        left: &Expr,
        right: &Expr,
        lt: &Ty,
        rt: &Ty,
    ) -> Result<Ty, TypeError> {
        let nominal_ok = match rt {
            Ty::Struct(name, _) if !self.structs.contains_key(name) => {
                if let Some(element) = list_element(rt).or_else(|| set_element(rt)) {
                    coerces(lt, element) && is_list_equatable(element)
                } else if let Some((key, _)) = dict_elements(rt) {
                    coerces(lt, key) && is_list_equatable(key)
                } else if let Some(elements) = tuple_elements(rt) {
                    elements
                        .into_iter()
                        .any(|element| coerces(lt, element) && is_list_equatable(element))
                } else {
                    false
                }
            }
            _ => false,
        };
        let ok = nominal_ok
            || match rt {
                Ty::Tuple(_) => match lt {
                    Ty::Tuple(elements) => tuple_elements_equatable(elements),
                    other => is_list_equatable(other),
                },
                Ty::StringLiteral => *lt == Ty::StringLiteral,
                _ => false,
            };
        if ok {
            return Ok(Ty::Bool);
        }
        // `x in c` on a user struct dispatches to the container's `__contains__`
        // (`c.__contains__(x)`), which must return `Bool`.
        if let Some(span) = span
            && matches!(rt, Ty::Struct(name, _) if self.structs.contains_key(name))
        {
            let ret = self.infer_method_call(
                span,
                right,
                "__contains__",
                MethodCallArguments::ordinary(std::slice::from_ref(left), &[]),
            )?;
            return require_dunder_ret(ret, &Ty::Bool, "__contains__");
        }
        if let Some(r) = self.struct_dunder(rt, "__contains__", &[lt]) {
            return r.and_then(|ret| require_dunder_ret(ret, &Ty::Bool, "__contains__"));
        }
        Err(TypeError::BadOperator {
            op: infix_symbol(op).to_string(),
            operands: format!("{} and {}", lt, rt),
        })
    }

    /// Type an elementwise SIMD operator. Both operands must be the same SIMD
    /// type, except a numeric *literal* splats to the other operand's type.
    /// Arithmetic keeps the operand type; comparisons return a `bool` mask.
    pub(super) fn infer_simd_infix(&self, op: InfixOp, lt: &Ty, rt: &Ty) -> Result<Ty, TypeError> {
        use InfixOp::*;
        let bad = || TypeError::BadOperator {
            op: infix_symbol(op).to_string(),
            operands: format!("{} and {}", lt, rt),
        };
        // Determine the common SIMD type, allowing a numeric literal on one side.
        let simd = match (lt, rt) {
            (
                Ty::Simd {
                    dtype: d1,
                    width: w1,
                },
                Ty::Simd {
                    dtype: d2,
                    width: w2,
                },
            ) if d1 == d2 && w1 == w2 => Ty::Simd {
                dtype: *d1,
                width: *w1,
            },
            (Ty::Simd { dtype, width }, other) | (other, Ty::Simd { dtype, width })
                if splats_to(other, *dtype) =>
            {
                Ty::Simd {
                    dtype: *dtype,
                    width: *width,
                }
            }
            _ => return Err(bad()),
        };
        let Ty::Simd { dtype, width } = simd else {
            return Err(TypeError::InvariantViolation(
                "SIMD operator inference produced a non-SIMD type".to_string(),
            ));
        };
        match op {
            // Elementwise arithmetic on numeric lanes preserves the type.
            Add | Sub | Mul if dtype != Dtype::Bool => Ok(simd_ty(dtype, width)),
            BitAnd | BitOr | BitXor if !dtype.is_float() => Ok(simd_ty(dtype, width)),
            Shl | Shr if dtype != Dtype::Bool && !dtype.is_float() => Ok(simd_ty(dtype, width)),
            // True division is defined on float lanes only.
            Div if dtype.is_float() => Ok(simd_ty(dtype, width)),
            // Equality on any lanes; ordering on numeric lanes — a bool mask.
            Eq | Ne => Ok(simd_ty(Dtype::Bool, width)),
            Lt | Gt | Le | Ge if dtype != Dtype::Bool => Ok(simd_ty(Dtype::Bool, width)),
            _ => Err(bad()),
        }
    }

    /// Type `SIMD[DType.<dt>, width](args)`: `width` element arguments, or a
    /// single argument that splats across all lanes; each must fit the dtype.
    pub(super) fn infer_simd_construction(
        &self,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        let (dtype, mut width) = self.simd_dims(param_args)?;
        if width == -1 {
            width = i64::try_from(args.len()).unwrap_or(0);
            if width < 1 || (width & (width - 1)) != 0 {
                return Err(TypeError::BadSimdWidth(width.to_string()));
            }
        }
        self.check_simd_args(dtype, width, args)?;
        Ok(simd_ty(dtype, width))
    }

    /// Type a scalar-alias construction `Int32(x)` = `SIMD[DType.int32, 1](x)`.
    pub(super) fn infer_simd_alias_construction(
        &self,
        dtype: Dtype,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        if !param_args.is_empty() {
            return Err(TypeError::WrongTypeArgCount {
                name: dtype.scalar_alias().unwrap_or("SIMD").to_string(),
                expected: 0,
                got: param_args.len(),
            });
        }
        self.check_simd_args(dtype, 1, args)?;
        Ok(Ty::Simd { dtype, width: 1 })
    }

    /// Check the element arguments of a SIMD construction: either `width` of them
    /// (one per lane) or exactly one (splatted), each fitting `dtype`.
    pub(super) fn check_simd_args(
        &self,
        dtype: Dtype,
        width: i64,
        args: &[Expr],
    ) -> Result<(), TypeError> {
        if args.len() != width as usize && args.len() != 1 {
            return Err(TypeError::SimdArity {
                width,
                got: args.len(),
            });
        }
        for arg in args {
            let aty = self.infer(arg)?;
            // Integer lanes also construct from any Intable value (bounded
            // parameter or conforming struct) through its `__int__`; the
            // numeric scalars take the exact `converts_to_lane` matrix, so a
            // float source still spells its truncation explicitly.
            let intable_object = dtype != Dtype::Bool
                && !dtype.is_float()
                && matches!(&aty, Ty::Param { .. } | Ty::Struct(..))
                && self.conforms_to(&aty, "Intable");
            if !converts_to_lane(&aty, dtype) && !intable_object {
                return Err(TypeError::TypeMismatch {
                    expected: format!("a DType.{} element", dtype.name()),
                    found: aty.to_string(),
                    context: "SIMD element".to_string(),
                });
            }
        }
        Ok(())
    }

    /// Type the built-in `Error(msg)` constructor: one `String` argument.
    pub(super) fn infer_error_construction(&self, args: &[Expr]) -> Result<Ty, TypeError> {
        if args.len() != 1 {
            return Err(TypeError::ArityMismatch {
                name: "Error".to_string(),
                expected: 1,
                got: args.len(),
            });
        }
        let aty = self.infer(&args[0])?;
        let nominal_string = matches!(&aty, Ty::Struct(name, args)
            if args.is_empty() && mojito_symbol::symbol::is_stdlib_string_struct(name));
        if aty != Ty::StringLiteral && !nominal_string {
            return Err(TypeError::TypeMismatch {
                expected: "String".to_string(),
                found: aty.to_string(),
                context: "argument to 'Error'".to_string(),
            });
        }
        Ok(Ty::Error)
    }

    pub(super) fn infer_slice_construction(
        &self,
        name: &str,
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        let valid_arity = match name {
            "Slice" => matches!(args.len(), 2 | 3),
            "slice" => matches!(args.len(), 1..=3),
            _ => false,
        };
        if !valid_arity {
            return Err(TypeError::ArityMismatch {
                name: name.to_string(),
                expected: if name == "Slice" { 2 } else { 1 },
                got: args.len(),
            });
        }
        for argument in args {
            let found = self.infer(argument)?;
            if found != Ty::None && !coerces(&found, &Ty::Int) {
                return Err(TypeError::TypeMismatch {
                    expected: "Int or None".to_string(),
                    found: found.to_string(),
                    context: format!("argument to '{name}'"),
                });
            }
        }
        Ok(Ty::Struct("Slice".to_string(), Vec::new()))
    }

    /// Type `Pointer(to=place)` (also spelled through the deprecated
    /// `UnsafePointer` alias): an origin-bearing pointer to existing checked
    /// storage. The element type is the place's type and the origin is the
    /// place itself, so loan analysis keeps the owner alive and rejects
    /// conflicting access. Execution represents the value as a frame/slot
    /// handle; only the VM erases the origin.
    pub(super) fn infer_pointer_to(
        &self,
        span: SourceSpan,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        if !param_args.is_empty() {
            return Err(TypeError::Unsupported(
                "Pointer(to=...) infers its element type; explicit type \
                 arguments are not supported"
                    .to_string(),
            ));
        }
        if !args.is_empty() || kwargs.len() != 1 || kwargs[0].name != "to" {
            return Err(TypeError::BadCall {
                func: "Pointer".to_string(),
                reason: "expected exactly one 'to=' keyword argument".to_string(),
            });
        }
        let value = &kwargs[0].value;
        // A `ref[Self.o]` parameter names the enclosing struct's origin binder:
        // its pointer carries that binder exactly as a `Pointer[T, Self.o]`
        // field declares it (upstream's iterator storage `self.src =
        // Pointer(to=xs)`), so the store type-checks by identity and the
        // binder later resolves to the caller's source.
        if let ExprKind::Identifier(name) = &value.kind
            && let Some(origin) = self.lookup_reference_parameter_binder(name)
            && let Some(reference) = self.lookup_reference_parameter(name)
        {
            let mutable = origin.statically_mutable() == Some(true);
            self.operation_adjustments.borrow_mut().insert(
                span,
                mojito_checked::checked::SemanticAdjustment::PointerToPlace { mutable },
            );
            return Ok(Ty::Pointer {
                element: reference.referent,
                origin,
            });
        }
        if let ExprKind::Identifier(name) = &value.kind
            && (matches!(self.lookup(name), Some(Ty::Ref(_)))
                || self.lookup_reference_parameter(name).is_some())
        {
            // A `ref` binding names a borrowed region, not owned storage, so
            // the minted pointer's provenance is the conservative subtree of
            // the reference's origin: the referent is that base or some
            // descendant of it. Subtree staleness (any mutation at or below
            // the base, and the first-write rule) replaces the exact-place
            // loan the owned case gets.
            use mojito_types::origin::{Mutability, Origin, OriginSeg, PointerOrigin};
            let reference = self.reference_actual(value)?;
            let origin = match reference.origin {
                Origin::Place(mut place) => {
                    let mutable = matches!(reference.mutability, Mutability::Mutable);
                    place.path.push(OriginSeg::Subtree);
                    PointerOrigin::Place { place, mutable }
                }
                Origin::Param(id) => PointerOrigin::Param {
                    id,
                    mutability: reference.mutability,
                    interior: Vec::new(),
                    subtree: true,
                },
                Origin::SelfParam => PointerOrigin::SelfPlace {
                    mutability: reference.mutability,
                    interior: Vec::new(),
                    subtree: true,
                },
                other => {
                    return Err(TypeError::Unsupported(format!(
                        "Pointer(to=...) through a 'ref' binding requires a place or \
                         origin-parameter referent; a {other} origin is not supported"
                    )));
                }
            };
            let mutable = origin.statically_mutable() == Some(true);
            self.operation_adjustments.borrow_mut().insert(
                span,
                mojito_checked::checked::SemanticAdjustment::PointerToPlace { mutable },
            );
            return Ok(Ty::Pointer {
                element: reference.referent,
                origin,
            });
        }
        let place = self.origin_place(value).map_err(|error| match error {
            TypeError::UndefinedVariable(_) => error,
            _ => TypeError::Unsupported("Pointer(to=...) requires a place expression".to_string()),
        })?;
        let element = self.infer(value)?;
        let mutable = self.owner_is_mutable(place.root);
        self.operation_adjustments.borrow_mut().insert(
            span,
            mojito_checked::checked::SemanticAdjustment::PointerToPlace { mutable },
        );
        Ok(Ty::Pointer {
            element: Box::new(element),
            origin: mojito_types::origin::PointerOrigin::Place { place, mutable },
        })
    }
}

/// A readable symbol for an infix operator, for error messages.
pub(super) fn infix_symbol(op: InfixOp) -> &'static str {
    match op {
        InfixOp::Add => "+",
        InfixOp::Sub => "-",
        InfixOp::Mul => "*",
        InfixOp::Div => "/",
        InfixOp::FloorDiv => "//",
        InfixOp::Mod => "%",
        InfixOp::MatMul => "@",
        InfixOp::Shl => "<<",
        InfixOp::Shr => ">>",
        InfixOp::BitAnd => "&",
        InfixOp::BitOr => "|",
        InfixOp::BitXor => "^",
        InfixOp::Pow => "**",
        InfixOp::Eq => "==",
        InfixOp::Ne => "!=",
        InfixOp::Lt => "<",
        InfixOp::Gt => ">",
        InfixOp::Le => "<=",
        InfixOp::Ge => ">=",
        InfixOp::And => "and",
        InfixOp::Or => "or",
        InfixOp::In => "in",
        InfixOp::NotIn => "not in",
        InfixOp::Is => "is",
        InfixOp::IsNot => "is not",
    }
}

/// A readable symbol for a prefix operator, for error messages.
pub(super) fn prefix_symbol(op: PrefixOp) -> &'static str {
    match op {
        PrefixOp::Neg => "-",
        PrefixOp::Not => "not",
        PrefixOp::Invert => "~",
    }
}

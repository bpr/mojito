//! Operator, SIMD-construction, and pointer-construction type inference
//! for the checker. Extracted from `checker.rs`; see `docs/symbol-map.md`.

use super::traits::RegisterPassability;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::{CheckedCompareLink, SemanticAdjustment};
use mojito_types::types::TyKind;

impl Checker {
    pub(super) fn infer_prefix(
        &self,
        span: SourceSpan,
        op: PrefixOp,
        operand: &Expr,
    ) -> Result<Ty, TypeError> {
        let t = self.infer(operand)?;
        // `-x`, `~x`, and `+x` over a `SIMD` are its declared `x.__neg__()`,
        // `x.__invert__()`, and `x.__pos__()`, as upstream's
        // `UnaryOpNode::emitArith` emits them.
        if op != PrefixOp::Not
            && self.declared_receiver_member(&t, op.dunder()).is_some()
            && let Some(ty) =
                self.infer_operator_method_call(&span, operand, &t, op.dunder(), &[])?
        {
            return Ok(ty);
        }
        match (op, t.kind()) {
            // A literal's negation and unary plus are literals.
            (PrefixOp::Neg | PrefixOp::Pos, TyKind::IntLiteral | TyKind::FloatLiteral) => {
                return Ok(t);
            }
            (PrefixOp::Not, TyKind::Bool) => return Ok(Ty::bool()),
            // A numeric scalar is `Boolable`: `not n` over an `Int`, or
            // `not (a == b)` over `UInt64`s.
            (PrefixOp::Not, _) if is_numeric(&t) || is_scalar_simd(&t) => {
                return Ok(Ty::bool());
            }
            // A literal's and a `Bool`'s inversion keep their type; a
            // `SIMD`'s is its `__invert__` above.
            (PrefixOp::Invert, TyKind::IntLiteral | TyKind::Bool) => {
                return Ok(t);
            }
            _ => {}
        }
        // `-x` and `~x` over a type parameter or a projection are the calls
        // `x.__neg__()` and `x.__invert__()` of the requirement its bounds
        // declare, as any method call through a bound is.
        if op != PrefixOp::Not
            && is_bound_archetype(&t)
            && let Some(ty) =
                self.infer_operator_method_call(&span, operand, &t, op.dunder(), &[])?
        {
            return Ok(ty);
        }
        // A type parameter bounded by a compiler-known operation trait that
        // declares no requirement (`Negatable`, `Invertable`) or by
        // `Boolable` for `not x` dispatches after erasure; the concrete
        // impl runs on the erased type.
        if prefix_operation_trait(op).is_some_and(|bound| self.param_has_bound(&t, bound)) {
            self.record_read_temporary_argument(operand);
            return Ok(match op {
                PrefixOp::Neg | PrefixOp::Invert | PrefixOp::Pos => t,
                PrefixOp::Not => Ty::bool(),
            });
        }
        // A user struct's `-x`, `~x`, and `+x` are its dunder calls
        // `x.__neg__()`, `x.__invert__()`, and `x.__pos__()`, as upstream's
        // `UnaryOpNode::emitArith` emits them.
        if op != PrefixOp::Not
            && self.is_checked_struct(&t)
            && let Some(ty) =
                self.infer_operator_method_call(&span, operand, &t, op.dunder(), &[])?
        {
            return Ok(ty);
        }
        // `not x` is `not x.__bool__()`, raising what `__bool__` raises.
        if op == PrefixOp::Not
            && let Some(result) = self.struct_dunder(&t, op.dunder(), &[])
        {
            let ret = result?;
            let signature = self
                .struct_dunder_signature_for(&t, op.dunder(), &[])
                .map(|(_, sig, _)| sig);
            if let Some(error) = signature.and_then(dunder_error) {
                self.record_call_effect(span, error.clone());
                self.require_error(format!("raising operator '{}'", op.symbol()), error)?;
            }
            let convention = signature.map(|sig| sig.self_convention);
            if convention.is_some_and(reads) {
                self.borrow_operator_operand(operand, &t);
                self.record_read_temporary_argument(operand);
            }
            if convention.is_some_and(owns_argument) {
                self.record_forwarded_operand(operand);
            }
            return require_dunder_ret(ret, &Ty::bool(), "__bool__");
        }
        Err(TypeError::BadOperator {
            op: op.symbol().to_string(),
            operands: t.to_string(),
        })
    }

    #[allow(
        clippy::cognitive_complexity,
        clippy::too_many_lines,
        reason = "TODO: split this pass"
    )]
    pub(super) fn infer_infix(
        &self,
        span: Option<SourceSpan>,
        op: InfixOp,
        left: &Expr,
        right: &Expr,
    ) -> Result<Ty, TypeError> {
        use InfixOp::{
            Add, And, BitAnd, BitOr, BitXor, Div, Eq, FloorDiv, Ge, Gt, In, Le, Lt, Mod, Mul, Ne,
            NotIn, Or, Pow, Shl, Shr, Sub,
        };

        if matches!(op, And | Or) {
            return self.infer_short_circuit(span, op, left, right);
        }
        // An element of an unbound pack takes its operators from its bounds.
        let lt = self.infer(left)?;
        let lt = self.opaque_element(&lt).unwrap_or(lt);
        // A leading-dot right operand has no type of its own, so only the
        // left operand's dunder can take it, its parameter typing it, as
        // upstream tries no reflected dunder for an untyped operand.
        if let Some(span) = &span
            && super::inference::contextual_root_span(right).is_some()
            && self.is_checked_struct(&lt)
        {
            return self
                .infer_operator_call(span, op, left, right, &lt, None)?
                .ok_or_else(|| TypeError::BadOperator {
                    op: op.symbol().to_string(),
                    operands: format!("{lt} and a leading-dot operand"),
                });
        }
        // `DType`'s `__eq__` and `__ne__` take `rhs: Self`, so beside a
        // `DType` that parameter types the leading-dot operand.
        if matches!(op, Eq | Ne)
            && lt == TyKind::Dtype
            && super::inference::contextual_root_span(right).is_some()
        {
            let rt = self.infer_with_expected(right, &lt, true)?;
            return if rt == TyKind::Dtype {
                Ok(Ty::bool())
            } else {
                Err(bad_infix_operator(op, &lt, &rt))
            };
        }
        let rt = self.infer(right)?;
        let rt = self.opaque_element(&rt).unwrap_or(rt);
        let rt = self.simd_length_operand(right, &lt, rt)?;

        // Membership `in` / `not in` — the right operand is a container.
        if matches!(op, In | NotIn) {
            return self.infer_membership(span, op, left, right, &lt, &rt);
        }
        // `+`, `-`, `*`, `//`, the bitwise operators, the shifts, and the
        // comparisons beside a `SIMD` operand, a scalar's included, are
        // `SIMD`'s declared dunders, `a.__add__(b)` else the reflected
        // `b.__radd__(a)`, as upstream's `emitBinOpCall` resolves them. A
        // comparison has no reflected dunder, so a literal left operand
        // materializes to its default type (`Int`, `Float64`), whose dunder
        // takes the right one. In a compile-time position a comparison is
        // the dunder folded instead (`infix_value`), which asserts nothing.
        // An unlinked program (a phase test's) declares no `SIMD`, so its
        // scalar comparisons stay the built-in ones below.
        let pointer_or_archetype =
            |ty: &Ty| matches!(ty.kind(), TyKind::Pointer { .. }) || is_bound_archetype(ty);
        let comparison = matches!(op, Eq | Ne | Lt | Gt | Le | Ge);
        if let Some(span) = &span
            && (matches!(
                op,
                Add | Sub | Mul | FloorDiv | BitAnd | BitOr | BitXor | Shl | Shr
            ) || (comparison
                && self.comptime_positions.get() == 0
                && self.structs.contains_key("SIMD")))
            && (matches!(lt.kind(), TyKind::Simd { .. })
                || matches!(rt.kind(), TyKind::Simd { .. }))
            && !pointer_or_archetype(&lt)
            && !pointer_or_archetype(&rt)
        {
            let receiver = if comparison {
                mojito_types::types::default_literal(&lt)
            } else {
                lt.clone()
            };
            if receiver != lt {
                self.record_literal_materializations(left, &lt, &receiver)?;
            }
            return self
                .infer_operator_call(span, op, left, right, &receiver, Some(&rt))?
                .ok_or_else(|| bad_infix_operator(op, &receiver, &rt));
        }
        // The other SIMD operators are elementwise (handled before the
        // scalar-numeric path, which `Int`, `UInt`, and `Float64` operands
        // keep).
        let vector = |ty: &Ty| {
            matches!(ty.kind(), TyKind::Simd { .. })
                && !mojito_types::types::has_host_scalar_value(ty.kind())
        };
        if vector(&lt) || vector(&rt) {
            return simd_infix(op, &lt, &rt);
        }
        // Arithmetic and identity comparison reason about allocation layout,
        // which an origin-bearing pointer to a single checked value does not
        // have; Mojo leaves such use undefined, so Mojito rejects it early.
        let origin_bearing = |ty: &Ty| matches!(ty.kind(), TyKind::Pointer { origin, .. } if origin.as_origin().is_some());
        if matches!(op, Add | Sub | Eq | Ne)
            && (origin_bearing(&lt) || origin_bearing(&rt))
            && (matches!(lt.kind(), TyKind::Pointer { .. })
                || matches!(rt.kind(), TyKind::Pointer { .. }))
        {
            return Err(TypeError::Unsupported(
                "pointer arithmetic and comparison are not supported on an \
                 origin-bearing Pointer"
                    .to_string(),
            ));
        }
        if let TyKind::Pointer { element, .. } = lt.kind() {
            match (op, rt.kind()) {
                (Add | Sub, TyKind::INT | TyKind::IntLiteral) => return Ok(lt.clone()),
                (Sub, TyKind::Pointer { element: other, .. }) if element == other => {
                    return Ok(Ty::int());
                }
                (Eq | Ne, TyKind::Pointer { element: other, .. }) if element == other => {
                    return Ok(Ty::bool());
                }
                _ => {}
            }
        }

        // An operator over a type parameter or a projection is the call of
        // the requirement its bounds (or an assumed conformance) declare,
        // `a.__add__(b)`, else the right operand's reflected `b.__radd__(a)`,
        // as upstream's `emitBinOpCall` resolves it through any method
        // call's lookup.
        if let Some(span) = &span
            && (is_bound_archetype(&lt) || is_bound_archetype(&rt))
            && let Some(ty) = self.infer_operator_call(span, op, left, right, &lt, Some(&rt))?
        {
            return Ok(ty);
        }
        // Two equal type parameters bounded by a compiler-known arithmetic,
        // bitwise, or shift operation trait, which declares no requirement
        // (`def f[T: Addable](a: T, b: T) -> T: return a + b`), dispatch
        // after erasure.
        if lt == rt
            && matches!(
                op,
                Add | Sub | Mul | FloorDiv | Mod | Div | Shl | Shr | BitAnd | BitOr | BitXor
            )
            && let Some(trait_name) = infix_operation_trait(op)
            && self.param_has_bound(&lt, trait_name)
        {
            return Ok(if matches!(op, Div) { Ty::float64() } else { lt });
        }

        // `common` is the unified numeric type when both operands are numeric
        // (literals coerced as needed), else None.
        let common = common_numeric(&lt, &rt);
        if let Some(target) = common.as_ref()
            && is_scalar_simd(target)
        {
            self.record_literal_materializations(left, &lt, target)?;
            self.record_literal_materializations(right, &rt, target)?;
        }
        // A literal beside a scalar whose lane dtype is still a parameter
        // (`a + 1.5` over `a: Scalar[dt]`) materializes to that scalar, as it
        // does beside a closed one: the instance's lane decides the value.
        if common.is_none() {
            for (literal, literal_ty, lane) in [(right, &rt, &lt), (left, &lt, &rt)] {
                if let TyKind::Simd { dtype, width } = lane.kind()
                    && *width == SimdWidth::Known(1)
                    && dtype.is_symbolic()
                    && matches!(literal_ty.kind(), TyKind::IntLiteral | TyKind::FloatLiteral)
                    && splats_to(literal_ty, dtype)
                {
                    self.record_literal_materializations(literal, literal_ty, lane)?;
                }
            }
        }
        // Integer powers of exact literals stay exact. A fractional exponent
        // is not rational in general, so this is the semantic boundary where
        // both operands become Float64 and runtime `powf` takes over.
        if matches!(op, Pow) && matches!(common.as_ref().map(Ty::kind), Some(TyKind::FloatLiteral))
        {
            let exponent_is_integer = match self.exact_literal_value(right) {
                Some(CtValue::IntLiteral(_)) => true,
                Some(CtValue::FloatLiteral(value)) => value.to_int_if_whole().is_some(),
                _ => false,
            };
            if !exponent_is_integer {
                self.record_literal_materializations(left, &lt, &Ty::float64())?;
                self.record_literal_materializations(right, &rt, &Ty::float64())?;
                return Ok(Ty::float64());
            }
        }
        let result = match op {
            // Upstream's `Bool.__and__`/`__or__`/`__xor__`: the eager `i1` op.
            BitAnd | BitOr | BitXor if lt == TyKind::Bool && rt == TyKind::Bool => Some(Ty::bool()),
            // `+` concatenates String, or adds numbers (result = common type).
            // Two known texts concatenate to the literal of both.
            Add if lt.is_string_literal() && rt.is_string_literal() => Some(
                match (
                    lt.kind().string_literal_text(),
                    rt.kind().string_literal_text(),
                ) {
                    (Some(left), Some(right)) => Ty::string_literal_of(&format!("{left}{right}")),
                    _ => Ty::string_literal(),
                },
            ),
            // Literal arithmetic, bitwise logic, and shifts keep the literal
            // type; the same operators beside a `SIMD` are its dunders above.
            Add | Sub | Mul | FloorDiv
                if common.as_ref().is_some_and(|ty| {
                    matches!(ty.kind(), TyKind::IntLiteral | TyKind::FloatLiteral)
                }) =>
            {
                common
            }
            Shl | Shr | BitAnd | BitOr | BitXor
                if common.as_ref().is_some_and(|ty| *ty == TyKind::IntLiteral) =>
            {
                common
            }
            // `%` and `**` keep the common type until their `SIMD` dunders
            // land (R1623, R1567).
            Mod | Pow => common,
            // Literal division stays exact until a runtime context chooses
            // Float64; concrete operands perform fixed-width runtime division.
            Div if common.is_some() => Some(
                if matches!(
                    common.as_ref().map(Ty::kind),
                    Some(TyKind::IntLiteral | TyKind::FloatLiteral)
                ) {
                    Ty::float_literal()
                } else {
                    Ty::float64()
                },
            ),
            // Ordering between numbers, or between `Bool`s (upstream's `Bool`
            // is `Comparable`).
            Lt | Gt | Le | Ge if common.is_some() || (lt == rt && lt == TyKind::Bool) => {
                Some(Ty::bool())
            }
            // Equality: between numbers (any common type), or equal non-numeric
            // scalars (Bool/String/None).
            Eq | Ne
                if common.is_some()
                    || (lt == rt && is_scalar(&lt))
                    || (lt.is_string_literal() && rt.is_string_literal()) =>
            {
                Some(Ty::bool())
            }
            _ => None,
        };
        if let Some(ty) = result {
            return Ok(ty);
        }
        let Some(span) = span else {
            return Err(bad_infix_operator(op, &lt, &rt));
        };
        // A string literal before a nominal `String` converts to `String`
        // through the `@implicit` literal constructor, whose dunder the
        // operator then calls.
        if lt.is_string_literal()
            && self.is_nominal_string(&rt)
            && op.dunder().is_some()
            && self.record_implicit_conversion(left, &lt, &rt)?
            && let Some(ty) = self.infer_operator_call(&span, op, left, right, &rt, None)?
        {
            return Ok(ty);
        }
        if let Some(ty) = self.infer_operator_call(&span, op, left, right, &lt, Some(&rt))? {
            return Ok(ty);
        }
        Err(bad_infix_operator(op, &lt, &rt))
    }

    /// The common type two values merging into one result take (a
    /// conditional expression's arms, `and`/`or`'s operands), as upstream's
    /// `coerceTypesToEachOther` finds it: their shared or numeric common
    /// type, else the left's type when the right converts to it implicitly,
    /// else the right's when the left does; nothing converts to a type
    /// parameter it is not. Each value's conversion to it is
    /// recorded at the value, a literal's materialization included. `None`
    /// when neither converts.
    pub(super) fn coerce_to_each_other(
        &self,
        left: &Expr,
        lt: &Ty,
        right: &Expr,
        rt: &Ty,
    ) -> Result<Option<Ty>, TypeError> {
        if let Some(ty) = common_branch_ty(lt, rt) {
            for (value, value_ty) in [(left, lt), (right, rt)] {
                if *value_ty != ty {
                    self.record_literal_materializations(value, value_ty, &ty)?;
                }
            }
            return Ok(Some(ty));
        }
        // A type parameter is rigid in its body: a value converts to it
        // only by being of it, which the common type above covers.
        let converts = |value: &Expr, from: &Ty, to: &Ty| {
            if matches!(to.kind(), TyKind::Param { .. }) {
                Ok(false)
            } else {
                self.record_implicit_conversion(value, from, to)
            }
        };
        if converts(right, rt, lt)? {
            return Ok(Some(lt.clone()));
        }
        Ok(converts(left, lt, rt)?.then(|| rt.clone()))
    }

    /// Type `left and right` or `left or right` as upstream's `emitAndOr`
    /// does: the left operand is tested through its `Bool(x)` truthiness,
    /// a `Truthiness` mark on it where it is not a `Bool`, and the result is
    /// the operand the test selects. Operands with a common type
    /// ([`Self::coerce_to_each_other`]) yield that type (`n or 2` is an
    /// `Int`), each place operand copied into the result; operands with
    /// none are each converted to `Bool`, which is then the result, and
    /// read in place for the conversion. A left operand converting to the
    /// right's type is tested after its conversion here, where upstream
    /// converts it in its arm, so only a literal's materialization is
    /// accepted.
    fn infer_short_circuit(
        &self,
        span: Option<SourceSpan>,
        op: InfixOp,
        left: &Expr,
        right: &Expr,
    ) -> Result<Ty, TypeError> {
        let lt = self.infer(left)?;
        let rt = self.infer(right)?;
        if lt == TyKind::Bool && rt == TyKind::Bool {
            return Ok(Ty::bool());
        }
        let Some(span) = span else {
            return Err(bad_infix_operator(op, &lt, &rt));
        };
        let context = format!("operand of '{}'", op.symbol());
        let Some(ty) = self.coerce_to_each_other(left, &lt, right, &rt)? else {
            self.expect_bool(left, &context)?;
            self.expect_bool(right, &context)?;
            return Ok(Ty::bool());
        };
        if ty != lt && !matches!(lt.kind(), TyKind::IntLiteral | TyKind::FloatLiteral) {
            return Err(TypeError::Unsupported(format!(
                "a left operand of '{}' converted to the right operand's type '{ty}' is \
                 not supported yet (roadmap R1602)",
                op.symbol()
            )));
        }
        if self.condition_truthiness(&lt, &context)? {
            self.record_at(left.source_span()).truthiness_condition = true;
        }
        for (operand, operand_ty) in [(left, &lt), (right, &rt)] {
            if self.designates_place(operand) && self.is_copyable(operand_ty) {
                if !self.is_implicitly_copyable(operand_ty) {
                    return Err(TypeError::ImplicitCopy {
                        ty: operand_ty.to_string(),
                        context,
                        transferable: true,
                        copyable: true,
                    });
                }
                self.record_at(operand.source_span()).copy_place_value_use = true;
            }
        }
        if self.register_passability(&ty) == RegisterPassability::Proven {
            self.record_at(span).register_passable_result = true;
        }
        Ok(ty)
    }

    /// The type a right `SIMDLength` operand takes beside a left `lt`:
    /// beside a SIMD operand, that operand's type, and after a literal, which
    /// materializes to `Int`, `Int`, through upstream's `@implicit
    /// SIMD.__init__(SIMDLength)`, so the left operand's dunder takes it
    /// (`n * w`, `2 * w`). `rt` itself for any other pair.
    fn simd_length_operand(&self, right: &Expr, lt: &Ty, rt: Ty) -> Result<Ty, TypeError> {
        if !mojito_types::types::is_simd_length(&rt) {
            return Ok(rt);
        }
        let target = match lt.kind() {
            TyKind::Simd { .. } => lt.clone(),
            TyKind::IntLiteral => Ty::int(),
            _ => return Ok(rt),
        };
        Ok(if self.record_implicit_conversion(right, &rt, &target)? {
            target
        } else {
            rt
        })
    }

    /// Type a chained comparison `a < b < c`: each adjacent pair must
    /// compare to a `Bool`, as a single comparison does, and each link is
    /// its dunder's call. The chain's record holds, per link, the dunder
    /// that link selected ([`CheckedCompareLink`]): `infer_infix` records
    /// it at the chain's own occurrence, as it would at a lone operator,
    /// and the link takes it from there. Every operand a link's dunder
    /// reads is read in place, and a temporary end is its link's read
    /// temporary, destroyed once that link's dunder returns. Only the first
    /// middle operand outlives its link: right after the first link it
    /// becomes an owned temporary for the second, so a place there is
    /// copied, which demands `ImplicitlyCopyable` of it; a temporary or a
    /// `^` is already owned. Later middles are read in place by both of
    /// their links. Every middle is the chain's, not a link's temporary.
    pub(super) fn infer_compare_chain(
        &self,
        span: SourceSpan,
        first: &Expr,
        rest: &[(InfixOp, Expr)],
    ) -> Result<Ty, TypeError> {
        let mut left = first;
        let mut links = Vec::with_capacity(rest.len());
        for (op, right) in rest {
            let membership = matches!(op, InfixOp::In | InfixOp::NotIn);
            let site = (!membership).then(|| span.clone());
            if self.infer_infix(site, *op, left, right)? != TyKind::Bool {
                return Err(TypeError::BadOperator {
                    op: op.symbol().to_string(),
                    operands: "a chained comparison must compare to Bool".to_string(),
                });
            }
            let target = self.clear_fact(&span, |facts| facts.overload_target.take());
            let call = self.clear_fact(&span, |facts| facts.selected_call.take());
            self.clear_fact(&span, |facts| facts.declared_result.take());
            let instantiated_args = self
                .clear_fact(&span, |facts| facts.method_instantiation.take())
                .map(|instantiation| instantiation.arguments)
                .unwrap_or_default();
            self.call_parameters.borrow_mut().remove(&span);
            // The method call reads its receiver where it lies through its
            // retained place; a link reads it as an operator operand.
            if call
                .as_ref()
                .is_some_and(|call| reads(call.receiver_convention))
            {
                self.borrow_operator_operand(left, &self.chain_operand_ty(left)?);
            }
            links.push(CheckedCompareLink {
                target,
                call,
                instantiated_args,
            });
            left = right;
        }
        for (_, middle) in rest.split_last().map_or(&[][..], |(_, middles)| middles) {
            self.clear_flag(&middle.source_span(), |facts| {
                &mut facts.read_temporary_argument
            });
        }
        if let [(_, middle), _, ..] = rest {
            if self.designates_place(middle) {
                self.demand_chain_middle_copy(middle)?;
            }
            if self.designates_place(middle) || matches!(middle.kind, ExprKind::Transfer(_)) {
                self.demand_chain_middle_destroyable(middle)?;
            }
        }
        self.record_at(span).compare_links = Some(links);
        Ok(Ty::bool())
    }

    /// Type `left op right` over a struct or a type parameter as upstream's
    /// `emitBinOpCall` does: the receiver `lt`'s dunder call
    /// `left.__op__(right)`, else, when the right operand has a type `rt`,
    /// its reflected dunder call `right.__rop__(left)`, marked reflected so
    /// MIR still evaluates the operands in source order. `None` when
    /// neither selects a candidate.
    fn infer_operator_call(
        &self,
        span: &SourceSpan,
        op: InfixOp,
        left: &Expr,
        right: &Expr,
        lt: &Ty,
        rt: Option<&Ty>,
    ) -> Result<Option<Ty>, TypeError> {
        if let Some(dunder) = op.dunder()
            && (self.has_operator_methods(lt)
                || self.declared_receiver_member(lt, dunder).is_some())
            && let Some(ty) = self.infer_operator_method_call(
                span,
                left,
                lt,
                dunder,
                std::slice::from_ref(right),
            )?
        {
            return Ok(Some(ty));
        }
        let (Some(reflected), Some(rt)) = (op.reflected_dunder(), rt) else {
            return Ok(None);
        };
        if !self.has_operator_methods(rt) && self.declared_receiver_member(rt, reflected).is_none()
        {
            return Ok(None);
        }
        let ty = self.infer_operator_method_call(
            span,
            right,
            rt,
            reflected,
            std::slice::from_ref(left),
        )?;
        if ty.is_some() {
            self.record_at(span.clone()).adjustment =
                Some(Box::new(SemanticAdjustment::ReflectedOperator));
        }
        Ok(ty)
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
        let nominal_ok = match rt.kind() {
            TyKind::Struct(name, _) if !self.structs.contains_key(name) => {
                if let Some(element) = list_element(rt).or_else(|| set_element(rt)) {
                    coerces(lt, element) && self.is_list_equatable(element)
                } else if let Some((key, _)) = dict_elements(rt) {
                    coerces(lt, key) && self.is_list_equatable(key)
                } else if let Some(elements) = tuple_elements(rt) {
                    elements
                        .into_iter()
                        .any(|element| coerces(lt, element) && self.is_list_equatable(element))
                } else {
                    false
                }
            }
            _ => false,
        };
        let ok = nominal_ok
            || match rt.kind() {
                TyKind::Tuple(_) => match lt.kind() {
                    TyKind::Tuple(elements) => self.tuple_elements_equatable(elements),
                    _ => self.is_list_equatable(lt),
                },
                TyKind::StringLiteral(_) => lt.is_string_literal(),
                _ => false,
            };
        if ok {
            return Ok(Ty::bool());
        }
        // `x in c` on a user struct, or on a `SIMD` whose struct declares it,
        // dispatches to the container's `__contains__` (`c.__contains__(x)`),
        // which must return `Bool`.
        if let Some(span) = span
            && (matches!(rt.kind(), TyKind::Struct(name, _) if self.structs.contains_key(name))
                || self.declared_receiver_member(rt, "__contains__").is_some())
        {
            let ret = self.infer_method_call(
                &span,
                right,
                "__contains__",
                MethodCallArguments::ordinary(std::slice::from_ref(left), &[]),
            )?;
            return require_dunder_ret(ret, &Ty::bool(), "__contains__");
        }
        if let Some(r) = self.struct_dunder(rt, "__contains__", &[lt]) {
            return r.and_then(|ret| require_dunder_ret(ret, &Ty::bool(), "__contains__"));
        }
        Err(TypeError::BadOperator {
            op: op.symbol().to_string(),
            operands: format!("{lt} and {rt}"),
        })
    }

    /// Record the value of `expr`, the comparison `left op right` of a
    /// `SIMD` vector in a compile-time position, as the node's parameter
    /// value: upstream's dunder folded (`infix_value`), whose `comptime
    /// assert` the fold drops, never an operator over lanes.
    pub(super) fn record_folded_comparison(
        &self,
        expr: &Expr,
        op: InfixOp,
        left: &Expr,
        right: &Expr,
    ) {
        use InfixOp::{Eq, Ge, Gt, Le, Lt, Ne};
        if self.comptime_positions.get() == 0 || !matches!(op, Eq | Ne | Lt | Gt | Le | Ge) {
            return;
        }
        let vector = |operand: &Expr| {
            self.compile_dependent_ct_expr(operand)
                .ok()
                .and_then(|built| built.meta().as_value().cloned())
                .is_some_and(|ty| {
                    matches!(ty.kind(), TyKind::Simd { .. })
                        && !mojito_types::types::has_host_scalar_value(ty.kind())
                })
        };
        if (vector(left) || vector(right))
            && let Ok(value) = self.compile_dependent_ct_expr(expr)
        {
            self.record_member_value(expr.source_span(), value);
        }
    }

    /// Record at `span` the `comptime assert Self.length == 1, message` of
    /// the builtin `SIMD` dunder or constructor applied there, over `width`
    /// lanes, for each reached instance to judge. A width-one application
    /// holds it already, and a compile-time position folds the application,
    /// dropping it, as the pin's fold of a builtin call does.
    pub(super) fn record_instance_assertion(
        &self,
        span: &SourceSpan,
        width: &SimdWidth,
        message: &str,
    ) -> Result<(), TypeError> {
        if matches!(width, SimdWidth::Known(1)) || self.comptime_positions.get() != 0 {
            return Ok(());
        }
        let lanes = match width {
            SimdWidth::Known(lanes) => ConstraintOperand::Value(CtValue::Int(*lanes)),
            SimdWidth::Expr(_) => {
                ConstraintOperand::Expr(width.lanes(&self.param_context).map_err(param_error)?)
            }
        };
        let holds = GenericConstraint::Eq(lanes, ConstraintOperand::Value(CtValue::Int(1)));
        self.record_instance_constraint(span, holds, message);
        Ok(())
    }

    /// Record at `span` the assertion that `holds`, failing with `message`,
    /// for each reached instance to judge, once.
    pub(super) fn record_instance_constraint(
        &self,
        span: &SourceSpan,
        holds: GenericConstraint,
        message: &str,
    ) {
        let assertion = GenericConstraint::WithMessage(Box::new(holds), message.to_string());
        let mut record = self.record_at(span.clone());
        let assertions = record.instance_assertions.get_or_insert_with(Vec::new);
        if !assertions.contains(&assertion) {
            assertions.push(assertion);
        }
    }

    /// Type `SIMD[DType.<dt>, width](args)`: `width` element arguments, or a
    /// single argument that splats across all lanes; each must fit the dtype
    /// ([`Self::check_simd_args`]).
    pub(super) fn infer_simd_construction(
        &self,
        span: &SourceSpan,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        let (dtype, mut width, attributes) = self.simd_dims(param_args)?;
        self.record_bracket_attributes(span.clone(), attributes);
        if width.is_inferred() {
            width = self.argument_simd_width(&dtype, args)?;
        }
        self.construct_simd(span, &dtype, &width, args)
    }

    /// Check the construction of `SIMD[dtype, width]` from `args`.
    pub(super) fn construct_simd(
        &self,
        span: &SourceSpan,
        dtype: &SimdDtype,
        width: &SimdWidth,
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        let ty = simd_of(dtype.clone(), width.clone())?;
        if let [arg] = args
            && self.simd_length_construction(span, &ty, arg, &self.infer(arg)?)?
        {
            return Ok(ty);
        }
        self.check_simd_args(dtype, width, args)?;
        Ok(ty)
    }

    /// The closed dimensions of a module vector alias
    /// (`comptime U256 = SIMD[DType.uint64, 4]`), which a call constructs
    /// as the spelled `SIMD[DType.uint64, 4](...)` does. The elaborator
    /// writes that spelling in every clone; source validation meets the
    /// alias itself.
    pub(super) fn vector_alias(&self, name: &str) -> Option<(Dtype, i64)> {
        let alias = self.comptime_aliases.get(name)?;
        let AliasBody::Type(ty) = &alias.body else {
            return None;
        };
        match *ty.kind() {
            TyKind::Simd {
                dtype: SimdDtype::Known(dtype),
                width: SimdWidth::Known(width),
            } if alias.decls.is_empty() => Some((dtype, width)),
            _ => None,
        }
    }

    /// Type a vector alias's construction `U256(a, b, c, d)` as the spelled
    /// `SIMD[DType.uint64, 4](a, b, c, d)` ([`Self::vector_alias`]).
    pub(super) fn infer_vector_alias_construction(
        &self,
        name: &str,
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        let (dtype, width) = self
            .vector_alias(name)
            .ok_or_else(|| TypeError::UndefinedVariable(name.to_string()))?;
        let (dtype, width) = (SimdDtype::Known(dtype), SimdWidth::Known(width));
        self.check_simd_args(&dtype, &width, args)?;
        simd_of(dtype, width)
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
        self.check_simd_args(&SimdDtype::Known(dtype), &SimdWidth::Known(1), args)?;
        Ok(TyKind::Simd {
            dtype: SimdDtype::Known(dtype),
            width: SimdWidth::Known(1),
        }
        .into())
    }

    /// Check the arguments of a SIMD construction against upstream's
    /// `SIMD.__init__` overloads: none (all lanes zero), exactly one, or one
    /// per lane. A symbolic width fixes no count and a symbolic dtype gates
    /// nothing: both are the instantiation's to check.
    ///
    /// One argument binds a one-value overload: the cast from a
    /// `SIMD[other_dtype, Self.length]` (a scalar reaching every lane through
    /// the implicit splat), the `IntLiteral` and `FloatLiteral` splats, and,
    /// for a scalar only, `__init__[T: Intable]`, whose result is
    /// `Scalar[target_dtype]`. Several bind `*elems: Scalar[Self.dtype]`, so
    /// each element is already the lane's scalar or converts to it
    /// implicitly ([`splats_to`]): a literal, a one-lane value of the dtype,
    /// or a `Bool` into a mask.
    pub(super) fn check_simd_args(
        &self,
        dtype: &SimdDtype,
        width: &SimdWidth,
        args: &[Expr],
    ) -> Result<(), TypeError> {
        if let Some(width) = width.known()
            && args.len() != width as usize
            && args.len() > 1
        {
            return Err(TypeError::SimdArity {
                width,
                got: args.len(),
            });
        }
        if let [arg] = args {
            return self.check_simd_value(dtype, width, arg);
        }
        for arg in args {
            let aty = self.infer(arg)?;
            if !splats_to(&aty, dtype) {
                return Err(simd_argument_error(dtype, width, &aty));
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
        let nominal_string = matches!(aty.kind(), TyKind::Struct(name, args)
            if args.is_empty() && mojito_symbol::symbol::is_stdlib_string_struct(name));
        if !aty.is_string_literal() && !nominal_string {
            return Err(TypeError::TypeMismatch {
                expected: "String".to_string(),
                found: aty.to_string(),
                context: "argument to 'Error'".to_string(),
            });
        }
        Ok(Ty::error())
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
            self.record_pointer_adjustment(
                span,
                SemanticAdjustment::PointerToPlace {
                    mutable,
                    materialized: None,
                },
            );
            return Ok(TyKind::Pointer {
                element: reference.referent,
                origin,
            }
            .into());
        }
        if let ExprKind::Identifier(name) = &value.kind
            && (matches!(self.lookup(name).map(Ty::kind), Some(TyKind::Ref(_)))
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
                // A union of places under one owner (`ref[self.a, self.b]`)
                // designates some descendant of their deepest common base.
                Origin::Union(members) if let Some(place) = union_subtree_place(&members) => {
                    PointerOrigin::Place {
                        place,
                        mutable: matches!(reference.mutability, Mutability::Mutable),
                    }
                }
                other => {
                    return Err(TypeError::Unsupported(format!(
                        "Pointer(to=...) through a 'ref' binding requires a place or \
                         origin-parameter referent; a {other} origin is not supported"
                    )));
                }
            };
            let mutable = origin.statically_mutable() == Some(true);
            self.record_pointer_adjustment(
                span,
                SemanticAdjustment::PointerToPlace {
                    mutable,
                    materialized: None,
                },
            );
            return Ok(TyKind::Pointer {
                element: reference.referent,
                origin,
            }
            .into());
        }
        // A call, or a subscript's accessor call, is a place only through
        // the reference its inference records (`w.get()` returning
        // `ref[self.x] Int`); any other operand is a place before it is read.
        let call = matches!(
            value.kind,
            ExprKind::Call { .. }
                | ExprKind::MethodCall { .. }
                | ExprKind::Invoke { .. }
                | ExprKind::Index { .. }
        );
        let inferred = call.then(|| self.infer(value)).transpose()?;
        let place = self.origin_place(value).map_err(|error| match error {
            TypeError::UndefinedVariable(_) => error,
            _ => TypeError::Unsupported("Pointer(to=...) requires a place expression".to_string()),
        })?;
        let element = inferred.map_or_else(|| self.infer(value), Ok)?;
        // A place an accessor returns by reference (`t[0]` on a `Tuple`) is
        // the pointer's referent: the handle is retained, never read out.
        if let Some(reference) = self.infer_reference_value(value) {
            self.record_at(value.source_span()).reference_value_use =
                Some(reference.mutability == mojito_types::origin::Mutability::Mutable);
        }
        let mutable = self.owner_is_mutable(place.root);
        self.record_pointer_adjustment(
            span,
            SemanticAdjustment::PointerToPlace {
                mutable,
                materialized: None,
            },
        );
        Ok(TyKind::Pointer {
            element,
            origin: mojito_types::origin::PointerOrigin::Place { place, mutable },
        }
        .into())
    }

    /// The width an unbound `SIMD[dtype, _]` construction binds from its
    /// arguments, as upstream's one-value overloads do: a `SIMD[other_dtype,
    /// Self.length]` argument gives its own width (a literal materializes to
    /// a scalar), and an `Intable` one binds `__init__[T: Intable]`, a scalar
    /// whose `comptime assert` wants an integral dtype. No overload binds
    /// `_` from an element count. Each is this rule's until a `_` width
    /// binds the declared overloads (R1544).
    fn argument_simd_width(
        &self,
        dtype: &SimdDtype,
        args: &[Expr],
    ) -> Result<SimdWidth, TypeError> {
        let no_match = || TypeError::BadCall {
            func: "SIMD".to_string(),
            reason: "no matching function in initialization: an unbound '_' width binds \
                     only from a single argument's own width"
                .to_string(),
        };
        let [arg] = args else {
            return Err(no_match());
        };
        let aty = self.infer(arg)?;
        match aty.kind() {
            TyKind::IntLiteral | TyKind::FloatLiteral => Ok(SimdWidth::Known(1)),
            TyKind::Simd { width, .. } if !width.is_inferred() => Ok(width.clone()),
            TyKind::Bool | TyKind::Param { .. } | TyKind::Struct(..)
                if !mojito_types::types::is_simd_length(&aty)
                    && (*aty == TyKind::Bool || self.conforms_to(&aty, "Intable")) =>
            {
                if !dtype.licenses(|d| d != Dtype::Bool && !d.is_float()) {
                    return Err(TypeError::BadCall {
                        func: "SIMD".to_string(),
                        reason: format!(
                            "constraint failed: {}",
                            mojito_types::types::INTABLE_CONVERSION_MESSAGE
                        ),
                    });
                }
                Ok(SimdWidth::Known(1))
            }
            _ => Err(no_match()),
        }
    }

    /// Check the one argument of a SIMD construction: the cast from another
    /// dtype ([`converts_to_lane`], a scalar splatting across the width), or
    /// a scalar's construction from an `Intable` value through its
    /// `__int__`. Only `SIMD[dt, _](x)` reaches the `Intable` clause: every
    /// spelled width binds the declared `__init__[T: Intable]` (R1544).
    fn check_simd_value(
        &self,
        dtype: &SimdDtype,
        width: &SimdWidth,
        arg: &Expr,
    ) -> Result<(), TypeError> {
        let aty = self.infer(arg)?;
        if converts_to_lane(&aty, dtype) {
            return Ok(());
        }
        let intable_object = width.known() == Some(1)
            && dtype.licenses(|d| d != Dtype::Bool && !d.is_float())
            && matches!(aty.kind(), TyKind::Param { .. } | TyKind::Struct(..))
            && self.conforms_to(&aty, "Intable");
        if !intable_object {
            return Err(simd_argument_error(dtype, width, &aty));
        }
        self.borrow_nominal_place_argument(arg, &aty);
        self.borrow_parameter_place_argument(arg, &aty);
        self.record_read_temporary_argument(arg);
        Ok(())
    }

    /// A read dunder binds its operand as a method call binds a read
    /// argument: a named struct place, or a struct element an accessor
    /// returns by reference (`xs[0]` through `List.__getitem__`), is read
    /// where it lies rather than copied out.
    fn borrow_operator_operand(&self, operand: &Expr, ty: &Ty) {
        self.borrow_nominal_place_argument(operand, ty);
        self.borrow_parameter_place_argument(operand, ty);
        if matches!(operand.kind, ExprKind::Index { .. })
            && matches!(ty.kind(), TyKind::Struct(name, _) if self.structs.contains_key(name))
            && self.infer_reference_value(operand).is_some()
        {
            self.record_at(operand.source_span())
                .borrowed_read_call_place = true;
        }
    }

    /// Whether `ty` is a struct the program declares, whose operators are
    /// its dunders' method calls.
    fn is_checked_struct(&self, ty: &Ty) -> bool {
        matches!(ty.kind(), TyKind::Struct(name, _) if self.structs.contains_key(name))
    }

    /// Whether `ty`'s operators are method calls: a struct the program
    /// declares, or a type parameter or projection whose bounds declare
    /// requirements.
    fn has_operator_methods(&self, ty: &Ty) -> bool {
        self.is_checked_struct(ty) || is_bound_archetype(ty)
    }

    /// Whether `ty` is the declared nominal `String`.
    fn is_nominal_string(&self, ty: &Ty) -> bool {
        matches!(ty.kind(), TyKind::Struct(name, args)
            if args.is_empty()
                && mojito_symbol::symbol::is_stdlib_string_struct(name)
                && self.structs.contains_key(name))
    }

    /// The type an inferred chain operand takes at its link, as
    /// [`Self::infer_infix`] sees it.
    fn chain_operand_ty(&self, operand: &Expr) -> Result<Ty, TypeError> {
        let ty = self.infer(operand)?;
        Ok(self.opaque_element(&ty).unwrap_or(ty))
    }

    /// A chain's first middle place or `^` becomes the owned temporary its
    /// second link reads, which the body must be able to destroy: one whose
    /// [`Self::destruction_verdict`] denies it is abandoned, as upstream
    /// rejects it.
    fn demand_chain_middle_destroyable(&self, operand: &Expr) -> Result<(), TypeError> {
        let ty = self.infer(operand)?;
        self.check_overwritten_referent("(expression temporary)", &ty)
    }

    /// A chain's first middle place is copied into the temporary its
    /// second link reads, as upstream copies it.
    fn demand_chain_middle_copy(&self, operand: &Expr) -> Result<(), TypeError> {
        let ty = self.chain_operand_ty(operand)?;
        if self.is_implicitly_copyable(&ty) {
            return Ok(());
        }
        Err(TypeError::ImplicitCopy {
            context: "middle operand of a chained comparison".to_string(),
            transferable: self.is_movable(&ty),
            copyable: self.is_copyable(&ty),
            ty: ty.to_string(),
        })
    }
}

/// Whether `ty` is an archetype whose methods its bounds' requirements
/// declare: a type parameter or an associated projection of one.
fn is_bound_archetype(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::Param { .. } | TyKind::Assoc { .. })
}

/// The error a SIMD construction argument of type `aty` no overload takes
/// reports: a float literal binds `@implicit __init__(FloatLiteral)` and
/// fails its float-dtype assert, anything else matches no overload.
fn simd_argument_error(dtype: &SimdDtype, width: &SimdWidth, aty: &Ty) -> TypeError {
    if *aty == TyKind::FloatLiteral && dtype.known().is_some_and(|dtype| !dtype.is_float()) {
        let target = Ty::from(TyKind::Simd {
            dtype: dtype.clone(),
            width: width.clone(),
        });
        return super::builtins::float_literal_conversion_error(&target);
    }
    TypeError::TypeMismatch {
        expected: format!("a {dtype} element"),
        found: aty.to_string(),
        context: "SIMD element".to_string(),
    }
}

/// The error an infix operator its operands do not define reports. A KGEN
/// string is an attribute that implements no operator method.
fn bad_infix_operator(op: InfixOp, lt: &Ty, rt: &Ty) -> TypeError {
    if let (TyKind::KgenString, Some(method)) = (lt.kind(), op.dunder()) {
        return TypeError::MatchPattern(format!(
            "'{}' does not implement the '{method}' method",
            mojito_types::types::KGEN_STRING_TYPE_SPELLING
        ));
    }
    TypeError::BadOperator {
        op: op.symbol().to_string(),
        operands: format!("{lt} and {rt}"),
    }
}

/// The error a selected dunder raises, if it raises.
fn dunder_error(sig: &MethodSig) -> Option<Ty> {
    sig.raises
        .then(|| sig.error.clone().unwrap_or_else(Ty::error))
}

/// Whether a dunder's convention for one operand reads it.
const fn reads(convention: Option<ArgConvention>) -> bool {
    matches!(convention, None | Some(ArgConvention::Imm))
}

/// The subtree of the deepest common base of a union of places that share one
/// owner, or `None` when a member is not a place or the owners differ.
fn union_subtree_place(
    members: &[mojito_types::origin::Origin],
) -> Option<mojito_types::origin::OriginPlace> {
    use mojito_types::origin::{Origin, OriginPlace, OriginSeg};
    let places = members
        .iter()
        .map(|member| match member {
            Origin::Place(place) => Some(place),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    let (first, rest) = places.split_first()?;
    if rest.iter().any(|place| place.root != first.root) {
        return None;
    }
    let common = first
        .path
        .iter()
        .enumerate()
        .take_while(|(index, segment)| {
            rest.iter()
                .all(|place| place.path.get(*index) == Some(segment))
        })
        .count();
    let mut path = first.path[..common].to_vec();
    path.push(OriginSeg::Subtree);
    Some(OriginPlace {
        root: first.root,
        path,
    })
}

/// Type a SIMD operator that is not yet a declared dunder (`%` and `/`,
/// R1623 and R184), or a comparison in a compile-time position,
/// which is the dunder folded, a `Bool`. Both operands must be the same
/// SIMD type, except a numeric *literal* splats to the other operand's
/// type. Arithmetic is elementwise and keeps the operand type.
fn simd_infix(op: InfixOp, lt: &Ty, rt: &Ty) -> Result<Ty, TypeError> {
    use InfixOp::{Div, Eq, Ge, Gt, Le, Lt, Mod, Ne};
    let bad = || TypeError::BadOperator {
        op: op.symbol().to_string(),
        operands: format!("{lt} and {rt}"),
    };
    // Determine the common SIMD type, allowing a numeric literal on one
    // side. The slots may be symbolic: identity is structural, and a
    // dtype gate a symbolic lane cannot answer is the instantiation's.
    let (dtype, width) = match (lt.kind(), rt.kind()) {
        (
            TyKind::Simd {
                dtype: d1,
                width: w1,
            },
            TyKind::Simd {
                dtype: d2,
                width: w2,
            },
        ) if d1 == d2 && w1 == w2 => (d1.clone(), w1.clone()),
        (TyKind::Simd { dtype, width }, other) | (other, TyKind::Simd { dtype, width })
            if splats_to(other, dtype) =>
        {
            (dtype.clone(), width.clone())
        }
        _ => return Err(bad()),
    };
    let same = || simd_of(dtype.clone(), width.clone());
    match op {
        // Remainder on integer lanes (upstream defines it on float lanes
        // too, R850).
        Mod if dtype.licenses(|d| d != Dtype::Bool && !d.is_float()) => same(),
        // True division is defined on float lanes only.
        Div if dtype.licenses(Dtype::is_float) => same(),
        // A comparison in a compile-time position is the dunder folded
        // ([`Checker::record_folded_comparison`]), a `Bool`.
        Eq | Ne | Lt | Gt | Le | Ge => Ok(Ty::bool()),
        _ => Err(bad()),
    }
}

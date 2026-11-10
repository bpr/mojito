//! Operator, SIMD-construction, and pointer-construction type inference
//! for the checker. Extracted from `checker.rs`; see `docs/symbol-map.md`.

use super::traits::RegisterPassability;
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::{CheckedCompareLink, SemanticAdjustment};
use mojito_types::types::TyKind;

/// The type two merging values share, as upstream's `findCommonType`
/// reports it.
pub(super) enum CommonType {
    Common(Ty),
    /// Each value converts to the other's type, or the two convert to
    /// different materialization targets.
    Ambiguous,
    /// No step finds a type both values convert to.
    Incompatible,
}

impl CommonType {
    /// The common type, or the merge's error over the `left` and `right`
    /// value types.
    pub(super) fn into_result(self, left: &Ty, right: &Ty) -> Result<Ty, TypeError> {
        match self {
            Self::Common(ty) => Ok(ty),
            Self::Ambiguous => Err(TypeError::AmbiguousMerge {
                left: left.to_string(),
                right: right.to_string(),
            }),
            Self::Incompatible => Err(TypeError::IncompatibleMerge {
                left: left.to_string(),
                right: right.to_string(),
            }),
        }
    }
}

impl Checker {
    pub(super) fn infer_prefix(
        &self,
        span: SourceSpan,
        op: PrefixOp,
        operand: &Expr,
    ) -> Result<Ty, TypeError> {
        let t = self.infer(operand)?;
        // `-x`, `~x`, and `+x` over a `SIMD` or a `Bool` are its declared
        // `x.__neg__()`, `x.__invert__()`, and `x.__pos__()`, as upstream's
        // `UnaryOpNode::emitArith` emits them.
        if op != PrefixOp::Not
            && self.declared_receiver_member(&t, op.dunder()).is_some()
            && let Some(ty) =
                self.infer_operator_method_call(&span, operand, &t, op.dunder(), &[])?
        {
            return Ok(ty);
        }
        match (op, t.kind()) {
            // A literal's `-x`, `~x`, and `+x` are the literal its dunder's
            // signature types (`IntLiteral.__neg__` is `type_of(0 - self)`,
            // `__invert__` `type_of(self ^ -1)`, `FloatLiteral.__neg__`
            // `type_of(self * -1)`, `__pos__` `Self`), so `-2.5` is
            // `FloatLiteral[-2.5]`.
            (PrefixOp::Pos, kind) if kind.is_numeric_literal() => return Ok(t),
            (PrefixOp::Neg, kind) if kind.is_int_literal() => {
                return self.literal_rule_type(InfixOp::Sub, &Ty::int_literal_of(&0.into()), &t);
            }
            (PrefixOp::Neg, kind) if kind.is_float_literal() => {
                let minus_one = Ty::float_literal_of(
                    &mojito_common::literal::FloatLiteral::from_int(&(-1).into()),
                );
                return self.literal_rule_type(InfixOp::Mul, &t, &minus_one);
            }
            (PrefixOp::Invert, kind) if kind.is_int_literal() => {
                return self.literal_rule_type(
                    InfixOp::BitXor,
                    &t,
                    &Ty::int_literal_of(&(-1).into()),
                );
            }
            (PrefixOp::Not, kind) if kind.is_bool() => return Ok(Ty::bool()),
            // A numeric scalar is `Boolable`: `not n` over an `Int`, or
            // `not (a == b)` over `UInt64`s.
            (PrefixOp::Not, _) if is_numeric(&t) || is_scalar_simd(&t) => {
                return Ok(Ty::bool());
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
        // `not x` over a type parameter bounded by `Boolable` is
        // `not Bool(x)`, its `__bool__` requirement's conversion, which
        // reads a place where it lies.
        if op == PrefixOp::Not && self.param_has_bound(&t, "Boolable") {
            self.borrow_operator_operand(operand, &t);
            self.record_read_temporary_argument(operand);
            return Ok(Ty::bool());
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
                self.require_call_error(
                    span,
                    format!("raising operator '{}'", op.symbol()),
                    error,
                )?;
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
        let rt = self.infer(right)?;
        let rt = self.opaque_element(&rt).unwrap_or(rt);
        let rt = self.simd_length_operand(right, &lt, rt)?;
        let rt = self.literal_beside_simd_length(right, op, &lt, rt)?;

        // Membership `in` / `not in` — the right operand is a container.
        if matches!(op, In | NotIn) {
            return self.infer_membership(span, op, left, right, &lt, &rt);
        }
        // `+`, `-`, `*`, `/`, `//`, `**`, the bitwise operators, the shifts,
        // and the comparisons beside a `SIMD` operand, a scalar's included, are
        // `SIMD`'s declared dunders, `a.__add__(b)` else the reflected
        // `b.__radd__(a)`, as upstream's `emitBinOpCall` resolves them. A
        // comparison has no reflected dunder, so a literal left operand
        // materializes to its default type (`Int`, `Float64`), whose dunder
        // takes the right one. In a compile-time position the comparison's
        // value is that dunder folded (`infix_value`), which asserts nothing.
        // An unlinked program (a phase test's) declares no `SIMD`, so its
        // scalar comparisons stay the built-in ones below.
        let pointer_or_archetype =
            |ty: &Ty| matches!(ty.kind(), TyKind::Pointer { .. }) || is_bound_archetype(ty);
        let comparison = matches!(op, Eq | Ne | Lt | Gt | Le | Ge);
        if let Some(span) = &span
            && (matches!(
                op,
                Add | Sub | Mul | Div | FloorDiv | Mod | Pow | BitAnd | BitOr | BitXor | Shl | Shr
            ) || (comparison && self.structs.contains_key("SIMD")))
            && (lt.is_simd() || rt.is_simd())
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
        // An operator beside a `Bool` is `Bool`'s declared dunder,
        // `a.__and__(b)` else the reflected `b.__rand__(a)`, as upstream's
        // `emitBinOpCall` resolves it. In a compile-time position its value
        // is the shared folder's (`infix_value`).
        let declares = |ty: &Ty, dunder: Option<&str>| {
            ty.is_bool()
                && dunder.is_some_and(|dunder| self.declared_receiver_member(ty, dunder).is_some())
        };
        if let Some(span) = &span
            && (declares(&lt, op.dunder()) || declares(&rt, op.reflected_dunder()))
        {
            return self
                .infer_operator_call(span, op, left, right, &lt, Some(&rt))?
                .ok_or_else(|| bad_infix_operator(op, &lt, &rt));
        }
        // An operator over two literals is the left literal's declared
        // dunder, `a.__add__(b)` else the reflected `b.__radd__(a)`, as
        // upstream's `emitBinOpCall` resolves it, so its type carries the
        // value the dunder's attribute folds (`2 + 3` is `IntLiteral[5]`).
        // An operator neither literal declares (`2 / 3`) falls through to
        // the literal rule below. In a compile-time position its
        // value is the shared folder's (`infix_value`).
        if let Some(span) = &span
            && let Some(ty) = self.infer_literal_operator(span, op, left, right, &lt, &rt)?
        {
            return Ok(ty);
        }
        // `IntLiteral.__pow__` takes only an exponent its `where` clause
        // admits, and no `Int` power stands in for a rejected one: the
        // pair is that call's error.
        if matches!(op, Pow) && lt.is_int_literal() && rt.is_int_literal() {
            let call = super::initializer_list::spelled_node(
                left,
                u32::MAX,
                ExprKind::MethodCall {
                    object: Box::new(left.clone()),
                    method: "__pow__".to_string(),
                    args: vec![right.clone()],
                    kwargs: Vec::new(),
                },
                left.span,
            );
            return Err(self
                .probe(|| self.infer(&call))
                .err()
                .unwrap_or_else(|| bad_infix_operator(op, &lt, &rt)));
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
                (Add | Sub, kind) if kind.is_int() || kind.is_int_literal() => {
                    return Ok(lt.clone());
                }
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
                if let Some((dtype, width)) = lane.as_simd()
                    && width == SimdWidth::Known(1)
                    && dtype.is_symbolic()
                    && literal_ty.kind().is_numeric_literal()
                    && splats_to(literal_ty, &dtype)
                {
                    self.record_literal_materializations(literal, literal_ty, lane)?;
                }
            }
        }
        // A float literal's `**` is `Float64`'s `__pow__` (`4.0 ** 0.5`), its
        // left operand materialized, as upstream's `FloatLiteral` declares
        // none; an `IntLiteral` pair's is `IntLiteral.__pow__` above.
        if let Some(span) = &span
            && matches!(op, Pow)
            && common.as_ref().is_some_and(Ty::is_float_literal)
            && self.structs.contains_key("SIMD")
        {
            let receiver = mojito_types::types::default_literal(&lt);
            self.record_literal_materializations(left, &lt, &receiver)?;
            return self
                .infer_operator_call(span, op, left, right, &receiver, Some(&rt))?
                .ok_or_else(|| bad_infix_operator(op, &receiver, &rt));
        }
        if let Some(ty) = self.literal_rule(op, &lt, &rt)? {
            return Ok(ty);
        }
        // A literal pair no dunder takes (`2 < 2.5`, `7 // 2.5`) materializes
        // its left operand to its default type and calls that type's dunder,
        // as upstream's `emitBinOpCall` does once neither the left dunder nor
        // the reflected one (a comparison has none) applies.
        if let Some(span) = &span
            && numeric_literals(&lt, &rt)
            && self.structs.contains_key("SIMD")
        {
            let receiver = mojito_types::types::default_literal(&lt);
            self.record_literal_materializations(left, &lt, &receiver)?;
            return self
                .infer_operator_call(span, op, left, right, &receiver, Some(&rt))?
                .ok_or_else(|| bad_infix_operator(op, &receiver, &rt));
        }
        let result = match op {
            // Two literals concatenate through `StringLiteral.__add__`, except
            // in an unlinked program (a phase test's), whose two texts
            // concatenate here as the declared dunder's `#pop.string_concat`.
            Add if lt.is_string_literal()
                && rt.is_string_literal()
                && self.declared_receiver_member(&lt, "__add__").is_none() =>
            {
                Some(self.concatenated_literal(&lt, &rt)?)
            }
            // `%` and `/` over concrete operands keep their common type.
            Mod | Div => common,
            // Ordering between numbers.
            Lt | Gt | Le | Ge if common.is_some() => Some(Ty::bool()),
            // Equality: between numbers (any common type), or equal non-numeric
            // scalars (`None`, and a `Bool` in an unlinked program, which
            // declares no `Bool`). A string literal compares through
            // `StringLiteral`'s dunders, except in an unlinked program (a
            // phase test's), which declares none.
            Eq | Ne
                if common.is_some()
                    || (lt == rt && is_scalar(&lt) && !lt.is_string_literal())
                    || (lt.is_string_literal()
                        && rt.is_string_literal()
                        && self.declared_receiver_member(&lt, "__eq__").is_none()) =>
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
        if let Some(ty) = self.infer_operator_call(&span, op, left, right, &lt, Some(&rt))? {
            return Ok(ty);
        }
        Err(bad_infix_operator(op, &lt, &rt))
    }

    /// The common type of two values `lt` and `rt` merging into one result,
    /// as upstream's `findCommonType` finds it, in its order: equal types;
    /// the `contextual` type when both convert to it; the one type the
    /// other value converts to, ambiguous when each converts to the other;
    /// else the materialization target the other value converts to (the
    /// `Int` of `1 if c else 2`), ambiguous when both convert to different
    /// targets (`1 if c else 2.5`). Upstream's meta-type bound merge and
    /// `__merge_with__` steps are not modelled: no bundled struct declares
    /// `__merge_with__`.
    pub(super) fn common_type(
        &self,
        lt: &Ty,
        rt: &Ty,
        contextual: Option<&Ty>,
    ) -> Result<CommonType, TypeError> {
        if lt == rt {
            return Ok(CommonType::Common(lt.clone()));
        }
        if let Some(expected) = contextual
            && self.converts_implicitly(lt, expected)?
            && self.converts_implicitly(rt, expected)?
        {
            return Ok(CommonType::Common(expected.clone()));
        }
        match (
            self.converts_implicitly(lt, rt)?,
            self.converts_implicitly(rt, lt)?,
        ) {
            (true, true) => return Ok(CommonType::Ambiguous),
            (true, false) => return Ok(CommonType::Common(rt.clone())),
            (false, true) => return Ok(CommonType::Common(lt.clone())),
            (false, false) => {}
        }
        let left_target = self.merge_target(lt);
        let right_target = self.merge_target(rt);
        let right_converts = match &left_target {
            Some(target) => self.converts_implicitly(rt, target)?,
            None => false,
        };
        let left_converts = match &right_target {
            Some(target) => self.converts_implicitly(lt, target)?,
            None => false,
        };
        Ok(match (left_target, right_target) {
            (Some(left), Some(right)) if right_converts && left_converts => {
                if left == right {
                    CommonType::Common(left)
                } else {
                    CommonType::Ambiguous
                }
            }
            (Some(left), _) if right_converts => CommonType::Common(left),
            (_, Some(right)) if left_converts => CommonType::Common(right),
            _ => CommonType::Incompatible,
        })
    }

    /// Merge two values into one result as upstream's
    /// `coerceTypesToEachOther` does: find their [`Self::common_type`],
    /// record each value's conversion to it, and in a run-time context
    /// materialize a nonmaterializable common type (two equal literals
    /// merge at `Int`, `1 if c else 1.0` at `Float64`), each value then
    /// converting straight to the target.
    pub(super) fn coerce_to_each_other(
        &self,
        left: &Expr,
        lt: &Ty,
        right: &Expr,
        rt: &Ty,
        contextual: Option<&Ty>,
    ) -> Result<CommonType, TypeError> {
        let common = match self.common_type(lt, rt, contextual)? {
            CommonType::Common(common) => common,
            failed => return Ok(failed),
        };
        let ty = match self.merge_target(&common) {
            Some(target) if self.crosses_to_runtime() => target,
            _ => common,
        };
        for (value, value_ty) in [(left, lt), (right, rt)] {
            if *value_ty != ty {
                self.record_implicit_conversion(value, value_ty, &ty)?;
            }
        }
        Ok(CommonType::Common(ty))
    }

    /// Whether a value of type `from` converts implicitly to `to`, as a
    /// merge asks it: by coercion or a declared `@implicit` constructor. A
    /// type parameter is rigid in its body, so a value converts to it only
    /// by being of it.
    pub(super) fn converts_implicitly(&self, from: &Ty, to: &Ty) -> Result<bool, TypeError> {
        if matches!(to.kind(), TyKind::Param { .. }) {
            return Ok(from == to);
        }
        Ok(self.value_coerces(from, to) || self.implicit_conversion_target(from, to)?.is_some())
    }

    /// The type a merged value of `ty` materializes to: its
    /// `@__nonmaterializable` target, a numeric literal's default type on
    /// a seam that links no literal struct.
    pub(super) fn merge_target(&self, ty: &Ty) -> Option<Ty> {
        self.materialization_target(ty).or_else(|| {
            ty.kind()
                .is_numeric_literal()
                .then(|| mojito_types::types::default_literal(ty))
        })
    }

    /// Type `left and right` or `left or right` as upstream's `emitAndOr`
    /// does: the left operand is tested through its `Bool(x)` truthiness,
    /// a `Truthiness` mark on it where it is not a `Bool`, and the result is
    /// the operand the test selects. Operands with a common type
    /// ([`Self::coerce_to_each_other`]) yield that type (`n or 2` is an
    /// `Int`), each place operand copied into the result; operands with
    /// none, an ambiguous merge included (`0 or 2.5`), are each converted
    /// to `Bool`, which is then the result, and
    /// read in place for the conversion. A left operand converting to the
    /// common type is tested at its own type and converted in its arm.
    fn infer_short_circuit(
        &self,
        span: Option<SourceSpan>,
        op: InfixOp,
        left: &Expr,
        right: &Expr,
    ) -> Result<Ty, TypeError> {
        let lt = self.infer(left)?;
        let rt = self.infer(right)?;
        if lt.is_bool() && rt.is_bool() {
            return Ok(Ty::bool());
        }
        let Some(span) = span else {
            return Err(bad_infix_operator(op, &lt, &rt));
        };
        let context = format!("operand of '{}'", op.symbol());
        let CommonType::Common(ty) = self.coerce_to_each_other(left, &lt, right, &rt, None)? else {
            self.expect_bool(left, &context)?;
            self.expect_bool(right, &context)?;
            return Ok(Ty::bool());
        };
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
            kind if kind.is_simd() => lt.clone(),
            kind if kind.is_int_literal() => Ty::int(),
            _ => return Ok(rt),
        };
        Ok(if self.record_implicit_conversion(right, &rt, &target)? {
            target
        } else {
            rt
        })
    }

    /// The type a right literal operand takes after a left `SIMDLength`
    /// whose struct declares no dunder for `op`: `Int`, its default
    /// materialization, whose reflected dunder takes the left operand
    /// through upstream's `@implicit SIMD.__init__(SIMDLength)` (`n // 2`
    /// is `Int(2).__rfloordiv__(n)`). `rt` itself for any other pair.
    fn literal_beside_simd_length(
        &self,
        right: &Expr,
        op: InfixOp,
        lt: &Ty,
        rt: Ty,
    ) -> Result<Ty, TypeError> {
        if !mojito_types::types::is_simd_length(lt)
            || !rt.is_int_literal()
            || op.reflected_dunder().is_none()
            || op
                .dunder()
                .is_some_and(|dunder| self.declared_receiver_member(lt, dunder).is_some())
        {
            return Ok(rt);
        }
        self.record_literal_materializations(right, &rt, &Ty::int())?;
        Ok(Ty::int())
    }

    /// Type a chained comparison `a < b < c`: each adjacent pair must
    /// compare to a `Bool`, as a single comparison does, and each link is
    /// its dunder's call. The chain's record holds, per link, the dunder
    /// that link selected ([`CheckedCompareLink`]): `infer_infix` records
    /// it at the chain's own occurrence, as it would at a lone operator,
    /// and the link takes it from there. A middle operand is evaluated once,
    /// unconverted, and each of its two links converts it as that link's
    /// dunder binds it, as upstream's call binding converts an argument per
    /// call: the link takes the conversion its check recorded at the middle
    /// (`0 <= n < w`, whose `n: SIMDLength` the first link converts to `Int`
    /// and the second does not). Every operand a link's dunder
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
        for (index, (op, right)) in rest.iter().enumerate() {
            let membership = matches!(op, InfixOp::In | InfixOp::NotIn);
            let site = (!membership).then(|| span.clone());
            if !self.infer_infix(site, *op, left, right)?.is_bool() {
                return Err(TypeError::BadOperator {
                    op: op.symbol().to_string(),
                    operands: "a chained comparison must compare to Bool".to_string(),
                });
            }
            let left_adjustments = if index > 0 {
                self.take_middle_conversion(left)
            } else {
                Vec::new()
            };
            let right_adjustments = if index + 1 < rest.len() {
                self.take_middle_conversion(right)
            } else {
                Vec::new()
            };
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
                left_adjustments,
                right_adjustments,
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

    /// Take the conversion a chain link's check recorded at `middle`, a
    /// middle operand, off its record, frozen as that link's own
    /// adaptations ([`super::method_calls::call_value_adjustments`]).
    fn take_middle_conversion(
        &self,
        middle: &Expr,
    ) -> Vec<mojito_checked::checked::CheckedCallValueAdjustment> {
        let mut records = self.node_facts.borrow_mut();
        let Some(facts) = records.get_mut(&middle.source_span()) else {
            return Vec::new();
        };
        let adjustments = super::method_calls::call_value_adjustments(facts);
        if facts.implicit_conversion.take().is_some() {
            facts.implicit_conversion_type = None;
            facts.conversion_materialization = None;
        } else if matches!(
            facts.adjustment.as_deref(),
            Some(mojito_checked::checked::SemanticAdjustment::MaterializeLiteral(_))
        ) {
            facts.adjustment = None;
        }
        adjustments
    }

    /// Type `left op right` over a struct or a type parameter as upstream's
    /// `emitBinOpCall` does: the receiver `lt`'s dunder call
    /// `left.__op__(right)`, else, when the right operand has a type `rt`,
    /// its reflected dunder call `right.__rop__(left)`, marked reflected so
    /// MIR still evaluates the operands in source order. A literal receiver
    /// whose own dunder takes no `right` first materializes to its default
    /// type (`Int`, `Float64`), whose dunder is tried before the reflected
    /// one. `None` when none selects a candidate.
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
        let materialized = mojito_types::types::default_literal(lt);
        if let Some(dunder) = op.dunder()
            && lt.kind().is_numeric_literal()
        {
            let before = self.record_at(left.source_span()).adjustment.clone();
            self.record_literal_materializations(left, lt, &materialized)?;
            if let Some(ty) = self.infer_operator_method_call(
                span,
                left,
                &materialized,
                dunder,
                std::slice::from_ref(right),
            )? {
                return Ok(Some(ty));
            }
            self.record_at(left.source_span()).adjustment = before;
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
            self.record_at(span.clone()).reflected_operator = true;
        }
        Ok(ty)
    }

    /// The literal rule's type of `op` over two numeric literals, for an
    /// operator whose upstream `IntLiteral` dunder the bundled library does
    /// not declare yet: the literal that dunder's signature computes,
    /// through [`Self::literal_rule_type`]. `IntLiteral.__truediv__` (`/`)
    /// and the `FloatLiteral`-taking `+ - *` overloads are R1784's, the
    /// reflected `FloatLiteral.__radd__`… R1854's. `None` for any other
    /// operator or operands, which the declared dunders and the left
    /// operand's materialization answer.
    fn literal_rule(&self, op: InfixOp, lt: &Ty, rt: &Ty) -> Result<Option<Ty>, TypeError> {
        use InfixOp::{Add, Div, Mul, Sub};
        if !numeric_literals(lt, rt) || !lt.is_int_literal() {
            return Ok(None);
        }
        match op {
            Div => self.literal_rule_type(op, lt, rt).map(Some),
            Add | Sub | Mul if rt.is_float_literal() => {
                self.literal_rule_type(op, lt, rt).map(Some)
            }
            _ => Ok(None),
        }
    }

    /// `IntLiteral[#pop<int_literal_bin<op l, r>>]` over the two literals'
    /// slots, or the `FloatLiteral` of `#pop<float_literal_bin<…>>` when
    /// either is a float or `op` is `/`, each integer slot first converted
    /// by `#pop<int_to_float_literal>`, as upstream's
    /// `IntLiteral.__truediv__` (`type_of(FloatLiteral(self) / rhs)`) and
    /// its implicit `FloatLiteral` conversion type them. Closed slots fold;
    /// a symbolic slot, or a closed fold that fails (`1 / 0`), keeps the
    /// attribute.
    fn literal_rule_type(&self, op: InfixOp, lt: &Ty, rt: &Ty) -> Result<Ty, TypeError> {
        let float = op == InfixOp::Div || lt.is_float_literal() || rt.is_float_literal();
        let slot = |ty: &Ty| -> Result<ParamExpr, TypeError> {
            let Some(slot) = ty.kind().numeric_literal_slot() else {
                return Err(bad_infix_operator(op, lt, rt));
            };
            let slot = self
                .param_context
                .constant(slot.clone())
                .map_err(param_error)?;
            if float && ty.is_int_literal() {
                self.param_context
                    .op(ParamOp::IntToFloatLiteral, &[slot])
                    .map_err(param_error)
            } else {
                Ok(slot)
            }
        };
        let param_op = match op {
            InfixOp::Add => ParamOp::Add,
            InfixOp::Sub => ParamOp::Sub,
            InfixOp::Mul => ParamOp::Mul,
            InfixOp::Div => ParamOp::Div,
            InfixOp::FloorDiv => ParamOp::FloorDiv,
            InfixOp::Mod => ParamOp::Mod,
            InfixOp::Pow => ParamOp::Pow,
            InfixOp::Shl => ParamOp::Shl,
            InfixOp::Shr => ParamOp::Shr,
            InfixOp::BitAnd => ParamOp::BitAnd,
            InfixOp::BitOr => ParamOp::BitOr,
            InfixOp::BitXor => ParamOp::BitXor,
            _ => return Err(bad_infix_operator(op, lt, rt)),
        };
        let value = self
            .param_context
            .op(param_op, &[slot(lt)?, slot(rt)?])
            .map_err(param_error)?
            .into_value();
        Ok(if float {
            Ty::float_literal_at(value)
        } else {
            Ty::int_literal_at(value)
        })
    }

    /// `StringLiteral[#pop.string_concat<l, r>]` over the two literals'
    /// `value` slots, as upstream's `StringLiteral.__add__` types its
    /// result; closed slots fold to the concatenated text.
    fn concatenated_literal(&self, lt: &Ty, rt: &Ty) -> Result<Ty, TypeError> {
        let (Some(left), Some(right)) = (
            lt.kind().string_literal_value(),
            rt.kind().string_literal_value(),
        ) else {
            return Err(TypeError::InvariantViolation(format!(
                "'{lt} + {rt}' concatenates no two string literals"
            )));
        };
        let context = &self.param_context;
        let slot = |value: &CtValue| context.constant(value.clone());
        slot(left)
            .and_then(|left| context.op(ParamOp::StringConcat, &[left, slot(right)?]))
            .map(|value| Ty::string_literal_at(value.into_value()))
            .map_err(param_error)
    }

    /// `left op right` over two value-carrying literals as the declared
    /// dunder of the left one, else the reflected dunder of the right one,
    /// with no materialization; `None` when neither declares a dunder that
    /// takes the other operand.
    fn infer_literal_operator(
        &self,
        span: &SourceSpan,
        op: InfixOp,
        left: &Expr,
        right: &Expr,
        lt: &Ty,
        rt: &Ty,
    ) -> Result<Option<Ty>, TypeError> {
        let carries_value = |ty: &Ty| ty.kind().is_numeric_literal();
        if !carries_value(lt) || !carries_value(rt) {
            return Ok(None);
        }
        if let Some(dunder) = op.dunder()
            && self.declared_receiver_member(lt, dunder).is_some()
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
        let Some(reflected) = op.reflected_dunder() else {
            return Ok(None);
        };
        if self.declared_receiver_member(rt, reflected).is_none() {
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
            self.record_at(span.clone()).reflected_operator = true;
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
            struct_ty
                if let Some((name, _)) = struct_ty.as_non_simd_struct()
                    && !self.structs.contains_key(name) =>
            {
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
                kind if kind.is_string_literal() => lt.is_string_literal(),
                _ => false,
            };
        if ok {
            return Ok(Ty::bool());
        }
        // `x in c` on a user struct, or on a `SIMD` whose struct declares it,
        // dispatches to the container's `__contains__` (`c.__contains__(x)`),
        // which must return `Bool`.
        if let Some(span) = span
            && (rt
                .kind()
                .as_non_simd_struct()
                .is_some_and(|(name, _)| self.structs.contains_key(name))
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

    /// Record the value of `expr`, the operator `left op right` over a
    /// `SIMD` vector, as the node's parameter value: upstream's builtin
    /// dunder folded (`infix_value`), whose `comptime assert` the fold
    /// drops, never an operator over lanes. A compile-time position folds
    /// a comparison; a run-time one folds an operator whose builtin dunder
    /// asserts ([`Self::record_folded_simd_operation`]).
    pub(super) fn record_folded_simd_operator(
        &self,
        expr: &Expr,
        op: InfixOp,
        left: &Expr,
        right: &Expr,
    ) {
        use InfixOp::{Eq, Ge, Gt, Le, Lt, Ne};
        let folds = if self.crosses_to_runtime() {
            op.dunder()
                .is_some_and(|dunder| self.spells_asserting_builtin(dunder))
        } else {
            matches!(op, Eq | Ne | Lt | Gt | Le | Ge)
        };
        if folds {
            self.record_folded_simd_operation(expr, &[left, right], false);
        }
    }

    /// Record the value of `expr`, the run-time prefix operator `op operand`
    /// over a `SIMD` vector, as the node's parameter value: upstream's
    /// builtin dunder (`__neg__`, `__invert__`, `__pos__`) folded over its
    /// parameter operand, whose `comptime assert` the fold drops, as
    /// [`Self::record_folded_simd_operator`] folds an infix one. `+`'s fold
    /// is its operand, which may be an application.
    pub(super) fn record_folded_simd_prefix(&self, expr: &Expr, op: PrefixOp, operand: &Expr) {
        if op != PrefixOp::Not
            && self.crosses_to_runtime()
            && self.spells_asserting_builtin(op.dunder())
        {
            self.record_folded_simd_operation(expr, &[operand], op == PrefixOp::Pos);
        }
    }

    /// Record at `span` the `comptime assert Self.length == 1, message` of
    /// the builtin `SIMD` dunder or constructor applied there, over `width`
    /// lanes, for each reached instance to judge, its `Bool` value the
    /// lanes' `==` with `1` as the pin's parameter emitter builds it. A
    /// width-one application holds it already, and a compile-time position
    /// folds the application, dropping it, as the pin's fold of a builtin
    /// call does.
    pub(super) fn record_instance_assertion(
        &self,
        span: &SourceSpan,
        width: &SimdWidth,
        message: &str,
    ) -> Result<(), TypeError> {
        if matches!(width, SimdWidth::Known(1)) || self.comptime_positions.get() != 0 {
            return Ok(());
        }
        let context = &self.param_context;
        let lanes = width.lanes(context).map_err(param_error)?;
        let value = context
            .constant(CtValue::Int(1))
            .and_then(|one| context.infix(InfixOp::Eq, &lanes, &one))
            .map_err(param_error)?;
        self.record_instance_constraint(span, value, message);
        Ok(())
    }

    /// Record at `span` the assertion whose `Bool` parameter value is
    /// `value`, failing with `message`, for each reached instance to judge,
    /// once.
    pub(super) fn record_instance_constraint(
        &self,
        span: &SourceSpan,
        value: ParamExpr,
        message: &str,
    ) {
        let assertion = (value, message.to_string());
        let mut record = self.record_at(span.clone());
        let assertions = record.instance_assertions.get_or_insert_with(Vec::new);
        if !assertions.contains(&assertion) {
            assertions.push(assertion);
        }
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
        if kwargs.len() == 1 && kwargs[0].name == "_mlir_value" && args.is_empty() {
            return self.infer_pointer_from_mlir_value(span, param_args, &kwargs[0].value);
        }
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
        let (element, origin) = self.infer_place_address(span, &kwargs[0].value)?;
        Ok(TyKind::Pointer { element, origin }.into())
    }

    /// Type upstream's `__get_mvalue_as_litref(place)`: the `!lit.ref` of a
    /// place, whose origin is the place, as `Pointer(to=place)` takes it.
    pub(super) fn infer_mvalue_as_litref(
        &self,
        span: SourceSpan,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        let ([value], [], []) = (args, kwargs, param_args) else {
            return Err(TypeError::BadCall {
                func: "__get_mvalue_as_litref".to_string(),
                reason: "expected exactly one place argument".to_string(),
            });
        };
        let (element, origin) = self.infer_place_address(span, value)?;
        Ok(TyKind::LitRef { element, origin }.into())
    }

    /// Record the value of `expr`, an operator over `operands` one of which
    /// is a `SIMD` vector, as the node's parameter value. At run time each
    /// operand is a parameter expression or a builtin callable's
    /// application, as upstream's parser folds a builtin call over
    /// parameter operands ([`Self::record_folded_builtin_call`]), so `a <
    /// SIMD[DType.int32, 4](2)` over a `comptime` vector `a` is `True`
    /// rather than a failed one-lane assertion, while `SIMD[DType.int32,
    /// 4](1, 2, 3, 4)`, no builtin, stays a call. A fold that is its
    /// operand (`folds_to_operand`, `+a`) keeps that operand's application.
    fn record_folded_simd_operation(
        &self,
        expr: &Expr,
        operands: &[&Expr],
        folds_to_operand: bool,
    ) {
        let runtime = self.crosses_to_runtime();
        if runtime && self.reads_run_time_binding(expr) {
            return;
        }
        let _position = runtime.then(|| self.comptime_position());
        if runtime
            && !operands.iter().all(|operand| {
                self.compile_dependent_ct_expr(operand).is_ok() || self.applies_builtin(operand)
            })
        {
            return;
        }
        // An operand may be an application (`SIMD[DType.int32, 4](3)`).
        let _lifting = self.lifting_position();
        let vector = |operand: &Expr| {
            self.compile_dependent_ct_expr(operand)
                .ok()
                .and_then(|built| built.meta().as_value().cloned())
                .is_some_and(|ty| {
                    ty.is_simd() && !mojito_types::types::has_host_scalar_value(ty.kind())
                })
        };
        if operands.iter().any(|operand| vector(operand))
            && let Ok(value) = self.compile_dependent_ct_expr(expr)
            && (folds_to_operand || !(runtime && matches!(value.kind(), ParamKind::Apply { .. })))
        {
            self.record_member_value(expr.source_span(), value);
        }
    }

    /// Type upstream's `Pointer[T, o](_mlir_value=)` overloads: a
    /// `Pointer` built from the `!kgen.pointer` it wraps, whose origin the
    /// bracket must supply, or from a `!lit.ref`, whose element and origin
    /// it takes. A bracket's `_` element binds from the operand, and a
    /// bracket that disagrees with a `!lit.ref` matches neither overload.
    fn infer_pointer_from_mlir_value(
        &self,
        span: SourceSpan,
        param_args: &[mojito_ast::ast::ParamArg],
        value: &Expr,
    ) -> Result<Ty, TypeError> {
        let no_match = |reason: String| TypeError::BadCall {
            func: "Pointer".to_string(),
            reason: format!("no matching function in initialization: {reason}"),
        };
        let bracket = match param_args {
            [] => None,
            [mojito_ast::ast::ParamArg::Named { name, value }] if name == "origin" => {
                Some((TyKind::Infer.into(), self.pointer_origin_arg(value)?))
            }
            [_] => {
                return Err(no_match(
                    "failed to infer parameter 'origin' of a bracket that names none".to_string(),
                ));
            }
            _ => {
                let (ty, _) = self.pointer_type("Pointer", param_args)?;
                let TyKind::Pointer { element, origin } = ty.kind() else {
                    return Err(no_match(format!("'{ty}' is not a Pointer")));
                };
                Some((element.clone(), origin.clone()))
            }
        };
        let operand = self.infer(value)?;
        let (element, origin) = match (operand.kind(), bracket) {
            (TyKind::LitRef { element, origin }, None) => (element.clone(), origin.clone()),
            (TyKind::LitRef { element, origin }, Some((expected, expected_origin))) => {
                if (*expected != TyKind::Infer && expected != *element)
                    || !origin.coerces_to(&expected_origin)
                {
                    return Err(no_match(format!(
                        "a '{operand}' does not convert to '{}'",
                        TyKind::LitRef {
                            element: expected,
                            origin: expected_origin,
                        }
                    )));
                }
                (element.clone(), origin.clone())
            }
            (TyKind::KgenPointer(_), None) => {
                return Err(no_match(
                    "failed to infer parameter 'mut': a '!kgen.pointer' carries no origin"
                        .to_string(),
                ));
            }
            (TyKind::KgenPointer(element), Some((expected, origin))) => {
                if *expected != TyKind::Infer && expected != *element {
                    return Err(no_match(format!(
                        "a '{operand}' does not convert to '{}'",
                        TyKind::KgenPointer(expected)
                    )));
                }
                (element.clone(), origin)
            }
            _ => {
                return Err(TypeError::TypeMismatch {
                    expected: "a '!kgen.pointer' or a '!lit.ref'".to_string(),
                    found: operand.to_string(),
                    context: "argument '_mlir_value' to 'Pointer'".to_string(),
                });
            }
        };
        self.record_pointer_adjustment(
            span,
            SemanticAdjustment::PointerConvert {
                origin: Some(origin.clone()),
                materialized: None,
            },
        );
        Ok(TyKind::Pointer { element, origin }.into())
    }

    /// The element and origin of the address of `value`, a place, as
    /// `Pointer(to=)` and `__get_mvalue_as_litref` take it, recording the
    /// `PointerToPlace` adjustment at `span`.
    fn infer_place_address(
        &self,
        span: SourceSpan,
        value: &Expr,
    ) -> Result<(Ty, mojito_types::origin::PointerOrigin), TypeError> {
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
            return Ok((reference.referent, origin));
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
            return Ok((reference.referent, origin));
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
        Ok((
            element,
            mojito_types::origin::PointerOrigin::Place { place, mutable },
        ))
    }

    /// A read dunder binds its operand as a method call binds a read
    /// argument: a named struct place, or a struct element an accessor
    /// returns by reference (`xs[0]` through `List.__getitem__`), is read
    /// where it lies rather than copied out.
    fn borrow_operator_operand(&self, operand: &Expr, ty: &Ty) {
        self.borrow_nominal_place_argument(operand, ty);
        self.borrow_parameter_place_argument(operand, ty);
        if matches!(operand.kind, ExprKind::Index { .. })
            && ty
                .kind()
                .as_non_simd_struct()
                .is_some_and(|(name, _)| self.structs.contains_key(name))
            && self.infer_reference_value(operand).is_some()
        {
            self.record_at(operand.source_span())
                .borrowed_read_call_place = true;
        }
    }

    /// Whether `ty` is a struct the program declares, whose operators are
    /// its dunders' method calls.
    fn is_checked_struct(&self, ty: &Ty) -> bool {
        ty.kind()
            .as_non_simd_struct()
            .is_some_and(|(name, _)| self.structs.contains_key(name))
    }

    /// Whether `ty`'s operators are method calls: a struct the program
    /// declares, or a type parameter or projection whose bounds declare
    /// requirements.
    fn has_operator_methods(&self, ty: &Ty) -> bool {
        self.is_checked_struct(ty) || is_bound_archetype(ty)
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

/// Whether both operands are numeric literals.
fn numeric_literals(lt: &Ty, rt: &Ty) -> bool {
    lt.kind().is_numeric_literal() && rt.kind().is_numeric_literal()
}

/// Whether `ty` is an archetype whose methods its bounds' requirements
/// declare: a type parameter or an associated projection of one.
fn is_bound_archetype(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::Param { .. } | TyKind::Assoc { .. })
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

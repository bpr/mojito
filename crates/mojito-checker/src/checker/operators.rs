//! Operator, SIMD-construction, and pointer-construction type inference
//! for the checker. Extracted from `checker.rs`; see `docs/symbol-map.md`.

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
        match (op, t.kind()) {
            // Negation preserves the (possibly literal) numeric type, except UInt.
            (
                PrefixOp::Neg,
                TyKind::INT | TyKind::FLOAT64 | TyKind::IntLiteral | TyKind::FloatLiteral,
            ) => {
                return Ok(t);
            }
            // Elementwise negation on numeric SIMD lanes preserves the type;
            // a bool mask does not negate.
            (PrefixOp::Neg, TyKind::Simd { dtype, .. }) if dtype.licenses(|d| d != Dtype::Bool) => {
                return Ok(t);
            }
            (PrefixOp::Not, TyKind::Bool) => return Ok(Ty::bool()),
            // A numeric scalar is `Boolable`: `not n` over an `Int`, or
            // `not (a == b)` over `UInt64`s.
            (PrefixOp::Not, _) if is_numeric(&t) || is_scalar_simd(&t) => {
                return Ok(Ty::bool());
            }
            // Bitwise inversion keeps an integer (or `Bool`) type; float
            // operands and masks have no inversion.
            (PrefixOp::Invert, TyKind::INT | TyKind::UINT | TyKind::IntLiteral | TyKind::Bool) => {
                return Ok(t);
            }
            (PrefixOp::Invert, TyKind::Simd { dtype, .. }) if dtype.licenses(|d| !d.is_float()) => {
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
        if self.param_has_bound(&t, prefix_operation_trait(op)) {
            self.record_read_temporary_argument(operand);
            return Ok(match op {
                PrefixOp::Neg | PrefixOp::Invert => t,
                PrefixOp::Not => Ty::bool(),
            });
        }
        // A user struct's `-x` and `~x` are its dunder calls `x.__neg__()`
        // and `x.__invert__()`, as upstream's `UnaryOpNode::emitArith` emits
        // them.
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

        // Membership `in` / `not in` — the right operand is a container.
        if matches!(op, In | NotIn) {
            return self.infer_membership(span, op, left, right, &lt, &rt);
        }
        // SIMD operators are elementwise (handled before the scalar-numeric
        // path, which `Int`, `UInt`, and `Float64` operands keep).
        let vector = |ty: &Ty| {
            matches!(ty.kind(), TyKind::Simd { .. })
                && !mojito_types::types::has_host_scalar_value(ty.kind())
        };
        if vector(&lt) || vector(&rt) {
            return self.infer_simd_infix(op, &lt, &rt);
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
            // Short-circuiting boolean logic requires `Bool` operands.
            And | Or if lt == TyKind::Bool && rt == TyKind::Bool => Some(Ty::bool()),
            // `+` concatenates String, or adds numbers (result = common type).
            Add if lt == TyKind::StringLiteral && rt == TyKind::StringLiteral => {
                Some(Ty::string_literal())
            }
            // Arithmetic that preserves the operand type.
            Add | Sub | Mul | FloorDiv | Mod | Pow => common,
            Shl | Shr | BitAnd | BitOr | BitXor
                if common.as_ref().is_some_and(|ty| {
                    matches!(ty.kind(), TyKind::INT | TyKind::UINT | TyKind::IntLiteral)
                }) =>
            {
                common
            }
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
            Eq | Ne if common.is_some() || (lt == rt && is_scalar(&lt)) => Some(Ty::bool()),
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
        if lt == TyKind::StringLiteral
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
            && self.has_operator_methods(lt)
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
        if !self.has_operator_methods(rt) {
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
                TyKind::StringLiteral => *lt == TyKind::StringLiteral,
                _ => false,
            };
        if ok {
            return Ok(Ty::bool());
        }
        // `x in c` on a user struct dispatches to the container's `__contains__`
        // (`c.__contains__(x)`), which must return `Bool`.
        if let Some(span) = span
            && matches!(rt.kind(), TyKind::Struct(name, _) if self.structs.contains_key(name))
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

    /// Type an elementwise SIMD operator. Both operands must be the same SIMD
    /// type, except a numeric *literal* splats to the other operand's type.
    /// Arithmetic keeps the operand type; comparisons return a `bool` mask.
    #[allow(
        clippy::unused_self,
        reason = "TODO: make an associated function or use the receiver"
    )]
    pub(super) fn infer_simd_infix(&self, op: InfixOp, lt: &Ty, rt: &Ty) -> Result<Ty, TypeError> {
        use InfixOp::{
            Add, BitAnd, BitOr, BitXor, Div, Eq, FloorDiv, Ge, Gt, Le, Lt, Mod, Mul, Ne, Shl, Shr,
            Sub,
        };
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
        let mask = || simd_of(SimdDtype::Known(Dtype::Bool), width.clone());
        match op {
            // Elementwise arithmetic on numeric lanes preserves the type.
            Add | Sub | Mul if dtype.licenses(|d| d != Dtype::Bool) => same(),
            // Floor division on numeric lanes (`pop.floordiv`); remainder on
            // integer lanes (upstream defines it on float lanes too, a
            // recorded gap).
            FloorDiv if dtype.licenses(|d| d != Dtype::Bool) => same(),
            Mod if dtype.licenses(|d| d != Dtype::Bool && !d.is_float()) => same(),
            BitAnd | BitOr | BitXor if dtype.licenses(|d| !d.is_float()) => same(),
            Shl | Shr if dtype.licenses(|d| d != Dtype::Bool && !d.is_float()) => same(),
            // True division is defined on float lanes only.
            Div if dtype.licenses(Dtype::is_float) => same(),
            // Equality on any lanes; ordering on numeric lanes — a bool mask.
            Eq | Ne => mask(),
            Lt | Gt | Le | Ge if dtype.licenses(|d| d != Dtype::Bool) => mask(),
            _ => Err(bad()),
        }
    }

    /// Type `SIMD[DType.<dt>, width](args)`: `width` element arguments, or a
    /// single argument that splats across all lanes; each must fit the dtype.
    pub(super) fn infer_simd_construction(
        &self,
        span: &SourceSpan,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        let (dtype, mut width, attributes) = self.simd_dims(param_args)?;
        self.record_bracket_attributes(span.clone(), attributes);
        if width.is_inferred() {
            let inferred = i64::try_from(args.len()).unwrap_or(0);
            if inferred < 1 || (inferred & (inferred - 1)) != 0 {
                return Err(TypeError::BadSimdWidth(inferred.to_string()));
            }
            width = SimdWidth::Known(inferred);
        }
        self.check_simd_args(&dtype, &width, args)?;
        simd_of(dtype, width)
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

    /// Type upstream's mask splat `SIMD[DType.bool, N](fill=b)`: `Bool` is not
    /// a `Scalar`, so a mask takes its one lane by keyword, not positionally.
    pub(super) fn infer_simd_fill(
        &self,
        span: &SourceSpan,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        let (dtype, width, attributes) = self.simd_dims(param_args)?;
        self.record_bracket_attributes(span.clone(), attributes);
        let not_a_fill = || TypeError::BadCall {
            func: "SIMD".to_string(),
            reason: "only a DType.bool mask of a known width takes 'fill=', its one Bool lane"
                .to_string(),
        };
        let [fill] = kwargs else {
            return Err(not_a_fill());
        };
        if fill.name != "fill"
            || !args.is_empty()
            || dtype.known() != Some(Dtype::Bool)
            || width.known().is_none_or(|width| width < 1)
        {
            return Err(not_a_fill());
        }
        let fill_ty = self.infer(&fill.value)?;
        if !converts_to_lane(&fill_ty, &dtype) {
            return Err(TypeError::TypeMismatch {
                expected: "a Bool fill".to_string(),
                found: fill_ty.to_string(),
                context: "SIMD fill".to_string(),
            });
        }
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

    /// Check the element arguments of a SIMD construction: none (all lanes
    /// zero), `width` of them (one per lane), or exactly one (splatted), each
    /// fitting `dtype`. A
    /// symbolic width fixes no count and a symbolic dtype gates nothing: both
    /// are the instantiation's to check.
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
        // `Bool` is not a `Scalar`, so a multi-lane mask splats one `Bool`
        // only through `fill=`.
        if dtype.known() == Some(Dtype::Bool)
            && width.known().is_some_and(|width| width > 1)
            && args.len() == 1
        {
            return Err(TypeError::BadCall {
                func: "SIMD".to_string(),
                reason: format!("a {width}-lane DType.bool mask splats one Bool only as 'fill='"),
            });
        }
        for arg in args {
            let aty = self.infer(arg)?;
            // Integer lanes also construct from any Intable value (bounded
            // parameter or conforming struct) through its `__int__`; the
            // numeric scalars take the exact `converts_to_lane` matrix, so a
            // float source still spells its truncation explicitly.
            let intable_object = dtype.licenses(|d| d != Dtype::Bool && !d.is_float())
                && matches!(aty.kind(), TyKind::Param { .. } | TyKind::Struct(..))
                && self.conforms_to(&aty, "Intable");
            if !converts_to_lane(&aty, dtype) && !intable_object {
                return Err(TypeError::TypeMismatch {
                    expected: format!("a {dtype} element"),
                    found: aty.to_string(),
                    context: "SIMD element".to_string(),
                });
            }
            if intable_object {
                self.borrow_nominal_place_argument(arg, &aty);
                self.borrow_parameter_place_argument(arg, &aty);
                self.record_read_temporary_argument(arg);
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
        if aty != TyKind::StringLiteral && !nominal_string {
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
        // A call is a place only through the reference its inference
        // records (`w.get()` returning `ref[self.x] Int`); any other operand
        // is a place before it is read.
        let call = matches!(
            value.kind,
            ExprKind::Call { .. } | ExprKind::MethodCall { .. } | ExprKind::Invoke { .. }
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

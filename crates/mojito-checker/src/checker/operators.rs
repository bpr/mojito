//! Operator, SIMD-construction, and pointer-construction type inference
//! for the checker. Extracted from `checker.rs`; see `docs/symbol-map.md`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_checked::checked::{CheckedCompareLink, SemanticAdjustment};
use mojito_checked::declaration::compiler_trait_declaration;
use mojito_symbol::symbol::TRAIT_DISPATCH;
use mojito_types::types::TyKind;

impl Checker {
    pub(super) fn infer_prefix(&self, op: PrefixOp, operand: &Expr) -> Result<Ty, TypeError> {
        let t = self.infer(operand)?;
        match (op, t.kind()) {
            // Negation preserves the (possibly literal) numeric type, except UInt.
            (
                PrefixOp::Neg,
                TyKind::Int | TyKind::Float64 | TyKind::IntLiteral | TyKind::FloatLiteral,
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
            (PrefixOp::Invert, TyKind::Int | TyKind::UInt | TyKind::IntLiteral | TyKind::Bool) => {
                return Ok(t);
            }
            (PrefixOp::Invert, TyKind::Simd { dtype, .. }) if dtype.licenses(|d| !d.is_float()) => {
                return Ok(t);
            }
            _ => {}
        }
        // An opaque type parameter bounded by the prefix operator's trait
        // dispatches after erasure (`-x` needs `Negatable`, `not x` needs
        // `Boolable`); the concrete impl runs on the erased type.
        if param_has_bound(&t, prefix_operation_trait(op)) {
            self.record_read_temporary_argument(operand);
            return Ok(match op {
                PrefixOp::Neg | PrefixOp::Invert => t,
                PrefixOp::Not => Ty::bool(),
            });
        }
        // A user struct routes through the operator's dunder (`-x` →
        // `x.__neg__() -> Self`, `not x` → `not x.__bool__() -> Bool`).
        if let Some(result) = self.struct_dunder(&t, op.dunder(), &[]) {
            let ret = result?;
            if self
                .struct_dunder_signature_for(&t, op.dunder(), &[])
                .is_some_and(|(_, sig, _)| reads(sig.self_convention))
            {
                self.borrow_operator_operand(operand, &t);
                self.record_read_temporary_argument(operand);
            }
            return match op {
                PrefixOp::Neg | PrefixOp::Invert => Ok(ret),
                PrefixOp::Not => require_dunder_ret(ret, &Ty::bool(), "__bool__"),
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
        elements.iter().all(|element| match element.kind() {
            TyKind::Tuple(nested) => self.tuple_elements_equatable_nominal(nested),
            _ => has_equality_bound_or_concrete(self, element),
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
        let rt = self.infer(right)?;
        let rt = self.opaque_element(&rt).unwrap_or(rt);

        // Membership `in` / `not in` — the right operand is a container.
        if matches!(op, In | NotIn) {
            return self.infer_membership(span, op, left, right, &lt, &rt);
        }
        // SIMD operators are elementwise (handled before the scalar-numeric path).
        if matches!(lt.kind(), TyKind::Simd { .. }) || matches!(rt.kind(), TyKind::Simd { .. }) {
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
                (Add | Sub, TyKind::Int | TyKind::IntLiteral) => return Ok(lt.clone()),
                (Sub, TyKind::Pointer { element: other, .. }) if element == other => {
                    return Ok(Ty::int());
                }
                (Eq | Ne, TyKind::Pointer { element: other, .. }) if element == other => {
                    return Ok(Ty::bool());
                }
                _ => {}
            }
        }

        // Tuple comparisons are structural. Equality accepts independently
        // equatable element packs (different element types simply compare
        // unequal); ordering requires a lexicographically compatible prefix.
        if let (TyKind::Tuple(left), TyKind::Tuple(right)) = (lt.kind(), rt.kind()) {
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
                return Ok(Ty::bool());
            }
        }
        if matches!((lt.kind(), rt.kind()), (TyKind::Struct(left, _), TyKind::Struct(right, _))
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
                return Ok(Ty::bool());
            }
        }
        // `Slice` is Equatable (upstream `builtin_slice.mojo`); its
        // `ContiguousSlice`/`StridedSlice` sub-kinds are not, so only exact
        // `Slice` operands compare. The VM compares the three bounds.
        if matches!(op, Eq | Ne)
            && matches!((lt.kind(), rt.kind()), (TyKind::Struct(left, left_args), TyKind::Struct(right, right_args))
                if left == "Slice" && right == "Slice" && left_args.is_empty() && right_args.is_empty())
        {
            return Ok(Ty::bool());
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
            return Ok(if matches!(op, Div) { Ty::float64() } else { lt });
        }

        // `common` is the unified numeric type when both operands are numeric
        // (literals coerced as needed), else None.
        let common = common_numeric(&lt, &rt);
        if let Some(target) = common.as_ref()
            && (matches!(target.kind(), TyKind::Int | TyKind::UInt | TyKind::Float64)
                || is_scalar_simd(target))
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
            // `**` between equal opaque type parameters bounded by `Powable`
            // (`__pow__(self, Self) -> Self`); the concrete impl runs after
            // erasure. Checked before the numeric arm since a `Param` isn't
            // numeric (so `common` is None here).
            Pow if lt == rt && param_has_bound(&lt, "Powable") => Some(lt.clone()),
            // Arithmetic that preserves the operand type.
            Add | Sub | Mul | FloorDiv | Mod | Pow => common,
            Shl | Shr | BitAnd | BitOr | BitXor
                if common.as_ref().is_some_and(|ty| {
                    matches!(ty.kind(), TyKind::Int | TyKind::UInt | TyKind::IntLiteral)
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
            // Ordering between numbers, or between equal opaque type parameters
            // whose bound promises an ordering (`T: Comparable`).
            Lt | Gt | Le | Ge
                if common.is_some()
                    || (lt == rt
                        && (has_order_bound(&lt)
                            || self.has_assumed_conformance(&lt, "Comparable"))) =>
            {
                Some(Ty::bool())
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
                Some(Ty::bool())
            }
            _ => None,
        };
        if let Some(ty) = result {
            // A comparison through a bound dispatches the requirement's read
            // dunder (`__eq__(self, other: Self)`), which reads a named
            // operand where it lies, as a struct's own dunder does.
            // It is that requirement's call, so the operator names it as
            // its target, as a struct operator names its dunder. An assumed
            // conformance names it too, though no bound declares it.
            if let Some(span) = span
                && matches!(lt.kind(), TyKind::Param { .. })
                && lt == rt
            {
                self.borrow_parameter_place_argument(left, &lt);
                self.borrow_parameter_place_argument(right, &rt);
                self.record_operator_temporaries(left, right, (true, true));
                if let Some(target) = self.bound_comparison_target(op) {
                    self.record_at(span).overload_target = Some(target);
                }
            }
            return Ok(ty);
        }
        // Mixed literal/nominal String operands normalize onto the nominal
        // struct: the literal side converts through the `@implicit` literal
        // constructor and the operator dispatches the struct's dunder, so
        // `s + "x"`, `"x" + s`, and mixed comparisons all run library code.
        let nominal_string = |ty: &Ty| {
            matches!(ty.kind(), TyKind::Struct(name, args)
                if args.is_empty()
                    && mojito_symbol::symbol::is_stdlib_string_struct(name)
                    && self.structs.contains_key(name))
        };
        let mixed_string = if nominal_string(&lt) && rt == TyKind::StringLiteral {
            Some((right, lt.clone()))
        } else if nominal_string(&rt) && lt == TyKind::StringLiteral {
            Some((left, rt.clone()))
        } else {
            None
        };
        if let Some((literal_operand, nominal_ty)) = mixed_string
            && let Some(dunder) = op.dunder()
            && self.record_implicit_conversion(
                literal_operand,
                &Ty::string_literal(),
                &nominal_ty,
            )?
            && let Some(r) = self.struct_dunder(&nominal_ty, dunder, &[&nominal_ty])
        {
            if let Some(span) = span
                && let Some(dispatch) = self.struct_infix_dispatch(op, &nominal_ty, &nominal_ty)?
                && !dispatch.negated_equality
            {
                self.record_operator_temporaries(left, right, dispatch.reads);
                self.record_at(span).overload_target = Some(dispatch.target);
            }
            return r;
        }
        // Operator overloading: `a OP b` on a user struct dispatches to the left
        // operand's dunder method (`a.__add__(b)`, `a.__eq__(b)`, …); see
        // `struct_infix_dispatch` for the selection.
        if let Some(dispatch) = self.struct_infix_dispatch(op, &lt, &rt)? {
            if dispatch.converted {
                self.record_implicit_conversion(right, &rt, &dispatch.operand_ty)?;
            }
            if dispatch.consumes {
                self.check_consuming_as(
                    right,
                    &rt,
                    &format!("operand of '{}'", infix_symbol(op)),
                    super::traits::ConsumeKind::Move,
                )?;
            }
            if let Some(span) = span {
                // A read dunder borrows a place operand where it lies, as a
                // read parameter of an ordinary method call would.
                if dispatch.borrows.0 {
                    self.borrow_operator_operand(left, &lt);
                }
                if dispatch.borrows.1 {
                    self.borrow_operator_operand(right, &rt);
                }
                self.record_operator_temporaries(left, right, dispatch.reads);
                if dispatch.negated_equality {
                    self.record_at(span.clone()).adjustment = Some(Box::new(
                        mojito_checked::checked::SemanticAdjustment::NegatedEquality,
                    ));
                }
                self.record_struct_instantiation(
                    &dispatch.struct_name,
                    &dispatch.struct_args,
                    span.source.as_deref(),
                );
                if let Some(error) = dispatch.error {
                    self.record_call_effect(span.clone(), error.clone());
                    self.require_error(format!("raising operator '{}'", infix_symbol(op)), error)?;
                }
                self.record_at(span).overload_target = Some(dispatch.target);
            }
            let dunder = if dispatch.negated_equality {
                "__eq__"
            } else {
                op.dunder().expect("a dispatched operator has a dunder")
            };
            return self
                .struct_dunder(&lt, dunder, &[&dispatch.operand_ty])
                .expect("dunder signature was resolved");
        }
        // The right operand's reflected dunder (`1 + m` → `m.__radd__(1)`)
        // answers when the left operand has no operator method for the
        // pair: MIR swaps the operands and calls the recorded symbol.
        if let Some(span) = span
            && let Some(dispatch) = self.struct_reflected_dispatch(op, &lt, &rt)
        {
            if dispatch.reads.0 {
                self.borrow_operator_operand(left, &lt);
            }
            if dispatch.reads.1 {
                self.borrow_operator_operand(right, &rt);
            }
            self.record_operator_temporaries(left, right, dispatch.reads);
            if let Some(error) = dispatch.error {
                self.record_call_effect(span.clone(), error.clone());
                self.require_error(format!("raising operator '{}'", infix_symbol(op)), error)?;
            }
            self.record_at(span.clone()).overload_target = Some(dispatch.target);
            self.record_at(span).adjustment = Some(Box::new(
                mojito_checked::checked::SemanticAdjustment::ReflectedOperator,
            ));
            let reflected = op
                .reflected_dunder()
                .expect("a reflected dispatch has a dunder");
            return self
                .struct_dunder(&rt, reflected, &[&lt])
                .expect("reflected dunder signature was resolved");
        }
        Err(TypeError::BadOperator {
            op: infix_symbol(op).to_string(),
            operands: format!("{lt} and {rt}"),
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
                    op: infix_symbol(*op).to_string(),
                    operands: "a chained comparison must compare to Bool".to_string(),
                });
            }
            let target = self.clear_fact(&span, |facts| facts.overload_target.take());
            let negated_equality = self
                .clear_fact(&span, |facts| {
                    facts
                        .adjustment
                        .take_if(|adjustment| **adjustment == SemanticAdjustment::NegatedEquality)
                })
                .is_some();
            links.push(CheckedCompareLink {
                target,
                negated_equality,
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

    /// The dunder `lt OP rt` dispatches on a struct left operand, decided
    /// from the operand types alone: what `infer_infix` then records at the
    /// operator, and what an instance of a checked template realizes at its
    /// own types.
    ///
    /// Among same-arity overloads the operand type selects, first by value
    /// coercion, then through an `@implicit` conversion of the right operand,
    /// as a call argument would convert; `BinOp.resolved` names the selected
    /// dunder's lowered symbol, so the operator is that method's call.
    /// `a != b` on an Equatable struct declaring `__eq__` without `__ne__` is
    /// Equatable's default `__ne__` (`not (a == b)`): the operator dispatches
    /// `__eq__` and MIR negates the result. The default is the trait's, so a
    /// struct that does not conform to `Equatable` has no `__ne__` (Mojo:
    /// "does not implement the '__ne__' method"). `struct_dunder_signature_for`
    /// falls back to the first same-arity declaration for diagnostics, so
    /// acceptance is re-checked here: a `__ne__` overload set that accepts
    /// no operand of this type still defers to `__eq__`. The dunder's
    /// availability clause (`__eq__ ... where conforms_to(Self.T,
    /// Equatable)`) is judged against the receiver's arguments, as a method
    /// call judges it; a failing clause leaves the operator undefined for
    /// these operands.
    ///
    /// `None` when the left operand has no dunder for the pair.
    pub(super) fn struct_infix_dispatch(
        &self,
        op: InfixOp,
        lt: &Ty,
        rt: &Ty,
    ) -> Result<Option<StructInfixDispatch>, TypeError> {
        let dunder_accepts = |dunder: &str| {
            self.struct_dunder_signature_for(lt, dunder, &[rt])
                .is_some_and(|(info, sig, targs)| {
                    self.value_coerces(rt, &substitute_at(&sig.params[0], info, targs))
                })
        };
        let negated_equality = op == InfixOp::Ne
            && !dunder_accepts("__ne__")
            && dunder_accepts("__eq__")
            && self.conforms_to(lt, "Equatable");
        let dunder = if negated_equality {
            Some("__eq__")
        } else {
            op.dunder()
        };
        let Some(dunder) = dunder else {
            return Ok(None);
        };
        let Some((info, sig, targs)) = self.struct_dunder_signature_for(lt, dunder, &[rt]) else {
            return Ok(None);
        };
        let TyKind::Struct(sname, _) = lt.kind() else {
            unreachable!("dunder signatures resolve on struct receivers")
        };
        let mut operand_ty = rt.clone();
        let mut selected = sig;
        let mut converted = false;
        let param = substitute_at(&sig.params[0], info, targs);
        if !self.value_coerces(rt, &param) {
            let same_arity = info
                .methods
                .get(dunder)
                .into_iter()
                .flatten()
                .filter(|sig| sig.params.len() == 1);
            for candidate in same_arity {
                let param = substitute_at(&candidate.params[0], info, targs);
                if self.implicit_conversion_target(rt, &param)?.is_some() {
                    operand_ty = param;
                    selected = candidate;
                    converted = true;
                    break;
                }
            }
        }
        if self
            .method_constraint_result(selected, &[], &info.decls, targs)
            .is_err()
        {
            return Err(TypeError::BadOperator {
                op: infix_symbol(op).to_string(),
                operands: format!("{lt} and {rt}"),
            });
        }
        let consumes = matches!(
            selected.conventions.first().copied().flatten(),
            Some(ArgConvention::Var | ArgConvention::Deinit)
        );
        let operand_reads = (
            reads(selected.self_convention),
            reads(selected.conventions.first().copied().flatten()),
        );
        let target = self.dunder_symbol(sname, dunder, selected, info);
        Ok(Some(StructInfixDispatch {
            target,
            error: dunder_error(selected),
            negated_equality,
            operand_ty,
            converted,
            consumes,
            borrows: (operand_reads.0, !converted && operand_reads.1),
            reads: operand_reads,
            struct_name: sname.clone(),
            struct_args: targs.to_vec(),
        }))
    }

    /// The dunder `lt OP rt` dispatches through the right operand's
    /// reflected dunder (`rt.__radd__(lt)`), decided from the operand types
    /// alone: what `infer_infix` records at the operator when the left
    /// operand has no dunder for the pair.
    ///
    /// `None` when the right operand is not a struct whose reflected dunder
    /// takes the left operand under its availability clause.
    pub(super) fn struct_reflected_dispatch(
        &self,
        op: InfixOp,
        lt: &Ty,
        rt: &Ty,
    ) -> Option<StructReflectedDispatch> {
        let reflected = op.reflected_dunder()?;
        let (info, sig, targs) = self.struct_dunder_signature_for(rt, reflected, &[lt])?;
        let TyKind::Struct(rname, _) = rt.kind() else {
            return None;
        };
        if !self.value_coerces(lt, &substitute_at(&sig.params[0], info, targs)) {
            return None;
        }
        self.method_constraint_result(sig, &[], &info.decls, targs)
            .ok()?;
        Some(StructReflectedDispatch {
            target: self.dunder_symbol(rname, reflected, sig, info),
            error: dunder_error(sig),
            reads: (
                reads(sig.conventions.first().copied().flatten()),
                reads(sig.self_convention),
            ),
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
        let nominal_ok = match rt.kind() {
            TyKind::Struct(name, _) if !self.structs.contains_key(name) => {
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
            || match rt.kind() {
                TyKind::Tuple(_) => match lt.kind() {
                    TyKind::Tuple(elements) => tuple_elements_equatable(elements),
                    _ => is_list_equatable(lt),
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
            op: infix_symbol(op).to_string(),
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
            op: infix_symbol(op).to_string(),
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
            // Floor division and remainder on integer lanes (upstream defines
            // them on every numeric dtype; float lanes stay a recorded gap).
            FloorDiv | Mod if dtype.licenses(|d| d != Dtype::Bool && !d.is_float()) => same(),
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
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        let (dtype, mut width) = self.simd_dims(param_args)?;
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
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        let (dtype, width) = self.simd_dims(param_args)?;
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
            // A bound is an `Int`, `None`, or an `Optional[Int]` value (the
            // descriptor's own field type, as upstream's `Slice.__init__`
            // declares): the nominal slot is read at construction.
            if found != TyKind::None && !coerces(&found, &Ty::int()) && !is_optional_int(&found) {
                return Err(TypeError::TypeMismatch {
                    expected: "Int or None".to_string(),
                    found: found.to_string(),
                    context: format!("argument to '{name}'"),
                });
            }
        }
        Ok(TyKind::Struct("Slice".to_string(), Vec::new()).into())
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
            self.record_at(span).adjustment = Some(Box::new(
                mojito_checked::checked::SemanticAdjustment::PointerToPlace { mutable },
            ));
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
            self.record_at(span).adjustment = Some(Box::new(
                mojito_checked::checked::SemanticAdjustment::PointerToPlace { mutable },
            ));
            return Ok(TyKind::Pointer {
                element: reference.referent,
                origin,
            }
            .into());
        }
        let place = self.origin_place(value).map_err(|error| match error {
            TypeError::UndefinedVariable(_) => error,
            _ => TypeError::Unsupported("Pointer(to=...) requires a place expression".to_string()),
        })?;
        let element = self.infer(value)?;
        // A place an accessor returns by reference (`t[0]` on a `Tuple`) is
        // the pointer's referent: the handle is retained, never read out.
        if let Some(reference) = self.infer_reference_value(value) {
            self.record_at(value.source_span()).reference_value_use =
                Some(reference.mutability == mojito_types::origin::Mutability::Mutable);
        }
        let mutable = self.owner_is_mutable(place.root);
        self.record_at(span).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::PointerToPlace { mutable },
        ));
        Ok(TyKind::Pointer {
            element,
            origin: mojito_types::origin::PointerOrigin::Place { place, mutable },
        }
        .into())
    }

    /// Record each operand an operator's dunder reads (`reads` = receiver,
    /// operand) as a read temporary argument: a temporary there is the
    /// caller's, destroyed once the dunder returns, as a method call's is.
    fn record_operator_temporaries(&self, left: &Expr, right: &Expr, reads: (bool, bool)) {
        if reads.0 {
            self.record_read_temporary_argument(left);
        }
        if reads.1 {
            self.record_read_temporary_argument(right);
        }
    }

    /// A read dunder binds its operand as a method call binds a read
    /// argument: a named struct place, or a struct element an accessor
    /// returns by reference (`xs[0]` through `List.__getitem__`), is read
    /// where it lies rather than copied out.
    fn borrow_operator_operand(&self, operand: &Expr, ty: &Ty) {
        self.borrow_nominal_place_argument(operand, ty);
        if matches!(operand.kind, ExprKind::Index { .. })
            && matches!(ty.kind(), TyKind::Struct(name, _) if self.structs.contains_key(name))
            && self.infer_reference_value(operand).is_some()
        {
            self.record_at(operand.source_span())
                .borrowed_read_call_place = true;
        }
    }

    /// The type an inferred chain operand takes at its link, as
    /// [`Self::infer_infix`] sees it.
    fn chain_operand_ty(&self, operand: &Expr) -> Result<Ty, TypeError> {
        let ty = self.infer(operand)?;
        Ok(self.opaque_element(&ty).unwrap_or(ty))
    }

    /// A chain's first middle place or `^` becomes the owned temporary its
    /// second link reads, which the body must be able to destroy: a type
    /// parameter whose bounds do not prove `Deinitable` is abandoned, as
    /// upstream rejects it, naming the parameter's first bound.
    fn demand_chain_middle_destroyable(&self, operand: &Expr) -> Result<(), TypeError> {
        const TEMPORARY: &str = "(expression temporary)";
        let ty = self.chain_operand_ty(operand)?;
        match ty.kind() {
            TyKind::Param { binder, bounds, .. }
                if self.is_own_parameter(binder) && !self.is_deinitable(&ty) =>
            {
                Err(TypeError::LinearAbandoned {
                    var: TEMPORARY.to_string(),
                    message: format!(
                        "unhandled explicitly destroyed type '{}'",
                        bounds.first().map_or("AnyType", String::as_str)
                    ),
                })
            }
            TyKind::Param { .. } => Ok(()),
            _ => self.check_overwritten_referent(TEMPORARY, &ty),
        }
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

    /// The lowered symbol of the dunder an operator selected on struct
    /// `sname`: its overload key when the dunder is overloaded, else
    /// `{struct}.{dunder}`.
    fn dunder_symbol(
        &self,
        sname: &str,
        dunder: &str,
        sig: &MethodSig,
        info: &StructInfo,
    ) -> String {
        if info.methods.get(dunder).is_some_and(|sigs| sigs.len() > 1) {
            method_lowered_name(sname, dunder, sig, self.self_instance_ty(sname).as_ref())
        } else {
            format!("{sname}.{dunder}")
        }
    }

    /// The requirement a comparison over a bound type parameter selects
    /// (`__trait_dispatch.__lt__$ov$Self`): the comparison trait's the
    /// program declares, else the compiler-known one's.
    fn bound_comparison_target(&self, op: InfixOp) -> Option<String> {
        let trait_name = match op {
            InfixOp::Eq | InfixOp::Ne => "Equatable",
            InfixOp::Lt | InfixOp::Gt | InfixOp::Le | InfixOp::Ge => "Comparable",
            _ => return None,
        };
        let dunder = op.dunder()?;
        if let Some(info) = self.traits.get(trait_name) {
            return info
                .methods
                .get(dunder)?
                .iter()
                .find(|sig| matches!(sig.params.as_slice(), [param] if *param == TyKind::SelfType))
                .map(|sig| method_lowered_name(TRAIT_DISPATCH, dunder, sig, None));
        }
        compiler_trait_declaration(trait_name)?
            .requirements
            .iter()
            .find(|requirement| requirement.name == dunder)
            .map(|requirement| requirement.dispatch.clone())
    }
}

/// A readable symbol for an infix operator, for error messages.
pub(super) const fn infix_symbol(op: InfixOp) -> &'static str {
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
pub(super) const fn prefix_symbol(op: PrefixOp) -> &'static str {
    match op {
        PrefixOp::Neg => "-",
        PrefixOp::Not => "not",
        PrefixOp::Invert => "~",
    }
}

/// What [`Checker::struct_infix_dispatch`] decided for one operator.
pub(super) struct StructInfixDispatch {
    /// The selected dunder's lowered symbol, which `BinOp.resolved` names.
    pub(super) target: String,
    /// The error the selected dunder raises, if it raises.
    pub(super) error: Option<Ty>,
    /// `!=` served by `__eq__` and a negation.
    pub(super) negated_equality: bool,
    /// The right operand's type as the selected dunder takes it.
    pub(super) operand_ty: Ty,
    /// Whether the right operand converts to `operand_ty` through an
    /// `@implicit` constructor.
    pub(super) converted: bool,
    /// Whether the dunder consumes its operand.
    pub(super) consumes: bool,
    /// Whether the dunder reads its receiver (`self`) and its operand in
    /// place: a read convention, and an operand taken without conversion.
    pub(super) borrows: (bool, bool),
    /// Whether the dunder reads its receiver and its operand, so a
    /// temporary on either side is the caller's to destroy once it returns.
    pub(super) reads: (bool, bool),
    pub(super) struct_name: String,
    pub(super) struct_args: Vec<TyArg>,
}

/// What [`Checker::struct_reflected_dispatch`] decided for one operator.
pub(super) struct StructReflectedDispatch {
    /// The reflected dunder's lowered symbol, which `BinOp.resolved` names.
    pub(super) target: String,
    /// The error the reflected dunder raises, if it raises.
    pub(super) error: Option<Ty>,
    /// Whether the reflected dunder reads the left operand (its parameter)
    /// and the right operand (its receiver).
    pub(super) reads: (bool, bool),
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

/// Whether `ty` is the nominal `Optional[Int]` (module-qualified or not).
fn is_optional_int(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::Struct(name, args)
        if (name == "Optional" || name.ends_with("$Optional"))
            && matches!(args.as_slice(), [mojito_types::types::TyArg::Ty(ty)] if *ty == TyKind::Int))
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

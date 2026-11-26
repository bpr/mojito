//! The atomic `__mlir_op` expressions `std.atomic` is written in
//! ([`mojito_types::mlir::AtomicOp`]): `pop.atomic.rmw`,
//! `pop.atomic.cmpxchg`, and `pop.fence`, typed as their MLIR verifiers type
//! them, and the `ordering` and `syncscope` attributes they and an atomic
//! `pop.load` or `pop.store` take. A closed ordering an op refuses is
//! rejected here; a template's is judged per instance by the elaborator.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::ParamArg;
use mojito_types::mlir::{AtomicBinOp, AtomicOp, AtomicOrdering};

/// The spelling of `pop.atomic.rmw`.
pub(super) const ATOMIC_RMW: &str = "pop.atomic.rmw";
/// The spelling of `pop.atomic.cmpxchg`.
pub(super) const ATOMIC_CMPXCHG: &str = "pop.atomic.cmpxchg";
/// The spelling of `pop.fence`.
pub(super) const FENCE: &str = "pop.fence";

impl Checker {
    /// Type ``__mlir_op.`pop.atomic.rmw`[bin_op=…, ordering=…, syncscope=…,
    /// _type=T](p, v)``, ``__mlir_op.`pop.atomic.cmpxchg`[weak=…,
    /// success_ordering=…, failure_ordering=…, syncscope=…](p, e, d)``, or
    /// ``__mlir_op.`pop.fence`[ordering=…, syncscope=…, _type=None]()``,
    /// recording it as `SemanticAdjustment::MlirAtomicOp`. An rmw or
    /// cmpxchg addresses one-lane `!kgen.simd` storage through its
    /// `!kgen.pointer`; the rmw yields the value it replaced, and the
    /// cmpxchg the literal struct of that value and whether it stored.
    pub(super) fn infer_atomic_op(
        &self,
        span: &SourceSpan,
        name: &str,
        call: MethodCallArguments<'_>,
    ) -> Result<Ty, TypeError> {
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            ..
        } = call;
        let invalid = |reason: String| TypeError::BadCall {
            func: name.to_string(),
            reason,
        };
        if !kwargs.is_empty() {
            return Err(invalid("op operands are positional".to_string()));
        }
        let known: &[&str] = match name {
            ATOMIC_RMW => &["bin_op", "ordering", "syncscope", "_type"],
            ATOMIC_CMPXCHG => &["weak", "success_ordering", "failure_ordering", "syncscope"],
            _ => &["ordering", "syncscope", "_type"],
        };
        let mut attributes: Vec<(&str, &ParamArg)> = Vec::new();
        for argument in param_args {
            let ParamArg::Named {
                name: attribute,
                value,
            } = argument
            else {
                return Err(invalid("op attributes are named".to_string()));
            };
            if !known.contains(&attribute.as_str()) {
                return Err(invalid(format!(
                    "attribute '{attribute}' is not an inherent attribute of '{name}'"
                )));
            }
            attributes.push((attribute.as_str(), value.as_ref()));
        }
        let attribute = |wanted: &str| {
            attributes
                .iter()
                .find_map(|(attribute, value)| (*attribute == wanted).then_some(*value))
        };
        let required = |wanted: &str| {
            attribute(wanted).ok_or_else(|| invalid(format!("op requires attribute '{wanted}'")))
        };
        let op = match name {
            ATOMIC_RMW => {
                let bin_op = super::pop_ops::mlir_attribute(required("bin_op")?)
                    .and_then(AtomicBinOp::parse)
                    .ok_or_else(|| {
                        invalid(
                            "attribute 'bin_op' takes a '#pop.bin_op<op>' attribute".to_string(),
                        )
                    })?;
                AtomicOp::Rmw(bin_op)
            }
            ATOMIC_CMPXCHG => {
                let weak = attribute("weak")
                    .map(|value| {
                        (super::pop_ops::mlir_attribute(value) == Some("unit"))
                            .then_some(true)
                            .ok_or_else(|| {
                                invalid("attribute 'weak' takes '__mlir_attr.unit'".to_string())
                            })
                    })
                    .transpose()?
                    .unwrap_or(false);
                AtomicOp::Cmpxchg { weak }
            }
            _ => AtomicOp::Fence,
        };
        let ordering_name = if matches!(op, AtomicOp::Cmpxchg { .. }) {
            "success_ordering"
        } else {
            "ordering"
        };
        let ordering =
            self.atomic_ordering_attribute(name, ordering_name, required(ordering_name)?)?;
        let failure_ordering = match op {
            AtomicOp::Cmpxchg { .. } => Some(self.atomic_ordering_attribute(
                name,
                "failure_ordering",
                required("failure_ordering")?,
            )?),
            _ => None,
        };
        let syncscope = attribute("syncscope")
            .map(|value| self.syncscope_attribute(name, value))
            .transpose()?;
        let arity = 1 + op.value_arity();
        let arity = if op == AtomicOp::Fence { 0 } else { arity };
        if args.len() != arity {
            return Err(invalid(format!(
                "op takes {arity} operand(s), got {}",
                args.len()
            )));
        }
        let operands = args
            .iter()
            .map(|argument| self.infer(argument))
            .collect::<Result<Vec<_>, _>>()?;
        if op == AtomicOp::Fence {
            if let Some(value) = attribute("_type")
                && !matches!(
                    value,
                    ParamArg::Value(Expr {
                        kind: ExprKind::None,
                        ..
                    })
                )
            {
                return Err(invalid(
                    "op produces no result; '_type' must be None".to_string(),
                ));
            }
            self.record_atomic_op(span, op, ordering, failure_ordering, syncscope)?;
            return Ok(Ty::none());
        }
        let element = match operands[0].kind() {
            TyKind::KgenPointer { element, .. } if is_scalar_storage(element) => element.clone(),
            _ => {
                return Err(invalid(format!(
                    "operand #0 must be a '!kgen.pointer' to one-lane '!kgen.simd' storage, but \
                     got '{}'",
                    operands[0]
                )));
            }
        };
        if let Some((index, found)) = operands
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, found)| **found != element)
        {
            return Err(invalid(format!(
                "operand #{index} of type '{found}' is not the element of '{}'",
                operands[0]
            )));
        }
        let result = if let AtomicOp::Rmw(_) = op {
            let declared = self.mlir_result_type(required("_type")?)?;
            if declared != element {
                return Err(invalid(format!(
                    "result type '{declared}' is not the element of '{}'",
                    operands[0]
                )));
            }
            element
        } else {
            Ty::from(TyKind::Tuple(vec![
                element,
                mojito_types::types::kgen_simd_ty(mojito_ast::ast::Dtype::Bool, 1),
            ]))
        };
        self.record_atomic_op(span, op, ordering, failure_ordering, syncscope)?;
        Ok(result)
    }

    /// Record the atomic op `op` at `span` with its attributes, after
    /// rejecting closed orderings it refuses ([`AtomicOp::ordering_error`]).
    pub(super) fn record_atomic_op(
        &self,
        span: &SourceSpan,
        op: AtomicOp,
        ordering: ParamExpr,
        failure_ordering: Option<ParamExpr>,
        syncscope: Option<ParamExpr>,
    ) -> Result<(), TypeError> {
        let closed = |value: &ParamExpr| match value.as_constant() {
            Some(CtValue::MlirAttr(attribute)) => AtomicOrdering::parse(attribute),
            _ => None,
        };
        let failure = failure_ordering.as_ref().map(closed);
        if let Some(success) = closed(&ordering)
            && !matches!(failure, Some(None))
            && let Some(reason) = op.ordering_error(success, failure.flatten(), syncscope.is_some())
        {
            return Err(TypeError::BadCall {
                func: op.name().to_string(),
                reason,
            });
        }
        self.record_at(span.clone()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::MlirAtomicOp {
                op,
                ordering,
                failure_ordering,
                syncscope,
            },
        ));
        Ok(())
    }

    /// Check an atomic op's ordering attribute `attribute`: a compile-time
    /// `!kgen.deferred` (`ordering.__mlir_attr()`), whose closed value is a
    /// `#pop.atomic_ordering<…>` attribute.
    pub(super) fn atomic_ordering_attribute(
        &self,
        name: &str,
        attribute: &str,
        value: &ParamArg,
    ) -> Result<ParamExpr, TypeError> {
        let rejected = || TypeError::BadCall {
            func: name.to_string(),
            reason: format!(
                "attribute '{attribute}' takes a '#pop.atomic_ordering<ordering>' attribute"
            ),
        };
        let ParamArg::Value(expression) = value else {
            return Err(rejected());
        };
        let built = {
            let _lifting = self.lifting_position();
            self.compile_dependent_ct_expr(expression)?
        };
        if !matches!(built.meta(), MetaTy::Value(ty) if matches!(ty.kind(), TyKind::KgenDeferred)) {
            return Err(rejected());
        }
        match built.as_constant() {
            Some(CtValue::MlirAttr(closed)) if AtomicOrdering::parse(closed).is_none() => {
                Err(TypeError::BadCall {
                    func: name.to_string(),
                    reason: format!(
                        "Invalid attribute `{attribute}` in property conversion: {closed}"
                    ),
                })
            }
            _ => Ok(built),
        }
    }

    /// Check an atomic op's `syncscope` attribute: a compile-time
    /// `!kgen.string` (`_get_kgen_string[scope]()`), closed or a
    /// template's parameter expression.
    pub(super) fn syncscope_attribute(
        &self,
        name: &str,
        value: &ParamArg,
    ) -> Result<ParamExpr, TypeError> {
        let rejected = || TypeError::BadCall {
            func: name.to_string(),
            reason: "attribute 'syncscope' takes a '!kgen.string'".to_string(),
        };
        let ParamArg::Value(expression) = value else {
            return Err(rejected());
        };
        let built = {
            let _lifting = self.lifting_position();
            self.compile_dependent_ct_expr(expression)?
        };
        if !matches!(built.meta(), MetaTy::Value(ty) if matches!(ty.kind(), TyKind::KgenString)) {
            return Err(rejected());
        }
        Ok(built)
    }
}

/// Whether `ty` is one-lane `!kgen.simd` storage, the element an rmw or
/// cmpxchg updates.
fn is_scalar_storage(ty: &Ty) -> bool {
    matches!(
        ty.kind(),
        TyKind::KgenSimd {
            width: mojito_types::types::SimdWidth::Known(1),
            ..
        }
    )
}

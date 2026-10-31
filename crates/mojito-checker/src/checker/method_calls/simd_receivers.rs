//! Method calls on `SIMD` and `DType` receivers.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::mlir::MlirOp;

impl Checker {
    /// `DType`'s `Bool` queries (`is_integral()`, `is_floating_point()`, …).
    pub(super) fn infer_dtype_predicate(&self, site: MethodCallSite<'_>) -> Result<Ty, TypeError> {
        let MethodCallSite { method, call, .. } = site;
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            ..
        } = call;
        reject_kwargs(kwargs)?;
        if !param_args.is_empty() {
            return Err(TypeError::WrongTypeArgCount {
                name: format!("DType.{method}"),
                expected: 0,
                got: param_args.len(),
            });
        }
        self.builtin_args(&format!("DType.{method}"), 0, args)?;
        Ok(Ty::bool())
    }

    /// The compiler-known methods of a `SIMD` value.
    pub(super) fn infer_simd_method(
        &self,
        site: MethodCallSite<'_>,
        dtype: &SimdDtype,
    ) -> Result<Ty, TypeError> {
        let MethodCallSite {
            span,
            object,
            method,
            call,
            obj_ty,
        } = site;
        let MethodCallArguments {
            param_args,
            args,
            kwargs,
            ..
        } = call;
        let dtype = dtype.clone();
        // A name no compiler-known method answers falls through, keywords
        // and all, to the declared members (`__setitem__(i, val=…)`).
        if !matches!(
            method,
            "copy"
                | "reduce_add"
                | "reduce_mul"
                | "reduce_min"
                | "reduce_max"
                | "reduce_and"
                | "reduce_or"
        ) {
            return Err(TypeError::NoSuchMethod {
                object_type: obj_ty.to_string(),
                method: method.to_string(),
            });
        }
        reject_kwargs(kwargs)?;
        // Compiler-known SIMD methods: the lane reductions collapse to
        // the canonicalized width-1 scalar (`reduce_and`/`reduce_or` to
        // `Bool`). A symbolic dtype licenses
        // every dtype-gated method, and a lane fact records the symbolic
        // slot: both are the instantiation's to check.
        match method {
            // `Copyable.copy` on a scalar or vector is the value read
            // itself (the builtin-copy rule of the nominal resolver).
            "copy" if args.is_empty() && param_args.is_empty() => {
                if is_place_expr(object) {
                    self.record_at(object.source_span()).copy_place_value_use = true;
                }
                Ok(obj_ty.clone())
            }
            "reduce_add" | "reduce_mul" | "reduce_min" | "reduce_max"
                if dtype.licenses(|d| d != Dtype::Bool) && args.is_empty() =>
            {
                simd_of(dtype, SimdWidth::Known(1))
            }
            // Upstream's `bool` and integer reductions give one lane,
            // which Mojito types `Bool` for `bool` lanes (and a symbolic
            // dtype's), so the reduction combines with `and` and `or`.
            "reduce_and" | "reduce_or" if dtype.licenses(|d| !d.is_float()) && args.is_empty() => {
                self.record_simd_receiver_op(
                    span,
                    if method == "reduce_and" {
                        MlirOp::PopSimdReduceAnd
                    } else {
                        MlirOp::PopSimdReduceOr
                    },
                );
                match dtype.known() {
                    Some(lanes) if lanes != Dtype::Bool => {
                        simd_of(SimdDtype::Known(lanes), SimdWidth::Known(1))
                    }
                    _ => Ok(Ty::bool()),
                }
            }
            _ => Err(TypeError::NoSuchMethod {
                object_type: obj_ty.to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Record the method call at `span` as the `pop.simd` lane `op` over
    /// its receiver and arguments.
    fn record_simd_receiver_op(&self, span: &SourceSpan, op: MlirOp) {
        self.record_at(span.clone()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::SimdReceiverOp(op),
        ));
    }
}

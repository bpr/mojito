//! Method calls on `SIMD` and `DType` receivers.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::mlir::MlirOp;
use mojito_types::types::LaneMask;

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
        width: &SimdWidth,
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
        let (dtype, width) = (dtype.clone(), width.clone());
        // A name no compiler-known method answers falls through, keywords
        // and all, to the declared members (`__setitem__(i, val=…)`).
        if !matches!(
            method,
            "copy"
                | "slice"
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
            // `v.slice[output_width, offset=o]()`: `output_width`
            // consecutive lanes starting at lane `o` (default 0).
            "slice" if !param_args.is_empty() && args.is_empty() => {
                self.infer_simd_slice(site, dtype, &width)
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

    /// `v.slice[output_width, offset=o]()`: `output_width` consecutive lanes
    /// starting at lane `o` (default 0). A width or an offset over a
    /// template's binders keeps the slice's form, whose bounds the instance
    /// checks.
    fn infer_simd_slice(
        &self,
        site: MethodCallSite<'_>,
        dtype: SimdDtype,
        width: &SimdWidth,
    ) -> Result<Ty, TypeError> {
        let MethodCallSite { span, call, .. } = site;
        let MethodCallArguments { param_args, .. } = call;
        let bad_argument = || TypeError::TypeMismatch {
            expected: "a compile-time output width and an optional 'offset='".to_string(),
            found: format!("{} parameter arguments", param_args.len()),
            context: "SIMD.slice".to_string(),
        };
        let mut output_width = None;
        let mut offset = None;
        for argument in param_args {
            match argument {
                mojito_ast::ast::ParamArg::Value(expression) if output_width.is_none() => {
                    let count = self.gather_parameter(expression)?;
                    output_width = Some(match count.as_i64() {
                        Some(count) if count < 1 || (count & (count - 1)) != 0 => {
                            return Err(TypeError::BadSimdWidth(count.to_string()));
                        }
                        Some(count) => SimdWidth::Known(count),
                        // Upstream's `output_width: Int` converts into the
                        // result's `SIMDLength` slot.
                        None => self.width_of_lanes(&count)?,
                    });
                }
                mojito_ast::ast::ParamArg::Named { name, value } if name == "offset" => {
                    let mojito_ast::ast::ParamArg::Value(expression) = value.as_ref() else {
                        return Err(bad_argument());
                    };
                    offset = Some(self.gather_parameter(expression)?);
                }
                _ => return Err(bad_argument()),
            }
        }
        let output_width = output_width.ok_or_else(bad_argument)?;
        let offset = match offset {
            Some(offset) => offset,
            None => self
                .param_context
                .constant(CtValue::Int(0))
                .map_err(param_error)?,
        };
        if let (Some(count), Some(start)) = (output_width.known(), offset.as_i64())
            && (start < 0 || width.known().is_some_and(|width| start + count > width))
        {
            return Err(TypeError::TypeMismatch {
                expected: format!("an output width and offset within the receiver's {width} lanes"),
                found: format!("width {count} at offset {start}"),
                context: "SIMD.slice".to_string(),
            });
        }
        let mask = LaneMask::Slice {
            start: offset,
            count: output_width.clone(),
        };
        self.record_lane_gather(span, &mask, width);
        simd_of(dtype, output_width)
    }

    /// The width slot of the symbolic lane count `lanes`, an `Int`
    /// converted to the `SIMDLength` the slot holds.
    fn width_of_lanes(&self, lanes: &ParamExpr) -> Result<SimdWidth, TypeError> {
        let length = self
            .simd_length_of_lanes(lanes)
            .ok_or_else(|| TypeError::BadSimdWidth(lanes.to_string()))?;
        SimdWidth::of_length(&self.param_context, length).map_err(param_error)
    }

    /// A gather's compile-time `Int` argument: its constant, or the
    /// expression over a template's binders that only an instance closes.
    fn gather_parameter(&self, expression: &Expr) -> Result<ParamExpr, TypeError> {
        let literal = match self.eval_ct(expression) {
            Ok(literal) => literal,
            Err(error) => {
                return match self.eval_associated_ct(expression) {
                    Ok(CtValue::Expr(expr)) if expr.meta().is_integer() => Ok(expr),
                    _ => Err(error),
                };
            }
        };
        let value = literal.to_i64().ok_or_else(|| TypeError::TypeMismatch {
            expected: "a compile-time Int".to_string(),
            found: literal.to_string(),
            context: "SIMD lane gather".to_string(),
        })?;
        self.param_context
            .constant(CtValue::Int(value))
            .map_err(param_error)
    }

    /// Record a gather's mask: known where the receiver width and the
    /// gather's arguments are, else the template's form for the instance to
    /// close.
    fn record_lane_gather(&self, span: &SourceSpan, mask: &LaneMask, width: &SimdWidth) {
        let mask = width
            .known()
            .and_then(|width| mask.resolve(width))
            .and_then(Result::ok)
            .map_or_else(|| mask.clone(), LaneMask::Known);
        self.record_at(span.clone()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::SimdShuffle { mask },
        ));
    }

    /// Record the method call at `span` as the `pop.simd` lane `op` over
    /// its receiver and arguments.
    fn record_simd_receiver_op(&self, span: &SourceSpan, op: MlirOp) {
        self.record_at(span.clone()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::SimdReceiverOp(op),
        ));
    }
}

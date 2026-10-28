//! Method calls on `SIMD` and `DType` receivers.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::mlir::MlirOp;
use mojito_types::types::LaneMask;

impl Checker {
    /// `v.to_bits[DType.target]()`: the target must be unsigned and at least
    /// as wide as the source lane, and defaults to the unsigned dtype of the
    /// source width.
    pub(super) fn infer_simd_to_bits(
        &self,
        site: MethodCallSite<'_>,
        source: &SimdDtype,
        width: SimdWidth,
    ) -> Result<Ty, TypeError> {
        let MethodCallSite { span, call, .. } = site;
        let MethodCallArguments {
            param_args, kwargs, ..
        } = call;
        reject_kwargs(kwargs)?;
        let target = match (param_args.first(), source) {
            (Some(argument), _) => self.dtype_from_arg(argument)?,
            (None, SimdDtype::Known(source)) => {
                SimdDtype::Known(unsigned_dtype_of_width(source.bit_width()))
            }
            (None, SimdDtype::Expr(source)) => SimdDtype::Expr(
                symbolic_unsigned_dtype_of(&self.param_context, source).map_err(param_error)?,
            ),
        };
        if let Some(target_dtype) = target.known() {
            let unsigned = matches!(
                target_dtype,
                Dtype::UInt8 | Dtype::UInt16 | Dtype::UInt32 | Dtype::UInt64
            );
            let narrower = source
                .known()
                .is_some_and(|source| target_dtype.bit_width() < source.bit_width());
            if !unsigned || narrower {
                return Err(TypeError::TypeMismatch {
                    expected: "an unsigned dtype at least as wide as the source lane".to_string(),
                    found: target.to_string(),
                    context: "SIMD.to_bits".to_string(),
                });
            }
        }
        // A lane still a parameter is judged per instance, as upstream's
        // `comptime assert` in the body is.
        if let (Some(target_dtype), SimdDtype::Expr(lane)) = (target.known(), source)
            && source.is_symbolic()
            && let Some(holds) = Dtype::ALL
                .into_iter()
                .filter(|wider| wider.bit_width() > target_dtype.bit_width())
                .map(|wider| {
                    GenericConstraint::Ne(
                        ConstraintOperand::Expr(lane.clone()),
                        ConstraintOperand::Value(CtValue::Dtype(wider)),
                    )
                })
                .reduce(|left, right| GenericConstraint::And(Box::new(left), Box::new(right)))
        {
            self.record_instance_constraint(
                span,
                holds,
                "the target type must be at least as wide as the source type",
            );
        }
        self.record_at(span.clone()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::SimdToBits {
                dtype: target.clone(),
                width: width.clone(),
            },
        ));
        simd_of(target, width)
    }

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
            "shuffle" if !param_args.is_empty() && args.is_empty() => {
                self.infer_simd_shuffle(site, dtype, width)
            }
            // `v.slice[output_width, offset=o]()`: `output_width`
            // consecutive lanes starting at lane `o` (default 0).
            "slice" if !param_args.is_empty() && args.is_empty() => {
                self.infer_simd_slice(site, dtype, &width)
            }
            // `v.join(w)`: the receiver's lanes then `w`'s, at twice the
            // width; `w` has the receiver's own type.
            "join" if args.len() == 1 && param_args.is_empty() => {
                self.infer_simd_join(site, dtype, &width)
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
            // A float scalar's fused multiply-add (`k.__fma__(step,
            // start)`, a float range element) is an intrinsic at the
            // scalar's own precision.
            "__fma__"
                if width.known() == Some(1)
                    && dtype.licenses(Dtype::is_float)
                    && args.len() == 2 =>
            {
                for argument in args {
                    let found = self.infer(argument)?;
                    if found != *obj_ty {
                        return Err(TypeError::TypeMismatch {
                            expected: obj_ty.to_string(),
                            found: found.to_string(),
                            context: "argument to '__fma__'".to_string(),
                        });
                    }
                }
                Ok(obj_ty.clone())
            }
            _ => Err(TypeError::NoSuchMethod {
                object_type: obj_ty.to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// `v.shuffle[i, j, …]()`: one compile-time lane index per receiver lane.
    /// A receiver width or an index over a template's binders keeps the
    /// mask's form, whose count and range the instance checks.
    fn infer_simd_shuffle(
        &self,
        site: MethodCallSite<'_>,
        dtype: SimdDtype,
        width: SimdWidth,
    ) -> Result<Ty, TypeError> {
        let MethodCallSite { span, call, .. } = site;
        let MethodCallArguments { param_args, .. } = call;
        // Compile-time lane indices, one per receiver lane:
        // upstream spells a narrowing gather `slice` and a
        // widening one `join`.
        let mut lanes = Vec::with_capacity(param_args.len());
        for argument in param_args {
            let mojito_ast::ast::ParamArg::Value(index) = argument else {
                return Err(TypeError::TypeMismatch {
                    expected: "a compile-time lane index".to_string(),
                    found: "a type argument".to_string(),
                    context: "SIMD.shuffle".to_string(),
                });
            };
            let lane = self.gather_parameter(index)?;
            if let Some(known) = lane.as_i64()
                && (known < 0 || width.known().is_some_and(|width| known >= width))
            {
                return Err(TypeError::TypeMismatch {
                    expected: format!("a lane index below {width}"),
                    found: known.to_string(),
                    context: "SIMD.shuffle".to_string(),
                });
            }
            lanes.push(lane);
        }
        if let Some(known) = width.known()
            && lanes.len() as i64 != known
        {
            return Err(TypeError::TypeMismatch {
                expected: format!("{width} lane indices, one per receiver lane"),
                found: format!("{} indices", lanes.len()),
                context: "SIMD.shuffle".to_string(),
            });
        }
        let mask = LaneMask::Shuffle(lanes);
        self.record_lane_gather(span, &mask, &width, false);
        simd_of(dtype, width)
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
        self.record_lane_gather(span, &mask, width, false);
        simd_of(dtype, output_width)
    }

    /// `v.join(w)`: the receiver's lanes then `w`'s, at twice the width.
    fn infer_simd_join(
        &self,
        site: MethodCallSite<'_>,
        dtype: SimdDtype,
        width: &SimdWidth,
    ) -> Result<Ty, TypeError> {
        let MethodCallSite {
            span, call, obj_ty, ..
        } = site;
        let MethodCallArguments { args, .. } = call;
        let other = self.infer(&args[0])?;
        if other != *obj_ty {
            return Err(TypeError::TypeMismatch {
                expected: obj_ty.to_string(),
                found: other.to_string(),
                context: "SIMD.join".to_string(),
            });
        }
        self.record_lane_gather(span, &LaneMask::Join, width, true);
        let joined = match width {
            SimdWidth::Known(width) => {
                let joined = width * 2;
                if joined > 1 << 15 {
                    return Err(TypeError::BadSimdWidth(joined.to_string()));
                }
                SimdWidth::Known(joined)
            }
            // Upstream's `SIMD[Self.dtype, 2 * Int(Self.length)]`.
            SimdWidth::Expr(_) => {
                let context = &self.param_context;
                let two = context.constant(CtValue::Int(2)).map_err(param_error)?;
                let lanes = width.lanes(context).map_err(param_error)?;
                self.width_of_lanes(
                    &context
                        .infix(InfixOp::Mul, &lanes, &two)
                        .map_err(param_error)?,
                )?
            }
        };
        simd_of(dtype, joined)
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
    fn record_lane_gather(
        &self,
        span: &SourceSpan,
        mask: &LaneMask,
        width: &SimdWidth,
        joined: bool,
    ) {
        let mask = width
            .known()
            .and_then(|width| mask.resolve(width))
            .and_then(Result::ok)
            .map_or_else(|| mask.clone(), LaneMask::Known);
        self.record_at(span.clone()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::SimdShuffle { mask, joined },
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

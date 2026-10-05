//! Certificate grammar for `SIMD` values: constructions, lane reads,
//! comparisons, and float lane methods.

use super::{BodyShape, comparison, fact_at, grammar_scalar, lane_mask, push_unique};
use mojito_ast::ast::{Expr, ExprKind};
use mojito_checked::templates::{MethodFeatures, OccurrenceId};
use mojito_types::types::Ty;
use std::collections::HashSet;

impl BodyShape<'_> {
    /// A construction of a closed `SIMD`, `Scalar`, or scalar-alias value
    /// from closed scalars (`UInt8(1)`, `SIMD[DType.uint8, 2](1, 2)`).
    ///
    /// Inference records a construction's dtype and width as its slots: a
    /// closed one is the same under every instance, and one naming the
    /// declaration's own value binders (`Scalar[dt](x)`, `SIMD[DType.int32,
    /// w](x)`) closes under the instance's arguments as the construction
    /// type does. It selects no callee and converts nothing, and its value
    /// is admitted only where a closed value may go ([`Self::simd_value`]).
    pub(super) fn simd_construction(
        &self,
        id: OccurrenceId,
        name: &str,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> bool {
        let admitted = (name == "SIMD"
            || name == "Scalar"
            || mojito_ast::ast::Dtype::from_scalar_alias(name).is_some()
            || self.vector_aliases.contains(&name))
            && kwargs.is_empty()
            && args
                .iter()
                .all(|argument| self.expression(argument) && self.scalar(argument))
            && self.facts.is_none_or(|facts| {
                let recorded = fact_at(&facts.simd_constructions, id).is_some();
                fact_at(&facts.expression_types, id).is_some_and(|ty| {
                    (recorded && !mojito_types::types::is_symbolic(ty))
                        || self.value_shaped_simd(ty)
                        || self.struct_lane_simd(ty)
                }) && fact_at(&facts.call_parameters, id).is_none()
                    && fact_at(&facts.overload_targets, id).is_none()
                    && fact_at(&facts.generic_instantiations, id).is_none()
                    && fact_at(&facts.selected_calls, id).is_none()
                    && fact_at(&facts.conversions, id).is_none()
                    && fact_at(&facts.operation_adjustments, id).is_none()
            });
        admitted && self.holds(MethodFeatures::SIMD_CONSTRUCTIONS)
    }

    /// A lane read on a vector the body holds: `v.to_bits[DType.<name>]()`
    /// or `v.cast[DType.<name>]()` (an `Invoke` with a `Member` callee and
    /// one `DType` argument), `v.to_bits()` or `v.reduce_*()` (a
    /// `MethodCall` without arguments),
    /// `v.length` (a `Member`), or `v[i]` (an `Index` over a scalar), on a
    /// receiver [`Self::lane_receiver`] admits: a parameter, a `var` local,
    /// a field of `self`, or another lane read, of a closed vector type or a
    /// value-shaped one.
    ///
    /// Each is a compiler-known operation on `Ty::Simd`: it selects no
    /// callee, converts nothing, and records at most its shape, the
    /// `SimdToBits`, `SimdCast`, or `SimdLength` adjustment, whose slots
    /// close under the instance as the types do. Over a value-shaped
    /// receiver the instance records its own from the substituted types
    /// (`realize_simd_intrinsics`), checking its source lane. A lane read and a reduction stand
    /// on a receiver whose dtype is closed, so their results are closed, or,
    /// a lane read, on a value-shaped vector of a known width above one
    /// ([`Self::value_shaped_vector`]), whose lane is value-shaped; a
    /// cast's or a reinterpretation's explicit target closes the result's
    /// dtype itself, a defaulted reinterpretation's is the unsigned dtype of
    /// the receiver's lane width, and a lane count is an `Int`.
    pub(super) fn simd_intrinsic(&self, expr: &Expr) -> bool {
        self.lane_read(expr, false)
    }

    /// A comparison between two values of one value-shaped scalar type
    /// (`self.pos < Scalar[Self.dtype](limit)`), or between such a value and
    /// an integer or float literal (`x < 0`, `x < 0.5`), read only as a condition, through
    /// `Bool(...)`, or bound to a local that is itself read only so
    /// ([`LocalKind::Mask`]).
    ///
    /// Over the open lane it is a `SIMD[DType.bool, 1]` mask, which a
    /// condition tests through `__bool__`; an instance whose lane folds to a
    /// sized vector records the same, but one folding to a native scalar
    /// compares natively to a `Bool`, which a condition reads as it stands.
    /// Each instance re-types the comparison (`realize_lane_comparisons`),
    /// and the truthiness mark follows the type, so its value may go only
    /// where that is all that changes. A literal operand materializes at a
    /// native lane (`realize_lane_literals`).
    pub(super) fn lane_comparison(&self, expr: &Expr) -> bool {
        let ExprKind::Infix(op, left, right) = &expr.kind else {
            return false;
        };
        let id = self.occurrence(expr);
        let lane = |operand: &Expr| {
            !self.folding(operand) && self.expression(operand) && self.value_shaped(operand)
        };
        let literal = |operand: &Expr| {
            let ty = match operand.kind {
                ExprKind::Int(_) => Ty::IntLiteral,
                ExprKind::Float(_) => Ty::FloatLiteral,
                _ => return false,
            };
            !self.folding(operand)
                && self.facts.is_none_or(|facts| {
                    fact_at(&facts.expression_types, self.occurrence(operand)) == Some(&ty)
                })
        };
        let admitted = comparison(*op)
            && (lane(left) || lane(right))
            && [left, right]
                .iter()
                .all(|operand| lane(operand) || literal(operand))
            && self.facts.is_none_or(|facts| {
                let operand =
                    |operand: &Expr| fact_at(&facts.expression_types, self.occurrence(operand));
                (operand(left) == operand(right) || literal(left) || literal(right))
                    && fact_at(&facts.expression_types, id) == Some(&lane_mask())
                    && fact_at(&facts.conversions, self.occurrence(left)).is_none()
                    && fact_at(&facts.conversions, self.occurrence(right)).is_none()
                    && fact_at(&facts.call_parameters, id).is_none()
                    && fact_at(&facts.selected_calls, id).is_none()
                    && fact_at(&facts.overload_targets, id).is_none()
                    && fact_at(&facts.conversions, id).is_none()
                    && fact_at(&facts.operation_adjustments, id).is_none()
            });
        admitted && self.holds(MethodFeatures::SIMD_INTRINSICS)
    }

    /// A float lane's rounding dunder or fused multiply-add on a value of a
    /// value-shaped scalar (`raw.__ceil__()`, `k.__fma__(step, start)` over
    /// `Scalar[Self.dtype]`), each argument a value of the receiver's type.
    ///
    /// Over the open lane it is a SIMD intrinsic that records only its
    /// result, the receiver's type. An instance whose lane folds to a sized
    /// float records the same, but one folding to the native `Float64`
    /// resolves it as that scalar's method, whose argument conventions each
    /// instance records itself (`realize_lane_float_methods`).
    pub(super) fn lane_float_method(
        &self,
        expr: &Expr,
        object: &Expr,
        method: &str,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> bool {
        let arity = match method {
            "__floor__" | "__ceil__" | "__trunc__" => 0,
            "__fma__" => 2,
            _ => return false,
        };
        // Each argument is a named place, which the native `Float64`'s
        // method borrows where an instance resolves to it, or a temporary,
        // which it reads and the caller destroys.
        let argument = |argument: &Expr| match &argument.kind {
            ExprKind::Identifier(name) => {
                self.params.contains(&name.as_str()) || self.declared(name)
            }
            ExprKind::Member { .. } => self.receiver_field(argument),
            ExprKind::Transfer(_) => false,
            _ => !crate::checker::places::is_place_expr(argument),
        };
        let id = self.occurrence(expr);
        let admitted = args.len() == arity
            && args.iter().all(argument)
            && kwargs.is_empty()
            && std::iter::once(object)
                .chain(args)
                .all(|operand| self.expression(operand) && self.value_shaped(operand))
            && self.facts.is_none_or(|facts| {
                let lane = fact_at(&facts.expression_types, id);
                lane.is_some_and(|ty| {
                    self.value_shaped_scalar(ty)
                        && matches!(
                            ty,
                            Ty::Simd {
                                width: mojito_types::types::SimdWidth::Known(1),
                                ..
                            }
                        )
                }) && std::iter::once(object).chain(args).all(|operand| {
                    fact_at(&facts.expression_types, self.occurrence(operand)) == lane
                }) && fact_at(&facts.call_parameters, id).is_none()
                    && fact_at(&facts.selected_calls, id).is_none()
                    && fact_at(&facts.overload_targets, id).is_none()
                    && fact_at(&facts.generic_instantiations, id).is_none()
                    && fact_at(&facts.conversions, id).is_none()
                    && fact_at(&facts.operation_adjustments, id).is_none()
                    && fact_at(&facts.parameterized_method_calls, id).is_none()
                    && fact_at(&facts.method_instantiations, id).is_none()
            });
        admitted && self.holds(MethodFeatures::SIMD_INTRINSICS)
    }

    /// Whether `expr` is a lane read whose value a `var` local may hold as
    /// a scalar: its recorded type is a closed one
    /// (`var bits = value.to_bits()`). A lane or a reduction of the local
    /// is a closed scalar only when its dtype is, and one over an open
    /// dtype stands only as a `cast` source ([`Self::lane_read`]).
    pub(super) fn lane_local_value(&self, expr: &Expr) -> bool {
        self.simd_intrinsic(expr)
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.expression_types, self.occurrence(expr)).is_some_and(grammar_scalar)
            })
    }

    /// A `SIMD` type whose open slots name only the declaration's own
    /// value binders, which every instance folds to literals.
    pub(super) fn value_shaped_simd(&self, ty: &Ty) -> bool {
        let mut named = HashSet::new();
        mojito_types::types::referenced_parameters(ty, &mut named);
        matches!(ty, Ty::Simd { .. })
            && !named.is_empty()
            && named
                .iter()
                .all(|name| self.values.contains(&name.as_str()))
    }

    /// Whether `expr` is an admitted closed `SIMD` construction, a value of
    /// a closed type that is not one of the grammar's scalars.
    pub(super) fn simd_value(&self, expr: &Expr) -> bool {
        matches!(&expr.kind, ExprKind::Call { name, args, kwargs, .. }
            if self.simd_construction(self.occurrence(expr), name, args, kwargs))
    }

    /// [`Self::simd_intrinsic`], where a `cast_source` lane read is the
    /// receiver of a `cast`: its dtype may stay a parameter expression over
    /// the lane binders (a lane of a defaulted `to_bits()`), since the
    /// cast's explicit target closes the value the body goes on to use.
    fn lane_read(&self, expr: &Expr, cast_source: bool) -> bool {
        use mojito_ast::ast::ParamArg;
        use mojito_checked::checked::SemanticAdjustment;
        let dtype_argument = |argument: &ParamArg| match argument {
            ParamArg::Type(_) => true,
            ParamArg::Value(value) => matches!(&value.kind, ExprKind::Member { object, .. }
                if matches!(&object.kind, ExprKind::Identifier(name) if name == "DType")),
            ParamArg::Named { .. } => false,
        };
        let reinterpretation = |method: &str| method == "to_bits";
        let cast = |method: &str| method == "cast";
        let reduction = |method: &str| {
            matches!(
                method,
                "reduce_add"
                    | "reduce_mul"
                    | "reduce_min"
                    | "reduce_max"
                    | "reduce_and"
                    | "reduce_or"
            )
        };
        let (receiver, closed_dtype) = match &expr.kind {
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => {
                let ExprKind::Member { object, field } = &callee.kind else {
                    return false;
                };
                if !(reinterpretation(field) || cast(field))
                    || !args.is_empty()
                    || !kwargs.is_empty()
                    || !matches!(param_args.as_slice(), [argument] if dtype_argument(argument))
                {
                    return false;
                }
                if !self.lane_receiver(object, false, cast(field)) {
                    return false;
                }
                (object, false)
            }
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => {
                if !(reinterpretation(method) || reduction(method))
                    || !args.is_empty()
                    || !kwargs.is_empty()
                {
                    return false;
                }
                (object, reduction(method))
            }
            ExprKind::Member { object, field } if field == "length" => (object, false),
            ExprKind::Index { object, index } => {
                if !(self.expression(index) && self.scalar(index)) {
                    return false;
                }
                (object, !cast_source && !self.value_shaped_vector(object))
            }
            _ => return false,
        };
        if !matches!(expr.kind, ExprKind::Invoke { .. })
            && !self.lane_receiver(receiver, closed_dtype, false)
        {
            return false;
        }
        let id = self.occurrence(expr);
        let Some(facts) = self.facts else {
            return self.holds(MethodFeatures::SIMD_INTRINSICS);
        };
        let Some(ty) = fact_at(&facts.expression_types, id) else {
            return false;
        };
        let shaped = match &expr.kind {
            ExprKind::Member { .. } => *ty == Ty::Int,
            _ => {
                mojito_types::types::simd_slots(ty).is_some()
                    && (!mojito_types::types::is_symbolic(ty) || self.value_shaped_scalar(ty))
            }
        };
        let adjustment = fact_at(&facts.operation_adjustments, id);
        // A lane read into a place is copied out of the vector; each
        // instance re-proves the copy implicit at its own lane type.
        let lane = matches!(expr.kind, ExprKind::Index { .. });
        let source = fact_at(&facts.expression_types, self.occurrence(receiver));
        let open_source = source.is_some_and(mojito_types::types::is_symbolic);
        let adjusted = match (adjustment, &expr.kind) {
            (None, _) => true,
            (
                Some(SemanticAdjustment::SimdToBits { .. }),
                ExprKind::Invoke { .. } | ExprKind::MethodCall { .. },
            ) => true,
            // A recorded cast stands under every instance only when its
            // source lane is closed too, since a `bool` source refuses; over
            // a value-shaped source the instance checks its own lane.
            (Some(SemanticAdjustment::SimdCast { .. }), ExprKind::Invoke { .. }) => source
                .is_some_and(|source| {
                    !mojito_types::types::is_symbolic(source) || self.value_shaped_scalar(source)
                }),
            (Some(SemanticAdjustment::SimdLength { .. }), ExprKind::Member { .. }) => true,
            _ => false,
        };
        let admitted = shaped
            && adjusted
            && fact_at(&facts.call_parameters, id).is_none()
            && fact_at(&facts.selected_calls, id).is_none()
            && fact_at(&facts.overload_targets, id).is_none()
            && fact_at(&facts.generic_instantiations, id).is_none()
            && fact_at(&facts.conversions, id).is_none()
            && fact_at(&facts.parameterized_method_calls, id).is_none()
            && fact_at(&facts.method_instantiations, id).is_none()
            && fact_at(&facts.subscript_descriptors, id).is_none()
            && (!facts.copy_place_value_uses.contains(&id) || lane);
        // A reinterpretation or cast the template recorded over an open
        // source lane is noted too: its closed shape stands, but the
        // instance checks its own source lane against it.
        if admitted && (adjustment.is_none() || open_source) {
            let read = (id, self.occurrence(receiver));
            match &expr.kind {
                ExprKind::Invoke { callee, .. } => match &callee.kind {
                    ExprKind::Member { field, .. } if cast(field) => {
                        push_unique(&mut self.simd_casts.borrow_mut(), read);
                    }
                    _ => push_unique(&mut self.simd_to_bits.borrow_mut(), read),
                },
                ExprKind::MethodCall { method, .. } if reinterpretation(method) => {
                    push_unique(&mut self.simd_to_bits.borrow_mut(), read);
                }
                ExprKind::Member { .. } => push_unique(&mut self.simd_lengths.borrow_mut(), read),
                _ => {}
            }
        }
        admitted && self.holds(MethodFeatures::SIMD_INTRINSICS)
    }

    /// The receiver of a lane read: a parameter, a `var` local, a field of
    /// `self`, or another lane read, whose recorded type is a closed vector
    /// or a value-shaped one. With `closed_dtype`, the dtype slot must be
    /// closed, so a lane or a reduction of it is a closed scalar. A
    /// `cast_source` receiver is the source of a `cast`, which may be a lane
    /// read over an open dtype ([`Self::lane_read`]).
    fn lane_receiver(&self, expr: &Expr, closed_dtype: bool, cast_source: bool) -> bool {
        let named = match &expr.kind {
            ExprKind::Identifier(name) => {
                (self.params.contains(&name.as_str())
                    && !self.callable_params.contains(&name.as_str())
                    && self.local_kind(name).is_none())
                    || self.declared(name)
            }
            ExprKind::Member { .. } => self.receiver_field(expr),
            _ => false,
        };
        let held = named || self.lane_read(expr, cast_source);
        held && self.facts.is_none_or(|facts| {
            fact_at(&facts.expression_types, self.occurrence(expr)).is_some_and(|ty| {
                let Some((dtype, _)) = mojito_types::types::simd_slots(ty) else {
                    return false;
                };
                let symbolic = mojito_types::types::is_symbolic(ty);
                // A native scalar (`Int`, `UInt`, `Float64`) has no lane
                // reads: a value-shaped receiver an instance may close to
                // one keeps its dtype open, and a closed one with such a
                // dtype closes to a vector only at a width above one.
                let vector = match dtype {
                    mojito_types::types::SimdDtype::Known(dtype) => {
                        !symbolic
                            || matches!(
                                mojito_types::types::canonical_simd_ty(dtype, 1),
                                Ty::Simd { .. }
                            )
                    }
                    mojito_types::types::SimdDtype::Expr(_) => false,
                };
                (!symbolic || self.value_shaped_scalar(ty)) && (!closed_dtype || vector)
            })
        })
    }

    /// Whether the recorded type of `expr` is a value-shaped vector of a
    /// known width above one (`SIMD[dt, 4]`): every instance closes it to a
    /// vector, whatever its dtype, so a lane of it is a value-shaped scalar
    /// (`Scalar[dt]`) the instance's own facts close.
    fn value_shaped_vector(&self, expr: &Expr) -> bool {
        self.facts.is_some_and(|facts| {
            fact_at(&facts.expression_types, self.occurrence(expr)).is_some_and(|ty| {
                self.value_shaped_simd(ty)
                    && matches!(
                        ty,
                        Ty::Simd {
                            width: mojito_types::types::SimdWidth::Known(width),
                            ..
                        } if *width > 1
                    )
            })
        })
    }
}

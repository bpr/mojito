//! The `pop` and `index` `__mlir_op` expressions: upstream's elementwise
//! ops over `!kgen.simd` storage and its `index` ops, typed as their MLIR
//! verifiers type them, with the `pred`, `fastmathFlags`, and `_type`
//! attributes they take. The admitted set is `mojito_types::mlir::MlirOp`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::mlir::{LaneKinds, MlirLowering, MlirOp, MlirOpError, is_fastmath_attribute};

impl Checker {
    /// Type `__mlir_op.`name`[attributes](operands)` for an admitted `pop`
    /// or `index` op, recording it as `SemanticAdjustment::MlirOp`. A `pop`
    /// op takes `!kgen.simd` operands of one type (`Int`, `UInt`, and
    /// `Float64` are width-one vectors), an `index` op `__mlir_type.index`
    /// ones (`Int`). Arithmetic keeps the operand type, `pop.cmp` gives the
    /// `bool` mask of the operand width, `index.cmp` an `i1` (`Bool`), and
    /// `pop.cast` and `pop.simd.splat` the type their `_type` attribute
    /// names.
    pub(super) fn infer_pop_op(
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
        let mut predicate = None;
        let mut result_type = None;
        for argument in param_args {
            let mojito_ast::ast::ParamArg::Named {
                name: attribute,
                value,
            } = argument
            else {
                return Err(invalid("op attributes are named".to_string()));
            };
            match attribute.as_str() {
                "pred" => {
                    predicate = Some(mlir_attribute(value).ok_or_else(|| {
                        invalid("attribute 'pred' takes an '__mlir_attr' predicate".to_string())
                    })?);
                }
                "_type" => result_type = Some(value.as_ref()),
                "fastmathFlags" if !name.starts_with("index.") => {
                    if !mlir_attribute(value).is_some_and(is_fastmath_attribute) {
                        return Err(invalid(
                            "attribute 'fastmathFlags' takes one '#pop.fmf<flag>' attribute"
                                .to_string(),
                        ));
                    }
                }
                _ => {
                    return Err(invalid(format!(
                        "attribute '{attribute}' is not an inherent attribute of '{name}'"
                    )));
                }
            }
        }
        let op = MlirOp::parse(name, predicate).map_err(|error| match error {
            MlirOpError::Unknown => TypeError::Unsupported(format!(
                "'__mlir_op.`{name}`' is outside the subset: Mojito admits the `pop` and \
                 `index` ops of upstream's `SIMD`, `SIMDLength`, and `Bool` arithmetic"
            )),
            MlirOpError::MissingPredicate => invalid("op requires attribute 'pred'".to_string()),
            MlirOpError::UnexpectedPredicate => invalid(format!(
                "attribute 'pred' is not an inherent attribute of '{name}'"
            )),
            MlirOpError::BadPredicate(attribute) => invalid(format!(
                "'{attribute}' is not a predicate Mojito admits here"
            )),
        })?;
        if args.len() != op.arity() {
            return Err(invalid(format!(
                "op takes {} operand(s), got {}",
                op.arity(),
                args.len()
            )));
        }
        let result_type = match (op.takes_result_type(), result_type) {
            (true, Some(value)) => Some(self.mlir_result_type(value)?),
            (true, None) => return Err(invalid("op requires attribute '_type'".to_string())),
            (false, Some(_)) => {
                return Err(invalid(format!(
                    "attribute '_type' is not an inherent attribute of '{name}'"
                )));
            }
            (false, None) => None,
        };
        let operands = args
            .iter()
            .map(|argument| self.infer(argument))
            .collect::<Result<Vec<_>, _>>()?;
        let result = if op.is_index() {
            index_op_result(op, &operands).map_err(invalid)?
        } else {
            pop_op_result(op, &operands, result_type).map_err(|error| match error {
                PopTypeError::Verifier(reason) => invalid(reason),
                PopTypeError::Subset(reason) => {
                    TypeError::Unsupported(format!("'{name}' {reason}"))
                }
            })?
        };
        self.record_at(span.clone()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::MlirOp(op),
        ));
        Ok(result)
    }

    /// The type a `_type` attribute names: a type annotation, or an
    /// expression denoting one (`Lanes[target, Self.length]._mlir_type`).
    fn mlir_result_type(&self, value: &mojito_ast::ast::ParamArg) -> Result<Ty, TypeError> {
        match value {
            mojito_ast::ast::ParamArg::Type(annotation) => self.ty_from_anno(annotation),
            mojito_ast::ast::ParamArg::Value(expression) => {
                self.denoted_type(expression, "attribute '_type'")
            }
            mojito_ast::ast::ParamArg::Named { value, .. } => self.mlir_result_type(value),
        }
    }
}

/// Why a `pop` op's operands do not type: upstream's verifier rejects them,
/// or the pin accepts them and Mojito does not compute them.
enum PopTypeError {
    Verifier(String),
    Subset(String),
}

/// The text of an ``__mlir_attr.`#…` `` attribute argument.
fn mlir_attribute(value: &mojito_ast::ast::ParamArg) -> Option<&str> {
    let mojito_ast::ast::ParamArg::Value(Expr {
        kind: ExprKind::Member { object, field },
        ..
    }) = value
    else {
        return None;
    };
    matches!(&object.kind, ExprKind::Identifier(name) if name == "__mlir_attr")
        .then_some(field.as_str())
}

/// The result of an `index` op: every operand is an `index` (`Int`); the
/// compare gives an `i1` (`Bool`), the rest an `index`.
fn index_op_result(op: MlirOp, operands: &[Ty]) -> Result<Ty, String> {
    if let Some((position, found)) = operands
        .iter()
        .enumerate()
        .find(|(_, operand)| **operand != Ty::int())
    {
        return Err(format!(
            "op operand #{position} must be index, but got '{found}'"
        ));
    }
    Ok(if matches!(op, MlirOp::IndexCmp(_)) {
        Ty::bool()
    } else {
        Ty::int()
    })
}

/// The result of a `pop` op over `operands`, with the `_type` attribute's
/// type when the op takes one.
fn pop_op_result(op: MlirOp, operands: &[Ty], result_type: Option<Ty>) -> Result<Ty, PopTypeError> {
    if let Some((position, operand)) = operands
        .iter()
        .enumerate()
        .find(|(_, operand)| !matches!(operand.kind(), TyKind::Simd { .. }))
    {
        return Err(PopTypeError::Verifier(format!(
            "op operand #{position} must be SIMD type, but got '{operand}'"
        )));
    }
    let Some((dtype, width)) = operands
        .first()
        .and_then(|operand| simd_slots(operand.kind()))
    else {
        return Err(PopTypeError::Verifier("op takes an operand".to_string()));
    };
    if let Some(other) = operands.iter().find(|operand| **operand != operands[0]) {
        return Err(PopTypeError::Verifier(format!(
            "op requires the same type for all operands and results, found '{}' and '{other}'",
            operands[0]
        )));
    }
    if let Some(lanes) = op.lanes()
        && !dtype.licenses(|dtype| lanes.admits(dtype))
    {
        let floats =
            lanes == LaneKinds::IntegralOrBool && dtype.known().is_some_and(Dtype::is_float);
        return Err(if floats {
            PopTypeError::Subset("over float lanes is outside the subset".to_string())
        } else {
            PopTypeError::Verifier(format!(
                "op operand #0 must be SIMD type whose element type is {} dtype, but got '{}'",
                lanes.describe(),
                operands[0]
            ))
        });
    }
    let bool_lanes = |slot: &SimdDtype| slot.known() == Some(Dtype::Bool);
    match op.lowering() {
        MlirLowering::Binary(_) if matches!(op, MlirOp::PopCmp(_)) => {
            simd_of(SimdDtype::Known(Dtype::Bool), width)
                .map_err(|error| PopTypeError::Verifier(error.to_string()))
        }
        MlirLowering::Cast => {
            let target = result_type.expect("a cast names its result type");
            let Some((target_dtype, target_width)) = simd_slots(target.kind()) else {
                return Err(PopTypeError::Verifier(format!(
                    "op result must be SIMD type, but got '{target}'"
                )));
            };
            if target_width != width {
                return Err(PopTypeError::Verifier(format!(
                    "op operand type '{}' and result type '{target}' are cast incompatible",
                    operands[0]
                )));
            }
            if bool_lanes(&dtype) || bool_lanes(&target_dtype) {
                return Err(PopTypeError::Subset(
                    "to or from 'bool' lanes is outside the subset".to_string(),
                ));
            }
            Ok(target)
        }
        MlirLowering::Splat => {
            let target = result_type.expect("a splat names its result type");
            match simd_slots(target.kind()) {
                Some((target_dtype, _))
                    if target_dtype == dtype && width == SimdWidth::Known(1) =>
                {
                    Ok(target)
                }
                _ => Err(PopTypeError::Verifier(format!(
                    "op splats a scalar of the result's element type, but got '{}' for '{target}'",
                    operands[0]
                ))),
            }
        }
        _ => Ok(operands[0].clone()),
    }
}

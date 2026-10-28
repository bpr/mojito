//! The `pop` and `index` `__mlir_op` expressions: upstream's elementwise
//! ops over `!kgen.simd` storage, its `pop.select` of one of two
//! register-passable values, its `pop.simd` lane ops, its `index` ops,
//! and its `pop.cast_from_builtin` and `pop.cast_to_builtin` casts between
//! `index` and `!kgen.scalar<index>`, typed as their MLIR verifiers type
//! them, with the `pred`, `fastmathFlags`, and `_type` attributes they
//! take. The admitted set is `mojito_types::mlir::MlirOp`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::mlir::{LaneKinds, MlirLowering, MlirOp, MlirOpError, is_fastmath_attribute};

impl Checker {
    /// Type `__mlir_op.`name`[attributes](operands)` for an admitted `pop`
    /// or `index` op, recording it as `SemanticAdjustment::MlirOp`. A `pop`
    /// op takes `!kgen.simd` storage operands of one type
    /// ([`TyKind::KgenSimd`], never the `SIMD` value holding it), an `index`
    /// op `__mlir_type.index` ones. Arithmetic keeps the operand type,
    /// `pop.cmp` gives the `bool` mask storage of the operand width,
    /// `index.cmp` an `i1` (`Bool`), and
    /// `pop.cast` and `pop.simd.splat` the type their `_type` attribute
    /// names. `pop.select` is typed by [`select_result`], the lane ops by
    /// [`lane_op_result`], the casts between `index` and its storage by
    /// [`builtin_cast_result`].
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
        let subset_error = |error| match error {
            PopTypeError::Verifier(reason) => invalid(reason),
            PopTypeError::Subset(reason) => TypeError::Unsupported(format!("'{name}' {reason}")),
        };
        let result = if op == MlirOp::PopSelect {
            select_result(&operands, |ty| self.is_trivial_register_passable(ty))
                .map_err(subset_error)?
        } else if op.is_builtin_cast() {
            let result_type = result_type.expect("a builtin cast names its result type");
            builtin_cast_result(op, &operands, result_type).map_err(subset_error)?
        } else if op.is_index() {
            index_op_result(op, &operands).map_err(invalid)?
        } else if op.is_lane() {
            lane_op_result(op, &operands).map_err(invalid)?
        } else {
            pop_op_result(op, &operands, result_type).map_err(subset_error)?
        };
        self.record_at(span.clone()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::MlirOp(op),
        ));
        self.record_symbolic_lane_rule(span, name, op, &operands);
        Ok(result)
    }

    /// A lane rule over a dtype still a parameter (`pop.mul` in `SIMD`'s
    /// `__mul__`) is the op verifier's in each instance: record it at
    /// `span` as an assertion that the dtype is none the rule excludes.
    fn record_symbolic_lane_rule(
        &self,
        span: &SourceSpan,
        name: &str,
        op: MlirOp,
        operands: &[Ty],
    ) {
        if self.comptime_positions.get() != 0
            || op == MlirOp::PopSelect
            || op.is_builtin_cast()
            || op.is_index()
            || op.is_lane()
        {
            return;
        }
        let (Some(lanes), Some((SimdDtype::Expr(dtype), _))) =
            (op.lanes(), operands.first().and_then(storage_slots))
        else {
            return;
        };
        if !SimdDtype::Expr(dtype.clone()).is_symbolic() {
            return;
        }
        let holds = Dtype::ALL
            .into_iter()
            .filter(|excluded| !lanes.admits(*excluded))
            .map(|excluded| {
                GenericConstraint::Ne(
                    ConstraintOperand::Expr(dtype.clone()),
                    ConstraintOperand::Value(CtValue::Dtype(excluded)),
                )
            })
            .reduce(|left, right| GenericConstraint::And(Box::new(left), Box::new(right)));
        if let Some(holds) = holds {
            self.record_instance_constraint(
                span,
                holds,
                &format!(
                    "'{name}' op operand #0 must be SIMD type whose element type is {} dtype",
                    lanes.describe()
                ),
            );
        }
    }

    /// The type a `_type` attribute names: a type annotation, or an
    /// expression denoting one (`Lanes[target, Self.length]._mlir_type`).
    pub(super) fn mlir_result_type(
        &self,
        value: &mojito_ast::ast::ParamArg,
    ) -> Result<Ty, TypeError> {
        match value {
            mojito_ast::ast::ParamArg::Type(annotation) => self.ty_from_anno(annotation),
            mojito_ast::ast::ParamArg::Value(expression) => {
                self.denoted_type(expression, "attribute '_type'")
            }
            mojito_ast::ast::ParamArg::Named { value, .. } => self.mlir_result_type(value),
        }
    }
}

/// The text of an ``__mlir_attr.`#…` `` attribute argument.
pub(in crate::checker) fn mlir_attribute(value: &mojito_ast::ast::ParamArg) -> Option<&str> {
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

/// Why a `pop` op's operands do not type: upstream's verifier rejects them,
/// or the pin accepts them and Mojito does not compute them.
enum PopTypeError {
    Verifier(String),
    Subset(String),
}

/// How an MLIR verifier spells `ty`: the builtin `index` as `index`, and a
/// `!kgen.simd` storage type unwrapped from its `__mlir_type` spelling.
fn mlir_spelling(ty: &Ty) -> String {
    match ty.kind() {
        TyKind::Index => "index".to_string(),
        TyKind::KgenSimd { .. } => ty
            .to_string()
            .trim_start_matches("__mlir_type.`")
            .trim_end_matches('`')
            .to_string(),
        _ => ty.to_string(),
    }
}

/// The slots of a `pop` op's `!kgen.simd` storage operand.
fn storage_slots(ty: &Ty) -> Option<(SimdDtype, SimdWidth)> {
    mojito_types::types::kgen_simd_slots(ty.kind())
}

/// The `!kgen.simd` storage of `dtype` lanes at `width`.
fn storage_of(dtype: SimdDtype, width: SimdWidth) -> Result<Ty, String> {
    simd_of(dtype, width)
        .map(|vector| mojito_types::types::simd_storage(&vector).unwrap_or(vector))
        .map_err(|error| error.to_string())
}

/// The result of an `index` op: every operand is an `index`; the compare
/// gives an `i1` (`Bool`), the rest an `index`.
fn index_op_result(op: MlirOp, operands: &[Ty]) -> Result<Ty, String> {
    if let Some((position, found)) = operands
        .iter()
        .enumerate()
        .find(|(_, operand)| **operand != Ty::index())
    {
        return Err(format!(
            "op operand #{position} must be index, but got '{}'",
            mlir_spelling(found)
        ));
    }
    Ok(if matches!(op, MlirOp::IndexCmp(_)) {
        Ty::bool()
    } else {
        Ty::index()
    })
}

/// The result of `pop.select`, as upstream's verifier types it: a
/// `!kgen.scalar<bool>` condition (a `Bool`'s `__mlir_bool__()`) and two
/// cases of one type, which the result has. Mojito selects trivially
/// register-passable values, and no literal: the pin types each literal
/// value apart, so `1` and `2` are cases of two types.
fn select_result(
    operands: &[Ty],
    trivially_register_passable: impl Fn(&Ty) -> bool,
) -> Result<Ty, PopTypeError> {
    if operands[0] != mojito_types::types::kgen_simd_ty(Dtype::Bool, 1) {
        return Err(PopTypeError::Verifier(format!(
            "op operand #0 must be !kgen.scalar<bool>, but got '{}'",
            mlir_spelling(&operands[0])
        )));
    }
    let case = &operands[1];
    if *case != operands[2] {
        return Err(PopTypeError::Verifier(
            "op failed to verify that all of {trueValue, falseValue, result} have same type"
                .to_string(),
        ));
    }
    if matches!(case.kind(), TyKind::IntLiteral | TyKind::FloatLiteral) {
        return Err(PopTypeError::Subset(
            "over literal cases is outside the subset: the pin types each literal value apart"
                .to_string(),
        ));
    }
    if !trivially_register_passable(case) {
        return Err(PopTypeError::Subset(format!(
            "over '{case}' is outside the subset: Mojito selects trivially register-passable \
             values"
        )));
    }
    Ok(case.clone())
}

/// The result of `pop.cast_from_builtin` (an `index` to the `_type`
/// storage `!kgen.scalar<index>`, an `Int`'s) or `pop.cast_to_builtin` (that
/// storage to the `_type` builtin `index`), as upstream's verifier types
/// them. The pin's other builtin pairs (`i1` and `!kgen.scalar<bool>`) are
/// outside the subset.
fn builtin_cast_result(op: MlirOp, operands: &[Ty], target: Ty) -> Result<Ty, PopTypeError> {
    let operand = &operands[0];
    let outside = || {
        PopTypeError::Subset(format!(
            "from '{}' to '{}' is outside the subset: Mojito admits the casts between              'index' and '!kgen.scalar<index>'",
            mlir_spelling(operand),
            mlir_spelling(&target)
        ))
    };
    let int_storage = mojito_types::types::kgen_simd_ty(Dtype::Int, 1);
    if op == MlirOp::PopCastFromBuiltin {
        if target != int_storage {
            return Err(outside());
        }
        if *operand != Ty::index() {
            return Err(PopTypeError::Verifier(format!(
                "op cannot convert to scalar dtype index from '{}'",
                mlir_spelling(operand)
            )));
        }
    } else {
        if storage_slots(operand).is_none() {
            return Err(PopTypeError::Verifier(format!(
                "op operand #0 must be parameterized SIMD vector type, but got '{}'",
                mlir_spelling(operand)
            )));
        }
        if target != Ty::index() || *operand != int_storage {
            return Err(outside());
        }
    }
    Ok(target)
}

/// The result of a `pop.simd` lane op, as upstream's verifier types it:
/// `select` takes a `bool` mask and two cases of one vector type at the
/// mask's width and gives that type; `extractelement` takes a vector and an
/// `index` and gives the lane, a width-one vector of its dtype
/// (`!kgen.scalar<dt>`); `insertelement` takes a vector, a lane of its
/// dtype, and an `index` and gives the vector's type; and `reduce_and` and
/// `reduce_or` take integer or `bool` lanes and give one lane of them.
fn lane_op_result(op: MlirOp, operands: &[Ty]) -> Result<Ty, String> {
    let vector = |position: usize| {
        storage_slots(&operands[position]).ok_or_else(|| {
            format!(
                "op operand #{position} must be SIMD type, but got '{}'",
                mlir_spelling(&operands[position])
            )
        })
    };
    let index = |position: usize| {
        (operands[position] == Ty::index())
            .then_some(())
            .ok_or_else(|| {
                format!(
                    "op operand #{position} must be index, but got '{}'",
                    mlir_spelling(&operands[position])
                )
            })
    };
    let lane = |dtype: SimdDtype| storage_of(dtype, SimdWidth::Known(1));
    match op.lowering() {
        MlirLowering::SimdSelect => {
            let (mask, width) = vector(0)?;
            if mask.known() != Some(Dtype::Bool) {
                return Err(format!(
                    "op operand #0 must be !kgen.simd<N, bool>, but got '{}'",
                    mlir_spelling(&operands[0])
                ));
            }
            let (_, case_width) = vector(1)?;
            vector(2)?;
            if operands[1] != operands[2] {
                return Err(
                    "op failed to verify that all of {trueValue, falseValue, result} have same \
                     type"
                        .to_string(),
                );
            }
            if case_width != width {
                return Err(format!(
                    "op mask '{}' and cases '{}' differ in width",
                    mlir_spelling(&operands[0]),
                    mlir_spelling(&operands[1])
                ));
            }
            Ok(operands[1].clone())
        }
        MlirLowering::Extract => {
            let (dtype, _) = vector(0)?;
            index(1)?;
            lane(dtype)
        }
        MlirLowering::Insert => {
            let (dtype, _) = vector(0)?;
            if vector(1)? != (dtype, SimdWidth::Known(1)) {
                return Err(
                    "op failed to verify that value type matches dtype of the vector operand"
                        .to_string(),
                );
            }
            index(2)?;
            Ok(operands[0].clone())
        }
        MlirLowering::Reduce(_) => {
            let (dtype, _) = vector(0)?;
            if !dtype.licenses(|dtype| LaneKinds::IntegralOrBool.admits(dtype)) {
                return Err(format!(
                    "op operand #0 must be SIMD type whose element type is {} dtype, but got '{}'",
                    LaneKinds::IntegralOrBool.describe(),
                    mlir_spelling(&operands[0])
                ));
            }
            lane(dtype)
        }
        _ => unreachable!("`{}` is a lane op", op.name()),
    }
}

/// The result of a `pop` op over `operands`, with the `_type` attribute's
/// type when the op takes one.
fn pop_op_result(op: MlirOp, operands: &[Ty], result_type: Option<Ty>) -> Result<Ty, PopTypeError> {
    if let Some((position, operand)) = operands
        .iter()
        .enumerate()
        .find(|(_, operand)| storage_slots(operand).is_none())
    {
        return Err(PopTypeError::Verifier(format!(
            "op operand #{position} must be SIMD type, but got '{}'",
            mlir_spelling(operand)
        )));
    }
    let Some((dtype, width)) = operands.first().and_then(storage_slots) else {
        return Err(PopTypeError::Verifier("op takes an operand".to_string()));
    };
    if let Some(other) = operands.iter().find(|operand| **operand != operands[0]) {
        return Err(PopTypeError::Verifier(format!(
            "op requires the same type for all operands and results, found '{}' and '{}'",
            mlir_spelling(&operands[0]),
            mlir_spelling(other)
        )));
    }
    if let Some(lanes) = op.lanes()
        && !dtype.licenses(|dtype| lanes.admits(dtype))
    {
        let floats =
            lanes == LaneKinds::IntegralOrBool && dtype.known().is_some_and(Dtype::is_float);
        return Err(if floats {
            PopTypeError::Subset(
                "over float lanes is outside the subset: the pin's LLVM translation rejects \
                 bitwise logic over float lanes"
                    .to_string(),
            )
        } else {
            PopTypeError::Verifier(format!(
                "op operand #0 must be SIMD type whose element type is {} dtype, but got '{}'",
                lanes.describe(),
                mlir_spelling(&operands[0])
            ))
        });
    }
    match op.lowering() {
        MlirLowering::Binary(_) if matches!(op, MlirOp::PopCmp(_)) => {
            storage_of(SimdDtype::Known(Dtype::Bool), width).map_err(PopTypeError::Verifier)
        }
        MlirLowering::Cast => {
            let target = result_type.expect("a cast names its result type");
            let Some((_, target_width)) = storage_slots(&target) else {
                return Err(PopTypeError::Verifier(format!(
                    "op result must be SIMD type, but got '{}'",
                    mlir_spelling(&target)
                )));
            };
            if target_width != width {
                return Err(PopTypeError::Verifier(format!(
                    "op operand type '{}' and result type '{}' are cast incompatible",
                    mlir_spelling(&operands[0]),
                    mlir_spelling(&target)
                )));
            }
            Ok(target)
        }
        MlirLowering::Splat => {
            let target = result_type.expect("a splat names its result type");
            match storage_slots(&target) {
                Some((target_dtype, _))
                    if target_dtype == dtype && width == SimdWidth::Known(1) =>
                {
                    Ok(target)
                }
                _ => Err(PopTypeError::Verifier(format!(
                    "op splats a scalar of the result's element type, but got '{}' for '{}'",
                    mlir_spelling(&operands[0]),
                    mlir_spelling(&target)
                ))),
            }
        }
        _ => Ok(operands[0].clone()),
    }
}

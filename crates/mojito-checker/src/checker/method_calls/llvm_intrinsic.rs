//! ``__mlir_op.`pop.call_llvm_intrinsic` ``: a call of an LLVM intrinsic by
//! name, the op upstream's `std.sys.intrinsics.llvm_intrinsic` spells, typed
//! as its verifier types it. The intrinsics Mojito computes are
//! `mojito_types::llvm_intrinsic::LlvmIntrinsic`.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_types::llvm_intrinsic::LlvmIntrinsic;

impl Checker {
    /// Type ``__mlir_op.`pop.call_llvm_intrinsic`[intrin=…, _type=…,
    /// hasSideEffects=…](operands)``, recording it as
    /// `SemanticAdjustment::LlvmIntrinsic`. All three attributes are
    /// required: `intrin` a compile-time `!kgen.string` (a string literal's
    /// `value`, or `_get_kgen_string[s]()` over a `StaticString`), `_type`
    /// the result type (`None` for `NoneType`), and
    /// `hasSideEffects` a compile-time `Bool`'s `__mlir_i1__()`. One operand
    /// may be a heterogeneous collector, upstream's loaded `!kgen.pack`,
    /// whose elements the instance passes. A closed name outside the table
    /// is outside the subset, and closed operand types are checked against
    /// the intrinsic's signature, as LLVM translation checks them. A struct
    /// result is checked where it is elaborated, since LLVM translation
    /// sees a one-field register-passable struct as its field
    /// (`MirDeclarations::translated_type`).
    pub(super) fn infer_llvm_intrinsic_op(
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
        let (mut intrinsic, mut result_type, mut has_side_effect) = (None, None, None);
        for argument in param_args {
            let mojito_ast::ast::ParamArg::Named {
                name: attribute,
                value,
            } = argument
            else {
                return Err(invalid("op attributes are named".to_string()));
            };
            let slot = match attribute.as_str() {
                "intrin" => &mut intrinsic,
                "_type" => &mut result_type,
                "hasSideEffects" => &mut has_side_effect,
                _ => {
                    return Err(invalid(format!(
                        "attribute '{attribute}' is not an inherent attribute of '{name}'"
                    )));
                }
            };
            *slot = Some(value.as_ref());
        }
        let required = |attribute: &str| invalid(format!("op requires attribute '{attribute}'"));
        let intrinsic = self.llvm_intrinsic_name(intrinsic.ok_or_else(|| required("intrin"))?)?;
        let has_side_effect = self.llvm_intrinsic_side_effect(
            has_side_effect.ok_or_else(|| required("hasSideEffects"))?,
        )?;
        let result = match result_type {
            Some(mojito_ast::ast::ParamArg::Value(Expr {
                kind: ExprKind::None,
                ..
            })) => Ty::none(),
            Some(value) => self.mlir_result_type(value)?,
            None => {
                return Err(TypeError::BadCall {
                    func: name.to_string(),
                    reason: "unable to infer result type from MLIR operation \
                             'pop.call_llvm_intrinsic'"
                        .to_string(),
                });
            }
        };
        let operands = args
            .iter()
            .map(|argument| self.infer(argument))
            .collect::<Result<Vec<_>, _>>()?;
        let spreads: Vec<usize> = operands
            .iter()
            .enumerate()
            .filter(|(_, operand)| mojito_types::types::is_heterogeneous_collector(operand))
            .map(|(position, _)| position)
            .collect();
        let spread = match spreads.as_slice() {
            [] => None,
            [position] => Some(*position),
            _ => {
                return Err(TypeError::Unsupported(format!(
                    "'{name}' over more than one variadic pack is outside the subset"
                )));
            }
        };
        if let Some(intrinsic) = intrinsic
            .as_constant()
            .and_then(closed_intrinsic_name)
            .and_then(LlvmIntrinsic::parse)
            && spread.is_none()
            && !matches!(result.kind(), TyKind::Struct(..))
            && std::iter::once(&result)
                .chain(&operands)
                .all(|ty| !mojito_types::types::has_free_parameters(ty))
            && let Some(reason) = intrinsic.signature_error(&result, &operands)
        {
            return Err(invalid(reason));
        }
        self.record_at(span.clone()).adjustment = Some(Box::new(
            mojito_checked::checked::SemanticAdjustment::LlvmIntrinsic {
                intrinsic,
                has_side_effect,
                spread,
            },
        ));
        Ok(result)
    }

    /// The `intrin` attribute: a compile-time `!kgen.string`, closed or a
    /// template's parameter expression. A closed name must be in the table.
    fn llvm_intrinsic_name(
        &self,
        value: &mojito_ast::ast::ParamArg,
    ) -> Result<mojito_types::param_expr::ParamExpr, TypeError> {
        let rejected = || TypeError::BadCall {
            func: "pop.call_llvm_intrinsic".to_string(),
            reason: "op attribute 'intrin' failed to satisfy constraint: TypedAttr instance \
                     String type for parameters"
                .to_string(),
        };
        let mojito_ast::ast::ParamArg::Value(expression) = value else {
            return Err(rejected());
        };
        let built = self.compile_dependent_ct_expr(expression)?;
        if !matches!(built.meta(), MetaTy::Value(ty) if matches!(ty.kind(), TyKind::KgenString)) {
            return Err(rejected());
        }
        match built.as_constant() {
            Some(constant) => {
                let name = closed_intrinsic_name(constant).ok_or_else(rejected)?;
                if LlvmIntrinsic::parse(name).is_none() {
                    return Err(TypeError::Unsupported(format!(
                        "the LLVM intrinsic '{name}' is outside the subset: Mojito computes {}",
                        LlvmIntrinsic::admitted_names()
                    )));
                }
                Ok(built)
            }
            None => Ok(built),
        }
    }

    /// The `hasSideEffects` attribute: a compile-time `Bool`'s
    /// `__mlir_i1__()`, closed or a template's parameter expression.
    fn llvm_intrinsic_side_effect(
        &self,
        value: &mojito_ast::ast::ParamArg,
    ) -> Result<mojito_types::param_expr::ParamExpr, TypeError> {
        let rejected = || TypeError::BadCall {
            func: "pop.call_llvm_intrinsic".to_string(),
            reason: "attribute 'hasSideEffects' takes a compile-time 'Bool.__mlir_i1__()'"
                .to_string(),
        };
        let mojito_ast::ast::ParamArg::Value(Expr {
            kind:
                ExprKind::MethodCall {
                    object,
                    method,
                    args,
                    kwargs,
                },
            ..
        }) = value
        else {
            return Err(rejected());
        };
        if method != "__mlir_i1__" || !args.is_empty() || !kwargs.is_empty() {
            return Err(rejected());
        }
        let built = self.compile_dependent_ct_expr(object)?;
        if !matches!(built.meta(), MetaTy::Value(ty) if matches!(ty.kind(), TyKind::Bool)) {
            return Err(rejected());
        }
        Ok(built)
    }
}

/// The name a closed `intrin` attribute holds.
fn closed_intrinsic_name(value: &CtValue) -> Option<&str> {
    match value {
        CtValue::KgenString(name) => Some(name),
        _ => None,
    }
}

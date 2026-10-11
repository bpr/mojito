//! Typing of the `external_call` builtin over the closed libc callee table
//! (`mojito_types::ffi`): upstream's spelling
//! `external_call[callee, return_type, num_fixed_args=](*args)` for the
//! allowlisted callees only.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::ParamArg;
use mojito_types::ffi;

impl Checker {
    /// Type `external_call["callee", ReturnType, num_fixed_args=n](args...)`.
    /// The callee must be a string literal naming an allowlisted libc
    /// function; the declared return type and every argument are checked
    /// against the callee's C prototype, so the backends never see a shape
    /// the table does not describe. The call never raises.
    ///
    /// The call records its parameters as any generic call does, in
    /// upstream's declaration order (callee, return type, fixed-argument
    /// count): the callee and the count are compile-time data on the call,
    /// which the backends read from its instantiated arguments.
    pub(super) fn infer_external_call(
        &self,
        span: &SourceSpan,
        param_args: &[ParamArg],
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        let (callee_name, callee_span) = match param_args.first() {
            Some(ParamArg::Value(
                callee @ Expr {
                    kind: ExprKind::Str(text),
                    ..
                },
            )) => (text.clone(), callee.source_span()),
            _ => {
                return Err(TypeError::BadCall {
                    func: "external_call".to_string(),
                    reason: "the callee must be a string literal parameter argument".to_string(),
                });
            }
        };
        let Some(row) = ffi::callee(&callee_name) else {
            return Err(TypeError::BadCall {
                func: "external_call".to_string(),
                reason: format!(
                    "callee '{callee_name}' is not in Mojito's libc allowlist ({})",
                    ffi::callee_names()
                ),
            });
        };
        let Some(return_arg) = param_args.get(1) else {
            return Err(TypeError::BadCall {
                func: "external_call".to_string(),
                reason: "expected the return type as the second parameter argument".to_string(),
            });
        };
        let ret = self.type_param_argument(return_arg, "external_call")?;
        if !ffi::accepts_ret(row.ret, &ret) {
            return Err(TypeError::TypeMismatch {
                expected: row.ret.describe().to_string(),
                found: ret.to_string(),
                context: format!("return type of external_call[\"{callee_name}\"]"),
            });
        }
        let mut arguments = vec![
            TyArg::Val(CtValue::Str(callee_name.clone())),
            TyArg::Ty(ret.clone()),
        ];
        let mut folded_arguments = vec![callee_span];
        for extra in &param_args[2..] {
            let fixed = match extra {
                ParamArg::Named { name, value } if name == "num_fixed_args" => match &**value {
                    ParamArg::Value(
                        count @ Expr {
                            kind: ExprKind::Int(literal),
                            ..
                        },
                    ) => literal.wrapping_signed(64).inspect(|fixed| {
                        arguments.push(TyArg::Val(CtValue::Int(*fixed)));
                        folded_arguments.push(count.source_span());
                    }),
                    _ => None,
                },
                _ => {
                    return Err(TypeError::BadCall {
                        func: "external_call".to_string(),
                        reason:
                            "only 'num_fixed_args=<count>' may follow the return type parameter"
                                .to_string(),
                    });
                }
            };
            if fixed != Some(row.params.len() as i64) {
                return Err(TypeError::BadCall {
                    func: "external_call".to_string(),
                    reason: format!(
                        "'{callee_name}' takes {} fixed argument(s) before its variadic tail",
                        row.params.len()
                    ),
                });
            }
        }
        let (min, max) = row.arity();
        if args.len() < min || args.len() > max {
            return Err(TypeError::ArityMismatch {
                name: format!("external_call[\"{callee_name}\"]"),
                expected: if args.len() < min { min } else { max },
                got: args.len(),
            });
        }
        for (index, arg) in args.iter().enumerate() {
            let kind = row.param(index).expect("arity checked above");
            let ty = self.infer(arg)?;
            self.borrow_reference_result_argument(arg);
            self.borrow_nominal_place_argument(arg, &ty);
            if kind.is_integer() {
                let runtime_ty = default_literal(&ty);
                if runtime_ty != ty {
                    self.record_literal_materializations(arg, &ty, &runtime_ty)?;
                }
            }
            if !ffi::accepts_arg(kind, &ty) {
                return Err(TypeError::TypeMismatch {
                    expected: kind.describe().to_string(),
                    found: ty.to_string(),
                    context: format!("argument {} to external_call[\"{callee_name}\"]", index + 1),
                });
            }
        }
        self.record_at(span.clone()).generic_instantiation =
            Some(mojito_checked::checked::GenericInstantiation {
                callee: "external_call".to_string(),
                parameter_names: Vec::new(),
                parameter_types: Vec::new(),
                variadic: None,
                arguments,
                folded_arguments,
            });
        Ok(ret)
    }
}

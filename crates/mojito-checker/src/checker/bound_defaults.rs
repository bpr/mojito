//! Requirement defaults bound at a call through a trait bound.
//!
//! Through a bound, current Mojo runs the requirement's default for a slot
//! the call leaves out, whatever the witness declares; on a nominal receiver
//! the witness's own. An instance reaches the witness nominally, so the
//! check records each such call's omitted requirement defaults, and the
//! checked tree spells them as arguments of the call, whose every instance
//! then runs the requirement's default as the pin does. A witness
//! may therefore default differently from its requirement, or not at all
//! (`traits_support::method_satisfies_requirement`).
//!
//! A requirement default reading the method's own value parameters
//! (`factor: Int = n`) is evaluated with the call's binding of them,
//! explicit, defaulted, or inferred, and spelled as the value that gives in
//! the caller's scope, which means the same in each clone of the call.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::{Dtype, KwArg, SyntaxOrigins};
use mojito_ast::visit::{MutVisitor, walk_block_mut, walk_expr_mut};
use mojito_checked::checked::CheckedConst;
use mojito_common::literal::IntLiteral;
use mojito_common::token::SyntaxId;
use mojito_types::param_expr::{ParamBindings, ParamEval, ParamOp};
use mojito_types::types::TyKind;

/// The requirement defaults one call through a bound leaves out.
///
/// Through a bound, current Mojo runs the requirement's default, not the
/// witness's; binding it at the call carries that into every instance, which
/// reaches the witness nominally.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct BoundDefaultArguments {
    /// The positional arguments the call was checked with; a call node with
    /// any other count is not the one these arguments complete.
    pub(super) positional: usize,
    pub(super) keywords: Vec<KwArg>,
    /// The requirement's parameter names, in declaration order: each
    /// keyword goes before the call's first keyword for a later parameter.
    pub(super) parameters: Vec<String>,
}

/// A call through a trait bound, as its requirement defaults are recorded.
pub(super) struct BoundCall<'a> {
    pub(super) span: &'a SourceSpan,
    /// The call's binding against the requirement.
    pub(super) slots: &'a [ArgSlot],
    pub(super) positional: usize,
    /// The compile-time arguments as solved against the requirement's
    /// binders, one per binder.
    pub(super) solved: &'a [TyArg],
}

/// Spell every recorded requirement default as an argument of the call it
/// completes and of each clone of that call; whether any call changed.
pub(super) fn bind_bound_default_arguments(
    statements: &mut [Stmt],
    origins: &SyntaxOrigins,
    arguments: &HashMap<SyntaxId, BoundDefaultArguments>,
) -> bool {
    struct Bind<'a> {
        origins: &'a SyntaxOrigins,
        arguments: &'a HashMap<SyntaxId, BoundDefaultArguments>,
        changed: bool,
    }

    impl MutVisitor for Bind<'_> {
        fn visit_expr_mut(&mut self, expr: &mut Expr) {
            let call = expr.syntax_id;
            let (ExprKind::MethodCall { args, kwargs, .. } | ExprKind::Invoke { args, kwargs, .. }) =
                &mut expr.kind
            else {
                return;
            };
            let Some(bound) = self.arguments.get(&self.origins.origin(call)) else {
                return;
            };
            let unbound = args.len() == bound.positional
                && bound
                    .keywords
                    .iter()
                    .all(|keyword| kwargs.iter().all(|kwarg| kwarg.name != keyword.name));
            if !unbound {
                return;
            }
            let mut identities = Identities {
                call,
                next: BOUND_DEFAULT_ORDINAL,
            };
            let index = |name: &str| bound.parameters.iter().position(|named| named == name);
            for keyword in &bound.keywords {
                let parameter = index(&keyword.name);
                let at = kwargs
                    .iter()
                    .position(|kwarg| index(&kwarg.name) > parameter)
                    .unwrap_or(kwargs.len());
                kwargs.insert(
                    at,
                    KwArg {
                        name: keyword.name.clone(),
                        value: identities.expr(&keyword.value),
                    },
                );
            }
            self.changed = true;
        }
    }

    let mut bind = Bind {
        origins,
        arguments,
        changed: false,
    };
    walk_block_mut(&mut bind, statements);
    bind.changed
}

impl Checker {
    /// Record the requirement defaults `call` through `bounds` leaves out
    /// and some conformer's witness declares otherwise. A default every
    /// witness declares alike runs the same through the bound and in an
    /// instance, so it stays an omitted slot. A default over the method's
    /// value parameters is always bound; one whose value at the call has no
    /// spelling in the caller's scope is rejected.
    pub(super) fn record_bound_default_arguments(
        &self,
        call: &BoundCall<'_>,
        bounds: &[String],
        method: &str,
        requirement: &MethodSig,
    ) -> Result<(), TypeError> {
        let renamed =
            call.slots
                .iter()
                .zip(&requirement.names)
                .enumerate()
                .find(|(index, (slot, name))| {
                    !matches!(slot, ArgSlot::Positional(_))
                        && self.witness_renames(bounds, method, requirement, *index, name)
                });
        if let Some((_, (_, name))) = renamed {
            return Err(TypeError::Unsupported(format!(
                "binding '{name}' of '{method}' by name through a trait bound whose \
                 conformer renames it"
            )));
        }
        let Some(syntax) = call.span.syntax else {
            return Ok(());
        };
        let at = Expr {
            kind: ExprKind::None,
            span: call.span.span,
            source: call.span.source.clone(),
            syntax_id: syntax,
        };
        // A requirement declares no positional-only marker, so each default
        // binds by name.
        let keywords: Vec<KwArg> = call
            .slots
            .iter()
            .zip(&requirement.defaults)
            .zip(&requirement.names)
            .filter_map(|((slot, default), name)| {
                match (slot, default) {
                    (ArgSlot::Default, Some(SignatureDefault::Parameter(default))) => {
                        Some(self.spelled_parameter_default(default, &requirement.decls, call, &at))
                    }
                    (ArgSlot::Default, Some(SignatureDefault::Constant(default)))
                        if !self.witnesses_default_alike(bounds, method, name, default) =>
                    {
                        Some(checked_literal(default, &at).ok_or_else(|| {
                            TypeError::InvariantViolation(format!(
                                "trait requirement '{method}' holds a converted default"
                            ))
                        }))
                    }
                    (ArgSlot::Default, Some(SignatureDefault::Evaluated)) => {
                        Some(Err(TypeError::InvariantViolation(format!(
                            "trait requirement '{method}' holds an evaluated default"
                        ))))
                    }
                    _ => None,
                }
                .map(|value| {
                    value.map(|value| KwArg {
                        name: name.clone(),
                        value,
                    })
                })
            })
            .collect::<Result<_, _>>()?;
        if keywords.is_empty() {
            return Ok(());
        }
        self.bound_default_arguments.borrow_mut().insert(
            self.syntax_origins.origin(syntax),
            BoundDefaultArguments {
                positional: call.positional,
                keywords,
                parameters: requirement.names.clone(),
            },
        );
        Ok(())
    }

    /// A compile-time value as source meaning it in the caller's scope: a
    /// constant as its literal, a value parameter in scope by its name, and
    /// integer arithmetic over them as the operators; `None` for anything
    /// else.
    pub(super) fn spelled_parameter_value(&self, value: &ParamExpr, at: &Expr) -> Option<Expr> {
        let spelled = |kind| Expr { kind, ..at.clone() };
        match value.kind() {
            ParamKind::Constant(constant) => constant_literal(constant, at),
            ParamKind::DeclRef(reference) => {
                let name = reference.name.to_string();
                if self.value_parameter_in_scope(&name).as_ref() == Some(value) {
                    return Some(spelled(ExprKind::Identifier(name)));
                }
                matches!(self.self_param_ct_value(&name), Some(CtValue::Expr(bound)) if &bound == value)
                    .then(|| {
                        spelled(ExprKind::Member {
                            object: Box::new(spelled(ExprKind::Identifier("Self".to_string()))),
                            field: name,
                        })
                    })
            }
            ParamKind::Op {
                op: ParamOp::Neg,
                operands,
            } if operands.len() == 1 => Some(spelled(ExprKind::Prefix(
                PrefixOp::Neg,
                Box::new(self.spelled_parameter_value(&operands[0], at)?),
            ))),
            ParamKind::Op { op, operands } => {
                let infix = match op {
                    ParamOp::Add => InfixOp::Add,
                    ParamOp::Sub => InfixOp::Sub,
                    ParamOp::Mul => InfixOp::Mul,
                    ParamOp::FloorDiv => InfixOp::FloorDiv,
                    ParamOp::Mod => InfixOp::Mod,
                    _ => return None,
                };
                let mut operands = operands
                    .iter()
                    .map(|operand| self.spelled_parameter_value(operand, at));
                let first = operands.next()??;
                operands.try_fold(first, |left, right| {
                    Some(spelled(ExprKind::Infix(
                        infix,
                        Box::new(left),
                        Box::new(right?),
                    )))
                })
            }
            _ => None,
        }
    }

    /// Whether some conformer to `bounds` declares a witness for
    /// `requirement` naming its regular parameter at `index` other than
    /// `name`, which a call through the bound binds by the requirement's
    /// name and the witness's body reads by its own.
    fn witness_renames(
        &self,
        bounds: &[String],
        method: &str,
        requirement: &MethodSig,
        index: usize,
        name: &str,
    ) -> bool {
        self.structs_declaring(method)
            .filter_map(|(struct_name, info)| Some((struct_name, info, info.methods.get(method)?)))
            .filter(|(struct_name, info, _)| {
                let implementation = Ty::new(TyKind::Struct(
                    (*struct_name).clone(),
                    params_as_args(&info.decls),
                ));
                bounds
                    .iter()
                    .all(|bound| self.conforms_to(&implementation, bound))
            })
            .flat_map(|(_, _, signatures)| signatures)
            .filter(|signature| signature.names.len() == requirement.names.len())
            .any(|signature| signature.names.get(index).is_some_and(|got| got != name))
    }

    /// Whether every conformer to `bounds` declaring `method` defaults its
    /// `parameter` to `default`, in each overload naming it.
    fn witnesses_default_alike(
        &self,
        bounds: &[String],
        method: &str,
        parameter: &str,
        default: &CheckedConst,
    ) -> bool {
        self.structs_declaring(method)
            .filter_map(|(name, info)| Some((name, info, info.methods.get(method)?)))
            .filter(|(name, info, _)| {
                let implementation =
                    Ty::new(TyKind::Struct((*name).clone(), params_as_args(&info.decls)));
                bounds
                    .iter()
                    .all(|bound| self.conforms_to(&implementation, bound))
            })
            .all(|(_, _, signatures)| {
                let mut named = signatures.iter().filter_map(|signature| {
                    let index = signature.names.iter().position(|name| name == parameter)?;
                    Some(signature.defaults.get(index).and_then(Option::as_ref))
                });
                named.all(|witness| witness.is_some_and(|witness| witness.is_literal(default)))
                    && signatures
                        .iter()
                        .any(|signature| signature.names.iter().any(|name| name == parameter))
            })
    }

    /// A requirement default over the method's value parameters, evaluated
    /// with the call's binding of them and spelled as the value that gives
    /// in the caller's scope. The spelling feeds only the call's rewrite to
    /// keyword arguments.
    fn spelled_parameter_default(
        &self,
        default: &ParamExpr,
        decls: &[ParamDecl],
        call: &BoundCall<'_>,
        at: &Expr,
    ) -> Result<Expr, TypeError> {
        let values = solved_value_bindings(decls, call.solved);
        let bindings = ParamBindings::from_named_values(&self.param_context, &values);
        let value = match self.param_context.evaluate(default, &bindings) {
            Ok(ParamEval::Constant(value)) => self.param_context.constant(value).ok(),
            Ok(ParamEval::Residual(value)) => Some(value),
            Err(_) => None,
        };
        value
            .as_ref()
            .and_then(|value| self.spelled_parameter_value(value, at))
            .ok_or_else(|| {
                let parameter = value
                    .as_ref()
                    .unwrap_or(default)
                    .free_parameters()
                    .first()
                    .map_or_else(|| default.to_string(), |parameter| parameter.name.to_string());
                TypeError::Unsupported(format!(
                    "a trait requirement's default reading compile-time parameter '{parameter}' at a call through the bound that does not spell it"
                ))
            })
    }
}

/// A scalar compile-time constant as the literal spelling it, at `at`'s
/// location.
fn constant_literal(value: &CtValue, at: &Expr) -> Option<Expr> {
    let spelled = |kind| Expr { kind, ..at.clone() };
    let integer = |value: IntLiteral| {
        if value.is_negative() {
            spelled(ExprKind::Prefix(
                PrefixOp::Neg,
                Box::new(spelled(ExprKind::Int(value.neg()))),
            ))
        } else {
            spelled(ExprKind::Int(value))
        }
    };
    match value {
        CtValue::Int(value) => Some(integer(IntLiteral::from(*value))),
        CtValue::IntLiteral(value) => Some(integer(value.clone())),
        CtValue::Bool(value) => Some(spelled(ExprKind::Bool(*value))),
        CtValue::Str(value) => Some(spelled(ExprKind::Str(value.clone()))),
        _ => None,
    }
}

/// A literal constant as its spelling at `at`'s location; `None` for a
/// conversion or an evaluated default.
fn checked_literal(constant: &CheckedConst, at: &Expr) -> Option<Expr> {
    let spelled = |kind| Expr { kind, ..at.clone() };
    match constant {
        CheckedConst::Int(value) => constant_literal(&CtValue::IntLiteral(value.clone()), at),
        CheckedConst::Float(value) if value.is_negative() => Some(spelled(ExprKind::Prefix(
            PrefixOp::Neg,
            Box::new(spelled(ExprKind::Float(value.neg()))),
        ))),
        CheckedConst::Float(value) => Some(spelled(ExprKind::Float(value.clone()))),
        CheckedConst::Bool(value) => Some(spelled(ExprKind::Bool(*value))),
        CheckedConst::String(value) => Some(spelled(ExprKind::Str(value.clone()))),
        CheckedConst::None => Some(spelled(ExprKind::None)),
        CheckedConst::Dtype(dtype) => Some(dtype_member(*dtype, at)),
        CheckedConst::Construct { .. } | CheckedConst::Evaluate { .. } => None,
    }
}

/// `DType.<dtype>` at `at`'s location.
fn dtype_member(dtype: Dtype, at: &Expr) -> Expr {
    Expr {
        kind: ExprKind::Member {
            object: Box::new(Expr {
                kind: ExprKind::Identifier("DType".to_string()),
                ..at.clone()
            }),
            field: dtype.name().to_string(),
        },
        ..at.clone()
    }
}

/// The first derivation ordinal of a bound default's nodes under its call,
/// clear of the few ordinals other rewrites derive under an expression.
const BOUND_DEFAULT_ORDINAL: u32 = 0x0b0d_0000;

/// Fresh, deterministic identities for the nodes a call's bound defaults
/// add: each is derived from the call, so a clone's node traces to the
/// template's.
struct Identities {
    call: SyntaxId,
    next: u32,
}

impl Identities {
    fn expr(&mut self, value: &Expr) -> Expr {
        struct Rekey<'a>(&'a mut Identities);

        impl MutVisitor for Rekey<'_> {
            fn visit_expr_mut(&mut self, expr: &mut Expr) {
                expr.syntax_id = SyntaxId::derived(self.0.call, self.0.next);
                self.0.next += 1;
            }
        }

        let mut value = value.clone();
        walk_expr_mut(&mut Rekey(self), &mut value);
        value
    }
}

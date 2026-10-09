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
//! (`factor: Int = n`) is spelled with the call's explicit compile-time
//! arguments in their place, which mean the same in the caller's scope and in
//! each clone of the call; a parameter the call leaves to its own constant
//! default is spelled as that constant.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::{KwArg, ParamArg, SyntaxOrigins};
use mojito_ast::visit::{MutVisitor, walk_block_mut, walk_expr_mut};
use mojito_common::literal::IntLiteral;
use mojito_common::token::SyntaxId;

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
    /// The compile-time arguments as spelled.
    pub(super) param_args: &'a [ParamArg],
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

/// A scalar compile-time constant as the literal spelling it, at `at`'s
/// location.
pub(super) fn constant_literal(value: &CtValue, at: &Expr) -> Option<Expr> {
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

impl Checker {
    /// Record the requirement defaults `call` through `bounds` leaves out
    /// and some conformer's witness declares otherwise. A default every
    /// witness spells alike runs the same through the bound and in an
    /// instance, so it stays an omitted slot. A default reading a parameter
    /// the call neither spells nor leaves to a constant default is rejected.
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
        // A requirement declares no positional-only marker, so each default
        // binds by name.
        let keywords: Vec<KwArg> = call
            .slots
            .iter()
            .zip(&requirement.defaults)
            .zip(&requirement.names)
            .filter_map(|((slot, default), name)| match (slot, default) {
                (ArgSlot::Default, Some(default))
                    if traits::reads_value_parameter(default, &requirement.decls) =>
                {
                    Some(
                        spell_parameters(default, &requirement.decls, call).map(|value| KwArg {
                            name: name.clone(),
                            value,
                        }),
                    )
                }
                (ArgSlot::Default, Some(default))
                    if !self.witnesses_default_alike(bounds, method, name, default) =>
                {
                    Some(Ok(KwArg {
                        name: name.clone(),
                        value: default.clone(),
                    }))
                }
                _ => None,
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
        self.structs
            .iter()
            .filter_map(|(struct_name, info)| Some((struct_name, info, info.methods.get(method)?)))
            .filter(|(struct_name, info, _)| {
                let implementation =
                    Ty::Struct((*struct_name).clone(), params_as_args(&info.decls).into());
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
        default: &Expr,
    ) -> bool {
        self.structs
            .iter()
            .filter_map(|(name, info)| Some((name, info, info.methods.get(method)?)))
            .filter(|(name, info, _)| {
                let implementation =
                    Ty::Struct((*name).clone(), params_as_args(&info.decls).into());
                bounds
                    .iter()
                    .all(|bound| self.conforms_to(&implementation, bound))
            })
            .all(|(_, _, signatures)| {
                let mut named = signatures.iter().filter_map(|signature| {
                    let index = signature.names.iter().position(|name| name == parameter)?;
                    Some(signature.defaults.get(index).cloned().flatten())
                });
                named.all(|witness| witness.as_ref() == Some(default))
                    && signatures
                        .iter()
                        .any(|signature| signature.names.iter().any(|name| name == parameter))
            })
    }
}

/// `default` with each of the requirement's value parameters it reads
/// replaced by the compile-time argument the call spells for it, or by the
/// parameter's constant default when the call leaves it to that.
fn spell_parameters(
    default: &Expr,
    decls: &[ParamDecl],
    call: &BoundCall<'_>,
) -> Result<Expr, TypeError> {
    struct Spell<'a> {
        arguments: HashMap<&'a str, Expr>,
        unspelled: Option<String>,
    }

    impl MutVisitor for Spell<'_> {
        fn visit_expr_mut(&mut self, expr: &mut Expr) {
            let ExprKind::Identifier(name) = &expr.kind else {
                return;
            };
            match self.arguments.get(name.as_str()) {
                Some(argument) => *expr = argument.clone(),
                None => self.unspelled = Some(name.clone()),
            }
        }
    }

    let mut positional = decls.iter().filter(|decl| {
        !matches!(
            decl,
            ParamDecl::Type {
                infer_only: true,
                ..
            } | ParamDecl::Value {
                infer_only: true,
                ..
            }
        )
    });
    let mut arguments = HashMap::new();
    for argument in call.param_args {
        let (decl, value) = match argument {
            ParamArg::Named { name, value } => {
                (decls.iter().find(|decl| decl.name() == name), &**value)
            }
            other => (positional.next(), other),
        };
        let Some(ParamDecl::Value { name, .. }) = decl else {
            continue;
        };
        let value = match value {
            ParamArg::Value(value) => Some(value.clone()),
            ParamArg::Type(SourceType::Named(spelled, arguments)) if arguments.is_empty() => {
                Some(Expr {
                    kind: ExprKind::Identifier(spelled.clone()),
                    ..default.clone()
                })
            }
            ParamArg::Type(_) | ParamArg::Named { .. } => None,
        };
        if let Some(value) = value {
            arguments.insert(name.as_str(), value);
        }
    }
    for (decl, solved) in decls.iter().zip(call.solved) {
        let (
            ParamDecl::Value {
                name,
                default: Some(declared),
                ..
            },
            TyArg::Val(value),
        ) = (decl, solved)
        else {
            continue;
        };
        if !arguments.contains_key(name.as_str())
            && let Some(literal) = declared
                .as_constant()
                .and_then(|constant| constant_literal(constant, default))
            && constant_literal(value, default).as_ref() == Some(&literal)
        {
            arguments.insert(name.as_str(), literal);
        }
    }
    let mut spell = Spell {
        arguments,
        unspelled: None,
    };
    let mut value = default.clone();
    walk_expr_mut(&mut spell, &mut value);
    spell.unspelled.map_or(Ok(value), |parameter| {
        Err(TypeError::Unsupported(format!(
            "a trait requirement's default reading compile-time parameter '{parameter}' at a call through the bound that does not spell it"
        )))
    })
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

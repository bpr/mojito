//! Requirement defaults bound at a call through a trait bound.
//!
//! Through a bound, current Mojo runs the requirement's default for a slot
//! the call leaves out, whatever the witness declares; on a nominal receiver
//! the witness's own. An instance clone reaches the witness nominally, so the
//! check records each such call's omitted requirement defaults, and the
//! checked tree spells them as arguments of the call and of every clone of
//! it, which then runs the requirement's default as the pin does. A witness
//! may therefore default differently from its requirement, or not at all
//! (`traits_support::method_satisfies_requirement`).

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::{KwArg, SyntaxOrigins};
use mojito_ast::visit::{MutVisitor, walk_block_mut, walk_expr_mut};
use mojito_checked::templates::BoundDefaultArguments;
use mojito_common::token::SyntaxId;

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
            let ExprKind::MethodCall { args, kwargs, .. } = &mut expr.kind else {
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
    /// Record the requirement defaults a call through `bounds` leaves out
    /// and some conformer's witness declares otherwise: `slots` is the
    /// call's binding against `requirement`. A default every witness spells
    /// alike runs the same through the bound and in an instance, so it stays
    /// an omitted slot.
    pub(super) fn record_bound_default_arguments(
        &self,
        span: &SourceSpan,
        bounds: &[String],
        method: &str,
        requirement: &MethodSig,
        slots: &[ArgSlot],
        positional: usize,
    ) {
        let Some(call) = span.syntax else {
            return;
        };
        // A requirement declares no positional-only marker, so each default
        // binds by name.
        let keywords: Vec<KwArg> = slots
            .iter()
            .zip(&requirement.defaults)
            .zip(&requirement.names)
            .filter_map(|((slot, default), name)| match (slot, default) {
                (ArgSlot::Default, Some(default))
                    if !self.witnesses_default_alike(bounds, method, name, default) =>
                {
                    Some(KwArg {
                        name: name.clone(),
                        value: default.clone(),
                    })
                }
                _ => None,
            })
            .collect();
        if keywords.is_empty() {
            return;
        }
        self.bound_default_arguments.borrow_mut().insert(
            self.syntax_origins.origin(call),
            BoundDefaultArguments {
                positional,
                keywords,
                parameters: requirement.names.clone(),
            },
        );
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
                let implementation = Ty::Struct((*name).clone(), params_as_args(&info.decls));
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

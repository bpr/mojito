//! Decorator validation, one rule per declaration site, as upstream resolves
//! each decorator by name against the declaration it decorates. The parser
//! keeps every decorator; this module accepts the ones the pin accepts at a
//! site, with the pin's argument checks and texts, reports a pin-known
//! decorator the compiler does not model as unsupported, and rejects every
//! other name as the pin does: an unsupported compiler decorator when it names
//! a declaration, an unknown declaration otherwise.

use mojito_ast::ast::{Decorator, Expr, ExprKind};
use mojito_checked::declaration::InlineLevel;
use mojito_common::error::TypeError;
use mojito_symbol::symbol::OverloadLookup;
use mojito_types::conformance::COMPILER_TRAITS;

use super::Checker;

/// The declaration kind a decorator list precedes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DecoratorSite<'a> {
    Struct {
        name: &'a str,
        /// Whether the struct declares a conditional `Deinitable`
        /// conformance, which `@explicit_destroy` needs.
        conditional_deinitable: bool,
    },
    /// A module-level or nested `def`.
    Function,
    Method {
        /// Whether the method is `__init__`, the one `@implicit` may mark.
        init: bool,
    },
    TraitMethod {
        /// Whether the method is `__init__`.
        init: bool,
    },
}

/// What a struct's decorators state beyond their validity.
#[derive(Debug, Default)]
pub(super) struct DecoratorFacts {
    /// The `@explicit_destroy` message, shown where a value of the type would
    /// be implicitly destroyed.
    pub(super) explicit_destroy: Option<String>,
    /// The `@unavailable` mark of a function, which withdraws every use of
    /// it.
    pub(super) unavailable: Option<Unavailable>,
}

/// What an `@unavailable` decorator says to a use of its function.
#[derive(Debug)]
pub(super) enum Unavailable {
    /// Its reason message.
    Reason(String),
    /// The replacement its `use=` names.
    Use(String),
}

impl Unavailable {
    /// The pin's rejection of a use of the function `name`.
    pub(super) fn message(&self, name: &str) -> String {
        match self {
            Self::Reason(reason) => reason.clone(),
            Self::Use(replacement) => {
                format!("'{name}' is unavailable, use '{replacement}' instead")
            }
        }
    }
}

impl Checker {
    /// Validate `decorators` at `site`, returning the facts they state.
    pub(super) fn check_decorators(
        &self,
        site: DecoratorSite<'_>,
        decorators: &[Decorator],
    ) -> Result<DecoratorFacts, TypeError> {
        let mut facts = DecoratorFacts::default();
        let mut fieldwise_init = false;
        for decorator in decorators {
            let [name] = decorator.path.as_slice() else {
                return Err(self.unknown_decorator(&decorator.path[0]));
            };
            match (name.as_str(), site) {
                ("doc_hidden", _) => {
                    if argument_count(decorator) != 0 {
                        return Err(invalid("invalid call to 'doc_hidden': unexpected argument"));
                    }
                }
                ("stable", _) => check_stable(decorator)?,
                ("deprecated", _) => check_deprecated(decorator, "deprecated")?,
                ("fieldwise_init", DecoratorSite::Struct { name, .. }) => {
                    check_fieldwise_init(decorator)?;
                    // A second one declares the initializer the first
                    // already generated.
                    if std::mem::replace(&mut fieldwise_init, true) {
                        return Err(invalid(format!(
                            "'{name}' has an explicitly declared fieldwise initializer"
                        )));
                    }
                }
                (
                    "explicit_destroy",
                    DecoratorSite::Struct {
                        conditional_deinitable,
                        ..
                    },
                ) => {
                    facts.explicit_destroy =
                        Some(explicit_destroy_message(decorator, conditional_deinitable)?);
                }
                ("align", DecoratorSite::Struct { .. }) => {
                    if argument_count(decorator) != 1 {
                        return Err(invalid("@align requires exactly one argument"));
                    }
                    return Err(unmodeled(name, site));
                }
                ("register_passable", DecoratorSite::Struct { .. }) => {
                    return Err(invalid(if decorator.args.is_empty() {
                        "decorator @register_passable is removed, conform to RegisterPassable \
                         trait instead"
                    } else {
                        "decorator @register_passable(\"trivial\") is removed, conform to \
                         TrivialRegisterPassable trait instead"
                    }));
                }
                ("unavailable", _) => {
                    check_deprecated(decorator, "unavailable")?;
                    match site {
                        DecoratorSite::Struct { .. } => {
                            return Err(invalid(
                                "@unavailable can only be applied to functions and methods",
                            ));
                        }
                        DecoratorSite::Function => {
                            facts.unavailable =
                                Some(unavailable(decorator).ok_or_else(|| unmodeled(name, site))?);
                        }
                        DecoratorSite::Method { .. } | DecoratorSite::TraitMethod { .. } => {
                            return Err(unmodeled(name, site));
                        }
                    }
                }
                (
                    "always_inline" | "inline" | "no_inline",
                    DecoratorSite::Function
                    | DecoratorSite::Method { .. }
                    | DecoratorSite::TraitMethod { .. },
                ) => {}
                (
                    "export",
                    DecoratorSite::Function
                    | DecoratorSite::Method { .. }
                    | DecoratorSite::TraitMethod { .. },
                ) => return Err(unmodeled(name, site)),
                (
                    "__parameter" | "parameter" | "__unsafe_nested_origins_read_only",
                    DecoratorSite::Function | DecoratorSite::Method { .. },
                ) => {}
                (
                    "__parameter" | "parameter" | "__unsafe_nested_origins_read_only",
                    DecoratorSite::TraitMethod { .. },
                ) => return Err(unmodeled(name, site)),
                ("staticmethod", DecoratorSite::Method { .. }) => {}
                ("staticmethod", DecoratorSite::Function) => {
                    return Err(invalid("only methods on structs may be declared static"));
                }
                ("staticmethod", DecoratorSite::TraitMethod { .. }) => {
                    return Err(unmodeled(name, site));
                }
                ("implicit", DecoratorSite::Method { init: true }) => {}
                ("implicit", DecoratorSite::TraitMethod { init: true }) => {
                    return Err(unmodeled(name, site));
                }
                (
                    "implicit",
                    DecoratorSite::Function
                    | DecoratorSite::Method { .. }
                    | DecoratorSite::TraitMethod { .. },
                ) => {
                    return Err(invalid(
                        "'@implicit' may only be applied to '__init__' methods",
                    ));
                }
                _ => return Err(self.unknown_decorator(name)),
            }
        }
        if !matches!(site, DecoratorSite::Struct { .. }) {
            InlineLevel::of(decorators)?;
        }
        Ok(facts)
    }

    /// The pin's rejection of a decorator `name` that is no decorator of the
    /// site: an unsupported compiler decorator when `name` is a declaration
    /// in scope, an unknown declaration otherwise.
    fn unknown_decorator(&self, name: &str) -> TypeError {
        let declared = self.lookup(name).is_some()
            || self.overload_sets.is_function(name)
            || self.structs.contains_key(name)
            || self.traits.contains_key(name)
            || COMPILER_TRAITS.contains(&name);
        invalid(if declared {
            "unsupported compiler decorator".to_string()
        } else {
            format!("use of unknown declaration '{name}'")
        })
    }
}

fn invalid(message: impl Into<String>) -> TypeError {
    TypeError::InvalidDecorator(message.into())
}

const fn argument_count(decorator: &Decorator) -> usize {
    decorator.args.len() + decorator.kwargs.len()
}

/// A decorator the pin accepts at `site` whose meaning is not modeled.
fn unmodeled(name: &str, site: DecoratorSite<'_>) -> TypeError {
    let site = match site {
        DecoratorSite::Struct { .. } => "struct",
        DecoratorSite::Function => "function",
        DecoratorSite::Method { .. } => "method",
        DecoratorSite::TraitMethod { .. } => "trait method",
    };
    TypeError::Unsupported(format!("decorator '@{name}' on a {site}"))
}

/// `@stable` takes nothing, or one `since=` or `recursive=` keyword.
fn check_stable(decorator: &Decorator) -> Result<(), TypeError> {
    if !decorator.args.is_empty() {
        return Err(invalid(
            "@stable requires a keyword argument ('since' or 'recursive'), not a positional \
             argument",
        ));
    }
    if decorator
        .kwargs
        .iter()
        .any(|kwarg| !matches!(kwarg.name.as_str(), "since" | "recursive"))
    {
        return Err(invalid(
            "@stable only accepts keyword arguments 'since' or 'recursive'",
        ));
    }
    if decorator.kwargs.len() > 1 {
        return Err(invalid(
            "@stable accepts only one argument, either 'since' or 'recursive'",
        ));
    }
    Ok(())
}

/// `@deprecated` and `@unavailable` take exactly one of a reason message,
/// positional or `reason=`, and a `use=` replacement symbol.
fn check_deprecated(decorator: &Decorator, name: &str) -> Result<(), TypeError> {
    match (decorator.args.as_slice(), decorator.kwargs.as_slice()) {
        ([], []) => Err(invalid(format!(
            "@{name} requires a reason message or a replacement symbol (with 'use')"
        ))),
        ([reason], []) => check_reason(reason),
        ([], [kwarg]) => match kwarg.name.as_str() {
            "reason" => check_reason(&kwarg.value),
            "use" => Ok(()),
            _ => Err(invalid(format!(
                "{name} must specify either a message or a symbol (with the 'use' argument)"
            ))),
        },
        _ => Err(invalid(format!(
            "@{name} accepts either a reason message or a replacement symbol (with 'use')"
        ))),
    }
}

/// The mark an `@unavailable` whose operands [`check_deprecated`] accepted
/// states; `None` for a `use=` naming something other than a declaration.
fn unavailable(decorator: &Decorator) -> Option<Unavailable> {
    let (use_, value) = match (decorator.args.as_slice(), decorator.kwargs.as_slice()) {
        ([reason], []) => (false, reason),
        ([], [kwarg]) => (kwarg.name == "use", &kwarg.value),
        _ => return None,
    };
    match &value.kind {
        ExprKind::Str(reason) if !use_ => Some(Unavailable::Reason(reason.clone())),
        ExprKind::Identifier(replacement) if use_ => Some(Unavailable::Use(replacement.clone())),
        _ => None,
    }
}

fn check_reason(reason: &Expr) -> Result<(), TypeError> {
    if matches!(reason.kind, ExprKind::Str(_)) {
        Ok(())
    } else {
        Err(invalid("'reason' argument must be a string literal"))
    }
}

/// `@fieldwise_init` takes nothing, or the `"implicit"` the compiler does
/// not model.
fn check_fieldwise_init(decorator: &Decorator) -> Result<(), TypeError> {
    match (decorator.args.as_slice(), decorator.kwargs.as_slice()) {
        ([], []) => Ok(()),
        (
            [
                Expr {
                    kind: ExprKind::Str(argument),
                    ..
                },
            ],
            [],
        ) if argument == "implicit" => Err(TypeError::Unsupported(
            "@fieldwise_init(\"implicit\")".to_string(),
        )),
        _ => Err(invalid(
            "@fieldwise_init only allows an \"implicit\" argument",
        )),
    }
}

/// The message of an `@explicit_destroy`, which needs exactly one string
/// literal and a struct whose `Deinitable` conformance is conditional.
fn explicit_destroy_message(
    decorator: &Decorator,
    conditional_deinitable: bool,
) -> Result<String, TypeError> {
    let message = match (decorator.args.as_slice(), decorator.kwargs.as_slice()) {
        ([], []) => {
            return Err(invalid(
                "@explicit_destroy requires an argument: `@explicit_destroy(\"...\")`",
            ));
        }
        ([message], []) => message,
        ([], [kwarg]) => &kwarg.value,
        _ => {
            return Err(invalid(
                "expected exactly one argument: `@explicit_destroy(\"...\")`",
            ));
        }
    };
    let ExprKind::Str(message) = &message.kind else {
        return Err(invalid(
            "expected a string literal argument: `@explicit_destroy(\"...\")`",
        ));
    };
    if !conditional_deinitable {
        return Err(invalid(
            "@explicit_destroy is not valid on `struct` with unconditional conformance to \
             `Deinitable`",
        ));
    }
    Ok(message.clone())
}

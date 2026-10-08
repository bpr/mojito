//! Which compile-time expressions are requests: one predicate, shared by the
//! elaborator above the check (which keeps such an expression for the
//! check) and the checker (which lifts it into the function MIR binds), so
//! the two never disagree about a binding. See
//! `docs/notes/ctfe-request-path.md`.

use mojito_ast::ast::{Expr, ExprKind};

/// What a call by name applies, as the phase asking sees its declarations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Callee {
    /// A module `def` or overload set, generic or not.
    Function,
    /// A struct, whose construction calls its `__init__`.
    Struct,
    /// Anything else: a builtin, a local, an unknown name.
    Other,
}

/// The declarations [`applies_callable`] consults.
pub trait CalleeOracle {
    /// What a call by `name` applies.
    fn callee(&self, name: &str) -> Callee;

    /// Whether `name`, the receiver of a method call, names a struct, which
    /// makes the call a static one (`S.f(n)`).
    fn names_struct(&self, name: &str) -> bool;
}

/// Whether `expression` applies a callable the elaborator does not fold
/// itself.
///
/// That is a call of a module `def`, a construction of a struct (`P(3)`),
/// or a static method of one (`S.f(n)`), anywhere below its root (`f(1)`,
/// `S.f(n) + 1`, `[f(1), 2]`, `L[f(2)]`). A compile-time evaluation of one
/// in a body is a request the elaborator below MIR serves from its
/// worklist. The constructions the elaborator folds as literals (`SIMD`,
/// `Dict`, and `Set` applications) are not applications here, but a method
/// of a literal is (`"abc".byte_length()`, `{"a": 1}.get("a")`), and so is
/// a subscript of a dictionary or set display, whose `__getitem__` raises.
pub fn applies_callable(expression: &Expr, oracle: &impl CalleeOracle) -> bool {
    struct Finder<'o, O> {
        oracle: &'o O,
        found: bool,
    }

    impl<O: CalleeOracle> mojito_ast::visit::Visitor for Finder<'_, O> {
        fn visit_expr(&mut self, expr: &Expr) {
            self.found |= match &expr.kind {
                ExprKind::Call { name, .. } => {
                    !matches!(
                        name.as_str(),
                        "len" | "range" | "reflect" | "conforms_to" | "materialize"
                    ) && match self.oracle.callee(name) {
                        Callee::Function => true,
                        Callee::Struct => !matches!(name.as_str(), "SIMD" | "Dict" | "Set"),
                        Callee::Other => false,
                    }
                }
                ExprKind::MethodCall { object, .. } => {
                    matches!(&object.kind,
                        ExprKind::Identifier(owner) if self.oracle.names_struct(owner))
                        || is_literal(object, self.oracle)
                }
                ExprKind::Index { object, .. } => is_keyed_display(object, self.oracle),
                _ => false,
            };
        }
    }

    if !computes_value(expression, oracle) {
        return false;
    }
    let mut finder = Finder {
        oracle,
        found: false,
    };
    mojito_ast::visit::walk_expr(&mut finder, expression);
    finder.found
}

/// Whether `expression` is rooted where it computes one value.
///
/// A call, an operator, a member, a display, or a subscript does (`f(1)`,
/// `S.f(n) + 1`, `[f(1), 2]`, `L[f(2)]`); a bracket application of a type
/// (`Buffer[f(2)]`) is a type, whatever its arguments apply.
pub fn computes_value(expression: &Expr, oracle: &impl CalleeOracle) -> bool {
    match &expression.kind {
        ExprKind::Index { object, .. } => !matches!(&object.kind,
            ExprKind::Identifier(name) if oracle.names_struct(name)),
        kind => matches!(
            kind,
            ExprKind::Call { .. }
                | ExprKind::MethodCall { .. }
                | ExprKind::Member { .. }
                | ExprKind::Infix(..)
                | ExprKind::Prefix(..)
                | ExprKind::Compare { .. }
                | ExprKind::ListLit(_)
                | ExprKind::TupleLit(_)
                | ExprKind::BraceLit(_)
        ),
    }
}

/// Whether `expression` is a dictionary or set display, bare or spelled by
/// its literal constructor (`Dict[K, V, H](keys, values, None)`).
fn is_keyed_display(expression: &Expr, oracle: &impl CalleeOracle) -> bool {
    match &expression.kind {
        ExprKind::BraceLit(_) => true,
        ExprKind::Call { name, .. } => {
            matches!(name.as_str(), "Dict" | "Set") && oracle.callee(name) == Callee::Struct
        }
        _ => false,
    }
}

/// Whether `expression` is a literal value: a scalar, a string, a display,
/// or a construction the elaborator folds as one.
fn is_literal(expression: &Expr, oracle: &impl CalleeOracle) -> bool {
    match &expression.kind {
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::Bool(_)
        | ExprKind::Str(_)
        | ExprKind::ListLit(_)
        | ExprKind::TupleLit(_)
        | ExprKind::BraceLit(_) => true,
        ExprKind::Call { name, .. } => {
            matches!(name.as_str(), "SIMD" | "Dict" | "Set")
                && oracle.callee(name) == Callee::Struct
        }
        _ => false,
    }
}

/// The checker's view: a name a local binding shadows applies nothing.
impl CalleeOracle for super::Checker {
    fn callee(&self, name: &str) -> Callee {
        let shadowed = self.binding_scope(name).is_some_and(|scope| scope != 0);
        let function = self.overload_sets.is_function(name)
            || matches!(
                self.lookup(name),
                Some(
                    mojito_types::types::Ty::Func { .. }
                        | mojito_types::types::Ty::GenericFunc { .. }
                        | mojito_types::types::Ty::Overload(_)
                )
            );
        if shadowed {
            Callee::Other
        } else if function {
            Callee::Function
        } else if self.structs.contains_key(name) {
            Callee::Struct
        } else {
            Callee::Other
        }
    }

    fn names_struct(&self, name: &str) -> bool {
        self.binding_scope(name).is_none() && self.structs.contains_key(name)
    }
}

//! `@always_inline("builtin")` bodies. Each builtin callable a compile-time
//! call applies ([`comptime_validation::applicable_declarations`]) is
//! folded once, at its declaration, into the parameter expression a call
//! substitutes its arguments into ([`Checker::called_application`]), as
//! upstream's `SharedState::foldInlineBuiltinFunction` clones the body with
//! a call's operands substituted. A body construct the pin's fold rejects
//! is rejected at the declaration, called or not, with the pin's message.
//! The body's operators are read by the one compile-time builder, so
//! `twice(n)` over `return n + n` and `n + n` spelled in place are one
//! expression.

use super::comptime_validation::{ApplicableDeclaration, applicable_declarations};
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::{InfixOp, PrefixOp};
use mojito_checked::declaration::InlineLevel;
use mojito_types::mlir::{MlirLowering, MlirOp};
use mojito_types::param_expr::{MetaTy, ParamKind};

impl Checker {
    /// Fold every `@always_inline("builtin")` body `stmts` declare into its
    /// callable's [`ApplicableFunction`] entry, callee first: a body that
    /// calls a builtin still unfolded waits for it, and one in a cycle, or
    /// one over a construct outside the fold (a host-implemented callee, a
    /// static method of a generic struct), keeps the application a call
    /// denotes. The first body the pin rejects is the error.
    ///
    /// [`ApplicableFunction`]: comptime_validation::ApplicableFunction
    pub(super) fn fold_builtin_bodies(&mut self, stmts: &[Stmt]) -> Result<(), TypeError> {
        let declarations: Vec<_> =
            applicable_declarations(stmts, &*self.overload_sets, &*self.program_constants)
                .into_iter()
                .filter(|declaration| declaration.function.inline == InlineLevel::Builtin)
                .collect();
        if let Some(error) = declarations
            .iter()
            .find_map(|declaration| self.builtin_body_rejection(declaration))
        {
            return Err(error);
        }
        let mut pending: Vec<&ApplicableDeclaration<'_>> = declarations.iter().collect();
        loop {
            let unfolded: HashSet<String> = pending
                .iter()
                .map(|declaration| declaration.function.symbol().to_string())
                .collect();
            let before = pending.len();
            pending.retain(|declaration| {
                let Some(body) = self
                    .folded_builtin_body(declaration)
                    .filter(|body| !applies_any(body, &unfolded))
                else {
                    return true;
                };
                if let Some(entry) = self
                    .applicable_functions
                    .get_mut(&declaration.name)
                    .and_then(|overloads| {
                        overloads
                            .iter_mut()
                            .find(|function| function.symbol() == declaration.function.symbol())
                    })
                {
                    entry.folded_body = Some(body);
                }
                false
            });
            if pending.len() == before {
                return Ok(());
            }
        }
    }

    /// The pin's rejection of a builtin body: a `raises` declaration, an
    /// early `return`, a statement `if` or a loop, a `var` reassigned after
    /// its initializer, or a call of a callable that is not itself builtin
    /// (an ordinary function, `//`, `%`, or `**` over `Int`, a
    /// `FloatLiteral` operand converted to `Float64`).
    fn builtin_body_rejection(&self, declaration: &ApplicableDeclaration<'_>) -> Option<TypeError> {
        if declaration.function.raises() {
            return Some(builtin_rejection(
                "does not support this argument convention",
            ));
        }
        let reassigned: HashSet<&str> = declaration
            .body
            .iter()
            .filter_map(|statement| match &statement.kind {
                StmtKind::Assign { name, .. } => Some(name.as_str()),
                StmtKind::AugAssign { place, .. } => match &place.kind {
                    ExprKind::Identifier(name) => Some(name.as_str()),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        let last = declaration.body.len().saturating_sub(1);
        declaration
            .body
            .iter()
            .enumerate()
            .find_map(|(index, statement)| match &statement.kind {
                StmtKind::VarDecl { name, .. } if reassigned.contains(name.as_str()) => {
                    Some(builtin_rejection("could not resolve operand value"))
                }
                StmtKind::Return(_) if index != last => Some(early_return()),
                StmtKind::If { .. } => Some(if returns(std::slice::from_ref(statement)) {
                    early_return()
                } else {
                    builtin_rejection("can only handle single-result if")
                }),
                StmtKind::While { .. } => Some(builtin_rejection(
                    "does not support MLIR operation lit.loop",
                )),
                StmtKind::For { .. } => Some(non_builtin_call()),
                _ => self.non_builtin_operation(statement),
            })
    }

    /// The first operation of `statement` that calls a callable the pin's
    /// fold cannot inline: an applicable function or static method not
    /// declared builtin, `//`, `%`, or `**` over a non-literal operand
    /// (upstream's `SIMD` declares none of the three builtin), or
    /// arithmetic converting a `FloatLiteral` operand.
    fn non_builtin_operation(&self, statement: &Stmt) -> Option<TypeError> {
        struct Operations<'a> {
            checker: &'a Checker,
            found: bool,
        }
        impl mojito_ast::visit::Visitor for Operations<'_> {
            fn visit_expr(&mut self, expr: &Expr) {
                self.found |= match &expr.kind {
                    ExprKind::Infix(
                        InfixOp::FloorDiv | InfixOp::Mod | InfixOp::Pow,
                        left,
                        right,
                    ) => {
                        !(matches!(left.kind, ExprKind::Int(_))
                            && matches!(right.kind, ExprKind::Int(_)))
                    }
                    ExprKind::Infix(
                        InfixOp::Add | InfixOp::Sub | InfixOp::Mul | InfixOp::Div,
                        left,
                        right,
                    ) => {
                        matches!(left.kind, ExprKind::Float(_))
                            != matches!(right.kind, ExprKind::Float(_))
                    }
                    ExprKind::Call { name, .. } => self.checker.calls_ordinary_function(name),
                    ExprKind::MethodCall { object, method, .. } => match &object.kind {
                        ExprKind::Identifier(owner) => self
                            .checker
                            .calls_ordinary_function(&format!("{owner}.{method}")),
                        _ => false,
                    },
                    _ => false,
                };
            }
        }
        let mut operations = Operations {
            checker: self,
            found: false,
        };
        mojito_ast::visit::walk_stmt(&mut operations, statement);
        operations.found.then(non_builtin_call)
    }

    /// Whether a call spelled `name` applies a callable no overload of which
    /// is builtin.
    fn calls_ordinary_function(&self, name: &str) -> bool {
        self.binding_scope(name).is_none_or(|scope| scope == 0)
            && self
                .applicable_functions
                .get(name)
                .is_some_and(|overloads| {
                    overloads
                        .iter()
                        .all(|function| function.inline != InlineLevel::Builtin)
                })
    }

    /// The body of `declaration` as a parameter expression over its argument
    /// slots ([`super::annotations::builtin_body_binder`]): a docstring, a
    /// `var` or `comptime` binding, and a closed `comptime assert` may stand
    /// before the final `return`. `None` for any other statement, or an
    /// expression the compile-time builder does not read.
    fn folded_builtin_body(
        &mut self,
        declaration: &ApplicableDeclaration<'_>,
    ) -> Option<ParamExpr> {
        let function = &declaration.function;
        let slots = function.argument_slots()?;
        let result = self.ty_from_anno(function.result()).ok()?;
        let symbol = function.symbol();
        self.push_param_scope(&[]);
        let level = self.tparams.len() - 1;
        if let Some(scope) = self.vparams.last_mut() {
            scope.extend(slots.iter().enumerate().map(|(slot, (name, ty))| {
                let binder = super::annotations::builtin_body_binder(symbol, slot, name);
                (
                    (*name).to_string(),
                    super::annotations::value_binder_expr(binder.id, name, ty),
                )
            }));
        }
        self.local_comptime_parameters.push(HashMap::new());
        let folded = self.folded_statements(declaration.body, level, &result);
        self.local_comptime_parameters.pop();
        self.tparams.pop();
        folded
    }

    /// [`Self::folded_builtin_body`] over `body`, its bindings bound at
    /// `level`.
    fn folded_statements(&mut self, body: &[Stmt], level: usize, result: &Ty) -> Option<ParamExpr> {
        let (last, leading) = body.split_last()?;
        for statement in leading {
            match &statement.kind {
                StmtKind::Pass => {}
                StmtKind::Expr(Expr {
                    kind: ExprKind::Str(_),
                    ..
                }) => {}
                StmtKind::VarDecl { name, value, .. }
                | StmtKind::Comptime {
                    name,
                    value,
                    type_params: _,
                    ty: _,
                    where_clauses: _,
                } => {
                    let bound = self.builtin_operand(value)?;
                    self.local_comptime_parameters
                        .last_mut()?
                        .insert(name.clone(), (level, bound));
                }
                // A closed assertion is the body check's to judge; one over
                // an argument would be judged per call.
                StmtKind::ComptimeAssert { cond, .. }
                    if names_read(cond)
                        .iter()
                        .all(|name| self.value_parameter_in_scope(name).is_none()) => {}
                _ => return None,
            }
        }
        let StmtKind::Return(Some(value)) = &last.kind else {
            return None;
        };
        let value = self.builtin_operand(value)?;
        let value = match value.as_constant() {
            Some(closed) => self
                .param_context
                .constant(closed.clone().materialize_as(result)?)
                .ok()?,
            None => value,
        };
        (value.meta().as_value() == Some(result)).then_some(value)
    }

    /// The parameter expression of a builtin body's operand, built as a
    /// type argument is.
    fn builtin_operand(&self, expr: &Expr) -> Option<ParamExpr> {
        let _lifting = self.lifting_position();
        self.compile_dependent_ct_expr(expr)
            .ok()
            .filter(|built| matches!(built.meta(), MetaTy::Value(_)))
    }
}

/// The operator expression ``__mlir_op.`op`(a, b)`` computes when `op` is a
/// `pop` or `index` op the compile-time builder reads, an infix operator or
/// a negation over the same operands (`a + b` for `pop.add`, `a < b` for
/// `index.cmp` at `slt`), as upstream's parameter folder reads the op.
/// `None` for any other expression or op.
pub(super) fn mlir_op_operator(expr: &Expr) -> Option<Expr> {
    let (callee, param_args, args, kwargs) = match &expr.kind {
        ExprKind::MethodCall {
            object,
            method,
            args,
            kwargs,
        } => ((&**object, method.as_str()), [].as_slice(), args, kwargs),
        ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } => match &callee.kind {
            ExprKind::Member { object, field } => (
                (&**object, field.as_str()),
                param_args.as_slice(),
                args,
                kwargs,
            ),
            _ => return None,
        },
        _ => return None,
    };
    let (object, name) = callee;
    if !matches!(&object.kind, ExprKind::Identifier(op) if op == "__mlir_op") || !kwargs.is_empty()
    {
        return None;
    }
    let mut predicate = None;
    for argument in param_args {
        match argument {
            mojito_ast::ast::ParamArg::Named { name, value } if name == "pred" => {
                predicate = Some(super::method_calls::mlir_attribute(value)?);
            }
            _ => return None,
        }
    }
    let kind = match (
        MlirOp::parse(name, predicate).ok()?.lowering(),
        args.as_slice(),
    ) {
        (MlirLowering::Binary(op), [left, right]) => {
            ExprKind::Infix(op, Box::new(left.clone()), Box::new(right.clone()))
        }
        (MlirLowering::Negate, [operand]) => {
            ExprKind::Prefix(PrefixOp::Neg, Box::new(operand.clone()))
        }
        _ => return None,
    };
    let mut operator = Expr::new(kind, expr.span);
    operator.source.clone_from(&expr.source);
    Some(operator)
}

/// A rejection of a builtin body, in the pin's words.
fn builtin_rejection(reason: &str) -> TypeError {
    TypeError::InvalidDecorator(format!("'@always_inline(\"builtin\")' {reason}"))
}

/// The pin's rejection of a call of a callable that is not builtin.
fn non_builtin_call() -> TypeError {
    builtin_rejection("only supports calls to other '@always_inline(\"builtin\")' functions")
}

/// The pin's rejection of a `return` before a builtin body's last
/// statement.
fn early_return() -> TypeError {
    builtin_rejection("does not support MLIR operation lit.return")
}

/// Whether `body` holds a `return` anywhere.
fn returns(body: &[Stmt]) -> bool {
    struct Returns(bool);
    impl mojito_ast::visit::Visitor for Returns {
        fn visit_stmt(&mut self, statement: &Stmt) {
            self.0 |= matches!(statement.kind, StmtKind::Return(_));
        }
    }
    let mut found = Returns(false);
    mojito_ast::visit::walk_block(&mut found, body);
    found.0
}

/// Whether `body` applies any callable of `symbols`.
fn applies_any(body: &ParamExpr, symbols: &HashSet<String>) -> bool {
    let mut applies = false;
    body.visit(&mut |node| {
        applies |= matches!(node.kind(), ParamKind::Apply { function, .. } if symbols.contains(function.as_str()));
    });
    applies
}

//! `@always_inline("builtin")` bodies. Each builtin callable a compile-time
//! call applies ([`comptime_validation::applicable_declarations`]) is
//! folded once, at its declaration, into the parameter expression a call
//! substitutes its arguments into ([`Checker::called_application`]), as
//! upstream's `SharedState::foldInlineBuiltinFunction` clones the body with
//! a call's operands substituted. An `__init__`'s body folds to the fields
//! it stores, which a call builds into the struct value it constructs. A body construct the pin's fold rejects
//! is rejected at the declaration, called or not, with the pin's message.
//! The body's operators are read by the one compile-time builder, so
//! `twice(n)` over `return n + n` and `n + n` spelled in place are one
//! expression.

use super::comptime_validation::{ApplicableDeclaration, FoldedBody, applicable_declarations};
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
    /// one over a construct outside the fold (a host-implemented callee),
    /// keeps the application a call denotes. A static method's body is
    /// folded and judged in its struct's scope. The first body the pin
    /// rejects is the error.
    ///
    /// [`ApplicableFunction`]: comptime_validation::ApplicableFunction
    pub(super) fn fold_builtin_bodies(&mut self, stmts: &[Stmt]) -> Result<(), TypeError> {
        if let Some(error) = self.builtin_convention_rejection(stmts) {
            return Err(error);
        }
        let declarations: Vec<_> =
            applicable_declarations(stmts, &*self.overload_sets, &*self.program_constants)
                .into_iter()
                .filter(|declaration| declaration.function.inline == InlineLevel::Builtin)
                .collect();
        if let Some(error) = declarations.iter().find_map(|declaration| {
            self.in_owner_scope(declaration, |checker| {
                checker.builtin_body_rejection(declaration)
            })
            .unwrap_or_else(|| self.builtin_body_rejection(declaration))
        }) {
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

    /// `f` run where each argument slot of `declaration` is a binder of its
    /// own ([`super::annotations::builtin_body_binder`]) under the slot's
    /// name, in a parameter scope opened at the level `f` receives, within
    /// its struct's member scope ([`Self::in_owner_scope`]).
    pub(super) fn in_argument_scope<T>(
        &mut self,
        declaration: &ApplicableDeclaration<'_>,
        f: impl FnOnce(&mut Self, usize) -> T,
    ) -> Option<T> {
        self.in_owner_scope(declaration, |checker| {
            let function = &declaration.function;
            let symbol = function.symbol();
            let slots = function.argument_slots(checker.self_ty.as_ref())?;
            checker.push_param_scope(&[]);
            let level = checker.tparams.len() - 1;
            if let Some(scope) = checker.vparams.last_mut() {
                scope.extend(slots.iter().enumerate().map(|(slot, (name, ty))| {
                    let binder = super::annotations::builtin_body_binder(symbol, slot, name);
                    (
                        (*name).to_string(),
                        super::annotations::value_binder_expr(binder.id, name, ty),
                    )
                }));
            }
            checker.local_comptime_parameters.push(HashMap::new());
            let result = f(checker, level);
            checker.local_comptime_parameters.pop();
            checker.tparams.pop();
            Some(result)
        })
        .flatten()
    }

    /// The pin's rejection of a builtin method's receiver, judged for every
    /// builtin member `stmts` declare, applicable or not: a `mut`, `var`,
    /// `ref`, or `deinit` `self`, or any `self` (an `__init__`'s `out self`
    /// included) of a struct that is not trivially register-passable.
    fn builtin_convention_rejection(&mut self, stmts: &[Stmt]) -> Option<TypeError> {
        stmts.iter().find_map(|statement| {
            let StmtKind::Struct { methods, .. } = &statement.kind else {
                return None;
            };
            let builtin: Vec<_> = methods
                .iter()
                .filter(|method| {
                    method.has_self
                        && matches!(
                            InlineLevel::of(&method.decorators),
                            Ok(InlineLevel::Builtin)
                        )
                })
                .collect();
            if builtin.is_empty() {
                return None;
            }
            let trivial = self
                .in_struct_scope(statement, |checker| {
                    checker
                        .self_ty
                        .clone()
                        .is_some_and(|self_ty| checker.is_trivial_register_passable(&self_ty))
                })
                .unwrap_or(true);
            builtin
                .iter()
                .any(|method| {
                    !trivial
                        || matches!(
                            method.self_convention,
                            Some(
                                ArgConvention::Mut
                                    | ArgConvention::Var
                                    | ArgConvention::Ref
                                    | ArgConvention::Deinit
                            )
                        )
                })
                .then(|| builtin_rejection("does not support this argument convention"))
        })
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
                _ => self.non_builtin_operation(declaration, statement),
            })
    }

    /// The first operation of `statement`, in `declaration`'s body, that
    /// calls a callable the pin's fold cannot inline: an applicable
    /// function or static method not declared builtin (`h(n)`,
    /// `Self.ok(n)`, `G[1].ok(n)`), a method of `self` or of a `Self`
    /// parameter no overload of which is builtin (`self.ord()`), `//`,
    /// `%`, or `**` over a non-literal operand (upstream's `SIMD` declares
    /// none of the three builtin), or arithmetic converting a
    /// `FloatLiteral` operand.
    fn non_builtin_operation(
        &self,
        declaration: &ApplicableDeclaration<'_>,
        statement: &Stmt,
    ) -> Option<TypeError> {
        struct Operations<'a> {
            checker: &'a Checker,
            /// The names of the body's operands typed `Self`.
            receivers: Vec<&'a str>,
            /// The methods of the struct declaring the body.
            methods: &'a [mojito_ast::ast::Method],
            found: bool,
        }
        impl Operations<'_> {
            /// Whether `object.method(...)` calls a method of a `Self`
            /// operand no overload of which is builtin.
            fn calls_ordinary_method(&self, object: &Expr, method: &str) -> bool {
                let ExprKind::Identifier(name) = &object.kind else {
                    return false;
                };
                let overloads: Vec<_> = self
                    .methods
                    .iter()
                    .filter(|declared| declared.name == method)
                    .collect();
                self.receivers.contains(&name.as_str())
                    && !overloads.is_empty()
                    && overloads.iter().all(|declared| {
                        !matches!(
                            InlineLevel::of(&declared.decorators),
                            Ok(InlineLevel::Builtin)
                        )
                    })
            }
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
                    ExprKind::MethodCall { object, method, .. } => {
                        self.checker.calls_ordinary_static_method(object, method)
                            || self.calls_ordinary_method(object, method)
                    }
                    ExprKind::Invoke { callee, .. } => match &callee.kind {
                        ExprKind::Member { object, field } => {
                            self.checker.calls_ordinary_static_method(object, field)
                                || self.calls_ordinary_method(object, field)
                        }
                        _ => false,
                    },
                    _ => false,
                };
            }
        }
        let methods = match declaration.owner.map(|owner| &owner.kind) {
            Some(StmtKind::Struct { methods, .. }) => methods.as_slice(),
            _ => &[],
        };
        let receivers = self
            .self_ty
            .as_ref()
            .and_then(|self_ty| {
                let slots = declaration.function.argument_slots(Some(self_ty))?;
                Some(
                    slots
                        .into_iter()
                        .filter(|(_, ty)| ty == self_ty)
                        .map(|(name, _)| name)
                        .collect(),
                )
            })
            .unwrap_or_default();
        let mut operations = Operations {
            checker: self,
            receivers,
            methods,
            found: false,
        };
        mojito_ast::visit::walk_stmt(&mut operations, statement);
        operations.found.then(non_builtin_call)
    }

    /// Whether a call of `method` on `object` applies a static method no
    /// overload of which is builtin, its struct spelled by name, by
    /// application, or as `Self`.
    fn calls_ordinary_static_method(&self, object: &Expr, method: &str) -> bool {
        self.applied_owner(object, method)
            .is_some_and(|(owner, _)| self.calls_ordinary_function(&format!("{owner}.{method}")))
    }

    /// Whether a call spelled `name` applies a callable no overload of which
    /// is builtin.
    fn calls_ordinary_function(&self, name: &str) -> bool {
        self.binding_scope(name).is_none_or(|scope| scope == 0)
            && self
                .applicable_functions
                .get(name)
                .is_some_and(|overloads| {
                    overloads.iter().all(|function| {
                        !function.constructs() && function.inline != InlineLevel::Builtin
                    })
                })
    }

    /// The body of `declaration` as a parameter expression over its argument
    /// slots ([`super::annotations::builtin_body_binder`]): a docstring, a
    /// `var` or `comptime` binding, and a `comptime assert`, which the fold
    /// drops, may stand before the final `return`. An `__init__`'s body is
    /// the fields it stores instead ([`Self::folded_construction`]). `None`
    /// for any other statement, or an expression the compile-time builder
    /// does not read.
    fn folded_builtin_body(
        &mut self,
        declaration: &ApplicableDeclaration<'_>,
    ) -> Option<FoldedBody> {
        self.in_argument_scope(declaration, |checker, level| {
            if declaration.function.constructs() {
                return checker
                    .folded_construction(declaration, level)
                    .map(FoldedBody::Fields);
            }
            let self_ty = checker.self_ty.clone();
            if let Some(self_ty) = &self_ty {
                checker.bind_struct_slots(declaration, level, self_ty)?;
            }
            let result = match declaration.function.result() {
                SourceType::SelfType => self_ty?,
                result => checker.ty_from_anno(result).ok()?,
            };
            checker
                .folded_statements(declaration.body, level, &result)
                .map(FoldedBody::Value)
        })
        .flatten()
    }

    /// Bind each argument slot of `declaration` typed `self_ty` (`self`, a
    /// `Self` parameter) at `level` to the struct value of its fields read
    /// off the slot, as upstream extracts a field of a struct-typed
    /// operand: the body reads them before its struct's field types are
    /// resolved, and a call's receiver replaces the slot under them.
    fn bind_struct_slots(
        &mut self,
        declaration: &ApplicableDeclaration<'_>,
        level: usize,
        self_ty: &Ty,
    ) -> Option<()> {
        let symbol = declaration.function.symbol();
        let slots = declaration.function.argument_slots(Some(self_ty))?;
        if slots.iter().all(|(_, ty)| ty != self_ty) {
            return Some(());
        }
        let fields = self.declared_fields(declaration)?;
        for (slot, (name, ty)) in slots.iter().enumerate() {
            if ty != self_ty {
                continue;
            }
            let operand = super::annotations::value_binder_expr(
                super::annotations::builtin_body_binder(symbol, slot, name).id,
                name,
                ty,
            );
            let read = fields
                .iter()
                .map(|(field, field_ty)| {
                    let value = self
                        .param_context
                        .field(&operand, field, MetaTy::value(field_ty.clone()))
                        .ok()?;
                    Some((field.clone(), value))
                })
                .collect::<Option<Vec<_>>>()?;
            let value = self.param_context.struct_value(&read, &operand);
            self.local_comptime_parameters
                .last_mut()?
                .insert((*name).to_string(), (level, value));
        }
        Some(())
    }

    /// The fields the struct declaring `declaration` declares, in order,
    /// each with its type, resolved in its member scope.
    fn declared_fields(
        &self,
        declaration: &ApplicableDeclaration<'_>,
    ) -> Option<Vec<(String, Ty)>> {
        let StmtKind::Struct { fields, .. } = &declaration.owner?.kind else {
            return None;
        };
        fields
            .iter()
            .map(|field| Some((field.name.clone(), self.ty_from_anno(&field.ty).ok()?)))
            .collect()
    }

    /// `f` run in the member scope of the struct declaring `declaration`,
    /// where `Self` and its parameters are the struct's own binders, as the
    /// struct's members are checked; `None` when its shell is not yet
    /// registered.
    fn in_owner_scope<T>(
        &mut self,
        declaration: &ApplicableDeclaration<'_>,
        f: impl FnOnce(&mut Self) -> T,
    ) -> Option<T> {
        match declaration.owner {
            Some(owner) => self.in_struct_scope(owner, f),
            None => Some(f(self)),
        }
    }

    /// `f` run in the member scope of the struct `statement` declares;
    /// `None` when its shell is not yet registered.
    fn in_struct_scope<T>(
        &mut self,
        statement: &Stmt,
        f: impl FnOnce(&mut Self) -> T,
    ) -> Option<T> {
        let (_, saved) = self
            .enter_struct_scope(&overload_support::struct_declaration(statement)?)
            .ok()?;
        let result = f(self);
        self.exit_struct_scope(saved);
        Some(result)
    }

    /// [`Self::folded_builtin_body`] over `body`, its bindings bound at
    /// `level`.
    fn folded_statements(&mut self, body: &[Stmt], level: usize, result: &Ty) -> Option<ParamExpr> {
        let (last, leading) = body.split_last()?;
        for statement in leading {
            self.folded_leading_statement(statement, level)?;
        }
        let StmtKind::Return(Some(value)) = &last.kind else {
            return None;
        };
        self.builtin_value(value, result)
    }

    /// The fields a builtin `__init__`'s body stores, in declaration order,
    /// over its argument slots, as upstream's fold reads the construction:
    /// each `self.f = e` stores `e`, a later store of a field replacing an
    /// earlier one, and a read of a stored field (`self.b = self.a + 1`) is
    /// the value stored. A docstring, a `var` or `comptime` binding, and a
    /// `comptime assert` may stand among the stores. `None` for any other
    /// statement (an augmented store, a store into a field's field), or a
    /// field the body leaves unstored.
    fn folded_construction(
        &mut self,
        declaration: &ApplicableDeclaration<'_>,
        level: usize,
    ) -> Option<Vec<(String, ParamExpr)>> {
        let declared = self.declared_fields(declaration)?;
        let self_ty = self.self_ty.clone()?;
        let symbol = declaration.function.symbol();
        let mut slots: Vec<ParamExpr> = declaration
            .function
            .argument_slots(Some(&self_ty))?
            .into_iter()
            .enumerate()
            .map(|(slot, (name, ty))| {
                super::annotations::value_binder_expr(
                    super::annotations::builtin_body_binder(symbol, slot, name).id,
                    name,
                    &ty,
                )
            })
            .collect();
        if !self.enclosing_type_params.is_empty() {
            slots.insert(0, self.param_context.type_shape(self_ty.clone()));
        }
        let spelling = self
            .param_context
            .apply(symbol, &slots, MetaTy::value(self_ty));
        let mut stored: Vec<Option<ParamExpr>> = vec![None; declared.len()];
        for statement in declaration.body {
            let so_far: Vec<(String, ParamExpr)> = declared
                .iter()
                .zip(&stored)
                .filter_map(|((name, _), value)| Some((name.clone(), value.clone()?)))
                .collect();
            let this = self.param_context.struct_value(&so_far, &spelling);
            self.local_comptime_parameters
                .last_mut()?
                .insert("self".to_string(), (level, this));
            let StmtKind::SetPlace { place, value } = &statement.kind else {
                self.folded_leading_statement(statement, level)?;
                continue;
            };
            let ExprKind::Member { object, field } = &place.kind else {
                return None;
            };
            if !matches!(&object.kind, ExprKind::Identifier(name) if name == "self") {
                return None;
            }
            let index = declared.iter().position(|(name, _)| name == field)?;
            stored[index] = Some(self.builtin_value(value, &declared[index].1)?);
        }
        declared
            .into_iter()
            .zip(stored)
            .map(|((name, _), value)| Some((name, value?)))
            .collect()
    }

    /// Bind or drop `statement`, one a builtin body may hold before its
    /// result: a docstring or `pass`, a `var` or `comptime` binding bound at
    /// `level`, or a `comptime assert`, which the pin's fold drops (a closed
    /// one is the body check's to judge). `None` for any other statement.
    fn folded_leading_statement(&mut self, statement: &Stmt, level: usize) -> Option<()> {
        match &statement.kind {
            StmtKind::Pass
            | StmtKind::Expr(Expr {
                kind: ExprKind::Str(_),
                ..
            })
            | StmtKind::ComptimeAssert { .. } => Some(()),
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
                Some(())
            }
            _ => None,
        }
    }

    /// The parameter expression of a builtin body's value of type `ty`, a
    /// closed one materialized at it.
    fn builtin_value(&self, value: &Expr, ty: &Ty) -> Option<ParamExpr> {
        let value = self.builtin_operand(value)?;
        let value = match value.as_constant() {
            Some(closed) => self
                .param_context
                .constant(closed.clone().materialize_as(ty)?)
                .ok()?,
            None => value,
        };
        (value.meta().as_value() == Some(ty)).then_some(value)
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
/// `pop.select` over `c.__mlir_bool__()` is the conditional `a if c else
/// b`. A builtin cast is its sugar: `pop.cast_to_builtin` over `s` is
/// `s.__mlir_index__()` and `pop.cast_from_builtin` over `i` is
/// `Int(mlir_value=i)`, the result type its `_type=` names being the one
/// the check gave the op. `None` for any other expression or op.
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
    let mut typed = false;
    for argument in param_args {
        match argument {
            mojito_ast::ast::ParamArg::Named { name, value } if name == "pred" => {
                predicate = Some(super::method_calls::mlir_attribute(value)?);
            }
            mojito_ast::ast::ParamArg::Named { name, .. } if name == "_type" => typed = true,
            _ => return None,
        }
    }
    let op = MlirOp::parse(name, predicate).ok()?;
    let kind = match (op.lowering(), args.as_slice()) {
        (MlirLowering::BuiltinCast, [operand]) if typed && op == MlirOp::PopCastToBuiltin => {
            ExprKind::MethodCall {
                object: Box::new(operand.clone()),
                method: "__mlir_index__".to_string(),
                args: Vec::new(),
                kwargs: Vec::new(),
            }
        }
        (MlirLowering::BuiltinCast, [operand]) if typed => ExprKind::Call {
            name: "Int".to_string(),
            param_args: Vec::new(),
            args: Vec::new(),
            kwargs: vec![mojito_ast::ast::KwArg {
                name: "mlir_value".to_string(),
                value: operand.clone(),
            }],
        },
        _ if typed => return None,
        (MlirLowering::Binary(op), [left, right]) => {
            ExprKind::Infix(op, Box::new(left.clone()), Box::new(right.clone()))
        }
        (MlirLowering::Negate, [operand]) => {
            ExprKind::Prefix(PrefixOp::Neg, Box::new(operand.clone()))
        }
        (MlirLowering::Select, [condition, on_true, on_false]) => match &condition.kind {
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } if method == "__mlir_bool__" && args.is_empty() && kwargs.is_empty() => {
                ExprKind::IfExpr {
                    cond: object.clone(),
                    then_branch: Box::new(on_true.clone()),
                    else_branch: Box::new(on_false.clone()),
                }
            }
            _ => return None,
        },
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
fn applies_any(body: &FoldedBody, symbols: &HashSet<String>) -> bool {
    let mut applies = false;
    let mut visit = |expr: &ParamExpr| {
        expr.visit(&mut |node| {
            applies |= matches!(node.kind(), ParamKind::Apply { function, .. } if symbols.contains(function.as_str()));
        });
    };
    match body {
        FoldedBody::Value(value) => visit(value),
        FoldedBody::Fields(fields) => fields.iter().for_each(|(_, value)| visit(value)),
    }
    applies
}

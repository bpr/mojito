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

use super::comptime_validation::{ApplicableDeclaration, BuiltinAssertions, FoldedBody};
#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;
use mojito_ast::ast::{InfixOp, PrefixOp};
use mojito_checked::declaration::InlineLevel;
use mojito_types::mlir::{MlirLowering, MlirOp};
use mojito_types::param_expr::{MetaTy, ParamKind, ParamOp};

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
        self.builtin_procedures = builtin_procedures(stmts);
        self.builtin_struct_fields = self.resolved_builtin_struct_fields(stmts);
        let declarations: Vec<_> = self
            .listed_declarations(stmts)
            .into_iter()
            .filter(|declaration| declaration.function.inline == InlineLevel::Builtin)
            // A callable with a collector folds no body: a call stays its
            // application, which the elaborator runs.
            .filter(|declaration| !declaration.function.has_collector())
            .collect();
        if let Some(error) = declarations.iter().find_map(|declaration| {
            self.in_owner_scope(declaration, |checker| {
                checker
                    .applicable_signature_rejection(declaration)
                    .or_else(|| checker.builtin_body_rejection(declaration))
            })
            .unwrap_or_else(|| self.builtin_body_rejection(declaration))
        }) {
            return Err(error);
        }
        self.mark_asserting_builtins(&declarations);
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
                if let Some(entry) = self.applicable_entry(declaration) {
                    entry.folded_body = Some(body);
                }
                false
            });
            if pending.len() == before {
                return Ok(());
            }
        }
    }

    /// Register each `comptime` type member (`comptime _Mask = SIMD[.bool,
    /// Self.length]`) that a builtin method of its struct returns as
    /// `Self.name`, before the method's signature classifies and its body
    /// folds, as the pin resolves a member on demand; the struct's other
    /// members register with its member types. A member whose body does
    /// not resolve yet waits for them.
    pub(super) fn register_builtin_result_members(&mut self, stmts: &[Stmt]) {
        for statement in stmts {
            let StmtKind::Struct {
                name,
                associated,
                methods,
                ..
            } = &statement.kind
            else {
                continue;
            };
            let returned: Vec<&mojito_ast::ast::StructComptime> = associated
                .iter()
                .filter(|member| member.params.is_empty() && member.ty.is_none())
                .filter(|member| {
                    methods.iter().any(|method| {
                        matches!(&method.ret, Some(SourceType::SelfParam(read)) if *read == member.name)
                            && InlineLevel::of(&method.decorators)
                                .is_ok_and(|level| level == InlineLevel::Builtin)
                    })
                })
                .collect();
            if returned.is_empty() {
                continue;
            }
            let resolved = self
                .in_struct_scope(statement, |checker| {
                    returned
                        .iter()
                        .filter_map(|member| {
                            let spelling =
                                super::constraints::assoc_body_source_type(&member.value).ok()?;
                            let ty = checker.ty_from_anno(&spelling).ok()?;
                            Some((member.name.clone(), ty, spelling))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let Some(info) = self.structs.get_mut(name) else {
                continue;
            };
            for (member, ty, spelling) in resolved {
                info.associated
                    .entry(member.clone())
                    .or_insert(CtValue::Type(ty));
                info.associated_spellings.entry(member).or_insert(spelling);
            }
        }
    }

    /// Record a run-time call `expr` of a builtin callable whose every
    /// argument is a compile-time value as the parameter value its folded
    /// body denotes over them, as upstream's parser folds such a call where
    /// it builds it (`foldInlineBuiltinFunction`): the call is a constant,
    /// never instantiated, so the body's `comptime assert`, which the fold
    /// drops, is not judged (`checked[-1](1)`). A call with a run-time
    /// argument (`checked[-1](x)`) stays a call, which the instance judges.
    pub(super) fn record_folded_builtin_call(&self, expr: &Expr) {
        if !self.crosses_to_runtime() {
            return;
        }
        let spelling = match &expr.kind {
            ExprKind::Call { name, .. } => name.as_str(),
            ExprKind::MethodCall { method, .. } => method.as_str(),
            ExprKind::Invoke { callee, .. } => match &callee.kind {
                ExprKind::Member { field, .. } => field.as_str(),
                _ => return,
            },
            _ => return,
        };
        if !self.spells_asserting_builtin(spelling) || self.reads_run_time_binding(expr) {
            return;
        }
        let span = expr.source_span();
        let cached = self.folded_builtin_calls.borrow().get(&span).cloned();
        let folded = cached.unwrap_or_else(|| {
            let _position = self.comptime_position();
            let folded = self
                .applies_folded_builtin(expr)
                .then(|| self.called_application(expr))
                .flatten()
                .filter(|value| !matches!(value.kind(), ParamKind::Apply { .. }));
            self.folded_builtin_calls
                .borrow_mut()
                .insert(span.clone(), folded.clone());
            folded
        });
        if let Some(value) = folded {
            self.record_member_value(span, value);
        }
    }

    /// Whether `expr` reads a name that denotes no parameter expression (a
    /// local `var`, an argument, `self`), so no call in it is one: the
    /// cheap test a run-time fold makes before selecting its callee.
    pub(super) fn reads_run_time_binding(&self, expr: &Expr) -> bool {
        struct RunTimeRead<'a> {
            checker: &'a Checker,
            found: bool,
        }
        impl mojito_ast::visit::Visitor for RunTimeRead<'_> {
            fn visit_expr(&mut self, expr: &Expr) {
                self.found = self.found
                    || matches!(&expr.kind, ExprKind::Identifier(name)
                        if self.checker.lookup(name).is_some())
                        && self.checker.compile_dependent_ct_expr(expr).is_err();
            }
        }
        let _position = self.comptime_position();
        let mut read = RunTimeRead {
            checker: self,
            found: false,
        };
        mojito_ast::visit::walk_expr(&mut read, expr);
        read.found
    }

    /// Whether a call spelled `spelling` (`f`, `plus` of `G[1].plus`, `SIMD`
    /// of a construction) may reach a builtin callable, the module's own or
    /// an imported one, whose body holds a `comptime assert` its fold drops
    /// ([`folded_spellings`]). Only the families so spelled resolve, once
    /// per spelling.
    pub(super) fn spells_asserting_builtin(&self, spelling: &str) -> bool {
        if let Some(&asserting) = self.asserting_builtin_spellings.borrow().get(spelling) {
            return asserting;
        }
        let asserting = self
            .applicable_functions
            .keys()
            .filter(|family| folded_spellings(family).iter().any(|own| own == spelling))
            .any(|family| {
                self.applicable_functions
                    .get(family)
                    .is_some_and(|overloads| {
                        overloads
                            .iter()
                            .any(|function| function.assertions == BuiltinAssertions::Dropped)
                    })
            });
        self.asserting_builtin_spellings
            .borrow_mut()
            .insert(spelling.to_string(), asserting);
        asserting
    }

    /// The pin's rejection of a builtin callable's signature, whose fold
    /// takes every argument by value in a register: a variadic parameter, a
    /// `mut`, `var`, `ref`, or `deinit` one, or a parameter or result whose
    /// type (`tys`, one per parameter) is not trivially register-passable.
    /// `None` for any callable not declared builtin; its receiver is
    /// [`Self::fold_builtin_bodies`]' to judge.
    pub(super) fn builtin_signature_rejection(
        &self,
        decorators: &[mojito_ast::ast::Decorator],
        params: &[mojito_ast::ast::FnParam],
        tys: &[Ty],
        result: &Ty,
    ) -> Option<TypeError> {
        if !matches!(InlineLevel::of(decorators), Ok(InlineLevel::Builtin)) {
            return None;
        }
        let in_register = |ty: &Ty| self.passes_in_register(ty);
        let by_value = |param: &mojito_ast::ast::FnParam, ty: &Ty| {
            param.kind == mojito_ast::ast::ParamKind::Regular
                && matches!(
                    param.convention,
                    None | Some(ArgConvention::Imm | ArgConvention::Out)
                )
                && in_register(ty)
        };
        let result_by_value = matches!(result.kind(), TyKind::None) || in_register(result);
        (params
            .iter()
            .zip(tys)
            .any(|(param, ty)| !by_value(param, ty))
            || !result_by_value)
            .then(|| builtin_rejection("does not support this argument convention"))
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
            let slots = declaration
                .function
                .argument_slots(checker.self_ty.as_ref())?;
            checker.push_param_scope(&slots);
            let level = checker.tparams.len() - 1;
            checker.local_comptime_parameters.push(HashMap::new());
            let result = f(checker, level);
            checker.local_comptime_parameters.pop();
            checker.tparams.pop();
            Some(result)
        })
        .flatten()
    }

    /// Mark each of `declarations` whose body holds a `comptime assert`,
    /// or calls a builtin callable so marked, as the pin's fold of a call
    /// drops the assertion of every builtin it inlines
    /// ([`ApplicableFunction::assertions`]). A callee is known by the
    /// name the call spells, so a call of an overload sharing it marks too,
    /// which only folds a call whose value its instance computes alike.
    ///
    /// [`ApplicableFunction::assertions`]: comptime_validation::ApplicableFunction
    fn mark_asserting_builtins(&mut self, declarations: &[ApplicableDeclaration<'_>]) {
        let mut unmarked: Vec<_> = declarations.iter().collect();
        loop {
            let before = unmarked.len();
            unmarked.retain(|declaration| {
                let asserts = declaration
                    .body
                    .iter()
                    .any(|statement| matches!(statement.kind, StmtKind::ComptimeAssert { .. }))
                    || call_spellings(declaration.body)
                        .iter()
                        .any(|spelling| self.spells_asserting_builtin(spelling));
                if asserts && let Some(entry) = self.applicable_entry(declaration) {
                    entry.assertions = BuiltinAssertions::Dropped;
                }
                !asserts
            });
            self.asserting_builtin_spellings.borrow_mut().clear();
            if unmarked.len() == before {
                return;
            }
        }
    }

    /// The field types of each non-generic struct `stmts` declare with a
    /// builtin member, each resolved in its member scope
    /// ([`Checker::builtin_struct_fields`]). A struct a field of which does
    /// not resolve yet is left out.
    fn resolved_builtin_struct_fields(
        &mut self,
        stmts: &[Stmt],
    ) -> HashMap<String, Vec<(String, Ty)>> {
        let mut resolved = HashMap::new();
        for statement in stmts {
            let StmtKind::Struct {
                name,
                type_params,
                fields,
                methods,
                ..
            } = &statement.kind
            else {
                continue;
            };
            let builtin = methods.iter().any(|method| {
                matches!(
                    InlineLevel::of(&method.decorators),
                    Ok(InlineLevel::Builtin)
                )
            });
            if !builtin || !type_params.is_empty() {
                continue;
            }
            if let Some(Some(types)) = self.in_struct_scope(statement, |checker| {
                fields
                    .iter()
                    .map(|field| Some((field.name.clone(), checker.ty_from_anno(&field.ty).ok()?)))
                    .collect::<Option<Vec<_>>>()
            }) {
                resolved.insert(name.clone(), types);
            }
        }
        resolved
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

    /// [`Self::builtin_signature_rejection`] of an applicable `declaration`,
    /// judged before its body, as the pin judges a signature first: a
    /// run-time argument slot or a result whose type is not trivially
    /// register-passable.
    fn applicable_signature_rejection(
        &self,
        declaration: &ApplicableDeclaration<'_>,
    ) -> Option<TypeError> {
        let slots = declaration.function.argument_slots(self.self_ty.as_ref())?;
        let result = match declaration.function.result() {
            SourceType::SelfType => self.self_ty.clone(),
            result => self.ty_from_anno(result).ok(),
        };
        let runtime = slots.len() - declaration.function.runtime_slot_count();
        let by_value = slots[runtime..]
            .iter()
            .filter_map(slot_ty)
            .chain(
                result
                    .as_ref()
                    .filter(|_| !declaration.function.constructs()),
            )
            .all(|ty| matches!(ty.kind(), TyKind::None) || self.passes_in_register(ty));
        (!by_value).then(|| builtin_rejection("does not support this argument convention"))
    }

    /// Whether a builtin callable's fold takes a `ty` argument by value in
    /// a register: `ty` is trivially register-passable.
    fn passes_in_register(&self, ty: &Ty) -> bool {
        self.is_trivial_register_passable(ty) || self.conforms_to(ty, "TrivialRegisterPassable")
    }

    /// The pin's rejection of a builtin body: a `raises` declaration, an
    /// early `return`, a statement `if` or a loop, a `var` reassigned after
    /// its initializer, or a call of a callable that is not itself builtin
    /// (an ordinary function, a construction by a non-builtin `__init__`,
    /// `//`, `%`, or `**` over `Int`, a `FloatLiteral` operand converted to
    /// `Float64`).
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
    /// parameter no overload of which is builtin (`self.ord()`), an
    /// operator over a non-literal operand whose `SIMD` dunder is not
    /// builtin (`//`, `%`, or `**`: `SIMD.__floordiv__`, `__mod__`, and
    /// `__pow__`), or arithmetic or a comparison converting a
    /// `FloatLiteral` operand.
    fn non_builtin_operation(
        &self,
        declaration: &ApplicableDeclaration<'_>,
        statement: &Stmt,
    ) -> Option<TypeError> {
        struct Operations<'a> {
            checker: &'a Checker,
            /// The names of the body's operands typed `Self`.
            receivers: Vec<String>,
            /// The methods of the struct declaring the body.
            methods: &'a [mojito_ast::ast::Method],
            /// The name of the struct declaring the body, which `Self`
            /// constructs.
            owner: Option<&'a str>,
            /// The names of the body's operands typed `FloatLiteral`.
            float_literals: Vec<String>,
            found: bool,
        }
        impl Operations<'_> {
            /// Whether `operand` is a `FloatLiteral`: a literal, or an
            /// operand of that type.
            fn float_literal(&self, operand: &Expr) -> bool {
                match &operand.kind {
                    ExprKind::Float(_) => true,
                    ExprKind::Identifier(name) => self.float_literals.contains(name),
                    _ => false,
                }
            }

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
                self.receivers.contains(name)
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
                        op @ (InfixOp::FloorDiv | InfixOp::Mod | InfixOp::Pow),
                        left,
                        right,
                    ) => {
                        !(matches!(left.kind, ExprKind::Int(_))
                            && matches!(right.kind, ExprKind::Int(_)))
                            && op
                                .dunder()
                                .is_none_or(|dunder| !self.checker.simd_dunder_is_builtin(dunder))
                    }
                    ExprKind::Infix(
                        InfixOp::Add
                        | InfixOp::Sub
                        | InfixOp::Mul
                        | InfixOp::Div
                        | InfixOp::Eq
                        | InfixOp::Ne
                        | InfixOp::Lt
                        | InfixOp::Gt
                        | InfixOp::Le
                        | InfixOp::Ge,
                        left,
                        right,
                    ) => self.float_literal(left) != self.float_literal(right),
                    ExprKind::Call { name, .. } => self.checker.calls_ordinary_function(
                        self.owner.filter(|_| name == "Self").unwrap_or(name),
                    ),
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
        let (methods, owner) = match declaration.owner.map(|owner| &owner.kind) {
            Some(StmtKind::Struct { methods, name, .. }) => {
                (methods.as_slice(), Some(name.as_str()))
            }
            _ => (&[][..], None),
        };
        let slots = declaration
            .function
            .argument_slots(self.self_ty.as_ref())
            .unwrap_or_default();
        let named = |typed: &dyn Fn(&Ty) -> bool| {
            slots
                .iter()
                .filter(|slot| slot_ty(slot).is_some_and(typed))
                .map(|slot| slot.name().to_string())
                .collect()
        };
        let mut operations = Operations {
            checker: self,
            receivers: self
                .self_ty
                .as_ref()
                .map(|self_ty| named(&|ty| ty == self_ty))
                .unwrap_or_default(),
            methods,
            owner,
            float_literals: named(&Ty::is_float_literal),
            found: false,
        };
        mojito_ast::visit::walk_stmt(&mut operations, statement);
        operations.found.then(non_builtin_call)
    }

    /// Whether `SIMD` declares its `method` dunder builtin, as an operator
    /// over a non-literal operand calls it.
    fn simd_dunder_is_builtin(&self, method: &str) -> bool {
        self.applicable_functions
            .get(&format!("SIMD.{method}"))
            .is_some_and(|overloads| {
                overloads
                    .iter()
                    .any(|function| function.inline == InlineLevel::Builtin)
            })
    }

    /// Whether a call of `method` on `object` applies a static method no
    /// overload of which is builtin, its struct spelled by name, by
    /// application, or as `Self`.
    fn calls_ordinary_static_method(&self, object: &Expr, method: &str) -> bool {
        self.applied_owner(object, method)
            .is_some_and(|(owner, _)| self.calls_ordinary_function(&format!("{owner}.{method}")))
    }

    /// Whether a call spelled `name` applies a callable no overload of which
    /// is builtin: a function, or a construction by a struct's `__init__`
    /// (a synthesized fieldwise one is never builtin).
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
        let slots = declaration.function.argument_slots(Some(self_ty))?;
        if slots.iter().all(|slot| slot_ty(slot) != Some(self_ty)) {
            return Some(());
        }
        let fields = self.declared_fields(declaration)?;
        for slot in &slots {
            if slot_ty(slot) != Some(self_ty) {
                continue;
            }
            let name = slot.name();
            let operand = super::annotations::value_binder_expr(slot.id().clone(), name, self_ty);
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
            let value = self.param_context.construction(&read, &operand);
            self.local_comptime_parameters
                .last_mut()?
                .insert((*name).to_string(), (level, value));
        }
        Some(())
    }

    /// The instance an initializer `declaration` constructs and its fields,
    /// in order, each with its type there: a refined initializer's `out
    /// self` annotation (`out self: Int` on `SIMD`), whose fields are the
    /// instance's, and otherwise `Self` with the declared fields.
    fn constructed_fields(
        &self,
        declaration: &ApplicableDeclaration<'_>,
    ) -> Option<(Ty, Vec<(String, Ty)>)> {
        let declared = self.declared_fields(declaration)?;
        let Some(refined) = declaration.function.refined_self() else {
            return Some((self.self_ty.clone()?, declared));
        };
        let refined = self.ty_from_anno(refined).ok()?;
        if let Some(storage) = mojito_types::types::simd_storage(&refined) {
            let fields = declared
                .into_iter()
                .map(|(name, _)| (name, storage.clone()));
            return Some((refined, fields.collect()));
        }
        let (owner, arguments) = mojito_types::types::nominal_parts(&refined)?;
        let owner = mojito_symbol::symbol::specialization_template(owner).unwrap_or(owner);
        let decls = &self.structs.get(owner)?.decls;
        let fields = mojito_types::types::struct_field_types(decls, &arguments, &declared)?;
        Some((refined.clone(), fields.into_owned()))
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
    pub(super) fn in_owner_scope<T>(
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
    /// earlier one, `self = e` stores every field of `e`, and a read of a stored
    /// field (`self.b = self.a + 1`) is the value stored. A docstring, a `var` or `comptime` binding, and a
    /// `comptime assert` may stand among the stores. `None` for any other
    /// statement (an augmented store, a store into a field's field), or a
    /// field the body leaves unstored.
    fn folded_construction(
        &mut self,
        declaration: &ApplicableDeclaration<'_>,
        level: usize,
    ) -> Option<Vec<(String, ParamExpr)>> {
        let self_ty = self.self_ty.clone()?;
        let (constructed, declared) = self.constructed_fields(declaration)?;
        let symbol = declaration.function.symbol();
        let mut slots: Vec<ParamExpr> = declaration
            .function
            .argument_slots(Some(&self_ty))?
            .iter()
            .map(|slot| match slot {
                ParamDecl::Value { id, name, ty, .. } => {
                    Some(super::annotations::value_binder_expr(id.clone(), name, ty))
                }
                ParamDecl::Type { .. } => super::annotations::type_parameter(slot)
                    .map(|ty| self.param_context.type_shape(ty)),
            })
            .collect::<Option<_>>()?;
        if !self.enclosing_type_params.is_empty() {
            slots.insert(0, self.param_context.type_shape(self_ty));
        }
        let spelling = self
            .param_context
            .apply(symbol, &slots, MetaTy::value(constructed.clone()));
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
            if let StmtKind::Assign { name, value, .. } = &statement.kind
                && name == "self"
            {
                let whole = self.builtin_value(value, &constructed)?;
                for ((field, field_ty), slot) in declared.iter().zip(&mut stored) {
                    *slot = Some(
                        self.param_context
                            .field(&whole, field, MetaTy::value(field_ty.clone()))
                            .ok()?,
                    );
                }
                continue;
            }
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
    /// `level`, or a `comptime assert` or a call of a builtin function with
    /// no result (`_simd_construction_checks[Self.dtype, Self.length]()`),
    /// which the pin's fold drops (a closed assert is the body check's to
    /// judge). `None` for any other statement.
    fn folded_leading_statement(&mut self, statement: &Stmt, level: usize) -> Option<()> {
        match &statement.kind {
            StmtKind::Pass
            | StmtKind::Expr(Expr {
                kind: ExprKind::Str(_),
                ..
            })
            | StmtKind::ComptimeAssert { .. } => Some(()),
            StmtKind::Expr(Expr {
                kind: ExprKind::Call { name, .. },
                ..
            }) if self.builtin_procedures.contains(name)
                && self.binding_scope(name).is_none_or(|scope| scope == 0) =>
            {
                Some(())
            }
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
    /// closed one materialized at it. A closed value of type `!kgen.simd`
    /// is the `SIMD` value whose storage it is, as the parameter domain
    /// holds a vector's storage, and a symbolic one is typed at the
    /// storage.
    fn builtin_value(&self, value: &Expr, ty: &Ty) -> Option<ParamExpr> {
        let vector = mojito_types::types::storage_value_ty(ty);
        let value = self.builtin_operand(value)?;
        let value = match value.as_constant() {
            Some(closed) => self
                .param_context
                .typed_constant(closed.clone().materialize_as(&vector)?, ty)
                .ok()?,
            None => value,
        };
        value
            .meta()
            .as_value()
            .is_some_and(|found| found == ty)
            .then_some(value)
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
/// `Int(mlir_value=i)._mlir_value`, the result type its `_type=` names being the one
/// the check gave the op; at the `bool` pair they are
/// `Bool(mlir_value=s).__mlir_i1__()` and `Bool(i).__mlir_bool__()`.
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
    let mut typed = false;
    let mut boolean = false;
    for argument in param_args {
        match argument {
            mojito_ast::ast::ParamArg::Named { name, value } if name == "pred" => {
                predicate = Some(super::method_calls::mlir_attribute(value)?);
            }
            // The `ui8` pair has no operator sugar to fold through.
            mojito_ast::ast::ParamArg::Named { name, value }
                if name == "_type" && names_builtin_pair(value, &["ui8", "!kgen.scalar<ui8>"]) =>
            {
                return None;
            }
            mojito_ast::ast::ParamArg::Named { name, value } if name == "_type" => {
                typed = true;
                boolean = names_builtin_pair(value, &["i1", "!kgen.scalar<bool>"]);
            }
            _ => return None,
        }
    }
    let at_call = |kind: ExprKind| {
        let mut call = Expr::new(kind, expr.span);
        call.source.clone_from(&expr.source);
        call
    };
    let method_call = |object: Expr, method: &str| ExprKind::MethodCall {
        object: Box::new(object),
        method: method.to_string(),
        args: Vec::new(),
        kwargs: Vec::new(),
    };
    let storage_call = |name: &str, value: &Expr| ExprKind::Call {
        name: name.to_string(),
        param_args: Vec::new(),
        args: Vec::new(),
        kwargs: vec![mojito_ast::ast::KwArg {
            name: "mlir_value".to_string(),
            value: value.clone(),
        }],
    };
    let op = MlirOp::parse(name, predicate).ok()?;
    let kind = match (op.lowering(), args.as_slice()) {
        (MlirLowering::BuiltinCast, [operand]) if boolean && op == MlirOp::PopCastToBuiltin => {
            method_call(at_call(storage_call("Bool", operand)), "__mlir_i1__")
        }
        (MlirLowering::BuiltinCast, [operand]) if boolean => method_call(
            at_call(ExprKind::Call {
                name: "Bool".to_string(),
                param_args: Vec::new(),
                args: vec![operand.clone()],
                kwargs: Vec::new(),
            }),
            "__mlir_bool__",
        ),
        (MlirLowering::BuiltinCast, [operand]) if typed && op == MlirOp::PopCastToBuiltin => {
            method_call(operand.clone(), "__mlir_index__")
        }
        (MlirLowering::BuiltinCast, [operand]) if typed => ExprKind::Member {
            object: Box::new(at_call(storage_call("Int", operand))),
            field: mojito_types::ct::MLIR_VALUE_FIELD.to_string(),
        },
        _ if typed => return None,
        // `pop.cmp` is elementwise, where an infix comparison is the
        // reducing dunder: [`mask_op`] reads it.
        _ if matches!(op, MlirOp::PopCmp(_)) => return None,
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

/// The op and operands of ``__mlir_op.`pop.cmp`[pred=p](a, b)`` or
/// ``__mlir_op.`pop.simd.reduce_and`(m)`` (or `reduce_or`), the lane
/// compares and mask reductions `SIMD`'s comparison bodies fold over, as
/// `ParamContext::simd_cmp` and `simd_reduce`. `None` for any other
/// expression.
pub(super) fn mask_op(expr: &Expr) -> Option<(MlirOp, &[Expr])> {
    let (object, name, param_args, args) = match &expr.kind {
        ExprKind::MethodCall {
            object,
            method,
            args,
            kwargs,
        } if kwargs.is_empty() => (&**object, method.as_str(), [].as_slice(), args),
        ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } if kwargs.is_empty() => match &callee.kind {
            ExprKind::Member { object, field } => {
                (&**object, field.as_str(), param_args.as_slice(), args)
            }
            _ => return None,
        },
        _ => return None,
    };
    if !matches!(&object.kind, ExprKind::Identifier(op) if op == "__mlir_op") {
        return None;
    }
    let predicate = match param_args {
        [] => None,
        [mojito_ast::ast::ParamArg::Named { name, value }] if name == "pred" => {
            Some(super::method_calls::mlir_attribute(value)?)
        }
        _ => return None,
    };
    let op = MlirOp::parse(name, predicate).ok()?;
    match (op, args.as_slice()) {
        (MlirOp::PopCmp(_), [_, _]) | (MlirOp::PopSimdReduceAnd | MlirOp::PopSimdReduceOr, [_]) => {
            Some((op, args))
        }
        _ => None,
    }
}

/// The op, `_type=` attribute, and operand of ``__mlir_op.`pop.simd.splat`[_type=T](x)``
/// or ``__mlir_op.`pop.cast`[_type=T](x)``, the typed ops a builtin
/// `SIMD` initializer folds over, as `ParamOp::SimdSplat` and
/// `ParamOp::Cast`. `None` for any other expression.
pub(super) fn typed_lane_op(expr: &Expr) -> Option<(ParamOp, &mojito_ast::ast::ParamArg, &Expr)> {
    let ExprKind::Invoke {
        callee,
        param_args,
        args,
        kwargs,
    } = &expr.kind
    else {
        return None;
    };
    let ExprKind::Member { object, field } = &callee.kind else {
        return None;
    };
    let op = match field.as_str() {
        "pop.simd.splat" => ParamOp::SimdSplat,
        "pop.cast" => ParamOp::Cast,
        _ => return None,
    };
    match (&object.kind, param_args.as_slice(), args.as_slice()) {
        (
            ExprKind::Identifier(base),
            [mojito_ast::ast::ParamArg::Named { name, value }],
            [operand],
        ) if base == "__mlir_op" && name == "_type" && kwargs.is_empty() => {
            Some((op, value.as_ref(), operand))
        }
        _ => None,
    }
}

/// Whether a builtin cast's `_type=` names one side of `pair`, the MLIR
/// spellings of a builtin and its storage (`i1` and `!kgen.scalar<bool>`).
fn names_builtin_pair(argument: &mojito_ast::ast::ParamArg, pair: &[&str]) -> bool {
    let named = |name: &str| pair.contains(&name);
    match argument {
        mojito_ast::ast::ParamArg::Type(ty) => {
            super::comptime_validation::mlir_source_name(ty).is_some_and(named)
        }
        mojito_ast::ast::ParamArg::Value(Expr {
            kind: ExprKind::Member { object, field },
            ..
        }) => {
            matches!(&object.kind, ExprKind::Identifier(base) if base == "__mlir_type")
                && named(field)
        }
        _ => false,
    }
}

/// The module functions `stmts` declare every declaration of which is
/// `@always_inline("builtin")` with no result
/// ([`Checker::builtin_procedures`]).
fn builtin_procedures(stmts: &[Stmt]) -> HashSet<String> {
    let mut procedures: HashMap<&str, bool> = HashMap::new();
    for statement in stmts {
        if let StmtKind::Def {
            name,
            decorators,
            ret,
            ..
        } = &statement.kind
        {
            let procedure =
                ret.is_none() && matches!(InlineLevel::of(decorators), Ok(InlineLevel::Builtin));
            *procedures.entry(name).or_insert(true) &= procedure;
        }
    }
    procedures
        .into_iter()
        .filter(|&(_, procedure)| procedure)
        .map(|(name, _)| name.to_string())
        .collect()
}

/// The names a call of the callable family `name` (`f`, `G.f`,
/// `G.__init__`) spells: its own last segment, and a constructor's struct.
fn folded_spellings(name: &str) -> Vec<String> {
    let mut segments = name.rsplit('.');
    let own = segments.next().unwrap_or(name);
    let owner = segments.next().filter(|_| own == "__init__");
    std::iter::once(own)
        .chain(owner)
        .map(str::to_string)
        .collect()
}

/// The names the calls and operators of `body` spell
/// ([`folded_spellings`]): a function's, a method's, and an operator's
/// dunders.
fn call_spellings(body: &[Stmt]) -> Vec<String> {
    #[derive(Default)]
    struct Spellings(Vec<String>);
    impl mojito_ast::visit::Visitor for Spellings {
        fn visit_expr(&mut self, expr: &Expr) {
            match &expr.kind {
                ExprKind::Call { name, .. } => self.0.push(name.clone()),
                ExprKind::MethodCall { method, .. } => self.0.push(method.clone()),
                ExprKind::Invoke { callee, .. } => {
                    if let ExprKind::Member { field, .. } = &callee.kind {
                        self.0.push(field.clone());
                    }
                }
                ExprKind::Infix(op, ..) => self.0.extend(
                    op.dunder()
                        .into_iter()
                        .chain(op.reflected_dunder())
                        .map(str::to_string),
                ),
                _ => {}
            }
        }
    }
    let mut spellings = Spellings::default();
    mojito_ast::visit::walk_block(&mut spellings, body);
    spellings.0
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

/// The type of an argument slot ([`ApplicableFunction::argument_slots`])
/// holding a value; `None` for a type slot.
///
/// [`ApplicableFunction::argument_slots`]: comptime_validation::ApplicableFunction::argument_slots
const fn slot_ty(slot: &ParamDecl) -> Option<&Ty> {
    match slot {
        ParamDecl::Value { ty, .. } => Some(ty),
        ParamDecl::Type { .. } => None,
    }
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

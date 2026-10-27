//! The elaboration walk: every declaration and body, rebuilt for the check.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

/// The type packs a body names, under the spelling the body reads them
/// by: `Ts` for a `def`'s own pack, `Self.Ts` for its struct's. `None` at
/// module scope.
pub(super) type Packs<'p> = Option<&'p HashSet<String>>;

/// Elaborate the statements of a block whose type packs are `packs`.
pub(super) fn block(stmts: &[Stmt], packs: Packs<'_>) -> Vec<Stmt> {
    let mut out = Vec::new();
    // The pack elements the template's aliases denote: their source
    // spellings, and their parameter expressions as written.
    let mut element_sources: HashMap<String, Type> = HashMap::new();
    let mut element_aliases: HashMap<String, Expr> = HashMap::new();
    for stmt in stmts {
        // `comptime T = Ts[i]` over a template's own type pack is the
        // dependent element it denotes: later statements spell it
        // `Ts[i]`, and the elaborator below MIR closes it per instance.
        if let Some(packs) = packs
            && let Some((name, element)) =
                pack_element_alias(&stmt.kind, &|base| packs.contains(base))
        {
            if let StmtKind::Comptime { value, .. } = &stmt.kind {
                element_aliases.insert(name.clone(), (*value).clone());
            }
            element_sources.insert(name, element);
            // The check binds the alias too, so a read of it in an
            // expression (`F == Int`) is the element it names.
            out.push(stmt.clone());
            continue;
        }
        let first_new = out.len();
        let resolved = contextual_comptime(stmt);
        out.push(elaborated_stmt(resolved.as_ref().unwrap_or(stmt), packs));
        if !element_sources.is_empty() {
            substitute_type_bindings_in_block(&mut out[first_new..], &|name| {
                element_sources.get(name).cloned()
            });
        }
        if !element_aliases.is_empty() {
            spell_aliases_in_conditions(&mut out[first_new..], &element_aliases);
        }
        if let Some(source) = stmt.module.as_deref() {
            mojito_ast::ast::stamp_source(&mut out[first_new..], source);
        }
    }
    out
}

/// One statement with every body it holds elaborated. A `comptime`
/// declaration of any scope, and a `comptime if`, `comptime for`, or
/// `comptime match`, is the check's as written: it binds every parameter
/// expression, and the elaborator below MIR evaluates each one.
fn elaborated_stmt(stmt: &Stmt, packs: Packs<'_>) -> Stmt {
    let kind = match &stmt.kind {
        // A module-scope `comptime if`, `comptime for`, or `comptime match`
        // has no declaration form: it passes through for the parser or
        // `validate_module_scope` to reject.
        StmtKind::ComptimeIf { branches, orelse } if packs.is_some() => StmtKind::ComptimeIf {
            branches: branches
                .iter()
                .map(|(cond, body)| (cond.clone(), block(body, packs)))
                .collect(),
            orelse: opt_block(orelse.as_deref(), packs),
        },
        StmtKind::ComptimeFor { var, iter, body } if packs.is_some() => StmtKind::ComptimeFor {
            var: var.clone(),
            iter: iter.clone(),
            body: block(body, packs),
        },
        StmtKind::ComptimeMatch { subject, cases } if packs.is_some() => StmtKind::ComptimeMatch {
            subject: subject.clone(),
            cases: match_cases(cases, packs),
        },
        StmtKind::If { branches, orelse } => StmtKind::If {
            branches: branches
                .iter()
                .map(|(cond, body)| (cond.clone(), block(body, packs)))
                .collect(),
            orelse: opt_block(orelse.as_deref(), packs),
        },
        StmtKind::While { cond, body, orelse } => StmtKind::While {
            cond: cond.clone(),
            body: block(body, packs),
            orelse: opt_block(orelse.as_deref(), packs),
        },
        // A `match` is checked whole; its case bodies elaborate as a
        // branch's do.
        StmtKind::Match { subject, cases } => StmtKind::Match {
            subject: subject.clone(),
            cases: match_cases(cases, packs),
        },
        StmtKind::For {
            target,
            iter,
            body,
            orelse,
        } => StmtKind::For {
            target: target.clone(),
            iter: iter.clone(),
            body: block(body, packs),
            orelse: opt_block(orelse.as_deref(), packs),
        },
        StmtKind::Try {
            body,
            except,
            orelse,
            finalbody,
        } => StmtKind::Try {
            body: block(body, packs),
            except: except
                .as_ref()
                .map(|(name, body)| (name.clone(), block(body, packs))),
            orelse: opt_block(orelse.as_deref(), packs),
            finalbody: opt_block(finalbody.as_deref(), packs),
        },
        // The context-manager protocol is a checker desugar (the manager
        // type selects the `__enter__`/`__exit__` shape); elaborate the
        // body and keep the statement, like `While`.
        StmtKind::With { items, body } => StmtKind::With {
            items: items.clone(),
            body: block(body, packs),
        },
        StmtKind::Def {
            type_params, body, ..
        } => {
            let mut kind = stmt.kind.clone();
            if let StmtKind::Def {
                body: elaborated, ..
            } = &mut kind
            {
                *elaborated = def_body(&[], type_params, body);
            }
            kind
        }
        // The struct keeps every declaration part but its methods'
        // elaborated bodies.
        StmtKind::Struct {
            type_params,
            methods,
            ..
        } => {
            let methods = methods
                .iter()
                .map(|method| {
                    let mut method = method.clone();
                    method.body = def_body(type_params, &method.type_params, &method.body);
                    method
                })
                .collect();
            let mut kind = stmt.kind.clone();
            if let StmtKind::Struct {
                methods: elaborated,
                ..
            } = &mut kind
            {
                *elaborated = methods;
            }
            kind
        }
        // A default is its trait's member, a function over the trait's
        // `Self` (`_Self`): its body is elaborated in place as that
        // function's.
        StmtKind::Trait { methods, .. } => {
            let elaborated: Vec<_> = methods
                .iter()
                .enumerate()
                .map(|(index, method)| {
                    let member = mojito_checker::checker::trait_member_declaration(
                        stmt,
                        index,
                        method.name.clone(),
                    );
                    match member.as_ref().map(|member| &member.kind) {
                        Some(StmtKind::Def {
                            type_params, body, ..
                        }) => Some(def_body(&[], type_params, body)),
                        _ => None,
                    }
                })
                .collect();
            let mut kind = stmt.kind.clone();
            if let StmtKind::Trait { methods, .. } = &mut kind {
                for (method, body) in methods.iter_mut().zip(elaborated) {
                    if body.is_some() {
                        method.default_body = body;
                    }
                }
            }
            kind
        }
        _ => return stmt.clone(),
    };
    rebuilt(stmt, kind)
}

/// The cases of a `match` or `comptime match`, their bodies elaborated.
fn match_cases(
    cases: &[mojito_ast::ast::MatchCase],
    packs: Packs<'_>,
) -> Vec<mojito_ast::ast::MatchCase> {
    cases
        .iter()
        .map(|case| mojito_ast::ast::MatchCase {
            pattern: case.pattern.clone(),
            guard: case.guard.clone(),
            body: block(&case.body, packs),
            span: case.span,
        })
        .collect()
}

fn opt_block(block_stmts: Option<&[Stmt]>, packs: Packs<'_>) -> Option<Vec<Stmt>> {
    block_stmts.map(|body| block(body, packs))
}

/// Elaborate a `def`'s or a method's body. A method's struct type packs
/// (`struct_params`) are packs of its body as `Self.`-qualified names
/// only, which is how a method reads them.
fn def_body(struct_params: &[TypeParam], type_params: &[TypeParam], body: &[Stmt]) -> Vec<Stmt> {
    let packs: HashSet<String> = type_packs(type_params)
        .map(|parameter| parameter.name.trim_start_matches('*').to_string())
        .chain(type_packs(struct_params).map(|parameter| self_qualified(&parameter.name)))
        .collect();
    block(body, Some(&packs))
}

/// Spell each pack-element alias a kept `comptime if` condition names
/// (`comptime if T == Int` under `comptime T = Self.Ts[i]`) as the element
/// it denotes, which the check types with the pack symbolic.
fn spell_aliases_in_conditions(statements: &mut [Stmt], aliases: &HashMap<String, Expr>) {
    struct Spell<'a>(&'a HashMap<String, Expr>);

    impl mojito_ast::visit::MutVisitor for Spell<'_> {
        fn visit_expr_mut(&mut self, expr: &mut Expr) {
            if let ExprKind::Identifier(name) = &expr.kind
                && let Some(element) = self.0.get(name)
            {
                expr.kind = element.kind.clone();
            }
        }
    }

    struct Conditions<'a>(&'a HashMap<String, Expr>);

    impl mojito_ast::visit::MutVisitor for Conditions<'_> {
        fn visit_stmt_mut(&mut self, statement: &mut Stmt) {
            if let StmtKind::ComptimeIf { branches, .. } = &mut statement.kind {
                for (condition, _) in branches {
                    mojito_ast::visit::walk_condition_mut(&mut Spell(self.0), condition);
                }
            }
        }
    }

    mojito_ast::visit::walk_block_mut(&mut Conditions(aliases), statements);
}

/// The type packs among `parameters`: a variadic parameter the check does
/// not classify as a value.
fn type_packs(parameters: &[TypeParam]) -> impl Iterator<Item = &TypeParam> {
    parameters.iter().filter(|parameter| {
        parameter.name.starts_with('*')
            && !matches!(
                super::params::classify_ct_param(parameter, parameters, ""),
                Some(ParamDecl::Value { .. })
            )
    })
}

/// The spelling a method body reads its struct's parameter `name` by.
fn self_qualified(name: &str) -> String {
    format!("Self.{}", name.trim_start_matches('*'))
}

/// A `comptime` binding whose leading-dot initializer (`comptime shape: Shape
/// = .rect(2, 5)`) names a member of its annotated type, as it does in a typed
/// `var`, with that type spelled; `None` for any other statement.
fn contextual_comptime(stmt: &Stmt) -> Option<Stmt> {
    let StmtKind::Comptime {
        ty: Some(Type::Named(base, args)),
        value,
        ..
    } = &stmt.kind
    else {
        return None;
    };
    let resolved = with_contextual_base(value, base, args)?;
    let mut rewritten = stmt.clone();
    if let StmtKind::Comptime { value, .. } = &mut rewritten.kind {
        *value = resolved;
    }
    Some(rewritten)
}

/// `expression` with its leading-dot root (`$contextual`) named as the type
/// `base[args]`; `None` when it has no such root.
fn with_contextual_base(expression: &Expr, base: &str, args: &[ParamArg]) -> Option<Expr> {
    fn replace(expression: &mut Expr, root: &ExprKind) -> bool {
        match &mut expression.kind {
            ExprKind::Identifier(name) if name == mojito_ast::ast::CONTEXTUAL_SENTINEL => {
                expression.kind = root.clone();
                true
            }
            ExprKind::Member { object, .. }
            | ExprKind::MethodCall { object, .. }
            | ExprKind::Index { object, .. } => replace(object, root),
            ExprKind::Invoke { callee, .. } => replace(callee, root),
            _ => false,
        }
    }
    let root = if args.is_empty() {
        ExprKind::Identifier(base.to_string())
    } else {
        ExprKind::TypeApply {
            name: base.to_string(),
            args: args.to_vec(),
        }
    };
    let mut resolved = expression.clone();
    replace(&mut resolved, &root).then_some(resolved)
}

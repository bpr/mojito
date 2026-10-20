//! A `comptime for` over any iterable: upstream's iterator protocol. The iterable is spelled as its
//! `__iter__()` call, as upstream's `parseParamFor` emits it, and the loop
//! steps that iterator through the bundled `std.builtin._stubs` functions,
//! named by module path as upstream's `getBuiltinFunction` names them.

#[allow(clippy::wildcard_imports, reason = "page of one split module")]
use super::*;

impl Checker {
    /// Check a `comptime for` over `iter` by upstream's iterator protocol:
    /// the loop's binder is the iterator `iter.__iter__()` starts, and the
    /// variable is an alias of `paramfor_next_value` over it, which each
    /// body copy materializes. The body is checked once, as any `comptime
    /// for` body is. `source` is the iterable as written, which the spelled
    /// call stands in for and the loop header's record keys on.
    pub(super) fn check_comptime_iterator_for(
        &mut self,
        stmt: &Stmt,
        var: &str,
        (source, iter): (&Expr, &Expr),
        body: &[Stmt],
        ret: Option<&Ty>,
    ) -> Result<(), TypeError> {
        let (call, iterator_ty, element_ty) = {
            let _position = self.comptime_position();
            self.comptime_iterator_protocol(source, iter)?
        };
        let binder = comptime_iterator_binder(source, &iterator_ty);
        let reference = binder.as_decl_ref().cloned().ok_or_else(|| {
            TypeError::InvariantViolation("a comptime for iterator binder is no reference".into())
        })?;
        let key = reference.id.owner.to_string();
        let iterator_shape = self.param_context.type_shape(iterator_ty.clone());
        let stub = |name: &str, meta| {
            self.param_context.apply(
                &builtin_stub(name),
                &[iterator_shape.clone(), binder.clone()],
                meta,
            )
        };
        let has_next = stub(
            "paramfor_has_next",
            mojito_types::param_expr::MetaTy::value(Ty::bool()),
        );
        let next_iter = stub(
            "paramfor_next_iter",
            mojito_types::param_expr::MetaTy::value(iterator_ty.clone()),
        );
        let element = stub(
            "paramfor_next_value",
            mojito_types::param_expr::MetaTy::value(element_ty.clone()),
        );
        self.record_spelled_construction(source, call, &iterator_ty);
        self.record_at(source.source_span()).comptime_iteration =
            Some(Box::new(mojito_checked::checked::ComptimeIteration {
                binder: reference,
                iterator: mojito_types::param_expr::MetaTy::value(iterator_ty),
                has_next,
                next_iter,
                next_value: element.clone(),
            }));
        let before = self.uninitialized.borrow().clone();
        self.push_scope();
        if let Some(bindings) = self.compile_time_bindings.last_mut() {
            bindings.insert(var.to_string());
        }
        let level = self.tparams.len().saturating_sub(1);
        if let Some(scope) = self.local_comptime_parameters.last_mut() {
            scope.insert(var.to_string(), (level, element));
        }
        // The iterator is a binder in scope, which a function lifted in the
        // body over the variable takes; no spelling names it.
        if let Some(scope) = self.innermost_value_scope() {
            scope.insert(key.clone(), binder);
        }
        let result = self.declare_immutable(var, element_ty).and_then(|()| {
            self.record_statement_binding(stmt, var);
            self.check_block(body, ret, true)
        });
        if let Some(scope) = self.innermost_value_scope() {
            scope.remove(&key);
        }
        self.pop_scope();
        *self.uninitialized.borrow_mut() = before;
        result
    }

    /// Type upstream's desugaring of a `comptime for` over `iter`: the
    /// spelled call `iter.__iter__()`, the iterator type it gives, and the
    /// element type `paramfor_next_value` over that iterator gives, typed
    /// once per iterator type. The stub calls are typed in a probe, so a
    /// bound the iterator fails (an iterator that is not `Copyable`) is the
    /// call's own rejection and no fact of theirs survives.
    fn comptime_iterator_protocol(
        &self,
        source: &Expr,
        iter: &Expr,
    ) -> Result<(Expr, Ty, Ty), TypeError> {
        let ty = self.infer(iter)?;
        let has_iter = match ty.kind() {
            TyKind::Struct(name, _) => self
                .structs
                .get(name)
                .is_some_and(|info| info.methods.contains_key("__iter__")),
            // A homogeneous variadic is upstream's `VariadicList`, which
            // iterates; read at run time, it is dynamic.
            TyKind::VariadicPack(_) => true,
            _ => false,
        };
        if !has_iter {
            return Err(TypeError::Unsupported(format!(
                "'{}' does not implement the '__iter__' method",
                iterable_spelling(&ty)
            )));
        }
        if !self.reads_compile_time_alone(iter) {
            return Err(TypeError::Unsupported(
                "cannot use a dynamic value in 'for' iterator expression".to_string(),
            ));
        }
        // The receiver is the iterable under identities derived from the
        // call's, so the iterable's own occurrence can stand for the call.
        let mut object = iter.clone();
        mojito_ast::visit::walk_expr_mut(
            &mut mojito_ast::visit::DerivedIdentities {
                parent: mojito_common::token::SyntaxId::derived(source.syntax_id, 0),
                next: 1,
            },
            &mut object,
        );
        let call = initializer_list::spelled_node(
            source,
            0,
            ExprKind::MethodCall {
                object: Box::new(object),
                method: "__iter__".to_string(),
                args: Vec::new(),
                kwargs: Vec::new(),
            },
            source.span,
        );
        let iterator_ty = self.infer(&call)?;
        if let Some(element_ty) = self
            .comptime_iterator_elements
            .borrow()
            .get(&iterator_ty)
            .cloned()
        {
            return Ok((call, iterator_ty, element_ty));
        }
        let entry = builtin_stub("paramfor_next_value");
        if self.lookup(&entry).is_none() {
            return Err(TypeError::Unsupported(format!(
                "a 'comptime for' over '{ty}' needs the bundled '{STUBS_MODULE}' module"
            )));
        }
        let element_ty = self
            .probe(|| {
                [
                    "paramfor_has_next",
                    "paramfor_next_iter",
                    "paramfor_next_value",
                ]
                .iter()
                .enumerate()
                .map(|(ordinal, name)| {
                    let stub = initializer_list::spelled_node(
                        source,
                        u32::try_from(ordinal).unwrap_or(u32::MAX).saturating_add(1),
                        ExprKind::Call {
                            name: builtin_stub(name),
                            param_args: Vec::new(),
                            args: vec![call.clone()],
                            kwargs: Vec::new(),
                        },
                        source.span,
                    );
                    self.infer(&stub)
                })
                .collect::<Result<Vec<_>, _>>()
            })?
            .pop()
            .ok_or_else(|| TypeError::InvariantViolation("no paramfor stub was typed".into()))?;
        self.comptime_iterator_elements
            .borrow_mut()
            .insert(iterator_ty.clone(), element_ty.clone());
        Ok((call, iterator_ty, element_ty))
    }
}

/// The compile-time binder of a `comptime for` iterator of type `iterator`,
/// upstream's mangled `iter` parameter: the loop's iterable names it, as it
/// names an element binder.
fn comptime_iterator_binder(iter: &Expr, iterator: &Ty) -> ParamExpr {
    value_binder_expr(
        ParamId::new(&comptime_validation::comptime_for_owner(iter), 0),
        "$iter",
        iterator,
    )
}

/// The iterable a not-iterable rejection names: a compile-time tuple spells
/// its element types as the elaborator does for the same program, any other
/// type its own spelling.
fn iterable_spelling(ty: &Ty) -> String {
    let elements: Vec<Ty> = match ty.kind() {
        TyKind::Tuple(elements) | TyKind::RuntimePack(elements) => elements.clone(),
        TyKind::Struct(name, args) if name == mojito_types::types::TUPLE_TYPE_NAME => args
            .iter()
            .filter_map(|argument| match argument {
                TyArg::Ty(element) => Some(element.clone()),
                TyArg::Val(_) | TyArg::Origin(_) => None,
            })
            .collect(),
        _ => return ty.to_string(),
    };
    format!(
        "Tuple[{}]",
        elements
            .iter()
            .map(materialized_element_spelling)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// The element spelling of a materialized compile-time tuple: a literal
/// element is named by the type it materializes to.
fn materialized_element_spelling(ty: &Ty) -> String {
    match ty.kind() {
        TyKind::IntLiteral => "Int".to_string(),
        TyKind::StringLiteral => "String".to_string(),
        TyKind::FloatLiteral => "Float64".to_string(),
        _ => mojito_types::types::unqualified_type_name(ty),
    }
}

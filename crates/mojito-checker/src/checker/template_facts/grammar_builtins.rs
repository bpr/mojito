//! Certificate grammar for checker builtins and calls through a bound:
//! `print`, `repr`, hasher sinks, and bound dispatch.

use super::{BodyShape, fact_at};
use mojito_ast::ast::{Expr, ExprKind};
use mojito_checked::templates::{BoundBuiltin, MethodFeatures, OccurrenceId};
use mojito_types::types::{ParamDecl, Ty};

impl BodyShape<'_> {
    /// `repr(value)` and `_unqualified_type_name[T]()`: checker builtins that
    /// make a string and select no callee.
    ///
    /// The reflection call names one type and records its spelling as an
    /// adjustment, which an instance re-renders from the substituted type.
    /// `repr` reads its argument where it lies, as a sink's argument is read,
    /// and wraps its compile-time string result as the nominal `String`: a
    /// conversion the instance selects again ([`Checker::realize_conversion`]),
    /// owing that the argument is still `Writable`
    /// ([`Checker::realize_repr_call`]). A declaration of either name would
    /// record call parameters and a binding, and is not this.
    pub(super) fn string_builtin(
        &self,
        id: OccurrenceId,
        name: &str,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> bool {
        let declared = self.facts.is_some_and(|facts| {
            fact_at(&facts.call_parameters, id).is_some()
                || fact_at(&facts.expression_bindings, id).is_some()
        });
        if declared || !kwargs.is_empty() {
            return false;
        }
        let repr = name == "repr";
        let admitted = if repr {
            param_args.is_empty() && matches!(args, [argument] if self.sink_argument(argument))
        } else {
            param_args.len() == 1 && args.is_empty()
        };
        if admitted && repr {
            let mut calls = self.repr_calls.borrow_mut();
            if !calls.contains(&id) {
                calls.push(id);
            }
            // `repr` reads its argument where it lies, as a `ref` parameter
            // would: the place use is the grammar's, not a stray one.
            let argument = self.occurrence(&args[0]);
            let mut places = self.places.borrow_mut();
            if !places.contains(&argument) {
                places.push(argument);
            }
        }
        admitted && self.holds(MethodFeatures::STRING_BUILTINS)
    }

    /// `print(...)` as a statement: a checker builtin that selects no
    /// callee, over closed scalars, pack elements, string literals,
    /// arguments it reads where they lie, as `repr` does
    /// ([`Self::sink_argument`], under `STRING_BUILTINS`), and call results
    /// of any type ([`Self::call_result`]), which it leaves unconsumed. Its
    /// `sep`, `end`, `flush`, and `file` keywords take closed values, string
    /// literals, values it reads where they lie, or temporaries of a closed
    /// type ([`Self::whole_value`]); any other keyword is an error the
    /// template already reported.
    ///
    /// What the builtin records at an argument its syntax decides (an
    /// unconsumed temporary, a literal's materialization); what it proves,
    /// that the argument is `Writable`, and whether a named place is read in
    /// place at a nominal type, the instance decides again at its own type
    /// ([`Checker::realize_print_call`]). A declaration of that name
    /// would record call parameters and a binding, and is not this.
    pub(super) fn print_call(&self, expr: &Expr) -> bool {
        let ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } = &expr.kind
        else {
            return false;
        };
        let id = self.occurrence(expr);
        let declared = self.facts.is_some_and(|facts| {
            fact_at(&facts.call_parameters, id).is_some()
                || fact_at(&facts.expression_bindings, id).is_some()
        });
        if name != "print" || declared || !param_args.is_empty() {
            return false;
        }
        let mut sinks = false;
        let keywords = kwargs.iter().all(|keyword| {
            let value = &keyword.value;
            let closed = match value.kind {
                ExprKind::Str(_) => self.facts.is_none_or(|facts| {
                    fact_at(&facts.expression_types, self.occurrence(value))
                        == Some(&Ty::StringLiteral)
                }),
                _ => self.expression(value) && self.scalar(value),
            };
            // A temporary's type is the keyword's own, which mentions no
            // parameter, so every instance builds and drops the same one.
            let sink = !closed
                && (self.sink_argument(value) || (self.whole_value(value) && self.closed(value)));
            sinks |= sink;
            matches!(keyword.name.as_str(), "sep" | "end" | "flush" | "file") && (closed || sink)
        });
        let admitted = keywords
            && args.iter().all(|argument| {
                let closed = if let ExprKind::Str(_) = argument.kind {
                    self.facts.is_none_or(|facts| {
                        fact_at(&facts.expression_types, self.occurrence(argument))
                            == Some(&Ty::StringLiteral)
                    })
                } else {
                    (self.expression(argument) && self.scalar(argument))
                        || self.pack_element(argument)
                };
                closed || {
                    let sink = self.sink_argument(argument)
                        || self.call_result(argument)
                        || (self.element_construction(argument)
                            && self.holds(MethodFeatures::ELEMENT_CONSTRUCTIONS));
                    sinks |= sink;
                    sink
                }
            });
        if admitted {
            let mut calls = self.print_calls.borrow_mut();
            if !calls.contains(&id) {
                calls.push(id);
            }
        }
        admitted && (!sinks || self.holds(MethodFeatures::STRING_BUILTINS))
    }

    /// A method call on a place whose type is a bare struct parameter, which
    /// the template proves through the parameter's bound and an instance
    /// re-selects on its own type ([`Checker::realize_bound_dispatch`]).
    /// The place may be the element a reference call yields
    /// (`self.items[j].copy()`), which the call borrows through that
    /// reference.
    ///
    /// The receiver is a place, so the template recorded at the call either
    /// the abstract contract (`__trait_dispatch.…`) or, for `write_to`, the
    /// inverted write, and nothing that depends on the receiver's type. Each
    /// argument, positional or keyword, is a closed scalar; a value handed
    /// by value, bound as a direct call's is ([`Self::argument`]) at a
    /// parameter of its own recorded type, which every witness declares
    /// alike or, typed `Self` or by a requirement binder, becomes the same
    /// type as the argument's under every instance; or a named
    /// place of a bare parameter type handed to a bounded `mut`/`ref`
    /// parameter of the requirement, whose facts (a kept place, its
    /// generation refresh) the convention decides. A parameter the call
    /// leaves out takes the requirement's default, which every witness
    /// declares alike or the check spells at the call.
    /// An instance's own check reads a built-in receiver's place, feeds a
    /// leaf to the hasher, or selects the struct's own method, each from the
    /// type alone.
    pub(super) fn bound_dispatch(
        &self,
        expr: &Expr,
        object: &Expr,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> bool {
        // A receiver a `var self` requirement consumes is a named place's
        // `^` transfer, which records the move at itself, or a named place
        // the call copies first, which the call marks.
        let transferred = matches!(&object.kind, ExprKind::Transfer(inner)
            if matches!(inner.kind, ExprKind::Identifier(_)))
            && self.whole_value(object);
        let copied = !transferred
            && self.facts.is_some_and(|facts| {
                facts
                    .implicitly_copied_consuming_receivers
                    .contains(&self.occurrence(expr))
            });
        let consumed = transferred || copied;
        let place = match &object.kind {
            ExprKind::Identifier(name) => {
                self.params.contains(&name.as_str()) || self.local_kind(name).is_some()
            }
            ExprKind::Member { .. } => self.receiver_field(object) || self.reference_member(object),
            ExprKind::Index { .. } => self.slot(object) || self.reference_receiver(object),
            ExprKind::Call { .. } => self.reference_receiver(object),
            _ => transferred,
        };
        // The receiver itself names a place too, as a `mut self` hasher
        // handed to its value's `__hash__`.
        let receiver_argument = |argument: &Expr| {
            self.receiver
                && self.self_convention == Some(mojito_ast::ast::ArgConvention::Mut)
                && matches!(&argument.kind, ExprKind::Identifier(name) if name == "self")
        };
        let named = |argument: &Expr| {
            receiver_argument(argument)
                || matches!(&argument.kind, ExprKind::Identifier(name)
                    if self.params.contains(&name.as_str()) || self.declared(name))
        };
        let supplied = || args.iter().chain(kwargs.iter().map(|kwarg| &kwarg.value));
        let shape = !self.keyed
            && place
            && supplied().all(|argument| {
                (self.expression(argument) && self.scalar(argument))
                    || named(argument)
                    || self.argument(expr, argument)
            });
        let admitted = shape
            && self.facts.is_none_or(|facts| {
                let id = self.occurrence(expr);
                // A pack element an index-keyed accessor reads is the
                // dependent `Ts[i]`, dispatched through the pack's bounds as a
                // view binder the result names ([`Self::pack_accessor`]).
                let element = fact_at(&facts.expression_types, self.occurrence(object))
                    .filter(|ty| matches!(ty, Ty::Dependent(_)) && self.pack_accessor(object));
                let Some(receiver) = element.or_else(|| {
                    fact_at(&facts.expression_types, self.occurrence(object))
                        .filter(|ty| matches!(ty, Ty::Param { .. }))
                }) else {
                    return false;
                };
                let viewed =
                    |result: &Ty| element.is_some_and(|element| views_element(result, element));
                // A named place handed to a bounded parameter of the
                // requirement: its own bounds prove the parameter's, or its
                // type is closed, the same in every instance, and the
                // template's check proved it against the bound. The receiver
                // is an instance of the struct whose declared conformances
                // proved it, which every specialization declares alike.
                let bounded = |argument: &Expr, parameter: &Ty| {
                    let ty = fact_at(&facts.expression_types, self.occurrence(argument));
                    if receiver_argument(argument) {
                        return matches!((ty, parameter), (Some(Ty::Struct(..)), Ty::Param { .. }));
                    }
                    match (ty, parameter) {
                        (Some(Ty::Param { bounds: given, .. }), Ty::Param { bounds, .. }) => {
                            bounds.iter().all(|bound| given.contains(bound))
                        }
                        (Some(ty), Ty::Param { .. }) => !mojito_types::types::is_symbolic(ty),
                        _ => false,
                    }
                };
                let Some(call) = fact_at(&facts.selected_calls, id) else {
                    // The inverted write: one writer, a bounded place the
                    // call only names.
                    let inverted =
                        fact_at(&facts.operation_adjustments, id).is_some_and(|adjustment| {
                            matches!(
                                adjustment,
                                mojito_checked::checked::SemanticAdjustment::InvertedWrite
                                    | mojito_checked::checked::SemanticAdjustment::InvertedReprWrite
                            )
                        });
                    let writer = Ty::Param {
                        binder: crate::checker::annotations::synthetic_binder("$writer_probe"),
                        bounds: vec!["Writer".to_string()],
                        callable_bound: None,
                    };
                    return inverted
                        && matches!(args, [argument]
                            if named(argument)
                                && bounded(argument, &writer)
                                && !facts.call_place_uses.contains(&self.occurrence(argument)))
                        && !facts.overload_targets.iter().any(|(site, _)| *site == id)
                        && !facts.call_parameters.iter().any(|(site, _)| *site == id);
                };
                let kept_argument = |parameter: &mojito_checked::checked::CheckedCallArgument| {
                    // A default the requirement declares, which every witness
                    // declares alike and evaluates in its own scope.
                    if parameter.source
                        == mojito_checked::checked::CheckedCallArgumentSource::Default
                    {
                        return !parameter.requires_place;
                    }
                    let Some(bound) = call
                        .arguments
                        .iter()
                        .find(|bound| bound.source == parameter.source)
                    else {
                        return false;
                    };
                    let Some(argument) =
                        supplied().find(|argument| self.occurrence(argument) == bound.value)
                    else {
                        return false;
                    };
                    // A by-value argument of the parameter's own type binds
                    // as a direct call's does ([`Self::argument`]): a closed
                    // type every witness declares alike, or `Self` or a
                    // requirement binder, the argument's type in every
                    // instance.
                    if !parameter.requires_place {
                        return (self.expression(argument) && self.scalar(argument))
                            || self.argument(expr, argument);
                    }
                    let kept = named(argument)
                        && bounded(argument, &parameter.parameter_ty)
                        && facts.call_place_uses.contains(&bound.value);
                    if kept {
                        let mut places = self.places.borrow_mut();
                        if !places.contains(&bound.value) {
                            places.push(bound.value);
                        }
                    }
                    kept
                };
                // A result typed by a requirement binder is the type the
                // binder was inferred from: a by-value argument's, which
                // `kept_argument` asks to be the parameter's own, so the
                // witness's result under every instance is the argument's
                // type there, whose copy, move, or drop the instance proves.
                let binder_result = |result: &Ty| {
                    !call.contract.param_decls.is_empty()
                        && call.contract.arguments.iter().any(|parameter| {
                            !parameter.requires_place
                                && parameter.source
                                    != mojito_checked::checked::CheckedCallArgumentSource::Default
                                && parameter.parameter_ty == *result
                        })
                };
                // A by-value parameter of a type other than a closed scalar
                // is judged at its argument (`kept_argument`). The
                // requirement's own type binders, inferred from the
                // arguments, are replaced by the witness's, which an instance
                // binds from its own argument types
                // ([`Checker::realize_bound_dispatch`]).
                let requirement_binders = call.contract.param_decls.iter().all(|decl| {
                    matches!(
                        decl,
                        ParamDecl::Type {
                            default: None,
                            variadic: false,
                            ..
                        }
                    )
                });
                let judged = if call.contract.param_decls.is_empty() {
                    std::borrow::Cow::Borrowed(call)
                } else {
                    let mut unbound = call.clone();
                    unbound.contract.param_decls.clear();
                    std::borrow::Cow::Owned(unbound)
                };
                let contract = if consumed {
                    mojito_checked::templates::consuming_nominal_contract(&judged)
                } else {
                    mojito_checked::templates::value_method_contract(&judged)
                };
                mojito_symbol::symbol::is_trait_dispatch_symbol(&call.contract.target)
                    && requirement_binders
                    && contract
                    && match call.contract.receiver_convention {
                        None => {
                            !call.contract.receiver_requires_place && call.invalidations.is_empty()
                        }
                        // A `mut self` requirement keeps the receiver's place
                        // and refreshes its generation, below the receiver's
                        // own binding, whatever the witness.
                        Some(mojito_ast::ast::ArgConvention::Mut) => {
                            call.contract.receiver_requires_place
                        }
                        Some(
                            mojito_ast::ast::ArgConvention::Var
                            | mojito_ast::ast::ArgConvention::Deinit,
                        ) => consumed && call.invalidations.is_empty(),
                        Some(_) => false,
                    }
                    && (call.contract.result_ty == *receiver
                        || viewed(&call.contract.result_ty)
                        || !mojito_types::types::is_symbolic(&call.contract.result_ty)
                        || binder_result(&call.contract.result_ty))
                    && call
                        .contract
                        .arguments
                        .iter()
                        .filter(|parameter| {
                            parameter.source
                                != mojito_checked::checked::CheckedCallArgumentSource::Default
                        })
                        .count()
                        == args.len() + kwargs.len()
                    && call.contract.arguments.iter().all(kept_argument)
            });
        admitted
            && self.holds(MethodFeatures::BOUND_DISPATCH)
            && (!copied || self.holds(MethodFeatures::COPIED_RECEIVERS))
    }

    /// `hasher.update(value)`, `hasher._update_with_simd(value)`, or
    /// `writer.write(values…)` on a parameter bounded by `Hasher` or
    /// `Writer`: a checker builtin that selects no callee
    /// ([`Checker::realize_bound_builtin`]).
    ///
    /// The template records the receiver's place and, at each argument, only
    /// what its syntax decides: a borrow of a named place or a reference
    /// result, an unconsumed temporary, a literal's materialization. The
    /// argument's type it proved through the bound, which the instance proves
    /// again at its own type.
    pub(super) fn bound_builtin(
        &self,
        expr: &Expr,
        object: &Expr,
        method: &str,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> bool {
        let (builtin, bound) = match (method, args.len()) {
            ("update", 1) => (BoundBuiltin::Update, "Hasher"),
            ("_update_with_simd", 1) => (BoundBuiltin::UpdateSimd, "Hasher"),
            ("write", 1..) => (BoundBuiltin::Write, "Writer"),
            ("finish", 0) => (BoundBuiltin::Finish, "Hasher"),
            _ => return false,
        };
        // `finish` consumes its hasher, which the `^` transfer records at
        // itself. Only the method's own binder stays a builtin receiver in
        // every instance: a struct binder's hasher selects its own method.
        let consumed = builtin == BoundBuiltin::Finish;
        let receiver = if consumed {
            matches!(&object.kind, ExprKind::Transfer(inner)
                if matches!(inner.kind, ExprKind::Identifier(_)))
                && self.whole_value(object)
        } else {
            matches!(&object.kind, ExprKind::Identifier(name)
                if self.params.contains(&name.as_str()) || self.local_kind(name).is_some())
        };
        let id = self.occurrence(expr);
        let admitted = !self.keyed
            && receiver
            && kwargs.is_empty()
            && args.iter().all(|argument| self.sink_argument(argument))
            && self.facts.is_none_or(|facts| {
                matches!(fact_at(&facts.expression_types, self.occurrence(object)),
                    Some(Ty::Param { binder, bounds, .. })
                        if bounds.iter().any(|carried| carried == bound)
                            && !(consumed && self.struct_binders.contains(&&binder.id)))
                    && !facts.selected_calls.iter().any(|(site, _)| *site == id)
                    && !facts.overload_targets.iter().any(|(site, _)| *site == id)
                    && !facts.call_parameters.iter().any(|(site, _)| *site == id)
                    && !facts
                        .operation_adjustments
                        .iter()
                        .any(|(site, _)| *site == id)
            });
        if admitted && self.facts.is_some() {
            let mut builtins = self.bound_builtins.borrow_mut();
            if !builtins.iter().any(|(site, _)| *site == id) {
                builtins.push((id, builtin));
            }
        }
        admitted && self.holds(MethodFeatures::BOUND_BUILTINS)
    }

    /// An argument a checker builtin reads where it lies: a closed scalar, a
    /// string literal, a named whole value, a `ref` local, a field read
    /// through a reference, a pointer slot, a reference call, or another
    /// string builtin's result. Each records by its syntax alone.
    fn sink_argument(&self, argument: &Expr) -> bool {
        match &argument.kind {
            ExprKind::Str(_) => true,
            ExprKind::Call { name, .. } => {
                self.simd_value(argument)
                    || (self.expression(argument)
                        && (self.scalar(argument)
                            || name == "repr"
                            || name == "_unqualified_type_name"))
            }
            ExprKind::Identifier(name) => {
                self.params.contains(&name.as_str()) || self.local_kind(name).is_some()
            }
            ExprKind::Member { .. } => {
                self.receiver_field(argument)
                    || self.reference_member(argument)
                    || (self.struct_value(argument) && self.expression(argument))
            }
            ExprKind::Index { .. } => {
                self.slot(argument)
                    || self.reference_call(argument)
                    || self.lent_pack_element(argument)
            }
            ExprKind::MethodCall { .. } => {
                self.reference_call(argument)
                    || (self.expression(argument) && self.scalar(argument))
            }
            _ => self.expression(argument) && self.scalar(argument),
        }
    }
}

/// Whether `result` is the bounded view binder standing for the dependent
/// pack element `element` (`$view:Ts[i]`), which a call through the
/// element's bound yields.
fn views_element(result: &Ty, element: &Ty) -> bool {
    matches!((result, element), (Ty::Param { binder, .. }, Ty::Dependent(dependent))
    if binder.id.owner.strip_prefix("$view:").is_some_and(|name| {
        name.ends_with(&dependent.expr().to_string())
    }))
}

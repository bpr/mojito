//! Certificate grammar for calls: methods, statics, siblings, generic
//! module functions, callables, and the arguments they take.

use super::{BodyShape, closed_differences, fact_at, names_method, push_unique};
use crate::checker::annotations::existential_binder;
use mojito_ast::ast::{Expr, ExprKind};
use mojito_checked::templates::{
    CheckedBodyFacts, MethodFeatures, OccurrenceId, TemplateCallContract,
};
use mojito_types::types::Ty;
use std::collections::HashMap;

impl BodyShape<'_> {
    /// The result of a sibling call, of a generic callee's explicit
    /// application, or of a function body's direct call, of any type: a
    /// temporary, whose type is the contract's or the application's
    /// substituted result.
    pub(super) fn call_result(&self, expr: &Expr) -> bool {
        let call = match &expr.kind {
            ExprKind::MethodCall { method, .. } => {
                !matches!(method.as_str(), "unsafe_take_pointee" | "unsafe_offset")
            }
            ExprKind::Invoke { callee, .. } => matches!(callee.kind, ExprKind::Member { .. }),
            ExprKind::MultiIndex { .. } => true,
            // A function body's direct call of a module-scope function,
            // whose application an instance realizes again
            // (`realize_direct_call`); a method body's take closed scalars.
            ExprKind::Call {
                name, param_args, ..
            } if param_args.is_empty()
                && !self.receiver
                && !self.keyed
                && self.moved_result.is_some()
                && !self.structs.contains_key(name)
                && self.local_kind(name).is_none() =>
            {
                true
            }
            ExprKind::Call { param_args, .. } => !param_args.is_empty(),
            _ => false,
        };
        call && self.expression(expr)
    }

    /// A call of a method on `self`, on one of its fields, on a `var` local,
    /// through a reference, or on a call's temporary result
    /// (`self.entries().size()`), passing admitted arguments, whose recorded
    /// contract changes per instance only in its target and its substituted
    /// result ([`Self::sibling_call`]). A temporary receiver's read and its
    /// destruction are recorded at the call's occurrence, by syntax.
    pub(super) fn method_call(
        &self,
        expr: &Expr,
        object: &Expr,
        method: &str,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> bool {
        let on_self = matches!(&object.kind, ExprKind::Identifier(name) if name == "self");
        ((self.receiver && (on_self || self.receiver_field(object)))
            || (!self.keyed
                && (self.reference_receiver(object)
                    || self.value_local(object)
                    || self.local_field(object)
                    || self.parameter_receiver(object)
                    || self.parameter_field(object)
                    || self.call_result(object))))
            && (!self.keyed || (args.is_empty() && kwargs.is_empty()))
            && args
                .iter()
                .chain(kwargs.iter().map(|keyword| &keyword.value))
                .all(|argument| self.argument(expr, argument))
            && self
                .facts
                .is_none_or(|facts| self.sibling_call(facts, expr, object, method))
    }

    /// A method call spelled with explicit compile-time arguments,
    /// `receiver.method[3](x)`, on a receiver [`Self::method_call`] admits,
    /// or a generic struct's static on its spelled type application
    /// (`Pair[Self.T].scaled[3](2)`, [`Self::static_call`]).
    ///
    /// The receiver's recorded type is a struct, so the method and the
    /// compile-time parameters it declares (`ParameterizedMethodCalls`) are
    /// selected from the struct's own declaration, alike under every
    /// instance. The arguments are literals or types, and the call retargets
    /// to the per-call clone its `MethodInstantiation` requests; an instance
    /// whose substitution would change that request refuses
    /// (`realize_instance_facts`), so a type argument naming a struct
    /// parameter keeps the clone check. Before the per-call clone exists the
    /// contract still carries the declared parameters, which
    /// [`Self::sibling_call`] refuses.
    pub(super) fn parameterized_call(
        &self,
        expr: &Expr,
        callee: &Expr,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> bool {
        use mojito_ast::ast::ParamArg;
        let ExprKind::Member { object, field } = &callee.kind else {
            return false;
        };
        let literal = |argument: &ParamArg| match argument {
            ParamArg::Type(_) => true,
            ParamArg::Value(value) => match &value.kind {
                ExprKind::Int(_) | ExprKind::Bool(_) | ExprKind::Float(_) => true,
                ExprKind::Identifier(name) => self.closed_type_argument(value, name),
                _ => false,
            },
            ParamArg::Named { .. } => false,
        };
        let id = self.occurrence(expr);
        let admitted = !self.keyed
            && !param_args.is_empty()
            && param_args.iter().all(literal)
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.parameterized_method_calls, id).is_some()
                    && fact_at(&facts.method_instantiations, id).is_some()
            })
            && (self.method_call(expr, object, field, args, kwargs)
                || self.static_call(expr, object, field, args, kwargs, true));
        if admitted {
            for argument in param_args {
                if let ParamArg::Value(
                    value @ Expr {
                        kind: ExprKind::Identifier(_),
                        ..
                    },
                ) = argument
                {
                    push_unique(
                        &mut self.type_arguments.borrow_mut(),
                        self.occurrence(value),
                    );
                }
            }
        }
        admitted && self.holds(MethodFeatures::PARAMETERIZED_CALLS)
    }

    /// A static method of a struct called on its type: `Color.of(n)`,
    /// `Pair[Self.T].twice(v)`, `Pair.twice(v)`, or `.twice(v)` where the
    /// expected type resolved the leading-dot root to such a struct
    /// (`ContextualBases`).
    ///
    /// The receiver is a type, so the call records no contract, no call
    /// parameters, and no application; it records at most the overload
    /// member the closed arguments ranked, and a contextual root records the
    /// head of the expected struct type, which is the same under every
    /// instance. An expected type that is a bare parameter refuses the
    /// leading-dot form outright.
    ///
    /// Each member of a generic struct's static takes its parameters by
    /// value, `var`, `mut`, or `ref` with an inferred origin, or in a
    /// positional pack, and returns no reference. An availability condition
    /// the template called it under was proved with the struct's parameters
    /// symbolic, so every instance meets it. A `mut` or `ref` argument is a
    /// named place kept as the caller's (`CallPlaceUses`); a lone static's
    /// read-only pack reads a named place where it lies. A lone static may
    /// declare binders of its own, as may a member of a family called on a
    /// spelled receiver, and the call may spell its compile-time arguments
    /// (`parameterized`, from [`Self::parameterized_call`]) on a spelled
    /// receiver: an instance calls the static's template, unless it declares
    /// its own clone of it (`realize_static_instantiations`).
    /// On an inferred or contextual receiver the members differ only in
    /// closed parameter types, so the call ranks the same member whatever
    /// solves the struct's parameters; on a spelled receiver every instance
    /// keeps the template's member (`realize_static_overloads`). The
    /// struct's parameters are solved from the receiver's `[...]` type arguments, or from the
    /// arguments' types, and an instance solves them at the substituted
    /// types, as its struct application is substituted. Where the instance's
    /// struct has a clone of the static the call retargets to it by the
    /// receiver's arguments: a lone declaration records nothing that names
    /// it, and an overloaded member's recorded target is re-keyed to the
    /// clone of that member (`realize_static_overloads`). An argument is a
    /// closed scalar or a whole value bound to a parameter of its own type
    /// ([`Self::static_argument`]).
    pub(super) fn static_call(
        &self,
        expr: &Expr,
        object: &Expr,
        method: &str,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
        parameterized: bool,
    ) -> bool {
        use mojito_ast::ast::{ArgConvention, ParamArg};
        let (spelled, applied) = match &object.kind {
            ExprKind::Identifier(spelled) => (spelled, &[][..]),
            ExprKind::TypeApply { name, args } => (name, args.as_slice()),
            _ => return false,
        };
        let contextual = spelled == mojito_ast::ast::CONTEXTUAL_SENTINEL;
        let base = if contextual {
            match self.facts {
                Some(facts) => match fact_at(&facts.contextual_bases, self.occurrence(object)) {
                    Some(base) => base.as_str(),
                    None => return false,
                },
                None => spelled.as_str(),
            }
        } else {
            spelled.as_str()
        };
        let info = self.structs.get(base);
        let generic = info.is_some_and(|info| !info.decls.is_empty());
        let derivable = |sig: &crate::checker::MethodSig| {
            !sig.has_self
                && sig.kw_variadic.is_none()
                && sig.ref_return.is_none()
                && sig
                    .ref_params
                    .iter()
                    .flatten()
                    .all(|reference| reference.origin == mojito_types::origin::SigOrigin::Infer)
                && sig.view_return.is_empty()
                && sig.conventions.iter().all(|convention| {
                    matches!(
                        convention,
                        None | Some(ArgConvention::Var | ArgConvention::Mut | ArgConvention::Ref)
                    )
                })
        };
        let static_member = info
            .and_then(|info| {
                let receiver = if generic {
                    applied
                        .iter()
                        .all(|argument| matches!(argument, ParamArg::Type(_)))
                } else {
                    applied.is_empty()
                };
                receiver.then(|| info.methods.get(method)).flatten()
            })
            .is_some_and(|signatures| {
                // A spelled receiver's instance keeps the template's member
                // (`realize_static_overloads`); any other ranks alike under
                // every instance only where the members differ in closed
                // parameter types.
                if generic {
                    signatures.iter().all(derivable)
                        && (signatures.len() == 1
                            || !applied.is_empty()
                            || signatures.iter().all(|sig| sig.decls.is_empty()))
                        && (!applied.is_empty()
                            || (!parameterized && closed_differences(signatures)))
                } else {
                    !parameterized && signatures.iter().all(|sig| !sig.has_self)
                }
            });
        // A lone static's read-only pack reads each named place it is
        // handed where it lies, recording nothing at the argument.
        let pack_from = info
            .and_then(|info| info.methods.get(method))
            .and_then(|family| match family.as_slice() {
                [sig] if sig.variadic_convention.is_none() => sig.variadic_index,
                _ => None,
            });
        let shadowed =
            self.local_kind(spelled).is_some() || self.params.contains(&spelled.as_str());
        let id = self.occurrence(expr);
        let admitted = !self.keyed
            && !shadowed
            && (static_member || (contextual && self.facts.is_none()))
            && kwargs.is_empty()
            && args.iter().enumerate().all(|(position, argument)| {
                let kept = self.facts.is_some_and(|facts| {
                    facts.call_place_uses.contains(&self.occurrence(argument))
                });
                let packed = pack_from.is_some_and(|from| position >= from);
                if kept {
                    self.static_argument(argument, packed)
                } else if matches!(argument.kind, ExprKind::Str(_)) {
                    self.static_literal(argument)
                } else {
                    (self.expression(argument) && self.scalar(argument))
                        || ((generic || self.facts.is_none())
                            && self.static_argument(argument, packed))
                }
            })
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.expression_types, id)
                    .is_some_and(|ty| generic || !mojito_types::types::is_symbolic(ty))
                    && fact_at(&facts.call_parameters, id).is_none()
                    && fact_at(&facts.selected_calls, id).is_none()
                    && fact_at(&facts.generic_instantiations, id).is_none()
                    && (generic || fact_at(&facts.method_instantiations, id).is_none())
                    && fact_at(&facts.parameterized_method_calls, id).is_some() == parameterized
                    && args.iter().all(|argument| {
                        matches!(argument.kind, ExprKind::Str(_))
                            || fact_at(&facts.conversions, self.occurrence(argument)).is_none()
                    })
            });
        if admitted {
            self.static_calls
                .borrow_mut()
                .push((id, format!("{base}.{method}")));
        }
        admitted && self.holds(MethodFeatures::STATIC_CALLS)
    }

    /// A method call on the `^` transfer of a named place the body owns, on
    /// a nominal struct, whose callee consumes the receiver: `var self`, or
    /// a named `deinit self` destructor (`entry^.reap_value()`).
    ///
    /// The transfer records the move at the receiver and owes `Movable` per
    /// instance, as every transfer does; the contract changes per instance
    /// only in its target and its substituted types
    /// (`consuming_nominal_contract`). The explicit-destroy mark the call
    /// records depends on which methods the receiver's struct declares with
    /// `deinit self`, which its arguments do not change.
    pub(super) fn consuming_call(
        &self,
        expr: &Expr,
        object: &Expr,
        method: &str,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> bool {
        let ExprKind::Transfer(inner) = &object.kind else {
            return self.copied_consuming_call(expr, object, method, args, kwargs);
        };
        !self.keyed
            && matches!(
                inner.kind,
                ExprKind::Identifier(_) | ExprKind::Member { .. }
            )
            && self.whole_value(object)
            && args
                .iter()
                .chain(kwargs.iter().map(|keyword| &keyword.value))
                .all(|argument| self.argument(expr, argument))
            && self.facts.is_none_or(|facts| {
                self.named_contract(facts, expr, inner, method)
                    .is_some_and(mojito_checked::templates::consuming_nominal_contract)
            })
            && self.holds(MethodFeatures::CONSUMING_CALLS)
    }

    /// The contract the call at `expr` recorded, when it names `method` on
    /// the struct `object` has.
    pub(super) fn named_contract<'f>(
        &self,
        facts: &'f CheckedBodyFacts,
        expr: &Expr,
        object: &Expr,
        method: &str,
    ) -> Option<&'f TemplateCallContract> {
        let Some(Ty::Struct(owner, _)) = fact_at(&facts.expression_types, self.occurrence(object))
        else {
            return None;
        };
        fact_at(&facts.selected_calls, self.occurrence(expr))
            .filter(|call| names_method(&call.contract.target, owner, method))
    }

    /// One argument of a method call: a closed scalar bound by value, a whole
    /// value of any type or a string literal bound by value, or a place the
    /// call keeps for a `mut` or bare `ref` parameter.
    ///
    /// A whole value binds a parameter of exactly its own type, or one an
    /// `@implicit` constructor converts it to, which the instance selects
    /// again at its own source and target types
    /// ([`Checker::realize_conversion`]) and writes back into the boundary
    /// that names it ([`realize_boundary_conversions`]). What the call
    /// records for it is decided without its type: a read parameter borrows
    /// a named place and reads a temporary, by the argument's syntax and the
    /// callee's conventions, and a `var` parameter takes a `^` transfer or a
    /// temporary as it stands. A place copied into a `var` parameter is
    /// admitted only where the template recorded the copy, which the
    /// instance owes again at its own type, as a transfer owes `Movable`. A
    /// reference is read, copied, or kept as a named place is
    /// ([`Self::reference_argument`]).
    ///
    /// A kept place is a local, a parameter, or a field of `self`, of exactly
    /// the parameter's type, so nothing converts it. Which arguments a call
    /// keeps is the callee's declared convention, and whether two of them
    /// conflict is judged on their places, so neither changes per instance.
    /// A field of `self` is kept only beside a receiver the call reads.
    ///
    /// A parameter type may mention a struct parameter. The callee has no
    /// binders of its own, so the call recorded its parameter types at the
    /// receiver's arguments, in the caller's binder scope, and an instance
    /// substitutes them in the contract and in the call's parameters alike.
    /// A parameter typed by the callee's existential `Some[…]` binder stays
    /// in the callee's scope under every instance: a whole value binds it
    /// when its caller binder's bounds carry or refine the existential's,
    /// which the instance's request discharged ([`existential_argument`]).
    pub(super) fn argument(&self, call: &Expr, argument: &Expr) -> bool {
        let named = match &argument.kind {
            ExprKind::Identifier(name) => {
                self.declared(name) || self.params.contains(&name.as_str())
            }
            _ => self.receiver_field(argument),
        };
        let literal = matches!(argument.kind, ExprKind::Str(_));
        let Some(facts) = self.facts else {
            return self.expression(argument)
                || (!self.keyed && (named || literal || self.whole_value(argument)));
        };
        let id = self.occurrence(argument);
        // An in-place update's contract is kept at its place, where no call
        // is selected.
        let contract = fact_at(&facts.selected_calls, self.occurrence(call))
            .or_else(|| fact_at(&facts.inplace_updates, self.occurrence(call)));
        let parameter = contract.and_then(|call| {
            let bound = call.arguments.iter().find(|bound| bound.value == id)?;
            call.contract
                .arguments
                .iter()
                .find(|parameter| parameter.source == bound.source)
        });
        if !facts.call_place_uses.contains(&id) {
            let by_value = parameter.is_none_or(|parameter| !parameter.requires_place);
            if (self.expression(argument) && self.scalar(argument)) || self.simd_value(argument) {
                return by_value;
            }
            // A whole value of any type, bound by value to a parameter of
            // exactly its own type: moved, a temporary, copied where the
            // template recorded the copy, or read where it lies, which the
            // call's conventions and the argument's syntax decide alone.
            let read_in_place = facts.borrowed_read_call_places.contains(&id)
                && (named || self.reference_argument(argument));
            // A reference copied into a `var` parameter, where the template
            // recorded the copy.
            let copied_reference =
                facts.copy_place_value_uses.contains(&id) && self.reference_argument(argument);
            // A value the boundary converts stands for its own type: the
            // conversion is kept beside the boundary, and both are
            // re-selected per instance.
            let converted = converted_argument(facts, contract, id);
            // A string literal is a temporary of its own closed type, bound
            // to a parameter of that type or converted into it.
            if literal {
                return !self.keyed
                    && by_value
                    && parameter.is_some_and(|parameter| {
                        converted || parameter.parameter_ty == Ty::StringLiteral
                    })
                    && fact_at(&facts.expression_types, id) == Some(&Ty::StringLiteral)
                    && self.holds(MethodFeatures::VALUE_ARGUMENTS);
            }
            return !self.keyed
                && by_value
                && parameter.is_some_and(|parameter| {
                    converted
                        || existential_argument(
                            self.traits,
                            &parameter.parameter_ty,
                            fact_at(&facts.expression_types, id),
                        )
                        || fact_at(&facts.expression_types, id) == Some(&parameter.parameter_ty)
                })
                && (read_in_place || copied_reference || self.whole_value(argument))
                && self.holds(MethodFeatures::VALUE_ARGUMENTS);
        }
        let read_receiver = contract.is_some_and(|call| {
            matches!(
                call.contract.receiver_convention,
                None | Some(mojito_ast::ast::ArgConvention::Imm)
            )
        });
        // A place reached through a reference may lie within `self`, as a
        // field of `self` does.
        let through = !named && self.reference_argument(argument);
        let admitted = !self.keyed
            && (named || through)
            && (read_receiver || !(through || self.receiver_field(argument)))
            && parameter.is_some_and(|parameter| {
                mojito_checked::templates::kept_place_argument(parameter)
                    && fact_at(&facts.expression_types, id) == Some(&parameter.parameter_ty)
            });
        if admitted {
            let mut places = self.places.borrow_mut();
            if !places.contains(&id) {
                places.push(id);
            }
        }
        admitted && self.holds(MethodFeatures::PLACE_ARGUMENTS)
    }

    /// A reference handed on as an argument: a `ref` local, a field reached
    /// through a reference, or a reference call's result.
    ///
    /// Like a receiver reached through a reference
    /// ([`Self::reference_receiver`]), what the call records for it is
    /// decided by what the argument is and never by its type: a read
    /// parameter borrows the place it names, a field read keeps its base as
    /// a handle, and a reference call records its own result, which an
    /// instance marks a copyable read again at its own referent.
    pub(super) fn reference_argument(&self, argument: &Expr) -> bool {
        let admitted = match &argument.kind {
            ExprKind::Identifier(name) => self.reference_local(name),
            ExprKind::Member { .. } => self.reference_member(argument),
            _ => self.reference_call(argument),
        };
        admitted && self.holds(MethodFeatures::REFERENCE_ARGUMENTS)
    }

    /// A named place a method body hands to a generic module function's
    /// read parameter (`hash(e)`, `hash(self.value)`), which the call reads
    /// where it lies.
    ///
    /// The template bound the callee's binder to the place's symbolic type
    /// and selected the callee once; whether the call borrows the place is
    /// decided by its syntax and the read convention, so an instance keeps
    /// both and substitutes only the application (`method_direct_calls`).
    pub(super) fn generic_call_place(&self, call: OccurrenceId, argument: &Expr) -> bool {
        let named = match &argument.kind {
            ExprKind::Identifier(name) => {
                self.declared(name)
                    || self.params.contains(&name.as_str())
                    || self.reference_local(name)
            }
            _ => self.receiver_field(argument),
        };
        let id = self.occurrence(argument);
        named
            && !self.keyed
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.generic_instantiations, call).is_some()
                    && facts.borrowed_read_call_places.contains(&id)
                    && fact_at(&facts.conversions, id).is_none()
            })
    }

    /// A whole value handed by value to a direct call in a function body
    /// (`pick(kept)`): a `^` transfer, a place the template copied, or a
    /// named place the call reads where it lies, with no conversion recorded
    /// at it.
    ///
    /// The callee is selected once, and its parameter's type lives in the
    /// callee's own binder scope: the argument bound it exactly with the
    /// function's parameter symbolic, so it binds the substituted application
    /// exactly too (`realize_direct_call`). The copy and the transfer are
    /// owed again per instance; a method body's direct calls take closed
    /// scalars only (`method_direct_calls`).
    pub(super) fn direct_call_value(&self, argument: &Expr) -> bool {
        if self.receiver || self.keyed || self.moved_result.is_none() {
            return false;
        }
        let id = self.occurrence(argument);
        let named = matches!(&argument.kind, ExprKind::Identifier(name)
            if self.declared(name) || self.params.contains(&name.as_str()));
        let read_in_place = named
            && self
                .facts
                .is_some_and(|facts| facts.borrowed_read_call_places.contains(&id));
        let unconverted = self
            .facts
            .is_none_or(|facts| fact_at(&facts.conversions, id).is_none());
        unconverted
            && (read_in_place || self.whole_value(argument))
            && self.holds(MethodFeatures::VALUE_ARGUMENTS)
    }

    /// `external_call["callee", T](args…)`, the libc crossing
    /// (`external_call["rmdir", Int32](fspath.as_c_string_slice())`), over
    /// closed scalars and whole values of closed types.
    ///
    /// The checker types it from the closed callee table and the spelled
    /// return type, and selects no callee: the call records only its closed
    /// result type, and each argument what its own syntax decides. A
    /// declaration of that name would record a selection at the call, and
    /// such a call is not this.
    pub(super) fn foreign_call(
        &self,
        id: OccurrenceId,
        name: &str,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> bool {
        use mojito_ast::ast::ParamArg;
        let callee = matches!(
            param_args.first(),
            Some(ParamArg::Value(Expr {
                kind: ExprKind::Str(_),
                ..
            }))
        );
        // The return type, then at most `num_fixed_args=<literal>`.
        let shape = matches!(param_args.get(1), Some(ParamArg::Type(_)))
            && param_args.iter().skip(2).all(|argument| {
                matches!(argument, ParamArg::Named { name, value }
                    if name == "num_fixed_args"
                        && matches!(&**value, ParamArg::Value(Expr { kind: ExprKind::Int(_), .. })))
            });
        let admitted = !self.keyed
            && name == "external_call"
            && callee
            && shape
            && kwargs.is_empty()
            && args.iter().all(|argument| {
                ((self.expression(argument) && self.scalar(argument)) || self.whole_value(argument))
                    && self.closed(argument)
            })
            && self.closed_value(id)
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.call_parameters, id).is_none()
                    && fact_at(&facts.selected_calls, id).is_none()
                    && fact_at(&facts.overload_targets, id).is_none()
                    && fact_at(&facts.generic_instantiations, id).is_none()
                    && fact_at(&facts.expression_bindings, id).is_none()
            });
        admitted && self.holds(MethodFeatures::FOREIGN_CALLS)
    }

    /// A call through a parameter declared with a `def(...)` type, passing
    /// closed scalars or whole values.
    ///
    /// The call records the parameter's own contract symbol and parameters,
    /// which an instance takes from its own parameter binding
    /// ([`Checker::realize_callable_call`]), and the residue it puts on the
    /// body's frame names the parameter's slot and each argument's signature
    /// place. What an argument records is decided as a sibling call's is: a
    /// read parameter borrows a named place, and a `var` one takes a `^`
    /// transfer or a temporary as it stands.
    pub(super) fn callable_call(
        &self,
        id: OccurrenceId,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
        known: bool,
    ) -> bool {
        let argument = |argument: &Expr| {
            let named = match &argument.kind {
                ExprKind::Identifier(name) => {
                    self.declared(name) || self.params.contains(&name.as_str())
                }
                _ => self.receiver_field(argument),
            };
            if self.expression(argument) && self.scalar(argument) {
                return true;
            }
            let read_in_place = named
                && self.facts.is_none_or(|facts| {
                    facts
                        .borrowed_read_call_places
                        .contains(&self.occurrence(argument))
                });
            !self.keyed && (read_in_place || self.whole_value(argument))
        };
        let admitted = known
            && !self.keyed
            && param_args.is_empty()
            && kwargs.is_empty()
            && args.iter().all(argument);
        if admitted {
            let mut calls = self.callable_calls.borrow_mut();
            if !calls.contains(&id) {
                calls.push(id);
            }
        }
        admitted && self.holds(MethodFeatures::CALLABLE_PARAMETERS)
    }

    /// `elt_handler[i](self.storage[i]^)`: a call through one of the
    /// method's own compile-time callable binders, applied at the innermost
    /// `comptime for` variable and handed pack elements transferred out of
    /// an owned receiver.
    ///
    /// The template recorded the binder's application at the loop's binder
    /// and the binder's parameters over the struct's pack. An instance keeps
    /// the binder, so it takes the application at the copy's literal and the
    /// parameters from its own binding of it
    /// ([`Checker::realize_callable_call`]); the residue the call puts on
    /// the body's frame names the binder and the receiver's place, which no
    /// instance renames, and is republished verbatim.
    pub(super) fn callable_binder_call(
        &self,
        id: OccurrenceId,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
        known: bool,
    ) -> bool {
        let loop_index = |argument: &mojito_ast::ast::ParamArg| {
            matches!(argument, mojito_ast::ast::ParamArg::Value(value)
                if matches!(&value.kind, ExprKind::Identifier(name)
                    if self.loop_vars.borrow().last() == Some(name))
                    && self.folded_value(value))
        };
        let owned_receiver = matches!(
            self.self_convention,
            Some(mojito_ast::ast::ArgConvention::Var | mojito_ast::ast::ArgConvention::Deinit)
        );
        let transferred_element = |argument: &Expr| {
            matches!(&argument.kind, ExprKind::Transfer(element)
                if owned_receiver
                    && matches!(&element.kind, ExprKind::Index { object, .. }
                        if self.receiver_field(object))
                    && self.pack_element(element))
        };
        let admitted = known
            && !param_args.is_empty()
            && param_args.iter().all(loop_index)
            && kwargs.is_empty()
            && args.iter().all(transferred_element);
        if admitted {
            let mut calls = self.callable_calls.borrow_mut();
            if !calls.contains(&id) {
                calls.push(id);
            }
        }
        admitted
            && self.holds(MethodFeatures::CALLABLE_BINDERS)
            && self.holds(MethodFeatures::COMPTIME_CONTROL)
    }

    /// A bare identifier the parser left as a value where it names a closed
    /// struct type (`pick[String](…)`): the check resolved it as the type
    /// and erased the argument, which every instance repeats.
    fn closed_type_argument(&self, argument: &Expr, name: &str) -> bool {
        let erased = mojito_checked::checked::SemanticAdjustment::EraseCompileTimeArgument;
        self.structs
            .get(name)
            .is_some_and(|info| info.decls.is_empty())
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.operation_adjustments, self.occurrence(argument)) == Some(&erased)
            })
    }

    /// A whole value passed to a static call ([`Self::static_call`]) that
    /// records no contract: moved, a temporary, copied where the template
    /// recorded the copy, a named place read where it lies (always, in a
    /// read-only pack, `packed`), or a named place a `mut` or `ref`
    /// parameter keeps as the caller's.
    ///
    /// The call records no conversion at it, so it binds a parameter of its
    /// own type under every instance, and what the call records for it is
    /// decided by its syntax and the callee's convention alone, as for a
    /// method's argument ([`Self::argument`]). A copy into a `var` parameter
    /// is owed again at the instance's type.
    fn static_argument(&self, argument: &Expr, packed: bool) -> bool {
        let named = match &argument.kind {
            ExprKind::Identifier(name) => {
                self.declared(name) || self.params.contains(&name.as_str())
            }
            _ => self.receiver_field(argument),
        };
        let id = self.occurrence(argument);
        let kept = self
            .facts
            .is_some_and(|facts| facts.call_place_uses.contains(&id));
        let admitted = !self.keyed
            && self.facts.is_none_or(|facts| {
                let read_in_place =
                    (packed || facts.borrowed_read_call_places.contains(&id)) && named;
                if kept {
                    named
                } else {
                    read_in_place || self.whole_value(argument)
                }
            });
        if admitted && kept {
            push_unique(&mut self.places.borrow_mut(), id);
        }
        admitted && self.holds(MethodFeatures::VALUE_ARGUMENTS)
    }

    /// A string literal passed to a static call ([`Self::static_call`]): a
    /// temporary of its own closed type, bound to a parameter of that type
    /// or converted into it.
    ///
    /// A conversion the template recorded (`pick[String]("s")`, or a
    /// parameter typed `String`) is selected again per instance
    /// ([`Checker::realize_conversion`]). A literal bound to the static's own
    /// binder records none in the template, whose callee keeps the binder
    /// symbolic (`realize_static_instantiations`).
    fn static_literal(&self, argument: &Expr) -> bool {
        let id = self.occurrence(argument);
        !self.keyed
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.expression_types, id) == Some(&Ty::StringLiteral)
            })
            && self.holds(MethodFeatures::VALUE_ARGUMENTS)
    }

    /// Whether the call at `expr` recorded a closed contract naming `method`
    /// on the receiver's own struct, and nothing a derivation lacks. A call
    /// that is not trivial is a sibling call, which only a method body holds,
    /// and which may raise (`raising_method_contract`).
    fn sibling_call(
        &self,
        facts: &CheckedBodyFacts,
        expr: &Expr,
        object: &Expr,
        method: &str,
    ) -> bool {
        self.named_contract(facts, expr, object, method)
            .is_some_and(|call| {
                mojito_checked::templates::trivial_method_contract(call)
                    || (!self.keyed
                        && (mojito_checked::templates::value_method_contract(call)
                            || mojito_checked::templates::raising_method_contract(call))
                        && self.holds(MethodFeatures::SIBLING_CALLS))
            })
    }

    /// A method call on a named place the call copies before its callee
    /// consumes the copy (`slice.start.or_else(0)`): a parameter, a `var`
    /// local, or a field of `self`, of a parameter, or of a local.
    ///
    /// `infer_method_call` copies such a place whatever its type, where the
    /// type is implicitly copyable, and refuses the program otherwise; the
    /// mark it records at the call is owed again at the instance's type
    /// (`realize_instance_facts`). The contract is a consuming call's, as on
    /// a `^` transfer, and nothing moves out of the place.
    fn copied_consuming_call(
        &self,
        expr: &Expr,
        object: &Expr,
        method: &str,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> bool {
        let named = |name: &str| self.params.contains(&name) || self.declared(name);
        let place = match &object.kind {
            ExprKind::Identifier(name) => named(name),
            ExprKind::Member { object: base, .. } => {
                self.receiver_field(object)
                    || matches!(&base.kind, ExprKind::Identifier(name) if named(name))
            }
            _ => false,
        };
        !self.keyed
            && place
            && args
                .iter()
                .chain(kwargs.iter().map(|keyword| &keyword.value))
                .all(|argument| self.argument(expr, argument))
            && self.facts.is_none_or(|facts| {
                facts
                    .implicitly_copied_consuming_receivers
                    .contains(&self.occurrence(expr))
                    && self
                        .named_contract(facts, expr, object, method)
                        .is_some_and(mojito_checked::templates::consuming_nominal_contract)
            })
            && self.holds(MethodFeatures::COPIED_RECEIVERS)
    }
}

/// Whether an argument of the recorded type binds a parameter typed by the
/// callee's existential `Some[…]` binder under every instance: the argument
/// is a caller binder each of whose existential's bounds one of its own
/// bounds names or refines, so every type an instance binds conforms.
fn existential_argument(
    traits: &HashMap<String, crate::checker::TraitInfo>,
    parameter: &Ty,
    argument: Option<&Ty>,
) -> bool {
    match (parameter, argument) {
        (
            Ty::Param {
                binder,
                bounds: wanted,
                ..
            },
            Some(Ty::Param { bounds: held, .. }),
        ) => {
            existential_binder(binder)
                && wanted.iter().all(|bound| {
                    held.iter().any(|own| {
                        own == bound || crate::checker::traits::refines_trait(traits, own, bound)
                    })
                })
        }
        _ => false,
    }
}

/// Whether the call's boundary converts the argument at `id` through an
/// `@implicit` constructor, and the conversion is kept at that occurrence
/// too, where [`Checker::realize_conversion`] selects it again.
fn converted_argument(
    facts: &CheckedBodyFacts,
    contract: Option<&TemplateCallContract>,
    id: OccurrenceId,
) -> bool {
    let converts = |adjustment: &mojito_checked::checked::CheckedCallValueAdjustment| {
        matches!(
            adjustment,
            mojito_checked::checked::CheckedCallValueAdjustment::ImplicitConversion { .. }
        )
    };
    contract.is_some_and(|call| {
        call.arguments
            .iter()
            .any(|bound| bound.value == id && bound.adjustments.iter().any(converts))
    }) && fact_at(&facts.conversions, id).is_some()
}

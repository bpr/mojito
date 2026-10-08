//! Realization of the calls, operators, and built-ins a template recorded,
//! re-decided for one instance as the clone check would decide them.

use super::{
    Occurrence, TUPLE_ELEMENT_ACCESSOR, closed_scalar, conversion_target, fact_at, names_method,
    note_realized_callee, set_fact, template_callee, upsert,
};
use crate::checker::builtins::{is_numeric, print_keyword};
use crate::checker::{Checker, callable_contract_target, callable_lowered_name};
use mojito_checked::templates::{
    CallParameterFact, CheckedBodyFacts, OccurrenceId, TemplateCallContract, TemplateOwner,
};
use mojito_types::types::{Ty, TySubst};

impl Checker {
    /// Realize one closed method call for an instance: its target, and its
    /// result type by substitution.
    ///
    /// A clone check retargets a method call on a closed struct instance to
    /// that instance's clone of the method, when the elaborator has minted
    /// one (`instance_method_clone`), and records the clone as the call's
    /// target, its overload target, and the key of the effect summaries it
    /// reads. Nothing else in a [`closed_method_contract`] can change.
    ///
    /// The template's selection stands: the declared member is the one whose
    /// lowered name the template recorded, and the clone member is the one
    /// with that signature (`method_clone_target`). A clone check ranks the
    /// clone family again, on arguments that are closed scalars in both
    /// checks, so every member of an overloaded family must declare closed
    /// parameter types for the two rankings to agree. The callee has no
    /// binders of its own. A clone that exists has met its `where` clauses; a
    /// callee with an availability condition and no clone is left to the
    /// clone check, as is an instance that has clones but not this one
    /// (withheld, or a collapsed overload family).
    pub(super) fn realize_method_call(
        &self,
        facts: &mut CheckedBodyFacts,
        index: usize,
        occurrences: &[Occurrence],
        substitution: &TySubst,
        substitute: &dyn Fn(&Ty) -> Ty,
    ) -> Result<(), &'static str> {
        let id = facts.selected_calls[index].0;
        let (receiver, method) = method_call_at(occurrences, id)
            .ok_or("a selected call is not a method call in the instance")?;
        let Some(Ty::Struct(owner, arguments)) =
            fact_at(&facts.expression_types, receiver).cloned()
        else {
            return Err("a method call's receiver is not a nominal struct");
        };
        let selected = facts.selected_calls[index].1.contract.target.clone();
        // A call of a method with binders of its own names the method,
        // whose template the elaborator instantiates per call. On a
        // non-generic receiver the record names no instance, and substitution
        // left its arguments as they were, so the clone check selects the
        // same method.
        if let Some(request) = fact_at(&facts.method_instantiations, id) {
            if !arguments.is_empty() || !request.owner_arguments.is_empty() {
                return Err("a method-call record is keyed by a generic receiver");
            }
            note_realized_callee(facts, &selected, &selected);
            return Ok(());
        }
        // A subscript that is the target of a store selected the setter.
        let method = if method == "__getitem__" && names_method(&selected, &owner, "__setitem__") {
            "__setitem__".to_string()
        } else {
            method
        };
        let Some(target) = self.realize_method_contract(
            &facts.expression_types,
            &mut facts.selected_calls[index].1,
            (&owner, &arguments),
            &method,
            (substitution, substitute),
        )?
        else {
            note_realized_callee(facts, &selected, &selected);
            return Ok(());
        };
        if let Some((_, parameters)) = facts
            .call_parameters
            .iter_mut()
            .find(|(site, _)| *site == id)
        {
            for parameter in parameters {
                parameter.ty = substitute(&parameter.ty);
            }
        }
        if let Some(entry) = facts
            .overload_targets
            .iter_mut()
            .find(|(site, _)| *site == id)
        {
            entry.1.clone_from(&target);
        }
        note_realized_callee(facts, &selected, &target);
        Ok(())
    }

    /// Refuse each pack accessor (`BodyShape::pack_accessor`) at an
    /// instance: the template's `Tuple.__getitem_param__[i]` request names
    /// no member an instance's check selects, so the instance's own clone
    /// check keeps the body.
    pub(super) fn realize_pack_accessors(facts: &CheckedBodyFacts) -> Result<(), &'static str> {
        let requested = facts.method_instantiations.iter().any(|(_, request)| {
            request.owner == mojito_types::types::TUPLE_TYPE_NAME
                && request.method == TUPLE_ELEMENT_ACCESSOR
        });
        if requested {
            return Err("an instance reads a pack element through the Tuple accessor");
        }
        Ok(())
    }

    /// Realize the value getter each element store through a setter embeds,
    /// on the instance's own subscripted value, as the setter selected at
    /// the site is realized ([`Self::realize_method_contract`]).
    pub(super) fn realize_element_getters(
        &self,
        facts: &mut CheckedBodyFacts,
        occurrences: &[Occurrence],
        substitution: &TySubst,
        substitute: &dyn Fn(&Ty) -> Ty,
    ) -> Result<(), &'static str> {
        let mut realized = Vec::new();
        for (id, store) in &mut facts.augmented_subscripts {
            let Some(getter) = &mut store.getter else {
                continue;
            };
            let (receiver, method) = method_call_at(occurrences, *id)
                .ok_or("an element store is not a subscript in the instance")?;
            let Some(Ty::Struct(owner, arguments)) = fact_at(&facts.expression_types, receiver)
            else {
                return Err("an element store's subscripted value is not a nominal struct");
            };
            let selected = getter.contract.target.clone();
            let target = self
                .realize_method_contract(
                    &facts.expression_types,
                    getter,
                    (owner, arguments),
                    &method,
                    (substitution, substitute),
                )?
                .unwrap_or_else(|| selected.clone());
            realized.push((selected, target));
        }
        for (selected, target) in realized {
            note_realized_callee(facts, &selected, &target);
        }
        Ok(())
    }

    /// Realize the in-place dunder each element store embeds: one the
    /// template dispatched through the element's bound is re-selected on
    /// the instance's element type ([`Self::realize_embedded_dispatch`]),
    /// and a closed one stands.
    pub(super) fn realize_element_dunders(
        &self,
        facts: &mut CheckedBodyFacts,
    ) -> Result<(), &'static str> {
        let mut realized = Vec::new();
        for (_, store) in &mut facts.augmented_subscripts {
            let Some(inplace) = &mut store.inplace else {
                continue;
            };
            let selected = inplace.contract.target.clone();
            let target = if mojito_symbol::symbol::is_trait_dispatch_symbol(&selected) {
                self.realize_embedded_dispatch(&facts.expression_types, inplace, &store.operand_ty)?
                    .target
            } else {
                selected.clone()
            };
            realized.push((selected, target));
        }
        for (selected, target) in realized {
            note_realized_callee(facts, &selected, &target);
        }
        Ok(())
    }

    /// Realize the in-place dunder each augmented assignment to a place
    /// selects, on the instance's type of the place. One the template
    /// dispatched through the place's bound is re-selected there, and the
    /// witness's parameters and raised type are recorded at the place, as
    /// the checker's own selection records them
    /// ([`Self::realize_embedded_dispatch`]). A
    /// nominal one is the place's struct's own method, realized as any call
    /// on that struct is ([`Self::realize_method_contract`]).
    pub(super) fn realize_inplace_updates(
        &self,
        facts: &mut CheckedBodyFacts,
        substitution: &TySubst,
        substitute: &dyn Fn(&Ty) -> Ty,
    ) -> Result<(), &'static str> {
        let mut realized = Vec::new();
        let mut parameters = Vec::new();
        let mut effects = Vec::new();
        for (id, call) in &mut facts.inplace_updates {
            let place = fact_at(&facts.expression_types, *id)
                .ok_or("an updated place has no retained type")?;
            let selected = call.contract.target.clone();
            if mojito_symbol::symbol::is_trait_dispatch_symbol(&selected) {
                let witness =
                    self.realize_embedded_dispatch(&facts.expression_types, call, place)?;
                effects.push((*id, call.contract.raises.clone()));
                parameters.push((*id, witness.parameters));
                realized.push((selected, witness.target));
                continue;
            }
            let Ty::Struct(owner, arguments) = place else {
                return Err("an updated place's in-place dunder is not a struct's method");
            };
            let method = mojito_symbol::symbol::split_method_symbol(&selected)
                .and_then(|(_, method)| method.split('$').next())
                .ok_or("an in-place update names no method")?;
            let target = self
                .realize_method_contract(
                    &facts.expression_types,
                    call,
                    (owner, arguments),
                    method,
                    (substitution, substitute),
                )?
                .unwrap_or_else(|| selected.clone());
            if let Some((_, parameters)) = facts
                .call_parameters
                .iter_mut()
                .find(|(site, _)| site == id)
            {
                for parameter in parameters {
                    parameter.ty = substitute(&parameter.ty);
                }
            }
            realized.push((selected, target));
        }
        for (id, parameters) in parameters {
            upsert(&mut facts.call_parameters, id, parameters);
        }
        for (id, raises) in effects {
            match raises {
                Some(raises) => upsert(
                    &mut facts.expression_effects,
                    id,
                    mojito_checked::checked::EffectFacts {
                        raises: Some(raises),
                        may_suspend: false,
                        diverges: false,
                    },
                ),
                None => facts.expression_effects.retain(|(site, _)| *site != id),
            }
        }
        for (selected, target) in realized {
            note_realized_callee(facts, &selected, &target);
        }
        Ok(())
    }

    /// Realize each overloaded static a body calls on a generic struct's
    /// type application (`Pair[Self.T].pick(v, 1)`,
    /// [`BodyShape::static_call`]): the member the template ranked, on the
    /// instance's clone of the static where the receiver's arguments,
    /// resolved in the instance, have one.
    ///
    /// The template ranked the family with the struct's parameters
    /// symbolic, and every instance keeps that member, as the pin does, even
    /// where its own argument types would rank another best: the instance
    /// calls its clone of the member (`method_clone_target`). An instance
    /// whose substitution makes two members identical has no clone of the
    /// family ([`collapses`]) and keeps the erased member, as does a
    /// receiver with no clone of a family differing only in closed parameter
    /// types, or a static on an inferred or contextual receiver, which no
    /// instance retargets. A member with binders of its own names itself
    /// (`realize_static_instantiations`).
    pub(super) fn realize_static_overloads(
        &self,
        facts: &mut CheckedBodyFacts,
        occurrences: &[Occurrence],
        substitution: &TySubst,
    ) -> Result<(), &'static str> {
        for index in 0..facts.overload_targets.len() {
            let (id, selected) = &facts.overload_targets[index];
            if facts.selected_calls.iter().any(|(call, _)| call == id)
                || fact_at(&facts.method_instantiations, *id).is_some()
            {
                continue;
            }
            let Some(occurrence) = occurrences.iter().find(|occurrence| occurrence.id == *id)
            else {
                continue;
            };
            let (Some((owner, applied)), Some((_, method))) =
                (&occurrence.type_receiver, &occurrence.method_call)
            else {
                continue;
            };
            let Some(info) = self
                .structs
                .get(owner)
                .filter(|info| !info.decls.is_empty())
            else {
                continue;
            };
            let arguments = self
                .partition_struct_origin_args(owner, &info.source_params, applied)
                .and_then(|partitioned| {
                    self.resolve_use_params(owner, &info.decls, &partitioned.forwarded, &[], &[])
                })
                .map_err(|_| "a static's receiver arguments do not resolve in the instance")?
                .1;
            // No clone of the static: the template's selected member serves
            // the instance.
            let Some(clone) = self.instance_method_clone(owner, method, &arguments) else {
                continue;
            };
            // A lone clone is no overload set, and its call records no member.
            if info
                .methods
                .get(&clone)
                .is_none_or(|family| family.len() < 2)
            {
                return Err("an instance collapsed a static's overload family");
            }
            let self_ty = self.self_instance_ty(owner);
            let declared = info
                .methods
                .get(method)
                .and_then(|family| {
                    family.iter().find(|member| {
                        crate::checker::overload_support::method_lowered_name(
                            owner,
                            method,
                            member,
                            self_ty.as_ref(),
                        ) == *selected
                    })
                })
                .ok_or("a static's selected overload is not declared")?;
            let target = self
                .method_clone_target(owner, method, &arguments, declared, substitution)
                .ok_or("a static's clone family has no overloaded member for the selection")?;
            facts.overload_targets[index].1 = target;
        }
        // A lone static records no member under the template, and the
        // instance names its struct's clone of it.
        for occurrence in occurrences {
            let (Some((owner, applied)), Some((_, method))) =
                (&occurrence.type_receiver, &occurrence.method_call)
            else {
                continue;
            };
            if applied.is_empty()
                || fact_at(&facts.overload_targets, occurrence.id).is_some()
                || fact_at(&facts.method_instantiations, occurrence.id).is_some()
            {
                continue;
            }
            let Some(info) = self
                .structs
                .get(owner)
                .filter(|info| !info.decls.is_empty())
            else {
                continue;
            };
            let arguments = self
                .partition_struct_origin_args(owner, &info.source_params, applied)
                .and_then(|partitioned| {
                    self.resolve_use_params(owner, &info.decls, &partitioned.forwarded, &[], &[])
                })
                .map_err(|_| "a static's receiver arguments do not resolve in the instance")?
                .1;
            if let Some(clone) = self.instance_method_clone(owner, method, &arguments) {
                facts
                    .overload_targets
                    .push((occurrence.id, format!("{owner}.{clone}")));
            }
        }
        self.realize_static_instantiations(facts, occurrences)
    }

    /// Realize one call through a callable parameter for an instance.
    ///
    /// The call recorded the parameter's own contract symbol and parameters,
    /// in the caller's binder scope: the instance takes both from its own
    /// binding of the parameter, which the elaborator already substituted.
    ///
    /// A call through a compile-time callable binder the instance keeps
    /// (`elt_handler[i](…)`) records the binder's application instead of a
    /// contract target: the instance applies its own binder at the literals
    /// the elaborator folded into its copy.
    pub(super) fn realize_callable_call(
        &self,
        facts: &mut CheckedBodyFacts,
        id: OccurrenceId,
        occurrences: &[Occurrence],
    ) -> Result<(), &'static str> {
        let occurrence = occurrences
            .iter()
            .find(|occurrence| occurrence.id == id)
            .ok_or("a callable call occurrence is not a direct call in the instance")?;
        let name = occurrence
            .callee
            .as_deref()
            .ok_or("a callable call occurrence is not a direct call in the instance")?;
        if let Some(index) = facts
            .generic_instantiations
            .iter()
            .position(|(site, _)| *site == id)
        {
            let Some(callee @ Ty::GenericFunc { names, params, .. }) = self.lookup(name) else {
                return Err("an applied callable binder is not bound to a generic function type");
            };
            let arguments = occurrence
                .compile_time_literals
                .iter()
                .map(|literal| literal.clone().map(mojito_types::types::TyArg::Val))
                .collect::<Option<Vec<_>>>()
                .filter(|arguments| !arguments.is_empty())
                .ok_or("an applied callable binder's argument is not a folded literal")?;
            facts.generic_instantiations[index].1 = mojito_checked::checked::GenericInstantiation {
                callee: name.to_string(),
                parameter_names: names.clone(),
                parameter_types: params
                    .iter()
                    .map(|ty| {
                        mojito_symbol::symbol::TypeKey::from_ty(ty)
                            .as_str()
                            .to_string()
                    })
                    .collect(),
                variadic: mojito_symbol::symbol::VariadicKey::from_callable(callee),
                arguments,
                inferred_values: Vec::new(),
                folded_arguments: Vec::new(),
            };
            set_fact(&mut facts.call_parameters, id, call_parameter_facts(callee));
            note_realized_callee(facts, name, name);
            return Ok(());
        }
        let Some(callee @ Ty::Func { .. }) = self.lookup(name) else {
            return Err("a called parameter is not bound to a function type");
        };
        let target = callable_contract_target(callee)
            .ok_or("a called parameter's type has no callable contract")?;
        set_fact(&mut facts.overload_targets, id, target);
        set_fact(&mut facts.call_parameters, id, call_parameter_facts(callee));
        // The call reads the summaries keyed by the parameter's name, which
        // no declaration publishes under.
        note_realized_callee(facts, name, name);
        Ok(())
    }

    /// Realize one direct call for an instance: obligation 5 of
    /// [`Self::realize_instance_facts`].
    ///
    /// The template's selection stands. The instance repeats the one
    /// concrete decision a clone check also makes after selection, whether
    /// the closed application already has a clone (`existing_def_clone`).
    /// When the elaborator already retargeted the call, the written name must
    /// be exactly that clone, and the call takes the clone's own declared
    /// parameters.
    pub(super) fn realize_direct_call(
        &self,
        facts: &mut CheckedBodyFacts,
        template: &CheckedBodyFacts,
        id: OccurrenceId,
        occurrences: &[Occurrence],
    ) -> Result<(), &'static str> {
        let selected = template_callee(template, id)
            .ok_or("a call's selected callee is not a module-scope declaration")?;
        let written = occurrences
            .iter()
            .find(|occurrence| occurrence.id == id)
            .and_then(|occurrence| occurrence.callee.as_deref())
            .ok_or("a call occurrence is not a direct call in the instance")?;
        let application = facts
            .generic_instantiations
            .iter()
            .position(|(site, _)| *site == id);
        // The template's selection, never a fresh ranking: a call through
        // an overload set keeps the member whose lowered symbol the
        // template recorded.
        let target = template
            .overload_targets
            .iter()
            .find(|(site, _)| *site == id)
            .map(|(_, target)| target.as_str());
        let member = match (self.lookup(selected), target) {
            (Some(Ty::Overload(members)), Some(target)) => members
                .iter()
                .find(|member| callable_lowered_name(selected, member).as_deref() == Some(target)),
            (Some(Ty::Overload(_)), None) | (None, _) => None,
            (Some(callee), _) => Some(callee),
        }
        .ok_or("a call's selected declaration is no longer in scope")?;
        let existing = application.and_then(|index| {
            let Ty::GenericFunc { decls, .. } = member else {
                return None;
            };
            self.existing_def_clone(
                selected,
                decls,
                &facts.generic_instantiations[index].1.arguments,
            )
        });
        if written == selected {
            if let (Some(index), Some(clone)) = (application, existing) {
                facts.generic_instantiations.remove(index);
                match facts
                    .overload_targets
                    .iter_mut()
                    .find(|(site, _)| *site == id)
                {
                    Some(entry) => entry.1 = clone,
                    None => facts.overload_targets.push((id, clone)),
                }
            }
        } else {
            // The elaborator retargeted this call. It must name exactly
            // the clone of the application the template selected.
            let Some(index) = application else {
                return Err("a retargeted call has no retained application");
            };
            if existing.as_deref() != Some(written) {
                return Err("a retargeted call does not name the selected application");
            }
            let Some(callee @ Ty::Func { .. }) = self.lookup(written) else {
                return Err("a retargeted call's clone is not declared yet");
            };
            facts.generic_instantiations.remove(index);
            facts.overload_targets.retain(|(site, _)| *site != id);
            set_fact(&mut facts.call_parameters, id, call_parameter_facts(callee));
            set_fact(
                &mut facts.expression_bindings,
                id,
                TemplateOwner::Global(written.to_string()),
            );
        }
        note_realized_callee(facts, selected, written);
        Ok(())
    }

    /// Realize one admitted operator for an instance, as `infer_infix`
    /// decides it on the substituted operand types.
    ///
    /// A closed scalar operates natively and records nothing, as the template
    /// did; it owes only that the primitive path has the operator and gives
    /// the type the template kept (`scalar_operator_result`), which a bound
    /// alone does not promise. A nominal struct dispatches the operator's
    /// dunder, whose selection `struct_infix_dispatch` makes from the types
    /// alone: the instance records the target it names, reaches the struct's
    /// application, and writes the three facts that dispatch carries and the
    /// symbolic template could not — the implicit copy of a consumed place
    /// operand (a temporary one moves and records nothing), the conversion
    /// of an adapted one, and the `NegatedEquality` adjustment of a `!=`
    /// served by `__eq__`. A literal right operand is only ever beside a
    /// struct built over a parameter, whose dunder the template dispatched
    /// too: a conversion it recorded there is the instance's to select again.
    /// The dunder's result must still be the type the template kept, which
    /// is what an arithmetic operator's bound promised. Anything else (a
    /// tuple, a vector, a pointer) is the clone check's to judge.
    ///
    /// A closed left operand (a literal, or a scalar such as `n` in
    /// `n + self.bag`) is the one admitted operand without the forward
    /// dunder: the template dispatched the right operand's reflected dunder
    /// and adjusted the operator (`ReflectedOperator`), which the instance
    /// keeps; `struct_reflected_dispatch` names the target again at the
    /// instance's types. The left operand is passed as it stands, so it
    /// records nothing.
    pub(super) fn realize_operator(
        &self,
        template: &CheckedBodyFacts,
        facts: &mut CheckedBodyFacts,
        id: OccurrenceId,
        occurrences: &[Occurrence],
    ) -> Result<(), &'static str> {
        let (op, left, right, place) = occurrences
            .iter()
            .find(|occurrence| occurrence.id == id)
            .and_then(|occurrence| occurrence.operator)
            .ok_or("an operator is not one in the instance")?;
        let operand = |syntax| OccurrenceId {
            syntax,
            copy: id.copy,
        };
        let (left, right) = (operand(left), operand(right));
        let retained = |operand| {
            fact_at(&facts.expression_types, operand)
                .cloned()
                .ok_or("an operand has no retained type")
        };
        let (left_ty, right_ty) = (retained(left)?, retained(right)?);
        let result = fact_at(&facts.expression_types, id)
            .ok_or("an operator has no retained result type")?
            .clone();
        // The template borrowed a named parameter-typed operand for the
        // requirement's read dunder; an instance whose operand is nominal
        // or scalar borrows it by its own dunder's convention, below.
        if !matches!(left_ty, Ty::Param { .. }) {
            let symbolic = |operand: &OccurrenceId| {
                matches!(
                    fact_at(&template.expression_types, *operand),
                    Some(Ty::Param { .. })
                )
            };
            facts
                .borrowed_read_call_places
                .retain(|borrowed| !([left, right].contains(borrowed) && symbolic(borrowed)));
        }
        if fact_at(&facts.operation_adjustments, id)
            == Some(&mojito_checked::checked::SemanticAdjustment::ReflectedOperator)
        {
            let target = self
                .struct_reflected_dispatch(op, &left_ty, &right_ty)
                .ok_or("the instance's type has no reflected dunder for the operator")?;
            let reflected = op
                .reflected_dunder()
                .ok_or("the operator has no reflected dunder")?;
            if self.struct_dunder(&right_ty, reflected, &[&left_ty]) != Some(Ok(result)) {
                return Err("the reflected dunder's result is not the type the template kept");
            }
            set_fact(&mut facts.overload_targets, id, target);
            return Ok(());
        }
        // A type parameter the instance keeps symbolic (the template's own
        // reuse, or a clone keeping a bounded binder) is operated on through
        // its bound or `where` clause, as the template was, which recorded
        // nothing at the operator.
        if matches!(left_ty, Ty::Param { .. }) && right_ty == left_ty {
            return Ok(());
        }
        if closed_scalar(&left_ty) {
            return if right_ty == left_ty
                && crate::checker::operators::scalar_operator_result(op, &left_ty) == Some(result)
            {
                Ok(())
            } else {
                Err("the operator is not the instance's scalar operation")
            };
        }
        let Ty::Struct(name, arguments) = &left_ty else {
            return Err("an operand is neither a scalar nor a struct");
        };
        if !self.structs.contains_key(name) {
            return Err("an operand is a built-in aggregate");
        }
        let dispatch = self
            .struct_infix_dispatch(op, &left_ty, &right_ty)
            .map_err(|_| "the operator is undefined for the instance's type")?
            .ok_or("the instance's type has no dunder for the operator")?;
        let dunder = if dispatch.negated_equality {
            "__eq__"
        } else {
            op.dunder().ok_or("the operator dispatches no dunder")?
        };
        if self.struct_dunder(&left_ty, dunder, &[&dispatch.operand_ty]) != Some(Ok(result)) {
            return Err("the dunder's result is not the type the template kept");
        }
        // `check_consuming_as` on the right operand: a place is copied, at
        // its own type rather than the converted one, under the demand the
        // bundle-wide check makes of every copy the template kept.
        // A copy the template's own dispatch recorded is kept, and the
        // bundle-wide check has judged it already.
        let copied = facts.copy_place_value_uses.contains(&right);
        if copied && !dispatch.consumes {
            return Err("the instance's dunder borrows an operand the template's consumed");
        }
        if dispatch.consumes && place && !copied {
            if !(self.is_copyable(&right_ty) && self.is_implicitly_copyable(&right_ty)) {
                return Err("a consumed operand is not implicitly copyable for the instance");
            }
            facts.copy_place_value_uses.push(right);
        }
        // `borrow_nominal_place_argument` on each operand a read dunder
        // takes where it lies.
        if dispatch.borrows.0 {
            self.borrow_nominal_place_operand(facts, left, &left_ty, occurrences);
        }
        if dispatch.borrows.1 {
            self.borrow_nominal_place_operand(facts, right, &right_ty, occurrences);
        }
        // The conversion `record_implicit_conversion` installs. Its
        // constructor is selected by [`Self::realize_conversion`], which runs
        // after every operator and inherits its refusals.
        let kept = fact_at(&facts.conversions, right).is_some();
        if kept && !dispatch.converted {
            return Err("the instance reaches the dunder without the template's conversion");
        }
        if dispatch.converted && !kept {
            facts.conversions.push((
                right,
                mojito_checked::templates::TemplateConversion {
                    target: String::new(),
                    result: Some(dispatch.operand_ty.clone()),
                    raises: None,
                    source_borrow: None,
                },
            ));
        }
        if dispatch.negated_equality {
            facts.operation_adjustments.push((
                id,
                mojito_checked::checked::SemanticAdjustment::NegatedEquality,
            ));
        }
        // The operand's application is recorded as a receiver's would be:
        // only from a source that records applications at all.
        let source = occurrences
            .iter()
            .find(|occurrence| occurrence.id == id)
            .and_then(|occurrence| occurrence.span.source.as_deref());
        let application = (name.clone(), arguments.to_vec());
        if source.is_some()
            && !crate::checker::overload_support::is_bundled_module_source(source)
            && !facts.struct_applications.contains(&application)
        {
            facts.struct_applications.push(application);
        }
        if let Some(target) = dispatch.target {
            facts.overload_targets.push((id, target));
        }
        Ok(())
    }

    /// Realize one `repr(value)` call for an instance: its argument must
    /// still be `Writable`, the demand the builtin makes of it.
    ///
    /// The call selects no callee. What it records is the wrap of its
    /// compile-time string result as the nominal `String`, which
    /// [`Self::realize_conversion`] repeats, and the argument's own place
    /// use, which its syntax decides.
    pub(super) fn realize_repr_call(
        &self,
        facts: &CheckedBodyFacts,
        id: OccurrenceId,
        occurrences: &[Occurrence],
    ) -> Result<(), &'static str> {
        let syntax = occurrences
            .iter()
            .find(|occurrence| occurrence.id == id)
            .and_then(|occurrence| occurrence.arguments.first().copied())
            .ok_or("a repr call has no argument in the instance")?;
        let argument = OccurrenceId {
            syntax,
            copy: id.copy,
        };
        let ty = fact_at(&facts.expression_types, argument)
            .ok_or("a repr call's argument has no retained type")?;
        if self.conforms_to(ty, "Writable") {
            Ok(())
        } else {
            Err("a repr call's argument is not Writable for the instance")
        }
    }

    /// [`TemplateObligation::PrintableArguments`] for one `print` call: each
    /// argument must still be printable at the instance's type, the demand
    /// the builtin makes of it. The call selects no callee and records at an
    /// argument what its syntax decides, beside the in-place read of a named
    /// place the instance's type makes nominal, as `len`'s operand is. A
    /// named parameter-typed argument the template read in place is read
    /// that way only while the instance keeps it a parameter. Each keyword
    /// must still have the type `print` takes for it.
    pub(super) fn realize_print_call(
        &self,
        template: &CheckedBodyFacts,
        facts: &mut CheckedBodyFacts,
        id: OccurrenceId,
        occurrences: &[Occurrence],
    ) -> Result<(), &'static str> {
        let call = occurrences
            .iter()
            .find(|occurrence| occurrence.id == id)
            .ok_or("a print call has no occurrence in the instance")?;
        call.keywords.iter().try_for_each(|(name, syntax)| {
            let value = OccurrenceId {
                syntax: *syntax,
                copy: id.copy,
            };
            let ty = fact_at(&facts.expression_types, value)
                .ok_or("a print keyword has no retained type")?
                .clone();
            if !print_keyword(name, &ty).is_some_and(|(accepted, _)| accepted) {
                return Err("a print keyword has the wrong type for the instance");
            }
            self.borrow_nominal_place_operand(facts, value, &ty, occurrences);
            Ok(())
        })?;
        call.arguments.iter().try_for_each(|syntax| {
            let argument = OccurrenceId {
                syntax: *syntax,
                copy: id.copy,
            };
            let ty = fact_at(&facts.expression_types, argument)
                .ok_or("a print call's argument has no retained type")?
                .clone();
            if !self.printable_argument(&ty) {
                return Err("a print call's argument is not Writable for the instance");
            }
            if !matches!(ty, Ty::Param { .. })
                && matches!(
                    fact_at(&template.expression_types, argument),
                    Some(Ty::Param { .. })
                )
            {
                facts
                    .borrowed_read_call_places
                    .retain(|borrowed| *borrowed != argument);
            }
            self.borrow_nominal_place_operand(facts, argument, &ty, occurrences);
            Ok(())
        })
    }

    /// `String(value)` ([`BodyShape::stringify`]) at the instance's type of
    /// its argument, as [`Checker::infer_stringify`] types it: a numeric or
    /// `Bool` value is written by the builtin itself, and any other value
    /// through its `Writable` conformance, which reads it where it lies. A
    /// string literal would construct instead, which the template did not.
    pub(super) fn realize_stringify_calls(
        &self,
        facts: &mut CheckedBodyFacts,
        occurrences: &[Occurrence],
    ) -> Result<(), &'static str> {
        for index in 0..facts.overload_targets.len() {
            let (id, target) = &facts.overload_targets[index];
            if target == "String" {
                let id = *id;
                self.realize_stringify(facts, id, occurrences)?;
            }
        }
        Ok(())
    }

    /// Realize one implicit conversion for an instance, as
    /// `record_selected_conversion` installs it on the substituted types.
    ///
    /// An `@implicit` constructor is chosen from the source and the target
    /// type alone (`implicit_conversion_constructor`), so the instance
    /// repeats the choice at its own types and records whichever constructor
    /// it names — another member of the family, or a clone of the same one.
    /// A type that reaches the target by no conversion refuses, and so does
    /// one whose constructor consumes its source, which would record an
    /// implicit copy the template did not, or borrows it otherwise than the
    /// template's. A conversion that kept no target
    /// type is the nominal-string wrap, whose literal constructor is the same
    /// under every instance.
    ///
    /// A conversion at a selected call's argument is also carried by the
    /// contract's own boundary, which capture and installation copy verbatim.
    /// [`realize_boundary_conversions`] writes the target selected here back
    /// into that second copy, once every conversion has been re-selected.
    pub(super) fn realize_conversion(
        &self,
        facts: &mut CheckedBodyFacts,
        index: usize,
        substitution: &TySubst,
    ) -> Result<(), &'static str> {
        let (id, conversion) = &facts.conversions[index];
        let Some(result) = conversion.result.clone() else {
            if conversion.target == mojito_symbol::symbol::nominal_string_literal_ctor_symbol() {
                return Ok(());
            }
            return Err("a conversion that kept no target type is not the literal wrap");
        };
        let from = fact_at(&facts.expression_types, *id)
            .ok_or("a converted expression has no retained type")?
            .clone();
        let to = mojito_types::types::substitute(&result, substitution);
        if self.value_coerces(&from, &to) {
            return Err("the instance's value reaches the target without a conversion");
        }
        let selected = self
            .implicit_conversion_constructor(&from, &to)
            .map_err(|_| "the implicit conversion is ambiguous for the instance")?
            .ok_or("the instance's type reaches the target by no implicit conversion")?;
        // Each of these makes the recorder do more than fill the four tables:
        // a consuming constructor copies its source, and a raising one records
        // a call effect. A view one borrows its source, as the template's did
        // when both borrow alike: a place is borrowed where it stands, and a
        // temporary's materialized owner is an adjustment with no recipe.
        if selected.consumes_source
            || selected.error.is_some()
            || selected.source_borrow != conversion.source_borrow
        {
            return Err("an implicit conversion consumes, raises, or borrows for the instance");
        }
        facts.conversions[index].1 = mojito_checked::templates::TemplateConversion {
            target: selected.target,
            result: Some(to),
            raises: selected.error,
            source_borrow: selected.source_borrow,
        };
        Ok(())
    }

    /// Realize the built-ins a template proved through a bound, each `len`
    /// call and each bound conversion, for an instance.
    pub(super) fn realize_bound_builtins(
        &self,
        template: &CheckedBodyFacts,
        facts: &mut CheckedBodyFacts,
        occurrences: &[Occurrence],
    ) -> Result<(), &'static str> {
        for call in &template.builtin_len_calls {
            self.realize_builtin_len(facts, *call, occurrences)?;
        }
        for (target, argument) in bound_conversions(template, occurrences) {
            self.realize_bound_conversion(facts, &target, argument, occurrences)?;
        }
        Ok(())
    }

    /// Rewrite one method call's contract for an instance whose receiver is
    /// the struct `owner` under `arguments`: the target is the instance's
    /// clone of the selected declaration, where one exists, and the result,
    /// raised, parameter, and referent types substitute under the whole
    /// instance (`substitute`: its packs and folded values too). The
    /// receiver's own arguments judge an availability condition under
    /// `substitution`. `None` where the template already selected the
    /// receiver's clone, whose contract stands.
    fn realize_method_contract(
        &self,
        types: &[(OccurrenceId, Ty)],
        call: &mut TemplateCallContract,
        (owner, arguments): (&str, &[mojito_types::types::TyArg]),
        method: &str,
        (substitution, substitute): (&TySubst, &dyn Fn(&Ty) -> Ty),
    ) -> Result<Option<String>, &'static str> {
        let info = self
            .structs
            .get(owner)
            .ok_or("a method call's receiver struct is not declared")?;
        let selected = &call.contract.target;
        let family = info
            .methods
            .get(method)
            .ok_or("a called method is missing")?;
        let self_ty = self.self_instance_ty(owner);
        let clone_name =
            mojito_symbol::symbol::instance_method_clone_name(method, &info.decls, arguments);
        // A receiver whose type was already closed in the template (`List[Pair]`)
        // selected its clone there, on the arguments a clone check ranks too.
        let selected_clone = clone_name
            .as_deref()
            .is_some_and(|clone| names_method(selected, owner, clone));
        if selected_clone {
            return Ok(None);
        }
        let declared = match family.as_slice() {
            [only] => only,
            members => members
                .iter()
                .find(|member| {
                    crate::checker::overload_support::method_lowered_name(
                        owner,
                        method,
                        member,
                        self_ty.as_ref(),
                    ) == *selected
                })
                .ok_or("a called method's selected overload is not declared")?,
        };
        if !declared.decls.is_empty() {
            return Err("a called method has binders of its own");
        }
        let closed_family = family.len() == 1
            || family.iter().all(|member| {
                member
                    .params
                    .iter()
                    .all(|ty| !mojito_types::types::is_symbolic(ty))
            });
        // An argument whose own type is its parameter's matches it exactly
        // under every instance, which no other member can outrank; two
        // members an instance makes identical collapse, and
        // `method_clone_target` finds no single clone for them.
        let exact = call.contract.arguments.iter().all(|parameter| {
            call.arguments
                .iter()
                .find(|bound| bound.source == parameter.source)
                .and_then(|bound| fact_at(types, bound.value))
                .is_some_and(|ty| *ty == substitute(&parameter.parameter_ty))
        });
        if !closed_family && !exact {
            return Err("an overloaded callee declares a parameter of a parameter type");
        }
        let target = if self
            .instance_method_clone(owner, method, arguments)
            .is_some()
        {
            self.method_clone_target(owner, method, arguments, declared, substitution)
                .ok_or("a called method's clone family has no member for the selected overload")?
        } else {
            // No clone of this method: its template serves the instance. Its
            // availability condition is judged at the instance's receiver
            // arguments, as the clone check judges it.
            if !declared.availability.is_empty() && !substitution.is_empty() {
                let Ty::Struct(_, bound) = mojito_types::types::substitute(
                    &Ty::Struct(owner.to_string(), arguments.to_vec().into()),
                    substitution,
                ) else {
                    return Err("a called method has an availability condition and no clone");
                };
                if self
                    .method_constraint_result(declared, &[], &info.decls, &bound)
                    .is_err()
                {
                    return Err("a called method is unavailable at the instance");
                }
            }
            selected.clone()
        };
        // The callee has no binders of its own, so its parameter types were
        // recorded at the receiver's arguments: in the caller's binder scope,
        // whether the receiver is `self` or a field of another struct.
        let contract = &mut call.contract;
        contract.target.clone_from(&target);
        contract.result_ty = substitute(&contract.result_ty);
        contract.raises = contract.raises.as_ref().map(substitute);
        for argument in &mut contract.arguments {
            argument.parameter_ty = substitute(&argument.parameter_ty);
        }
        if let Some(reference) = &mut call.reference_result {
            reference.referent = substitute(&reference.referent);
        }
        Ok(Some(target))
    }

    /// Reject an instance whose body calls a static with binders of its own
    /// on a generic struct's type application (`Pair[Self.T].show(n)`,
    /// [`BodyShape::static_call`]) where the instance declares its own clone
    /// of that static, which the clone check calls and no recipe repeats.
    /// Any other such call names the erased static in both.
    fn realize_static_instantiations(
        &self,
        facts: &CheckedBodyFacts,
        occurrences: &[Occurrence],
    ) -> Result<(), &'static str> {
        for (id, _) in &facts.method_instantiations {
            let Some(occurrence) = occurrences.iter().find(|occurrence| occurrence.id == *id)
            else {
                continue;
            };
            let (Some((owner, applied)), Some((_, method))) =
                (&occurrence.type_receiver, &occurrence.method_call)
            else {
                continue;
            };
            let Some(info) = self
                .structs
                .get(owner)
                .filter(|info| !info.decls.is_empty())
            else {
                continue;
            };
            let arguments = self
                .partition_struct_origin_args(owner, &info.source_params, applied)
                .and_then(|partitioned| {
                    self.resolve_use_params(owner, &info.decls, &partitioned.forwarded, &[], &[])
                })
                .map_err(|_| "a static's receiver arguments do not resolve in the instance")?
                .1;
            if self
                .instance_method_clone(owner, method, &arguments)
                .is_some()
            {
                return Err("an instance calls its own clone of a static with binders");
            }
        }
        Ok(())
    }

    fn realize_stringify(
        &self,
        facts: &mut CheckedBodyFacts,
        id: OccurrenceId,
        occurrences: &[Occurrence],
    ) -> Result<(), &'static str> {
        let syntax = occurrences
            .iter()
            .find(|occurrence| occurrence.id == id)
            .and_then(|occurrence| occurrence.arguments.first().copied())
            .ok_or("a String conversion has no argument in the instance")?;
        let argument = OccurrenceId {
            syntax,
            copy: id.copy,
        };
        let ty = fact_at(&facts.expression_types, argument)
            .ok_or("a String conversion's argument has no retained type")?;
        if *ty == Ty::StringLiteral {
            Err("a String conversion's argument is a string literal for the instance")
        } else if is_numeric(ty) || *ty == Ty::Bool {
            facts.call_place_uses.retain(|kept| *kept != argument);
            Ok(())
        } else if !self.conforms_to(ty, "Writable") {
            Err("a String conversion's argument is not Writable for the instance")
        } else if facts.call_place_uses.contains(&argument) {
            Ok(())
        } else {
            Err("a String conversion's argument is not kept where it lies")
        }
    }

    /// Realize one built-in conversion of a place whose type binder carries
    /// the conversion's bound (`Int(mode)` on `mode: intable`) for an
    /// instance, as `infer_conversion` decides it on the concrete type.
    ///
    /// The template proved the conversion through the bound and recorded
    /// only its closed result. The instance owes the witness: an address
    /// for `Int` of a pointer, a scalar the conversion reads natively, or a
    /// struct's conversion dunder returning the target. A struct place is
    /// then read in place, the one fact `infer_conversion` adds. A raising
    /// dunder would add an effect the template did not record, so it
    /// refuses the derivation, as does a missing witness.
    fn realize_bound_conversion(
        &self,
        facts: &mut CheckedBodyFacts,
        target: &Ty,
        argument: OccurrenceId,
        occurrences: &[Occurrence],
    ) -> Result<(), &'static str> {
        let (dunder, bound) = crate::checker::builtins::conversion_protocol(target)
            .ok_or("a bound conversion names no conversion protocol")?;
        let ty = fact_at(&facts.expression_types, argument)
            .cloned()
            .ok_or("a bound conversion's argument has no retained type")?;
        let ty = self.opaque_element(&ty).unwrap_or(ty);
        if *target == Ty::Int && matches!(ty, Ty::Pointer { .. }) {
            return Ok(());
        }
        if let Some(result) = self.struct_dunder(&ty, dunder, &[]) {
            if result.ok().as_ref() != Some(target) {
                return Err("the instance's conversion dunder does not return its target");
            }
            if self
                .struct_dunder_signature_for(&ty, dunder, &[])
                .is_some_and(|(_, signature, _)| signature.raises)
            {
                return Err("the instance's conversion dunder raises");
            }
            self.borrow_nominal_place_operand(facts, argument, &ty, occurrences);
            return Ok(());
        }
        let converts = crate::checker::builtins::is_numeric(&ty)
            || ty == Ty::Bool
            || crate::checker::builtins::param_has_bound(&ty, bound)
            || mojito_types::types::is_scalar_simd(&ty);
        converts
            .then_some(())
            .ok_or("the instance's type has no conversion witness")
    }

    /// Realize one built-in `len(x)` for an instance, as `infer_len` decides
    /// it on a concrete argument.
    ///
    /// The template proved `len` through the parameter's bound. The instance
    /// owes the witness that bound promised — a `__len__` returning `Int`,
    /// which `len_result_for_type` finds — and takes the one fact `infer_len`
    /// adds for a concrete type: a named nominal-struct place is read in
    /// place rather than copied. A borrow the template already recorded (a
    /// reference-valued operand) is kept. A missing witness refuses the derivation,
    /// so the clone check reports it in its own words.
    fn realize_builtin_len(
        &self,
        facts: &mut CheckedBodyFacts,
        call: OccurrenceId,
        occurrences: &[Occurrence],
    ) -> Result<(), &'static str> {
        let argument = occurrences
            .iter()
            .find(|occurrence| occurrence.id == call)
            .filter(|occurrence| occurrence.callee.as_deref() == Some("len"))
            .and_then(|occurrence| match occurrence.arguments.as_slice() {
                // A call and its arguments are copied together.
                [argument] => Some(OccurrenceId {
                    syntax: *argument,
                    copy: occurrence.id.copy,
                }),
                _ => None,
            })
            .ok_or("a built-in 'len' call is not one in the instance")?;
        let ty = facts
            .expression_types
            .iter()
            .find(|(id, _)| *id == argument)
            .map(|(_, ty)| ty.clone())
            .ok_or("a built-in 'len' argument has no retained type")?;
        if !matches!(self.len_result_for_type(&ty), Ok(Some(Ty::Int))) {
            return Err("the instance's type has no 'len' witness");
        }
        // The template's own entry stands: a reference-valued operand is
        // read through its handle whatever the instance, and a nominal type
        // stays nominal under substitution. Only the nominal-place rule can
        // newly hold for an instance.
        self.borrow_nominal_place_operand(facts, argument, &ty, occurrences);
        Ok(())
    }

    /// Record that an instance reads a named nominal-struct operand in place
    /// ([`Checker::borrow_nominal_place_argument`]), keeping the occurrence
    /// order of the facts.
    fn borrow_nominal_place_operand(
        &self,
        facts: &mut CheckedBodyFacts,
        operand: OccurrenceId,
        ty: &Ty,
        occurrences: &[Occurrence],
    ) {
        let named = occurrences
            .iter()
            .any(|occurrence| occurrence.id == operand && occurrence.identifier);
        if named
            && matches!(ty, Ty::Struct(name, _) if self.structs.contains_key(name))
            && !facts.borrowed_read_call_places.contains(&operand)
        {
            facts.borrowed_read_call_places.push(operand);
            let order = |id: &OccurrenceId| occurrences.iter().position(|found| found.id == *id);
            facts.borrowed_read_call_places.sort_by_key(order);
        }
    }
}

/// The receiver and method of the method call at `id`, the receiver in
/// `id`'s own copy.
fn method_call_at(occurrences: &[Occurrence], id: OccurrenceId) -> Option<(OccurrenceId, String)> {
    let (receiver, method) = occurrences
        .iter()
        .find(|occurrence| occurrence.id == id)?
        .method_call
        .clone()?;
    Some((
        OccurrenceId {
            syntax: receiver,
            copy: id.copy,
        },
        method,
    ))
}

/// The built-in conversions of a bound-typed place an instance realizes
/// (`Checker::realize_bound_conversion`): each call to `Int`, `Float64`, or
/// `Bool` whose one argument the template typed as a binder carrying the
/// conversion's bound, with its target and the argument's occurrence.
fn bound_conversions(
    template: &CheckedBodyFacts,
    occurrences: &[Occurrence],
) -> Vec<(Ty, OccurrenceId)> {
    occurrences
        .iter()
        .filter(|occurrence| fact_at(&template.call_parameters, occurrence.id).is_none())
        .filter_map(|occurrence| {
            let target = conversion_target(occurrence.callee.as_deref()?)?;
            let (_, bound) = crate::checker::builtins::conversion_protocol(&target)?;
            let [syntax] = occurrence.arguments.as_slice() else {
                return None;
            };
            let argument = OccurrenceId {
                syntax: *syntax,
                copy: occurrence.id.copy,
            };
            template
                .expression_types
                .iter()
                .any(|(id, ty)| {
                    id.syntax == *syntax && crate::checker::builtins::param_has_bound(ty, bound)
                })
                .then_some((target, argument))
        })
        .collect()
}

/// A direct callable's runtime parameters as `record_call_parameter_names`
/// records them.
fn call_parameter_facts(callee: &Ty) -> Vec<CallParameterFact> {
    let (Ty::Func {
        names,
        params,
        conventions,
        ..
    }
    | Ty::GenericFunc {
        names,
        params,
        conventions,
        ..
    }) = callee
    else {
        return Vec::new();
    };
    names
        .iter()
        .zip(params)
        .zip(conventions)
        .map(|((name, ty), convention)| CallParameterFact {
            name: name.clone(),
            convention: *convention,
            ty: ty.clone(),
        })
        .collect()
}

//! Re-selection of a call through a bound for an instance.
//!
//! A template proves `place.copy()`, `item.__hash__(hasher)`, or
//! `item.write_to(writer)` on a place of a bare parameter type through the
//! parameter's bound and records an abstract contract (`__trait_dispatch.…`),
//! or an inverted write, at the call. An instance has a type there, and its
//! own check would select the requirement's witness from that type alone:
//! [`Checker::bound_witness`] is that selection, made once from the types
//! rather than by inferring the body again. The checker builtins on a
//! bounded parameter (`hasher.update(x)`, `writer.write(x)`) select no
//! callee; an instance owes only the proof the template made through the
//! bound ([`Checker::realize_bound_builtin`]).

use super::{Occurrence, fact_at, set_fact};
use crate::checker::{Checker, MethodSig};
use mojito_ast::ast::ArgConvention;
use mojito_checked::checked::{
    CheckedCallArgument, CheckedCallArgumentSource, CheckedCallBoundary, CheckedCallContract,
    MethodInstantiation, SemanticAdjustment,
};
use mojito_checked::templates::{
    BoundBuiltin, CallParameterFact, CheckedBodyFacts, OccurrenceId, TemplateArgumentBoundary,
    TemplateCallContract, TemplateInvalidation,
};
use mojito_types::types::{ParamDecl, Ty, TyArg, TySubst};
use std::collections::HashMap;

/// The witness an embedded dispatch re-selects: its target, and the
/// parameters a call at an occurrence of its own records for it.
pub(super) struct EmbeddedWitness {
    pub(super) target: String,
    pub(super) parameters: Vec<CallParameterFact>,
}

/// What an instance's type selects for a requirement the template dispatched
/// through a bound.
enum BoundWitness<'a> {
    /// A built-in value's `copy`: the read of the place itself, no callee.
    CopyRead,
    /// A built-in hashable leaf's `__hash__`: the leaf feeds the hasher
    /// directly, no callee.
    HashLeaf,
    /// A nominal struct's own method.
    Method {
        owner: &'a str,
        arguments: &'a [TyArg],
        declared: &'a MethodSig,
        target: String,
        /// The witness's own binders, each bound to the caller's argument
        /// type at the parameter that names it.
        binders: TySubst,
        /// The per-call request a witness with binders records, whether its
        /// clone is selected or not yet minted.
        request: Option<Box<MethodInstantiation>>,
        /// The struct's own substitution under the receiver's arguments.
        substitution: TySubst,
    },
}

/// One argument of a dispatched call as the template recorded it: its
/// occurrence, its recorded type, the convention it was bound with, and
/// whether the callee needs the caller's place.
struct DispatchedArgument {
    /// The slot the call bound it from.
    source: CheckedCallArgumentSource,
    /// The argument's occurrence; `None` for a default the call leaves to
    /// the callee.
    value: Option<OccurrenceId>,
    ty: Ty,
    /// The parameter type the abstract call bound it to.
    parameter_ty: Ty,
    convention: Option<ArgConvention>,
    requires_place: bool,
    /// Whether the clone check would rank a member on the recorded type: a
    /// positional argument whose type no expected type changes.
    ranked: bool,
    /// Whether it hands over a value the callee may own, which a `var`
    /// parameter takes without a copy.
    owned: bool,
}

impl Checker {
    /// Realize one call the template dispatched through a bound, as
    /// `infer_method_call` decides it on the substituted receiver type
    /// (obligation 16 of `realize_instance_facts`).
    ///
    /// A built-in value's copy loses its contract, target, and parameters and
    /// marks the receiver a copied place; a built-in leaf's `__hash__` loses
    /// the same three and records the leaf. A nominal struct's witness
    /// rewrites the contract in place: the target, the result, each
    /// parameter's type, the witness's own binders, and the call parameters
    /// the abstract call recorded empty. A type that selects nothing of
    /// those kinds is the clone check's to judge.
    pub(super) fn realize_bound_dispatch(
        &self,
        facts: &mut CheckedBodyFacts,
        id: OccurrenceId,
        occurrences: &[Occurrence],
    ) -> Result<(), &'static str> {
        let (receiver, method) = occurrences
            .iter()
            .find(|occurrence| occurrence.id == id)
            .and_then(|occurrence| occurrence.method_call.clone())
            .ok_or("a bound dispatch is not a method call in the instance")?;
        let receiver = OccurrenceId {
            syntax: receiver,
            copy: id.copy,
        };
        let ty = fact_at(&facts.expression_types, receiver)
            .cloned()
            .ok_or("a dispatched receiver has no retained type")?;
        // A receiver typed by a binder the instance keeps (the method's own
        // `value: Some[Hashable]`) is dispatched through its bound there
        // too, as the template dispatched it, over the instance program's
        // conformers, whose summaries must stay empty.
        if let Ty::Param { bounds, .. } = &ty {
            for conformer in self.dispatch_conformers(bounds, &method) {
                if !facts.effect_free_callees.contains(&conformer) {
                    facts.effect_free_callees.push(conformer);
                }
            }
            return Ok(());
        }
        let receiver_transferred = occurrences
            .iter()
            .any(|occurrence| occurrence.id == receiver && occurrence.transfer);
        let call =
            fact_at(&facts.selected_calls, id).ok_or("a bound dispatch lost its contract")?;
        let receiver_convention = call.contract.receiver_convention;
        let arguments = call
            .contract
            .arguments
            .iter()
            .map(|parameter| {
                // A default the requirement declares binds by value at its
                // declared type, which every witness declares alike.
                if parameter.source == CheckedCallArgumentSource::Default {
                    return Ok(DispatchedArgument {
                        source: parameter.source,
                        value: None,
                        ty: parameter.parameter_ty.clone(),
                        parameter_ty: parameter.parameter_ty.clone(),
                        convention: parameter.convention,
                        requires_place: parameter.requires_place,
                        ranked: false,
                        owned: false,
                    });
                }
                let value = call
                    .arguments
                    .iter()
                    .find(|bound| bound.source == parameter.source)
                    .map(|bound| bound.value)
                    .ok_or("a dispatched argument has no occurrence")?;
                let ty = fact_at(&facts.expression_types, value)
                    .cloned()
                    .ok_or("a dispatched argument has no retained type")?;
                let occurrence = occurrences.iter().find(|occurrence| occurrence.id == value);
                Ok(DispatchedArgument {
                    source: parameter.source,
                    value: Some(value),
                    ty,
                    parameter_ty: parameter.parameter_ty.clone(),
                    convention: parameter.convention,
                    requires_place: parameter.requires_place,
                    ranked: matches!(parameter.source, CheckedCallArgumentSource::Positional(_))
                        && occurrence.is_some_and(|occurrence| occurrence.ranking.context_free),
                    owned: occurrence.is_some_and(|occurrence| occurrence.ranking.owned),
                })
            })
            .collect::<Result<Vec<_>, &'static str>>()?;
        match self.bound_witness(
            &ty,
            receiver_convention,
            receiver_transferred,
            false,
            &method,
            &arguments,
        )? {
            BoundWitness::CopyRead => {
                if !self.is_copyable(&ty) {
                    return Err("the instance's type is not copyable");
                }
                drop_call(facts, id);
                // Only a place is marked a copied read: a call's reference
                // result (`pick(a, b).copy()`) is read as the call yields it.
                let call_result = occurrences
                    .iter()
                    .any(|occurrence| occurrence.id == receiver && occurrence.callee.is_some());
                if !call_result && !facts.copy_place_value_uses.contains(&receiver) {
                    facts.copy_place_value_uses.push(receiver);
                    let order =
                        |id: &OccurrenceId| occurrences.iter().position(|found| found.id == *id);
                    facts.copy_place_value_uses.sort_by_key(order);
                }
            }
            BoundWitness::HashLeaf => {
                self.record_hash_leaf(&ty);
                drop_call(facts, id);
            }
            witness @ BoundWitness::Method { declared, .. } => {
                // A receiver the call copies is copied only for a witness
                // that consumes it, as the requirement does.
                if facts.implicitly_copied_consuming_receivers.contains(&id)
                    && !matches!(
                        declared.self_convention,
                        Some(ArgConvention::Var | ArgConvention::Deinit)
                    )
                {
                    return Err("a copied receiver's witness does not consume it");
                }
                self.install_witness(facts, id, &witness, &arguments, occurrences)?;
                // The struct's own method may be a named `deinit self`
                // destructor, which the template's receiver could not name.
                let destroys = matches!(&ty, Ty::Struct(name, _) if self
                    .structs
                    .get(name)
                    .is_some_and(|info| info.explicit_destructors.contains_key(&method)));
                if destroys && !facts.explicit_destroy_calls.contains(&id) {
                    facts.explicit_destroy_calls.push(id);
                    let order =
                        |id: &OccurrenceId| occurrences.iter().position(|found| found.id == *id);
                    facts.explicit_destroy_calls.sort_by_key(order);
                }
            }
        }
        Ok(())
    }

    /// Realize a call a statement embeds that the template dispatched
    /// through a bound (the in-place dunder of `self.items[i] += x` or
    /// `self.total += x` on a bare `T`), on the instance's type of its
    /// receiver, returning the witness.
    ///
    /// The call is not selected at an occurrence, so the witness rewrites
    /// only the contract: its target, result, raised, and parameter types,
    /// as [`Self::realize_bound_dispatch`] writes them for a selected call. A
    /// requirement declared `raises` admits a witness that raises or one that
    /// does not, whose contract then raises nothing. The
    /// witness's parameters are returned for a statement that records them
    /// at its place, as `install_witness` does. A witness with binders of
    /// its own, or no nominal struct's method, is the clone check's to
    /// judge.
    pub(super) fn realize_embedded_dispatch(
        &self,
        types: &[(OccurrenceId, Ty)],
        call: &mut TemplateCallContract,
        receiver: &Ty,
    ) -> Result<EmbeddedWitness, &'static str> {
        let method = mojito_symbol::symbol::split_method_symbol(&call.contract.target)
            .and_then(|(_, method)| method.split('$').next())
            .ok_or("an embedded dispatch names no method")?
            .to_string();
        let arguments = call
            .contract
            .arguments
            .iter()
            .map(|parameter| {
                let value = call
                    .arguments
                    .iter()
                    .find(|bound| bound.source == parameter.source)
                    .map(|bound| bound.value)
                    .ok_or("a dispatched argument has no occurrence")?;
                let ty = fact_at(types, value)
                    .cloned()
                    .ok_or("a dispatched argument has no retained type")?;
                Ok(DispatchedArgument {
                    source: parameter.source,
                    value: Some(value),
                    ty,
                    parameter_ty: parameter.parameter_ty.clone(),
                    convention: parameter.convention,
                    requires_place: parameter.requires_place,
                    ranked: false,
                    owned: false,
                })
            })
            .collect::<Result<Vec<_>, &'static str>>()?;
        let witness = self.bound_witness(
            receiver,
            call.contract.receiver_convention,
            false,
            call.contract.raises.is_some(),
            &method,
            &arguments,
        )?;
        let BoundWitness::Method {
            owner,
            arguments: struct_arguments,
            declared,
            target,
            binders,
            request: None,
            substitution,
        } = witness
        else {
            return Err("an embedded dispatch's witness is no plain struct method");
        };
        if !binders.is_empty() {
            return Err("an embedded dispatch's witness has binders of its own");
        }
        let receiver_ty = Ty::Struct(owner.to_string(), struct_arguments.to_vec());
        let parameter_ty =
            |ty: &Ty| self.witness_parameter_ty(ty, &receiver_ty, &substitution, &binders);
        if declared.params.len() != call.contract.arguments.len() {
            return Err("an embedded dispatch's witness takes other arguments");
        }
        call.contract.target.clone_from(&target);
        call.contract.result_ty = parameter_ty(&declared.ret);
        call.contract.raises = declared
            .raises
            .then(|| declared.error.as_deref().map_or(Ty::Error, &parameter_ty));
        for (argument, declared) in call.contract.arguments.iter_mut().zip(&declared.params) {
            argument.parameter_ty = parameter_ty(declared);
        }
        let parameters = declared
            .names
            .iter()
            .zip(&declared.conventions)
            .zip(&call.contract.arguments)
            .map(|((name, convention), argument)| CallParameterFact {
                name: name.clone(),
                convention: *convention,
                ty: argument.parameter_ty.clone(),
            })
            .collect();
        Ok(EmbeddedWitness { target, parameters })
    }

    /// Realize every inverted write whose receiver the instance makes a
    /// nominal struct: the struct's own `write_to` (or `write_repr_to`) is
    /// selected instead, as `infer_method_call` selects it, and the call
    /// records what a method call with one `mut` argument records. Every
    /// other receiver keeps the inverted write, which names no type. The
    /// calls converted are returned, so the closed-call recipe leaves them.
    pub(super) fn realize_inverted_writes(
        &self,
        facts: &mut CheckedBodyFacts,
        occurrences: &[Occurrence],
    ) -> Result<Vec<OccurrenceId>, &'static str> {
        let inverted: Vec<OccurrenceId> = facts
            .operation_adjustments
            .iter()
            .filter(|(_, adjustment)| {
                matches!(
                    adjustment,
                    SemanticAdjustment::InvertedWrite | SemanticAdjustment::InvertedReprWrite
                )
            })
            .map(|(id, _)| *id)
            .collect();
        let mut converted = Vec::new();
        for id in inverted {
            let occurrence = occurrences
                .iter()
                .find(|occurrence| occurrence.id == id)
                .ok_or("an inverted write is not an occurrence of the instance")?;
            let Some((receiver, method)) = &occurrence.method_call else {
                // A `print`-style inverted write on a direct call names no
                // symbolic receiver.
                continue;
            };
            let receiver = OccurrenceId {
                syntax: *receiver,
                copy: id.copy,
            };
            let ty = fact_at(&facts.expression_types, receiver)
                .cloned()
                .ok_or("an inverted write's receiver has no retained type")?;
            if !nominal_writer_receiver(&ty) {
                continue;
            }
            let [writer] = occurrence.arguments.as_slice() else {
                return Err("an inverted write takes one writer");
            };
            let writer = OccurrenceId {
                syntax: *writer,
                copy: id.copy,
            };
            let writer_ty = fact_at(&facts.expression_types, writer)
                .cloned()
                .ok_or("an inverted write's writer has no retained type")?;
            let arguments = [DispatchedArgument {
                source: CheckedCallArgumentSource::Positional(0),
                value: Some(writer),
                parameter_ty: writer_ty.clone(),
                ty: writer_ty,
                convention: Some(ArgConvention::Mut),
                requires_place: true,
                ranked: false,
                owned: false,
            }];
            let witness = self.bound_witness(&ty, None, false, false, method, &arguments)?;
            let BoundWitness::Method { .. } = &witness else {
                return Err("an inverted write's receiver selects no method");
            };
            facts.operation_adjustments.retain(|(site, _)| *site != id);
            // The inverted write borrowed its receiver in place; a method
            // call on a read `self` records no borrow there.
            facts
                .borrowed_read_call_places
                .retain(|site| *site != receiver);
            facts.selected_calls.push((
                id,
                TemplateCallContract {
                    contract: CheckedCallContract {
                        target: String::new(),
                        raises: None,
                        result_ty: Ty::None,
                        result_adapter: None,
                        receiver_requires_place: false,
                        receiver_elided: false,
                        receiver_convention: None,
                        arguments: Vec::new(),
                        captures: Vec::new(),
                        reference_result: None,
                        parameter_arguments: Vec::new(),
                        param_decls: Vec::new(),
                        boundary: CheckedCallBoundary::default(),
                    },
                    reference_result: None,
                    result_origins: Vec::new(),
                    arguments: Vec::new(),
                    invalidations: Vec::new(),
                },
            ));
            facts.overload_targets.push((id, String::new()));
            facts.call_parameters.push((id, Vec::new()));
            self.install_witness(facts, id, &witness, &arguments, occurrences)?;
            converted.push(id);
        }
        if !converted.is_empty() {
            let order = |id: &OccurrenceId| occurrences.iter().position(|found| found.id == *id);
            facts.selected_calls.sort_by_key(|(id, _)| order(id));
            facts.overload_targets.sort_by_key(|(id, _)| order(id));
            facts.call_parameters.sort_by_key(|(id, _)| order(id));
            facts
                .interior_invalidations
                .sort_by_key(|(id, _)| order(id));
            facts.call_place_uses.sort_by_key(order);
        }
        Ok(converted)
    }

    /// Discharge one checker builtin the template called on a bounded
    /// parameter (obligation 17): the argument the template proved through
    /// the bound must satisfy the same demand at the instance's type. A
    /// hashed value records its leaf, as `is_hashable` does. A hasher the
    /// instance binds to a nominal struct selects that struct's own method
    /// instead ([`Self::realize_nominal_hasher_call`]).
    pub(super) fn realize_bound_builtin(
        &self,
        facts: &mut CheckedBodyFacts,
        id: OccurrenceId,
        builtin: BoundBuiltin,
        occurrences: &[Occurrence],
    ) -> Result<(), &'static str> {
        let occurrence = occurrences
            .iter()
            .find(|occurrence| occurrence.id == id)
            .ok_or("a bound builtin call is not an occurrence of the instance")?;
        let Some((receiver, method)) = &occurrence.method_call else {
            return Err("a bound builtin call is not a method call in the instance");
        };
        let receiver = OccurrenceId {
            syntax: *receiver,
            copy: id.copy,
        };
        if matches!(builtin, BoundBuiltin::Update | BoundBuiltin::UpdateSimd)
            && let Some(ty @ Ty::Struct(..)) = fact_at(&facts.expression_types, receiver).cloned()
        {
            return self.realize_nominal_hasher_call(
                facts,
                occurrence,
                receiver,
                &ty,
                method,
                occurrences,
            );
        }
        for argument in &occurrence.arguments {
            let argument = OccurrenceId {
                syntax: *argument,
                copy: id.copy,
            };
            let ty = fact_at(&facts.expression_types, argument)
                .ok_or("a bound builtin's argument has no retained type")?;
            let accepted = match builtin {
                BoundBuiltin::Update => self.is_hashable(ty),
                BoundBuiltin::UpdateSimd => {
                    let vector = crate::checker::builtins::simd_valued_ty(ty);
                    if vector {
                        self.record_hash_leaf(ty);
                    }
                    vector
                }
                BoundBuiltin::Write => self.printable_argument(ty),
                BoundBuiltin::Finish => false,
            };
            if !accepted {
                return Err(
                    "a bound builtin's argument does not satisfy the bound for the instance",
                );
            }
        }
        Ok(())
    }

    /// The witness the instance's type selects for `method`, from the types
    /// alone.
    ///
    /// A nominal struct's witness is the declaration of that name that binds
    /// the recorded arguments ([`Self::witness_binders`]); of an overload
    /// set, the member that does which the clone check's ranking selects
    /// ([`Self::ranked_member`]), named by its overload symbol.
    fn bound_witness<'a>(
        &'a self,
        receiver: &'a Ty,
        receiver_convention: Option<ArgConvention>,
        receiver_transferred: bool,
        raising: bool,
        method: &str,
        arguments: &[DispatchedArgument],
    ) -> Result<BoundWitness<'a>, &'static str> {
        if method == "copy"
            && arguments.is_empty()
            && mojito_types::types::builtin_copy_is_value_read(receiver)
        {
            return Ok(BoundWitness::CopyRead);
        }
        if method == "__hash__"
            && arguments.len() == 1
            && crate::checker::builtins::builtin_hashable_ty(receiver)
        {
            return Ok(BoundWitness::HashLeaf);
        }
        let Ty::Struct(owner, struct_arguments) = receiver else {
            return Err("a dispatched receiver is neither a built-in value nor a struct");
        };
        let info = self
            .structs
            .get(owner)
            .ok_or("a dispatched receiver's struct is not declared")?;
        let candidates = info
            .methods
            .get(method)
            .map(Vec::as_slice)
            .ok_or("the instance's type declares no witness for the requirement")?;
        let substitution = crate::checker::annotations::struct_subst(&info.decls, struct_arguments);
        let (mut declared, mut binders) = if let [declared] = candidates {
            (
                declared,
                self.witness_binders(
                    declared,
                    receiver,
                    receiver_convention,
                    raising,
                    &substitution,
                    arguments,
                )?,
            )
        } else {
            // The member the clone check's ranking selects on the recorded
            // types must fit the recorded arguments.
            let selected = self.ranked_member(
                candidates,
                receiver,
                receiver_transferred,
                &substitution,
                arguments,
            )?;
            let binders = self
                .witness_binders(
                    selected,
                    receiver,
                    receiver_convention,
                    raising,
                    &substitution,
                    arguments,
                )
                .map_err(|_| "the clone check's ranking selects a member that is no witness")?;
            (selected, binders)
        };
        let overloaded = candidates.len() > 1;
        let request = (!declared.decls.is_empty()).then(|| {
            Box::new(MethodInstantiation {
                owner: owner.clone(),
                owner_arguments: self
                    .instance_arguments(owner, struct_arguments)
                    .unwrap_or_default(),
                method: method.to_string(),
                parameter_names: declared.names.clone(),
                arguments: declared
                    .decls
                    .iter()
                    .filter_map(|decl| binders.get(decl.id()).cloned())
                    .map(TyArg::Ty)
                    .collect(),
            })
        });
        // A binder the instance bakes selects the per-call clone once the
        // elaborator has minted it, as the clone check retargets to it; until
        // then the call names the method itself.
        let baked = request.as_ref().filter(|_| {
            binders
                .values()
                .all(|ty| !mojito_types::types::is_symbolic(ty))
        });
        if let Some(request) = baked {
            if overloaded {
                return Err("the instance bakes a binder of an overloaded witness");
            }
            // A generic owner keys its per-call clone by the instance too.
            let clone = if struct_arguments.is_empty() {
                self.specialized_method_clone(owner, method, &declared.decls, &request.arguments)
            } else {
                self.instance_call_method_clone(
                    owner,
                    struct_arguments,
                    method,
                    &declared.decls,
                    &request.arguments,
                )
            };
            if let Some(clone) = clone {
                let [minted] = info
                    .methods
                    .get(&clone)
                    .map(Vec::as_slice)
                    .ok_or("the instance's per-call witness clone is not declared")?
                else {
                    return Err("the instance's per-call witness clone is overloaded");
                };
                declared = minted;
                binders = self.witness_binders(
                    declared,
                    receiver,
                    receiver_convention,
                    raising,
                    &substitution,
                    arguments,
                )?;
                return Ok(BoundWitness::Method {
                    owner,
                    arguments: struct_arguments,
                    declared,
                    target: format!("{owner}.{clone}"),
                    binders,
                    request: Some(request.clone()),
                    substitution,
                });
            }
        } else if request.is_some()
            && binders
                .values()
                .any(|ty| !mojito_types::types::is_symbolic(ty))
        {
            return Err("the instance would bake some witness binders and not others");
        }
        let target = if self
            .instance_method_clone(owner, method, struct_arguments)
            .is_some()
        {
            if declared.decls.is_empty() {
                self.method_clone_target(owner, method, struct_arguments, declared, &substitution)
                    .ok_or("the instance's witness clone family has no member for it")?
            } else if overloaded {
                return Err("the instance's overloaded witness has binders and a clone family");
            } else {
                // A witness with binders of its own keeps them in its clone,
                // and a lone declaration's clone is a lone clone.
                let clone = self
                    .instance_method_clone(owner, method, struct_arguments)
                    .ok_or("the instance's witness has no clone")?;
                format!("{owner}.{clone}")
            }
        } else {
            let clone_name = mojito_symbol::symbol::instance_method_clone_name(
                method,
                &info.decls,
                struct_arguments,
            );
            // A synthesized trait default is never cloned: the template's
            // body serves every instance. Any other witness missing beside
            // the instance's clones was withheld from it or collapsed.
            let suffix = clone_name
                .as_deref()
                .and_then(|clone| clone.strip_prefix(method));
            if !declared.synthesized_default
                && suffix
                    .is_some_and(|suffix| info.methods.keys().any(|name| name.ends_with(suffix)))
            {
                return Err("the instance has clones, but not of the witness");
            }
            // The erased witness serves the instance, its availability
            // condition judged at the instance's arguments as the clone
            // check judges it.
            if !declared.availability.is_empty()
                && !struct_arguments.is_empty()
                && !self.method_constraints_apply(declared, &[], &info.decls, struct_arguments)
            {
                return Err("the instance's witness is unavailable at the instance");
            }
            if overloaded {
                crate::checker::overload_support::method_lowered_name(
                    owner,
                    method,
                    declared,
                    self.self_instance_ty(owner).as_ref(),
                )
            } else {
                format!("{owner}.{method}")
            }
        };
        Ok(BoundWitness::Method {
            owner,
            arguments: struct_arguments,
            declared,
            target,
            binders,
            request,
            substitution,
        })
    }

    /// How one declaration of the requirement's name binds the recorded
    /// arguments, if it is a witness of the requirement's shape: a read
    /// `self`, one parameter per argument bound with the recorded
    /// convention, a default wherever the call leaves one to the callee, and
    /// no variadic or reference result, raising only where the template's
    /// call may (`raising`).
    /// Each parameter's type, under `Self`, the struct's arguments, and the
    /// witness's own binders, is the argument's recorded type or a bounded
    /// parameter the argument's type satisfies. A binder of the witness is
    /// admitted where exactly one parameter is that binder and the argument
    /// there is itself a bare parameter, so the binding stays symbolic as
    /// the clone check leaves it.
    fn witness_binders(
        &self,
        declared: &MethodSig,
        receiver: &Ty,
        receiver_convention: Option<ArgConvention>,
        raising: bool,
        substitution: &TySubst,
        arguments: &[DispatchedArgument],
    ) -> Result<TySubst, &'static str> {
        let plain = declared.has_self
            && declared.self_convention == receiver_convention
            && declared.params.len() == arguments.len()
            && declared.variadic.is_none()
            && declared.kw_variadic.is_none()
            && (raising || !declared.raises)
            && declared.ref_return.is_none()
            && declared.view_return.is_empty()
            && declared.parametric_origin_writes.is_empty();
        if !plain {
            return Err("the instance's witness is not a plain method of the requirement's shape");
        }
        let mut binders = HashMap::new();
        for decl in &declared.decls {
            let ParamDecl::Type {
                bounds,
                callable_bound: None,
                default: None,
                variadic: false,
                ..
            } = decl
            else {
                return Err("the instance's witness declares a binder that is not a plain type");
            };
            let mut named = declared.params.iter().enumerate().filter(
                |(_, ty)| matches!(ty, Ty::Param { binder, .. } if binder.id == *decl.id()),
            );
            let (Some((index, _)), None) = (named.next(), named.next()) else {
                return Err("the instance's witness binder is not named by exactly one parameter");
            };
            let argument = &arguments[index];
            if argument.value.is_none() {
                return Err("the instance's witness binder is named by a defaulted parameter");
            }
            let argument = &argument.ty;
            let carried = match argument {
                Ty::Param { bounds: given, .. } => bounds.iter().all(|bound| {
                    given
                        .iter()
                        .any(|carried| carried == bound || self.trait_refines(carried, bound))
                }),
                closed if !mojito_types::types::is_symbolic(closed) => {
                    bounds.iter().all(|bound| self.conforms_to(closed, bound))
                }
                _ => false,
            };
            if !carried {
                return Err("an argument does not carry the witness binder's bounds");
            }
            binders.insert(decl.id().clone(), argument.clone());
        }
        for (index, argument) in arguments.iter().enumerate() {
            // A slot the call supplies binds any witness parameter; one it
            // leaves out binds the witness's default, which conformance
            // proved the requirement's.
            if declared.required[index] && argument.value.is_none() {
                return Err("the instance's witness declares no default the call leaves out");
            }
            let convention = declared.conventions[index];
            if convention != argument.convention
                || argument.requires_place
                    != matches!(convention, Some(ArgConvention::Mut | ArgConvention::Ref))
            {
                return Err("the instance's witness binds an argument by another convention");
            }
            let parameter = self.witness_parameter_ty(
                &declared.params[index],
                receiver,
                substitution,
                &binders,
            );
            // A literal binds the closed type the requirement declares,
            // which a witness declares too.
            let accepted = parameter == argument.ty
                || (parameter == argument.parameter_ty
                    && !mojito_types::types::is_symbolic(&parameter))
                || matches!(&parameter, Ty::Param { bounds, .. }
                    if bounds.iter().all(|bound| self.conforms_to(&argument.ty, bound)));
            if !accepted {
                return Err("an argument does not fit the instance's witness parameter");
            }
        }
        Ok(binders)
    }

    /// The member of an overload set the clone check's ranking selects on
    /// the recorded argument types, as `select_method_overload` selects it:
    /// the fewest-ranked member among those that take the arguments
    /// ([`Self::member_rank`]), then the fewest `SIMD`-pattern erasures, then
    /// the one whose receiver convention matches the call's transfer. A lone
    /// member of the arguments' arity is selected unranked, and a static
    /// member never competes beside the instance members, as the clone check
    /// drops it.
    /// Arguments or members the recorded types cannot rank, and a tie those
    /// terms leave, are the clone check's to judge.
    fn ranked_member<'a>(
        &self,
        candidates: &'a [MethodSig],
        receiver: &Ty,
        receiver_transferred: bool,
        substitution: &TySubst,
        arguments: &[DispatchedArgument],
    ) -> Result<&'a MethodSig, &'static str> {
        let taking: Vec<&MethodSig> = candidates
            .iter()
            .filter(|member| member.has_self && takes_arity(member, arguments.len()))
            .collect();
        if let [member] = taking.as_slice() {
            return Ok(member);
        }
        if !arguments.iter().all(|argument| argument.ranked) {
            return Err("an argument's type may come from the witness parameter it is handed to");
        }
        let mut ranked = Vec::new();
        for member in taking {
            if let Some(rank) = self.member_rank(member, receiver, substitution, arguments)? {
                ranked.push((member, rank));
            }
        }
        let best = ranked
            .iter()
            .map(|(_, (rank, _))| *rank)
            .min()
            .ok_or("no member of the instance's overloaded witness takes the arguments")?;
        ranked.retain(|(_, (rank, _))| *rank == best);
        let fewest_erasures = ranked
            .iter()
            .map(|(_, (_, erasures))| *erasures)
            .min()
            .unwrap_or(0);
        ranked.retain(|(_, (_, erasures))| *erasures == fewest_erasures);
        if let [(member, _)] = ranked.as_slice() {
            return Ok(member);
        }
        let mut consuming = ranked.iter().filter(|(member, _)| {
            matches!(
                member.self_convention,
                Some(ArgConvention::Var | ArgConvention::Deinit)
            ) == receiver_transferred
        });
        match (consuming.next(), consuming.next()) {
            (Some((member, _)), None) => Ok(member),
            _ => Err("the clone check's ranking leaves the instance's overloaded witness tied"),
        }
    }

    /// A member's rank and `SIMD`-pattern erasures on the recorded argument
    /// types, as `score_method_call` scores it, or `None` where some
    /// argument reaches no parameter of it or its availability condition
    /// fails at the instance: the conversions, the `**kwargs` bit, its own
    /// binders' count and generic bit, and the copies a `var` parameter
    /// makes of a place. A defaulted parameter past the arguments is not
    /// scored. A variadic member, a reference parameter, a parameter left
    /// symbolic, and binders [`Self::member_parameters`] cannot infer are
    /// the clone check's to rank.
    fn member_rank(
        &self,
        member: &MethodSig,
        receiver: &Ty,
        substitution: &TySubst,
        arguments: &[DispatchedArgument],
    ) -> Result<Option<(usize, usize)>, &'static str> {
        if member.variadic.is_some() {
            return Err("a member of the instance's overloaded witness is variadic");
        }
        let Some(parameters) = self.member_parameters(member, receiver, substitution, arguments)?
        else {
            return Ok(None);
        };
        let mut conversions = 0;
        let mut simd_erasures = 0;
        let mut binding = crate::checker::overload_support::ArgumentBinding::new(None);
        for (index, (argument, parameter)) in arguments.iter().zip(&parameters).enumerate() {
            if matches!(parameter, Ty::Ref(_))
                || (*parameter != argument.ty && mojito_types::types::is_symbolic(parameter))
            {
                return Err(
                    "a member of the instance's overloaded witness has a parameter \
                     the recorded types cannot rank",
                );
            }
            let reaches = self.value_coerces(&argument.ty, parameter)
                || self
                    .implicit_conversion_target(&argument.ty, parameter)
                    .is_ok_and(|target| target.is_some());
            if !reaches {
                return Ok(None);
            }
            conversions +=
                crate::checker::overload_support::conversion_count(&argument.ty, parameter);
            simd_erasures += usize::from(
                matches!(member.params[index], Ty::Param { .. })
                    && mojito_types::types::simd_shape(&argument.ty).is_some(),
            );
            binding.bind_value(member.conventions[index], false, argument.owned, false);
        }
        let baked = crate::checker::declarations::baked_decl_count(&member.decls);
        let rank = crate::checker::overload_support::overload_rank(
            conversions,
            member.kw_variadic.is_some(),
            baked,
            baked > 0,
        ) + binding.rank();
        Ok(Some((rank, simd_erasures)))
    }

    /// A member's parameter types at the recorded arguments, under the
    /// receiver and the member's own binders as the clone check binds them,
    /// or `None` where the member does not take the arguments.
    ///
    /// Over closed argument types the binders are inferred as
    /// `instantiate_method_generics` infers them, from the argument types
    /// alone — inside a parameter type (`List[U]`), a value or `DType`
    /// binder, a literal's own type — and the availability condition is
    /// judged at the inferred arguments; a member that fails either takes
    /// nothing, as the clone check drops it. Over a caller binder the
    /// instance keeps, each binder must be a plain type binder only one
    /// parameter names, as that parameter's whole type, bound to the
    /// argument's type when that carries its bounds; any other binder is
    /// the clone check's to infer.
    fn member_parameters(
        &self,
        member: &MethodSig,
        receiver: &Ty,
        substitution: &TySubst,
        arguments: &[DispatchedArgument],
    ) -> Result<Option<Vec<Ty>>, &'static str> {
        const UNINFERRED: &str = "a member's own binder is not inferred from one argument's type";
        let Ty::Struct(owner, struct_arguments) = receiver else {
            return Err("a dispatched receiver is not a struct");
        };
        let info = self
            .structs
            .get(owner)
            .ok_or("a dispatched receiver's struct is not declared")?;
        if arguments
            .iter()
            .all(|argument| !mojito_types::types::is_symbolic(&argument.ty))
        {
            let patterns: Vec<Ty> = member.params[..arguments.len()]
                .iter()
                .map(|ty| crate::checker::generics::substitute_at(ty, info, struct_arguments))
                .collect();
            let actuals: Vec<Ty> = arguments
                .iter()
                .map(|argument| argument.ty.clone())
                .collect();
            let Ok((binders, method_arguments)) =
                self.resolve_use_params(owner, &member.decls, &[], &patterns, &actuals)
            else {
                return Ok(None);
            };
            if !self.method_constraints_apply(
                member,
                &method_arguments,
                &info.decls,
                struct_arguments,
            ) {
                return Ok(None);
            }
            let values = Self::value_argument_environment(&member.decls, &method_arguments);
            return Ok(patterns
                .iter()
                .map(|ty| {
                    let substituted =
                        self.resolve_assoc_ty(&mojito_types::types::substitute(ty, &binders));
                    self.resolve_dependent_ty(&substituted, &values)
                })
                .collect::<Result<Vec<_>, _>>()
                .ok());
        }
        let mut binders = HashMap::new();
        for decl in &member.decls {
            let ParamDecl::Type {
                bounds,
                callable_bound: None,
                default: None,
                variadic: false,
                ..
            } = decl
            else {
                return Err(UNINFERRED);
            };
            let names = |ty: &Ty| matches!(ty, Ty::Param { binder, .. } if binder.id == *decl.id());
            let mut mentioning = member
                .params
                .iter()
                .enumerate()
                .filter(|(_, ty)| mojito_types::types::mentions(ty, &names));
            let (Some((index, parameter)), None) = (mentioning.next(), mentioning.next()) else {
                return Err(UNINFERRED);
            };
            let argument = arguments.get(index).ok_or(UNINFERRED)?;
            if !names(parameter) {
                return Err(UNINFERRED);
            }
            let carried = match &argument.ty {
                Ty::IntLiteral | Ty::FloatLiteral | Ty::StringLiteral => return Err(UNINFERRED),
                Ty::Param { bounds: given, .. } => {
                    if !bounds.iter().all(|bound| {
                        given
                            .iter()
                            .any(|carried| carried == bound || self.trait_refines(carried, bound))
                    }) {
                        return Err(UNINFERRED);
                    }
                    true
                }
                closed if !mojito_types::types::is_symbolic(closed) => {
                    bounds.iter().all(|bound| self.conforms_to(closed, bound))
                }
                _ => return Err(UNINFERRED),
            };
            if !carried {
                return Ok(None);
            }
            binders.insert(decl.id().clone(), argument.ty.clone());
        }
        let method_arguments: Vec<TyArg> = member
            .decls
            .iter()
            .filter_map(|decl| binders.get(decl.id()).cloned())
            .map(TyArg::Ty)
            .collect();
        if !self.method_constraints_apply(member, &method_arguments, &info.decls, struct_arguments)
        {
            return Ok(None);
        }
        Ok(Some(
            member.params[..arguments.len()]
                .iter()
                .map(|ty| self.witness_parameter_ty(ty, receiver, substitution, &binders))
                .collect(),
        ))
    }

    /// Realize `hasher.update(x)` or `hasher._update_with_simd(x)` whose
    /// receiver the instance binds to a nominal struct, as `infer_method_call`
    /// selects it there: the struct's own method, not the checker builtin
    /// the bound proved. The call records what a method call on a `mut`
    /// receiver records — the witness's contract, the receiver's generation
    /// refreshed at the call — and each read argument is borrowed where it
    /// lies, or, a temporary, borrowed and destroyed after the call. A
    /// transferred argument, or a place that overlaps the receiver, is the
    /// clone check's to judge.
    fn realize_nominal_hasher_call(
        &self,
        facts: &mut CheckedBodyFacts,
        occurrence: &Occurrence,
        receiver: OccurrenceId,
        ty: &Ty,
        method: &str,
        occurrences: &[Occurrence],
    ) -> Result<(), &'static str> {
        let id = occurrence.id;
        let root = fact_at(&facts.expression_bindings, receiver)
            .cloned()
            .ok_or("a hasher receiver is not a bound place")?;
        let arguments = occurrence
            .arguments
            .iter()
            .enumerate()
            .map(|(index, argument)| {
                let value = OccurrenceId {
                    syntax: *argument,
                    copy: id.copy,
                };
                let found = occurrences
                    .iter()
                    .find(|occurrence| occurrence.id == value)
                    .ok_or("a hasher argument is not an occurrence of the instance")?;
                if found.transfer {
                    return Err("a hasher argument is transferred");
                }
                let ty = fact_at(&facts.expression_types, value)
                    .cloned()
                    .ok_or("a hasher argument has no retained type")?;
                Ok(DispatchedArgument {
                    source: CheckedCallArgumentSource::Positional(index),
                    value: Some(value),
                    parameter_ty: ty.clone(),
                    ty,
                    convention: None,
                    requires_place: false,
                    ranked: false,
                    owned: found.ranking.owned,
                })
            })
            .collect::<Result<Vec<_>, &'static str>>()?;
        for argument in arguments.iter().filter(|argument| !argument.owned) {
            if argument
                .value
                .and_then(|value| fact_at(&facts.expression_bindings, value))
                .is_none_or(|bound| *bound == root)
            {
                return Err("a hasher argument's place is unbound or overlaps the receiver");
            }
        }
        let witness = self.bound_witness(
            ty,
            Some(ArgConvention::Mut),
            false,
            false,
            method,
            &arguments,
        )?;
        let BoundWitness::Method { .. } = &witness else {
            return Err("a hasher receiver selects no method");
        };
        let invalidation = TemplateInvalidation {
            root,
            path: Vec::new(),
            except: None,
            include_base_generation: false,
        };
        facts.selected_calls.push((
            id,
            TemplateCallContract {
                contract: CheckedCallContract {
                    target: String::new(),
                    raises: None,
                    result_ty: Ty::None,
                    result_adapter: None,
                    receiver_requires_place: true,
                    receiver_elided: false,
                    receiver_convention: Some(ArgConvention::Mut),
                    arguments: Vec::new(),
                    captures: Vec::new(),
                    reference_result: None,
                    parameter_arguments: Vec::new(),
                    param_decls: Vec::new(),
                    boundary: CheckedCallBoundary::default(),
                },
                reference_result: None,
                result_origins: Vec::new(),
                arguments: Vec::new(),
                invalidations: vec![invalidation.clone()],
            },
        ));
        facts.overload_targets.push((id, String::new()));
        facts.call_parameters.push((id, Vec::new()));
        facts.interior_invalidations.push((id, vec![invalidation]));
        self.install_witness(facts, id, &witness, &arguments, occurrences)?;
        for argument in &arguments {
            let tables = if argument.owned {
                vec![
                    &mut facts.read_temporary_arguments,
                    &mut facts.unconsumed_temporaries,
                ]
            } else {
                vec![&mut facts.borrowed_read_call_places]
            };
            let Some(value) = argument.value else {
                continue;
            };
            for table in tables {
                if !table.contains(&value) {
                    table.push(value);
                }
            }
        }
        let order = |id: &OccurrenceId| occurrences.iter().position(|found| found.id == *id);
        facts.selected_calls.sort_by_key(|(id, _)| order(id));
        facts.overload_targets.sort_by_key(|(id, _)| order(id));
        facts.call_parameters.sort_by_key(|(id, _)| order(id));
        facts
            .interior_invalidations
            .sort_by_key(|(id, _)| order(id));
        facts.method_instantiations.sort_by_key(|(id, _)| order(id));
        facts.read_temporary_arguments.sort_by_key(order);
        facts.unconsumed_temporaries.sort_by_key(order);
        facts.borrowed_read_call_places.sort_by_key(order);
        Ok(())
    }

    /// Write a struct witness into the instance's facts at `id`: the
    /// contract's target, result, parameter types, and binders; the call
    /// parameters; the overload target; the per-call request a witness with
    /// binders records; and the struct application a generic receiver
    /// reaches.
    fn install_witness(
        &self,
        facts: &mut CheckedBodyFacts,
        id: OccurrenceId,
        witness: &BoundWitness<'_>,
        arguments: &[DispatchedArgument],
        occurrences: &[Occurrence],
    ) -> Result<(), &'static str> {
        let BoundWitness::Method {
            owner,
            arguments: struct_arguments,
            declared,
            target,
            binders,
            request,
            substitution,
        } = witness
        else {
            return Err("a witness without a method has nothing to install");
        };
        let receiver_ty = Ty::Struct((*owner).to_string(), struct_arguments.to_vec());
        let parameter_tys: Vec<Ty> = declared
            .params
            .iter()
            .map(|ty| self.witness_parameter_ty(ty, &receiver_ty, substitution, binders))
            .collect();
        let result_ty =
            self.witness_parameter_ty(&declared.ret, &receiver_ty, substitution, binders);
        // A witness whose own declared result carries loans hands back a view
        // of its receiver, as `infer_method_call` records it.
        let view_result = declared.ref_return.is_none()
            && !matches!(
                declared.self_convention,
                Some(ArgConvention::Var | ArgConvention::Deinit)
            )
            && matches!(result_ty, Ty::Struct(..))
            && self.type_carries_loans(&result_ty)
            && self.type_carries_loans(declared.template_ret.as_ref().unwrap_or(&declared.ret));
        if view_result {
            if !declared.view_return_interior.is_empty() || !declared.view_return.is_empty() {
                return Err("a witness's view result binds origin slots of its own");
            }
            if !self.type_carries_loans(&receiver_ty) {
                return Err("a witness's view result needs its owning receiver materialized");
            }
        }
        let source = occurrences
            .iter()
            .find(|occurrence| occurrence.id == id)
            .and_then(|occurrence| occurrence.span.source.as_deref());
        // A generic-struct receiver records its application, as any method
        // call's does, from a source that records applications at all.
        let application = ((*owner).to_string(), struct_arguments.to_vec());
        if !struct_arguments.is_empty()
            && source.is_some()
            && !crate::checker::overload_support::is_bundled_module_source(source)
            && !facts.struct_applications.contains(&application)
        {
            facts.struct_applications.push(application);
        }
        // A kept argument's generation is refreshed at the argument, below
        // its own binding, as `solve_call_origins` records for a `mut` one.
        let invalidations = arguments
            .iter()
            .map(|argument| {
                if !argument.requires_place {
                    return Ok(Vec::new());
                }
                let root = argument
                    .value
                    .and_then(|value| fact_at(&facts.expression_bindings, value))
                    .cloned()
                    .ok_or("a kept argument of a witness is not a bound place")?;
                Ok(vec![TemplateInvalidation {
                    root,
                    path: Vec::new(),
                    except: None,
                    include_base_generation: false,
                }])
            })
            .collect::<Result<Vec<_>, &'static str>>()?;
        let call = &mut facts
            .selected_calls
            .iter_mut()
            .find(|(site, _)| *site == id)
            .ok_or("a bound dispatch lost its contract")?
            .1;
        // A literal the requirement's closed parameter materialized is
        // materialized to the witness's, the same type.
        let boundaries = arguments
            .iter()
            .zip(&invalidations)
            .filter_map(|(argument, invalidations)| {
                let value = argument.value?;
                let adjustments = call
                    .arguments
                    .iter()
                    .find(|bound| bound.source == argument.source)
                    .map(|bound| bound.adjustments.clone())
                    .unwrap_or_default();
                Some(TemplateArgumentBoundary {
                    source: argument.source,
                    value,
                    adjustments,
                    invalidations: invalidations.clone(),
                })
            })
            .collect();
        call.contract.target.clone_from(target);
        call.contract.result_ty = result_ty;
        call.contract.param_decls.clone_from(&declared.decls);
        call.contract.arguments = arguments
            .iter()
            .zip(&parameter_tys)
            .map(|(argument, parameter_ty)| CheckedCallArgument {
                source: argument.source,
                parameter_ty: parameter_ty.clone(),
                requires_place: argument.requires_place,
                convention: argument.convention,
            })
            .collect();
        call.arguments = boundaries;
        set_fact(
            &mut facts.call_parameters,
            id,
            declared
                .names
                .iter()
                .zip(&declared.conventions)
                .zip(&parameter_tys)
                .map(|((name, convention), ty)| CallParameterFact {
                    name: name.clone(),
                    convention: *convention,
                    ty: ty.clone(),
                })
                .collect(),
        );
        set_fact(&mut facts.overload_targets, id, target.clone());
        if view_result && fact_at(&facts.operation_adjustments, id).is_none() {
            facts.operation_adjustments.push((
                id,
                SemanticAdjustment::BorrowViewResult { materialized: None },
            ));
            let order = |id: &OccurrenceId| occurrences.iter().position(|found| found.id == *id);
            facts
                .operation_adjustments
                .sort_by_key(|(site, _)| order(site));
        }
        for (argument, invalidations) in arguments.iter().zip(invalidations) {
            let (true, Some(value)) = (argument.requires_place, argument.value) else {
                continue;
            };
            if !facts.call_place_uses.contains(&value) {
                facts.call_place_uses.push(value);
            }
            match facts
                .interior_invalidations
                .iter_mut()
                .find(|(site, _)| *site == value)
            {
                Some(entry) => {
                    for invalidation in invalidations {
                        if !entry.1.contains(&invalidation) {
                            entry.1.push(invalidation);
                        }
                    }
                }
                None => facts.interior_invalidations.push((value, invalidations)),
            }
        }
        if let Some(request) = request {
            match facts
                .method_instantiations
                .iter_mut()
                .find(|(site, _)| *site == id)
            {
                Some(entry) => entry.1 = (**request).clone(),
                None => facts.method_instantiations.push((id, (**request).clone())),
            }
        }
        if !facts.effect_free_callees.contains(target) {
            facts.effect_free_callees.push(target.clone());
        }
        Ok(())
    }

    /// A witness's declared type under the receiver, the struct's arguments,
    /// and the witness's own binder bindings.
    fn witness_parameter_ty(
        &self,
        declared: &Ty,
        receiver: &Ty,
        substitution: &TySubst,
        binders: &TySubst,
    ) -> Ty {
        // The receiver's origin tail binds the struct's own origin binders
        // before `Self` and the type arguments go in, whose binders share
        // their id space (`crate::checker::generics::substitute_at`).
        let declared = match receiver {
            Ty::Struct(name, arguments) => self.structs.get(name).map_or_else(
                || declared.clone(),
                |info| {
                    crate::checker::origins::substitute_struct_origin_tails(
                        declared,
                        &info.tail_origin_bindings(arguments),
                    )
                },
            ),
            _ => declared.clone(),
        };
        let ty = crate::checker::generics::substitute_self(&declared, receiver);
        let ty = mojito_types::types::substitute(&ty, substitution);
        self.resolve_assoc_ty(&mojito_types::types::substitute(&ty, binders))
    }
}

/// Whether a `write_to` receiver of this type selects the type's own method
/// rather than the inverted write: a nominal struct other than the stdlib
/// `String` and the slice family, as `infer_method_call` decides.
fn nominal_writer_receiver(ty: &Ty) -> bool {
    match ty {
        Ty::Struct(name, arguments) if arguments.is_empty() => {
            !mojito_types::types::is_stdlib_string_struct(name)
                && !matches!(name.as_str(), "Slice" | "ContiguousSlice" | "StridedSlice")
        }
        Ty::Struct(..) => true,
        _ => false,
    }
}

/// Whether a declaration could take `count` positional arguments: its
/// required positional parameters are no more, and its positional
/// parameters (or a variadic) no fewer, with no keyword-only parameter
/// required.
fn takes_arity(declared: &MethodSig, count: usize) -> bool {
    let positional = declared
        .keyword_only
        .unwrap_or(declared.params.len())
        .min(declared.required.len());
    let required = |range: &[bool]| range.iter().filter(|required| **required).count();
    required(&declared.required[..positional]) <= count
        && required(&declared.required[positional..]) == 0
        && (count <= positional || declared.variadic.is_some())
}

/// Forget a dispatched call that selects no callee for the instance: its
/// contract, its overload target, and its (empty) call parameters.
fn drop_call(facts: &mut CheckedBodyFacts, id: OccurrenceId) {
    facts.selected_calls.retain(|(site, _)| *site != id);
    facts.overload_targets.retain(|(site, _)| *site != id);
    facts.call_parameters.retain(|(site, _)| *site != id);
}

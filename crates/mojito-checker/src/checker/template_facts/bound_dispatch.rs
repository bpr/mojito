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
use mojito_types::types::{ParamDecl, Ty, TyArg};
use std::collections::HashMap;

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
        binders: HashMap<String, Ty>,
        /// The struct's own substitution under the receiver's arguments.
        substitution: HashMap<String, Ty>,
    },
}

/// One argument of a dispatched call as the template recorded it: its
/// occurrence, its recorded type, the convention it was bound with, and
/// whether the callee needs the caller's place.
struct DispatchedArgument {
    value: OccurrenceId,
    ty: Ty,
    convention: Option<ArgConvention>,
    requires_place: bool,
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
        let call =
            fact_at(&facts.selected_calls, id).ok_or("a bound dispatch lost its contract")?;
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
                let ty = fact_at(&facts.expression_types, value)
                    .cloned()
                    .ok_or("a dispatched argument has no retained type")?;
                Ok(DispatchedArgument {
                    value,
                    ty,
                    convention: parameter.convention,
                    requires_place: parameter.requires_place,
                })
            })
            .collect::<Result<Vec<_>, &'static str>>()?;
        match self.bound_witness(&ty, &method, &arguments)? {
            BoundWitness::CopyRead => {
                if !self.is_copyable(&ty) {
                    return Err("the instance's type is not copyable");
                }
                drop_call(facts, id);
                if !facts.copy_place_value_uses.contains(&receiver) {
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
            witness @ BoundWitness::Method { .. } => {
                self.install_witness(facts, id, &witness, &arguments, occurrences)?;
            }
        }
        Ok(())
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
                value: writer,
                ty: writer_ty,
                convention: Some(ArgConvention::Mut),
                requires_place: true,
            }];
            let witness = self.bound_witness(&ty, method, &arguments)?;
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
    /// hashed value records its leaf, as `is_hashable` does.
    pub(super) fn realize_bound_builtin(
        &self,
        facts: &CheckedBodyFacts,
        id: OccurrenceId,
        builtin: BoundBuiltin,
        occurrences: &[Occurrence],
    ) -> Result<(), &'static str> {
        let occurrence = occurrences
            .iter()
            .find(|occurrence| occurrence.id == id)
            .ok_or("a bound builtin call is not an occurrence of the instance")?;
        if occurrence.method_call.is_none() {
            return Err("a bound builtin call is not a method call in the instance");
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
    /// A nominal struct's witness is the lone declaration of that name whose
    /// shape is the requirement's: a read `self`, one parameter per
    /// argument bound with the recorded convention, and no variadic, default,
    /// `raises`, or reference result. Each parameter's type, under `Self`,
    /// the struct's arguments, and the witness's own binders, is the
    /// argument's recorded type or a bounded parameter the argument's type
    /// satisfies. A binder of the witness is admitted where exactly one
    /// parameter is that binder and the argument there is itself a bare
    /// parameter, so the binding stays symbolic as the clone check leaves it.
    fn bound_witness<'a>(
        &'a self,
        receiver: &'a Ty,
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
        let [declared] = info
            .methods
            .get(method)
            .map(Vec::as_slice)
            .ok_or("the instance's type declares no witness for the requirement")?
        else {
            return Err("the instance's type overloads the requirement");
        };
        let plain = declared.has_self
            && declared.self_convention.is_none()
            && declared.params.len() == arguments.len()
            && declared.required.iter().all(|required| *required)
            && declared.variadic.is_none()
            && declared.kw_variadic.is_none()
            && !declared.raises
            && declared.ref_return.is_none()
            && declared.view_return.is_empty()
            && declared.parametric_origin_writes.is_empty();
        if !plain {
            return Err("the instance's witness is not a plain method of the requirement's shape");
        }
        let substitution = crate::checker::annotations::struct_subst(&info.decls, struct_arguments);
        let mut binders = HashMap::new();
        for decl in &declared.decls {
            let ParamDecl::Type {
                name,
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
                |(_, ty)| matches!(ty, Ty::Param { name: parameter, .. } if parameter == name),
            );
            let (Some((index, _)), None) = (named.next(), named.next()) else {
                return Err("the instance's witness binder is not named by exactly one parameter");
            };
            let argument = &arguments[index].ty;
            let Ty::Param { bounds: given, .. } = argument else {
                return Err("the instance would bake a witness binder");
            };
            if !bounds.iter().all(|bound| {
                given
                    .iter()
                    .any(|carried| carried == bound || self.trait_refines(carried, bound))
            }) {
                return Err("an argument does not carry the witness binder's bounds");
            }
            binders.insert(name.clone(), argument.clone());
        }
        for (index, argument) in arguments.iter().enumerate() {
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
                &substitution,
                &binders,
            );
            let accepted = parameter == argument.ty
                || matches!(&parameter, Ty::Param { bounds, .. }
                    if bounds.iter().all(|bound| self.conforms_to(&argument.ty, bound)));
            if !accepted {
                return Err("an argument does not fit the instance's witness parameter");
            }
        }
        let target = if self
            .instance_method_clone(owner, method, struct_arguments)
            .is_some()
        {
            if declared.decls.is_empty() {
                self.method_clone_target(owner, method, struct_arguments, declared, &substitution)
                    .ok_or("the instance's witness clone family has no member for it")?
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
            let suffix = clone_name
                .as_deref()
                .and_then(|clone| clone.strip_prefix(method));
            if suffix.is_some_and(|suffix| info.methods.keys().any(|name| name.ends_with(suffix))) {
                return Err("the instance has clones, but not of the witness");
            }
            if !declared.availability.is_empty() && !struct_arguments.is_empty() {
                return Err("the instance's witness has an availability condition and no clone");
            }
            format!("{owner}.{method}")
        };
        Ok(BoundWitness::Method {
            owner,
            arguments: struct_arguments,
            declared,
            target,
            binders,
            substitution,
        })
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
                let root = fact_at(&facts.expression_bindings, argument.value)
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
        let boundaries = arguments
            .iter()
            .zip(&invalidations)
            .enumerate()
            .map(
                |(index, (argument, invalidations))| TemplateArgumentBoundary {
                    source: CheckedCallArgumentSource::Positional(index),
                    value: argument.value,
                    adjustments: Vec::new(),
                    invalidations: invalidations.clone(),
                },
            )
            .collect();
        let call = &mut facts
            .selected_calls
            .iter_mut()
            .find(|(site, _)| *site == id)
            .ok_or("a bound dispatch lost its contract")?
            .1;
        call.contract.target.clone_from(target);
        call.contract.result_ty = result_ty;
        call.contract.param_decls.clone_from(&declared.decls);
        call.contract.arguments = arguments
            .iter()
            .zip(&parameter_tys)
            .enumerate()
            .map(|(index, (argument, parameter_ty))| CheckedCallArgument {
                source: CheckedCallArgumentSource::Positional(index),
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
        for (argument, invalidations) in arguments.iter().zip(invalidations) {
            if !argument.requires_place {
                continue;
            }
            if !facts.call_place_uses.contains(&argument.value) {
                facts.call_place_uses.push(argument.value);
            }
            match facts
                .interior_invalidations
                .iter_mut()
                .find(|(site, _)| *site == argument.value)
            {
                Some(entry) => {
                    for invalidation in invalidations {
                        if !entry.1.contains(&invalidation) {
                            entry.1.push(invalidation);
                        }
                    }
                }
                None => facts
                    .interior_invalidations
                    .push((argument.value, invalidations)),
            }
        }
        if !declared.decls.is_empty() {
            let request = MethodInstantiation {
                owner: (*owner).to_string(),
                owner_arguments: self
                    .instance_arguments(owner, struct_arguments)
                    .unwrap_or_default(),
                method: method_source_name(target, owner).to_string(),
                parameter_names: declared.names.clone(),
                arguments: declared
                    .decls
                    .iter()
                    .filter_map(|decl| binders.get(decl.name()).cloned())
                    .map(TyArg::Ty)
                    .collect(),
            };
            match facts
                .method_instantiations
                .iter_mut()
                .find(|(site, _)| *site == id)
            {
                Some(entry) => entry.1 = request,
                None => facts.method_instantiations.push((id, request)),
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
        substitution: &HashMap<String, Ty>,
        binders: &HashMap<String, Ty>,
    ) -> Ty {
        let ty = crate::checker::generics::substitute_self(declared, receiver);
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

/// The source method name a lowered target names: the clone tag and the
/// overload suffix stripped.
fn method_source_name<'a>(target: &'a str, owner: &str) -> &'a str {
    let method = target
        .strip_prefix(owner)
        .and_then(|rest| rest.strip_prefix('.'))
        .unwrap_or(target);
    method.split('$').next().unwrap_or(method)
}

/// Forget a dispatched call that selects no callee for the instance: its
/// contract, its overload target, and its (empty) call parameters.
fn drop_call(facts: &mut CheckedBodyFacts, id: OccurrenceId) {
    facts.selected_calls.retain(|(site, _)| *site != id);
    facts.overload_targets.retain(|(site, _)| *site != id);
    facts.call_parameters.retain(|(site, _)| *site != id);
}

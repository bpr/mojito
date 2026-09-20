//! Checked-template capture and instance derivation.
//!
//! A generic declaration's body is inferred once, with its parameters
//! symbolic. [`Checker::check_def_body`] retains what that inference recorded
//! as a [`CheckedTemplate`], keyed by the body's own syntax occurrences, and
//! serves a later clone of the declaration from it: the clone's facts are the
//! template's, with the instance's arguments substituted and every occurrence
//! and binding identity remapped. A body outside every enabled class keeps
//! the clone check, and says why.
//!
//! The fact tables are enumerated by [`FactTable`]; `span_table` maps each
//! onto the checker's own storage, so a table without a derivation recipe
//! refuses a body instead of being dropped silently. The design record is
//! `docs/notes/instantiation-from-template.md`.

use super::{Checker, callable_lowered_name};
use mojito_ast::ast::{Expr, ExprKind, Stmt, StmtKind};
use mojito_checked::templates::{
    CallParameterFact, CheckedBodyFacts, CheckedTemplate, FactTable, IncompleteReason,
    InstanceName, InstanceTrace, OccurrenceId, TemplateClass, TemplateCoverage, TemplateId,
    TemplateInvalidation, TemplateObligation, TemplateOwner, TemplateProducer,
};
use mojito_common::error::TypeError;
use mojito_common::timing;
use mojito_common::token::SourceSpan;
use mojito_common::token::SyntaxId;
use mojito_types::origin::OwnerId;
use mojito_types::types::{ParamDecl, Ty};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

/// Which kind of declaration a body inference visit belongs to, for the
/// `body_inference.*` timing counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BodyClass {
    /// A declaration with no compile-time parameters of its own or of an
    /// enclosing struct.
    Plain,
    /// A generic declaration checked with its parameters symbolic.
    Template,
    /// A declaration the elaborator generated from a template.
    Clone,
}

impl BodyClass {
    pub(super) const fn of(generated: bool, generic: bool) -> Self {
        if generated {
            Self::Clone
        } else if generic {
            Self::Template
        } else {
            Self::Plain
        }
    }

    const fn counter(self) -> &'static str {
        match self {
            Self::Plain => "body_inference.plain",
            Self::Template => "body_inference.template",
            Self::Clone => "body_inference.clone",
        }
    }
}

/// Table sizes and the binding-identity watermark just before a body is
/// inferred, so capture can tell exactly what that one inference recorded.
pub(super) struct BodyFactBaseline {
    tables: [usize; FactTable::ALL.len()],
    unkeyed: [(&'static str, usize); UNKEYED_STORES],
    owner_start: u32,
}

/// The binding identities of a declaration's own parameters.
pub(super) struct BodyParams {
    /// Runtime parameters, in declaration order.
    runtime: Vec<Option<OwnerId>>,
    /// Value parameters, bound as locals while the body checks symbolically.
    compile_time: Vec<(String, OwnerId)>,
}

/// One statement or expression occurrence of a body.
struct Occurrence {
    /// The identity it had before the final re-key, shared with the template
    /// occurrence it was copied from.
    id: OccurrenceId,
    span: SourceSpan,
    /// The name a direct call is written with.
    callee: Option<String>,
    /// A direct call's positional arguments.
    arguments: Vec<SyntaxId>,
    /// Whether this is a bare identifier.
    identifier: bool,
}

impl Checker {
    pub(super) fn count_body_inference(class: BodyClass, name: impl FnOnce() -> String) {
        timing::count(class.counter(), 1);
        timing::note(class.counter(), name);
    }

    /// Check a module-level or nested `def` body, or serve it from a checked
    /// template.
    ///
    /// A clone the elaborator traced to a certified template has the
    /// template's facts installed and is not inferred. Any other body is
    /// inferred by `check_block`; a module-level generic one then has its
    /// facts retained for its instances. With fact verification on, a
    /// derivable clone is inferred as well and the two fact bundles must
    /// agree.
    pub(super) fn check_def_body(
        &mut self,
        stmt: &Stmt,
        decls: &[ParamDecl],
        ret_ty: &Ty,
        module_level: bool,
    ) -> Result<(), TypeError> {
        let StmtKind::Def {
            name, params, body, ..
        } = &stmt.kind
        else {
            return Err(TypeError::InvariantViolation(
                "check_def_body requires a function declaration".to_string(),
            ));
        };
        let param_owners = BodyParams {
            runtime: params
                .iter()
                .map(|param| self.lookup_owner(&param.name))
                .collect(),
            compile_time: decls
                .iter()
                .filter_map(|decl| match decl {
                    ParamDecl::Value { name, .. } => {
                        let name = name.trim_start_matches('*');
                        self.lookup_owner(name)
                            .map(|owner| (name.to_string(), owner))
                    }
                    ParamDecl::Type { .. } => None,
                })
                .collect(),
        };
        let generated = name.contains('$');
        // In an elaborated program, a declaration source validation already
        // recorded is that template's trapping stub, not a template.
        let stub = !self.source_validation
            && self.template_catalog.borrow().validated(&TemplateId {
                module: stmt.module.clone(),
                owner: None,
                name: name.clone(),
                declaration: stmt.span,
            });
        let template = module_level && !decls.is_empty() && !generated && !stub;
        let derived = if module_level && !self.source_validation {
            self.derivable_facts(stmt, template, &param_owners)
        } else {
            None
        };
        let verify = self.template_catalog.borrow().verify();
        if let Some((facts, spans)) = &derived
            && !verify
        {
            self.install_body_facts(facts, spans, &param_owners)?;
            let counter = if template {
                "template_bodies.reused"
            } else {
                "template_derivations.installed"
            };
            timing::count(counter, 1);
            timing::note(counter, || name.clone());
            let mut catalog = self.template_catalog.borrow_mut();
            let stats = catalog.stats_mut();
            if template {
                stats.reused.push(name.clone());
            } else {
                stats.derived.push(name.clone());
            }
            return Ok(());
        }
        let baseline = self.body_fact_baseline();
        Self::count_body_inference(BodyClass::of(generated, !decls.is_empty()), || name.clone());
        self.effect_query_frames.borrow_mut().push(Vec::new());
        let inferred = self.check_block(body, Some(ret_ty), false);
        let effect_queries = self
            .effect_query_frames
            .borrow_mut()
            .pop()
            .unwrap_or_default();
        inferred?;
        if let Some((facts, _)) = &derived {
            let inferred = self
                .capture_body_facts(body, &param_owners, &baseline, &effect_queries)
                .map_err(|reason| {
                    TypeError::InvariantViolation(format!(
                        "template fact verification: '{name}' was derived but its own check is \
                         not capturable: {reason}"
                    ))
                })?;
            // A clone check never selects a rebind's by-value overload: the
            // declaration's selection stands, so only the template states it.
            let facts = &without_rebind_selection(facts);
            let inferred = without_rebind_selection(&inferred);
            if inferred != *facts && overload_rebinding_only(facts, &inferred) {
                // The one expected difference: the clone check ranked an
                // overload set again on concrete arguments, which the
                // template's selection forbids. The derived facts stand.
                self.replace_body_facts(body, facts, &param_owners)?;
                timing::count("template_derivations.overload_rebinding", 1);
            } else if inferred != *facts {
                return Err(TypeError::InvariantViolation(format!(
                    "template fact verification: derived facts for '{name}' differ from its own \
                     check\n derived: {facts:?}\n inferred: {inferred:?}"
                )));
            }
            timing::count("template_derivations.verified", 1);
            self.template_catalog
                .borrow_mut()
                .stats_mut()
                .verified
                .push(name.clone());
        }
        if !template && self.instance_trace(stmt).is_some() {
            self.template_catalog
                .borrow_mut()
                .stats_mut()
                .inferred_clones
                .push(name.clone());
        }
        if template && derived.is_none() {
            self.record_template(
                stmt,
                decls,
                ret_ty,
                &param_owners,
                &baseline,
                &effect_queries,
            );
        } else if module_level && timing::notes_enabled() && self.instance_trace(stmt).is_some() {
            // Capture emits the `template_capture.tables` note: what this
            // body recorded, for comparing a clone with its template.
            timing::note("template_capture.body", || name.clone());
            let _ = self.capture_body_facts(body, &param_owners, &baseline, &effect_queries);
        }
        Ok(())
    }

    /// Record that the body being inferred read `callee`'s transfer or
    /// call-through summary, and whether it was empty.
    pub(super) fn note_effect_query(&self, callee: &str, empty: bool) {
        if let Some(frame) = self.effect_query_frames.borrow_mut().last_mut() {
            frame.push((callee.to_string(), empty));
        }
    }

    fn body_fact_baseline(&self) -> BodyFactBaseline {
        BodyFactBaseline {
            tables: FactTable::ALL.map(|table| self.span_table(table).entries()),
            unkeyed: self.unkeyed_fact_entries(),
            owner_start: self.next_owner.get(),
        }
    }

    /// The trace of a declaration the elaborator generated, if it left one.
    fn instance_trace(&self, stmt: &Stmt) -> Option<InstanceTrace> {
        let StmtKind::Def { name, .. } = &stmt.kind else {
            return None;
        };
        self.template_catalog
            .borrow()
            .trace(&InstanceName {
                module: stmt.module.clone(),
                name: name.clone(),
            })
            .cloned()
    }

    /// The facts a body may take instead of being inferred, realized for
    /// it, with the body's occurrence spans by the identity each kept from
    /// the template: a clone the elaborator traced to a certified template,
    /// or (`template`) a certified template's own body in a later pass,
    /// under the identity substitution. `None` infers the body; for a traced
    /// clone the reason is counted.
    fn derivable_facts(
        &self,
        stmt: &Stmt,
        template: bool,
        param_owners: &BodyParams,
    ) -> Option<(CheckedBodyFacts, HashMap<OccurrenceId, SourceSpan>)> {
        let StmtKind::Def {
            name,
            type_params,
            body,
            ..
        } = &stmt.kind
        else {
            return None;
        };
        let trace = if template {
            InstanceTrace {
                template: TemplateId {
                    module: stmt.module.clone(),
                    owner: None,
                    name: name.clone(),
                    declaration: stmt.span,
                },
                type_bindings: Vec::new(),
                value_bindings: Vec::new(),
                residual: Vec::new(),
            }
        } else {
            self.instance_trace(stmt)?
        };
        let catalog = self.template_catalog.borrow();
        let refuse = |reason: &'static str| {
            if !template {
                timing::count("template_derivations.ineligible", 1);
                timing::note("template_derivations.ineligible", || {
                    format!("{name}: {reason}")
                });
            }
            None
        };
        let Some(checked) = catalog.template(&trace.template) else {
            return refuse("its template has no retained facts");
        };
        let TemplateCoverage::Certified(class) = &checked.coverage else {
            return refuse("its template is not certified");
        };
        // Both enabled classes bake every parameter: a clone that keeps a
        // binder, or folds a value, is outside them.
        let baked = trace.residual.is_empty()
            && type_params.is_empty()
            && match class {
                TemplateClass::ClosedScalarBody
                | TemplateClass::FixedCalls
                | TemplateClass::BoundedOperations => trace.value_bindings.is_empty(),
                // The folded values selected the arms; no retained
                // occurrence names one.
                TemplateClass::ScalarBranches => true,
            };
        if !template && !baked {
            return refuse("the clone keeps or folds a compile-time parameter");
        }
        if param_owners.runtime.iter().any(Option::is_none) {
            return refuse("a parameter has no binding identity");
        }
        let Ok(substitution) = self.instance_substitution(&trace) else {
            return refuse("an instance argument does not resolve");
        };
        let occurrences = self.body_occurrences(body);
        // Every occurrence of the body is one the template checked. A class
        // without compile-time control flow keeps them all, once each; a
        // keyed one keeps the arms the elaborator selected, once per loop
        // iteration it unrolled, and drops the rest, facts and all.
        let keyed = *class == TemplateClass::ScalarBranches;
        let traced = occurrences.iter().all(|occurrence| {
            (keyed || occurrence.id.copy == 0)
                && checked
                    .facts
                    .occurrences
                    .iter()
                    .any(|id| id.syntax == occurrence.id.syntax)
        }) && (keyed || occurrences.len() == checked.facts.occurrences.len());
        if !traced {
            timing::note("template_derivations.untraced", || {
                let strangers: Vec<String> = occurrences
                    .iter()
                    .filter(|occurrence| {
                        !checked
                            .facts
                            .occurrences
                            .iter()
                            .any(|id| id.syntax == occurrence.id.syntax)
                    })
                    .map(|occurrence| format!("{:?}", occurrence.span.span))
                    .collect();
                format!("{name}: {}", strangers.join(" "))
            });
            return refuse("the clone's occurrences are not the template's");
        }
        let ids: Vec<OccurrenceId> = occurrences.iter().map(|occurrence| occurrence.id).collect();
        let selected = checked.facts.selected(&ids);
        match self.realize_instance_facts(&selected, &substitution, &occurrences) {
            Ok(facts) => Some((
                facts,
                occurrences
                    .into_iter()
                    .map(|occurrence| (occurrence.id, occurrence.span))
                    .collect(),
            )),
            Err(reason) => refuse(reason),
        }
    }

    /// The checked type each baked type parameter stands for in a clone:
    /// the source type the elaborator wrote, resolved as the clone's own
    /// annotations are.
    fn instance_substitution(
        &self,
        trace: &InstanceTrace,
    ) -> Result<HashMap<String, Ty>, TypeError> {
        trace
            .type_bindings
            .iter()
            .map(|(name, source)| Ok((name.clone(), self.ty_from_anno(source)?)))
            .collect()
    }

    /// A template's facts for one instance: every retained type substituted,
    /// and every direct call realized against the instance's own syntax and
    /// the declarations in scope now.
    ///
    /// The selected callee is the template's choice and is never re-ranked.
    /// What is per-instance is only what the clone check also decides after
    /// selection: whether the closed application already has a clone
    /// (`existing_def_clone`), and — when the elaborator already retargeted
    /// the call to that clone — the clone's own declared parameters. The
    /// callee's effect summaries must still be empty, as the template saw
    /// them. An `Err` is a refusal, never a verdict on the program.
    fn realize_instance_facts(
        &self,
        template: &CheckedBodyFacts,
        substitution: &HashMap<String, Ty>,
        occurrences: &[Occurrence],
    ) -> Result<CheckedBodyFacts, &'static str> {
        let substitute = |ty: &Ty| mojito_types::types::substitute(ty, substitution);
        let typed = |entries: &[(OccurrenceId, Ty)]| -> Vec<(OccurrenceId, Ty)> {
            entries
                .iter()
                .map(|(id, ty)| (*id, substitute(ty)))
                .collect()
        };
        let mut facts = CheckedBodyFacts {
            operation_adjustments: template
                .operation_adjustments
                .iter()
                .map(|(id, adjustment)| {
                    mojito_checked::templates::derive_adjustment(adjustment, &substitute)
                        .map(|derived| (*id, derived))
                })
                .collect::<Option<_>>()
                .ok_or("an operation adjustment has no derivation recipe")?,
            expression_types: typed(&template.expression_types),
            expression_place_types: typed(&template.expression_place_types),
            binding_types: typed(&template.binding_types),
            expression_effects: template
                .expression_effects
                .iter()
                .map(|(id, effects)| {
                    (
                        *id,
                        mojito_checked::checked::EffectFacts {
                            raises: effects.raises.as_ref().map(&substitute),
                            ..effects.clone()
                        },
                    )
                })
                .collect(),
            generic_instantiations: template
                .generic_instantiations
                .iter()
                .map(|(id, instantiation)| {
                    (
                        *id,
                        mojito_checked::checked::GenericInstantiation {
                            arguments: mojito_types::types::map_tyargs(
                                &instantiation.arguments,
                                &substitute,
                            ),
                            ..instantiation.clone()
                        },
                    )
                })
                .collect(),
            rebind_assertions: template
                .rebind_assertions
                .iter()
                .map(|(id, assertion)| {
                    (
                        *id,
                        mojito_checked::templates::RebindAssertion {
                            operand: substitute(&assertion.operand),
                            dest: substitute(&assertion.dest),
                            by_value: assertion.by_value,
                        },
                    )
                })
                .collect(),
            effect_free_callees: Vec::new(),
            ..template.clone()
        };
        // The equality source validation took on faith. A false one is the
        // clone check's to report, in its own words.
        if facts
            .rebind_assertions
            .iter()
            .any(|(_, assertion)| assertion.operand != assertion.dest)
        {
            return Err("a rebind assertion does not hold for the instance");
        }
        // `check_consuming`'s demand on a copied place, at the instance's
        // type.
        let copies = facts.copy_place_value_uses.iter().all(|place| {
            facts
                .expression_types
                .iter()
                .find(|(id, _)| id == place)
                .is_some_and(|(_, ty)| self.is_copyable(ty) && self.is_implicitly_copyable(ty))
        });
        if !copies {
            return Err("a copied place is not implicitly copyable for the instance");
        }
        renumber_locals(&mut facts);
        let folded = |owner: &TemplateOwner| matches!(owner, TemplateOwner::CompileTimeParam(_));
        if facts
            .expression_bindings
            .iter()
            .chain(&facts.statement_bindings)
            .any(|(_, owner)| folded(owner))
        {
            return Err("an occurrence still names a folded compile-time parameter");
        }
        for (id, _) in &template.call_parameters {
            let selected = template_callee(template, *id)
                .ok_or("a call's selected callee is not a module-scope declaration")?;
            let written = occurrences
                .iter()
                .find(|occurrence| occurrence.id == *id)
                .and_then(|occurrence| occurrence.callee.as_deref())
                .ok_or("a call occurrence is not a direct call in the instance")?;
            let application = facts
                .generic_instantiations
                .iter()
                .position(|(site, _)| site == id);
            // The template's selection, never a fresh ranking: a call through
            // an overload set keeps the member whose lowered symbol the
            // template recorded.
            let target = template
                .overload_targets
                .iter()
                .find(|(site, _)| site == id)
                .map(|(_, target)| target.as_str());
            let member = match (self.lookup(selected), target) {
                (Some(Ty::Overload(members)), Some(target)) => members.iter().find(|member| {
                    callable_lowered_name(selected, member).as_deref() == Some(target)
                }),
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
                        .find(|(site, _)| site == id)
                    {
                        Some(entry) => entry.1 = clone,
                        None => facts.overload_targets.push((*id, clone)),
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
                facts.overload_targets.retain(|(site, _)| site != id);
                set_fact(
                    &mut facts.call_parameters,
                    *id,
                    call_parameter_facts(callee),
                );
                set_fact(
                    &mut facts.expression_bindings,
                    *id,
                    TemplateOwner::Global(written.to_string()),
                );
            }
            if !facts
                .effect_free_callees
                .iter()
                .any(|callee| callee == written)
            {
                facts.effect_free_callees.push(written.to_string());
            }
        }
        for call in &template.builtin_len_calls {
            self.realize_builtin_len(&mut facts, *call, occurrences)?;
        }
        facts.effect_free_callees.sort();
        let summaries_empty = facts.effect_free_callees.iter().all(|callee| {
            self.transfer_effects
                .borrow()
                .get(callee)
                .is_none_or(Vec::is_empty)
                && self
                    .call_through_effects
                    .borrow()
                    .get(callee)
                    .is_none_or(Vec::is_empty)
        });
        if !summaries_empty {
            return Err("a callee's effect summary is no longer empty");
        }
        let position = |id: &OccurrenceId| {
            occurrences
                .iter()
                .position(|occurrence| occurrence.id == *id)
        };
        facts.overload_targets.sort_by_key(|(id, _)| position(id));
        Ok(facts)
    }

    /// Realize one built-in `len(x)` for an instance, as `infer_len` decides
    /// it on a concrete argument.
    ///
    /// The template proved `len` through the parameter's bound. The instance
    /// owes the witness that bound promised — a `__len__` returning `Int`,
    /// which `len_result_for_type` finds — and takes the one fact `infer_len`
    /// adds for a concrete type: a named nominal-struct place is read in
    /// place rather than copied. A missing witness refuses the derivation,
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
        let named = occurrences
            .iter()
            .any(|occurrence| occurrence.id == argument && occurrence.identifier);
        let in_place =
            named && matches!(&ty, Ty::Struct(name, _) if self.structs.contains_key(name));
        facts
            .borrowed_read_call_places
            .retain(|place| *place != argument);
        if in_place {
            facts.borrowed_read_call_places.push(argument);
            let order = |id: &OccurrenceId| occurrences.iter().position(|found| found.id == *id);
            facts.borrowed_read_call_places.sort_by_key(order);
        }
        Ok(())
    }

    /// Retain a module-level generic declaration's freshly inferred body
    /// facts, with the certificate its class earns.
    fn record_template(
        &self,
        stmt: &Stmt,
        decls: &[ParamDecl],
        ret_ty: &Ty,
        param_owners: &BodyParams,
        baseline: &BodyFactBaseline,
        effect_queries: &[(String, bool)],
    ) {
        let StmtKind::Def { name, body, .. } = &stmt.kind else {
            return;
        };
        let captured = self
            .capture_body_facts(body, param_owners, baseline, effect_queries)
            .and_then(|facts| {
                // Two template occurrences sharing one identity could not be
                // told apart from an instance's loop copies.
                if facts.occurrences.iter().all(|id| id.copy == 0) {
                    Ok(facts)
                } else {
                    Err(IncompleteReason::AmbiguousOccurrence)
                }
            });
        let (facts, coverage) = match captured {
            Ok(facts) => {
                let coverage = self.template_certificate(stmt, decls, ret_ty, &facts);
                (facts, coverage)
            }
            Err(reason) => (
                CheckedBodyFacts::default(),
                TemplateCoverage::Incomplete(reason),
            ),
        };
        match &coverage {
            TemplateCoverage::Certified(_) => {
                self.template_catalog
                    .borrow_mut()
                    .stats_mut()
                    .certified
                    .push(name.clone());
                timing::count("template_facts_recorded", 1);
                timing::count("template_fact_entries", facts.entries() as u64);
                timing::note("template_facts_recorded", || name.clone());
            }
            TemplateCoverage::Incomplete(reason) => {
                timing::count(reason.counter(), 1);
                timing::note(reason.counter(), || format!("{name}: {reason}"));
            }
        }
        self.template_catalog.borrow_mut().record(CheckedTemplate {
            id: TemplateId {
                module: stmt.module.clone(),
                owner: None,
                name: name.clone(),
                declaration: stmt.span,
            },
            producer: if self.source_validation {
                TemplateProducer::SourceValidation
            } else {
                TemplateProducer::ExecutableCheck
            },
            param_decls: decls.to_vec(),
            facts,
            coverage,
            obligations: vec![
                TemplateObligation::DeclarationConstraints,
                TemplateObligation::RebindEqualities,
                TemplateObligation::ImplicitCopies,
            ],
        });
    }

    /// The certificate a freshly captured module-level generic function earns.
    ///
    /// Both classes share a declaration shape: plain type parameters, immutable
    /// regular runtime parameters, a concrete scalar result, and no `raises`,
    /// captures, decorators, or `where` clauses, so the declaration's bounds —
    /// discharged where an instance is requested — are all an instance owes.
    ///
    /// [`TemplateClass::ClosedScalarBody`] bodies name nothing: every fact is
    /// closed and inherited unchanged. [`TemplateClass::FixedCalls`] bodies also
    /// call module-scope functions directly. The checks a clone would repeat for
    /// such a call are covered as follows.
    ///
    /// - Selection: the call resolved to one module-scope declaration, or to one
    ///   member of an overload set, whose lowered symbol is retained. An instance
    ///   inherits that choice and never ranks the set again on its concrete
    ///   arguments: the pinned Mojo binds a call inside a generic body once, when
    ///   it checks the body (`conformance/probes/template_overload_binding.mojo`).
    /// - Argument typing: the template recorded no conversion, copy, move, or
    ///   adjustment at the call (any such table refuses the capture), so each
    ///   argument matched its parameter exactly with `T` symbolic, and matches
    ///   exactly after substitution.
    /// - Binding conventions: borrows are decided from slots, conventions, and
    ///   argument shape, none of which mention a type.
    /// - Effects: the callee does not raise, and its transfer and call-through
    ///   summaries were empty; a realization re-reads them and installs the same
    ///   fixpoint observation a clone check would.
    /// - Requests: the retained application substitutes, and
    ///   `realize_instance_facts` repeats the clone check's only concrete
    ///   decision, whether that application's clone already exists.
    ///
    /// [`TemplateClass::BoundedOperations`] bodies also call the built-in
    /// `len`. With `T` symbolic its bound proves the call; an instance owes
    /// the witness and takes the concrete read-in-place fact
    /// (`realize_builtin_len`).
    ///
    /// An operator is admitted only over operands whose recorded types are
    /// closed scalars, so no operator in these classes dispatches through a
    /// bound.
    fn template_certificate(
        &self,
        stmt: &Stmt,
        decls: &[ParamDecl],
        ret_ty: &Ty,
        facts: &CheckedBodyFacts,
    ) -> TemplateCoverage {
        let outside =
            |what| TemplateCoverage::Incomplete(IncompleteReason::OutsideEnabledClass(what));
        let StmtKind::Def {
            type_params,
            params,
            captures,
            body,
            raises,
            raises_type,
            decorators,
            ..
        } = &stmt.kind
        else {
            return outside("not a function");
        };
        let plain_binders = type_params.iter().all(|parameter| {
            parameter.callable_bound.is_none()
                && parameter.default.is_none()
                && parameter.origin_mutability.is_none()
                && !parameter.name.starts_with('*')
        }) && decls.iter().all(|decl| match decl {
            // A binder's constraints are the declaration's `where` clauses:
            // the requesting call and the elaborator discharge them before an
            // instance exists (`TemplateObligation::DeclarationConstraints`).
            ParamDecl::Type {
                variadic: false,
                callable_bound: None,
                ..
            } => true,
            ParamDecl::Value {
                ty,
                variadic: false,
                ..
            } => matches!(**ty, Ty::Bool | Ty::Int),
            ParamDecl::Type { .. } | ParamDecl::Value { .. } => false,
        });
        if !plain_binders || decls.len() != type_params.len() {
            return outside("a compile-time parameter is not a plain type or scalar value");
        }
        // One producer per body: source validation owns every body it
        // checks (keyed by compile-time control flow or a `rebind`), the
        // executable check the surviving ones.
        let keyed = self.source_validation;
        if !keyed
            && (decls
                .iter()
                .any(|decl| matches!(decl, ParamDecl::Value { .. }))
                || body.iter().any(holds_comptime_if))
        {
            return outside("a compile-time-keyed body is source validation's to certify");
        }
        let plain_params = params.iter().all(|parameter| {
            parameter.kind == mojito_ast::ast::ParamKind::Regular
                && parameter.convention.is_none()
                && parameter.default.is_none()
                && parameter.origin.is_none()
        });
        if !plain_params {
            return outside("a parameter is not an immutable regular parameter");
        }
        if *raises || raises_type.is_some() || captures.is_some() || !decorators.is_empty() {
            return outside("the declaration raises, captures, or is decorated");
        }
        if !closed_scalar(ret_ty) {
            return outside("the return type is not a concrete scalar");
        }
        let shape = BodyShape {
            origins: &self.syntax_origins,
            facts,
            params: params
                .iter()
                .map(|parameter| parameter.name.as_str())
                .collect(),
            keyed,
            locals: RefCell::new(Vec::new()),
        };
        if !shape.block(body, false) {
            return outside("the body is not scalar returns over direct calls and 'len'");
        }
        let effects_closed = facts
            .expression_effects
            .iter()
            .all(|(_, effects)| *effects == mojito_checked::checked::EffectFacts::default());
        if !effects_closed {
            return outside("a call has an effect");
        }
        let adjustments_derive = facts.operation_adjustments.iter().all(|(_, adjustment)| {
            mojito_checked::templates::derive_adjustment(adjustment, &Ty::clone).is_some()
        });
        if !adjustments_derive {
            return TemplateCoverage::Incomplete(IncompleteReason::UnsupportedTable(
                FactTable::OperationAdjustments,
            ));
        }
        // Every call selected a module-scope declaration, every effect
        // summary read belongs to one of those calls, and no application
        // carries a pack.
        let callees: Option<Vec<&str>> = facts
            .call_parameters
            .iter()
            .map(|(id, _)| template_callee(facts, *id))
            .collect();
        let Some(callees) = callees else {
            return outside("a call's callee is not a module-scope declaration");
        };
        if !facts
            .effect_free_callees
            .iter()
            .all(|callee| callees.contains(&callee.as_str()))
        {
            return outside("an effect summary was read outside a direct call");
        }
        let at_call = |id: &OccurrenceId| facts.call_parameters.iter().any(|(call, _)| call == id);
        if !facts.overload_targets.iter().all(|(id, _)| at_call(id))
            || facts
                .generic_instantiations
                .iter()
                .any(|(id, application)| application.variadic.is_some() || !at_call(id))
        {
            return outside("an application carries a pack or is not a direct call");
        }
        let closed = facts
            .expression_types
            .iter()
            .chain(&facts.expression_place_types)
            .chain(&facts.binding_types)
            .all(|(_, ty)| !mojito_types::types::is_symbolic(ty));
        TemplateCoverage::Certified(if keyed {
            TemplateClass::ScalarBranches
        } else if !facts.builtin_len_calls.is_empty() {
            TemplateClass::BoundedOperations
        } else if !facts.call_parameters.is_empty() || !closed {
            TemplateClass::FixedCalls
        } else {
            TemplateClass::ClosedScalarBody
        })
    }

    /// Every statement and expression occurrence of a body in pre-order, by
    /// the identity it had before the final re-key and its copy number.
    fn body_occurrences(&self, body: &[Stmt]) -> Vec<Occurrence> {
        struct Occurrences<'a> {
            origins: &'a mojito_ast::ast::SyntaxOrigins,
            found: Vec<Occurrence>,
            copies: HashMap<SyntaxId, u32>,
        }

        impl Occurrences<'_> {
            /// The next copy of the template occurrence `syntax` was copied
            /// from: copies are numbered in pre-order.
            fn next_copy(&mut self, syntax: SyntaxId) -> OccurrenceId {
                let syntax = self.origins.origin(syntax);
                let copies = self.copies.entry(syntax).or_insert(0);
                let copy = *copies;
                *copies = copies.saturating_add(1);
                OccurrenceId { syntax, copy }
            }
        }

        impl mojito_ast::visit::Visitor for Occurrences<'_> {
            fn visit_stmt(&mut self, statement: &Stmt) {
                let id = self.next_copy(statement.syntax_id);
                self.found.push(Occurrence {
                    id,
                    span: statement.source_span(),
                    callee: None,
                    arguments: Vec::new(),
                    identifier: false,
                });
            }

            fn visit_expr(&mut self, expr: &Expr) {
                let id = self.next_copy(expr.syntax_id);
                self.found.push(Occurrence {
                    id,
                    span: expr.source_span(),
                    callee: match &expr.kind {
                        ExprKind::Call { name, .. } => Some(name.clone()),
                        _ => None,
                    },
                    arguments: match &expr.kind {
                        ExprKind::Call { args, .. } => args
                            .iter()
                            .map(|argument| self.origins.origin(argument.syntax_id))
                            .collect(),
                        _ => Vec::new(),
                    },
                    identifier: matches!(expr.kind, ExprKind::Identifier(_)),
                });
            }
        }

        let mut occurrences = Occurrences {
            origins: &self.syntax_origins,
            found: Vec::new(),
            copies: HashMap::new(),
        };
        mojito_ast::visit::walk_block(&mut occurrences, body);
        occurrences.found
    }

    /// What one body inference recorded, in template-local terms. Every
    /// table is accounted for: an entry in a table without a recipe, an
    /// entry keyed outside the body, growth in a store not keyed by
    /// occurrence, or a callee effect summary that was not empty refuses the
    /// body.
    fn capture_body_facts(
        &self,
        body: &[Stmt],
        param_owners: &BodyParams,
        baseline: &BodyFactBaseline,
        effect_queries: &[(String, bool)],
    ) -> Result<CheckedBodyFacts, IncompleteReason> {
        let occurrences = self.body_occurrences(body);
        timing::note("template_capture.tables", || {
            let recorded: Vec<String> = FactTable::ALL
                .into_iter()
                .flat_map(|table| {
                    let entries = self.span_table(table);
                    occurrences
                        .iter()
                        .filter_map(|occurrence| entries.describe(&occurrence.span))
                        .map(|fact| format!("{table:?}={fact}"))
                        .collect::<Vec<_>>()
                })
                .collect();
            recorded.join(" ; ")
        });
        if let Some((store, _)) = self
            .unkeyed_fact_entries()
            .into_iter()
            .zip(baseline.unkeyed)
            .find_map(|(now, before)| (now != before).then_some(now))
        {
            return Err(IncompleteReason::UnkeyedFact(store));
        }
        if effect_queries.iter().any(|(_, empty)| !empty)
            || self
                .transfer_frames
                .borrow()
                .last()
                .is_some_and(|frame| !frame.effects.is_empty() || !frame.call_throughs.is_empty())
        {
            return Err(IncompleteReason::UnkeyedFact("transfer effects"));
        }
        for (index, table) in FactTable::ALL.into_iter().enumerate() {
            let entries = self.span_table(table);
            let recorded = occurrences
                .iter()
                .filter(|occurrence| entries.has(&occurrence.span))
                .count();
            if entries.entries() != baseline.tables[index] + recorded {
                return Err(IncompleteReason::FactOutsideBody(table));
            }
            if recorded > 0 && !derivable_table(table) {
                return Err(IncompleteReason::UnsupportedTable(table));
            }
        }
        let owner_end = self.next_owner.get();
        let local_owner = |owner: OwnerId| {
            param_owners
                .runtime
                .iter()
                .position(|param| *param == Some(owner))
                .map(TemplateOwner::Param)
                .or_else(|| {
                    param_owners
                        .compile_time
                        .iter()
                        .find(|(_, param)| *param == owner)
                        .map(|(name, _)| TemplateOwner::CompileTimeParam(name.clone()))
                })
                .or_else(|| {
                    (baseline.owner_start..owner_end)
                        .contains(&owner.0)
                        .then(|| TemplateOwner::Local(owner.0 - baseline.owner_start))
                })
                .or_else(|| {
                    self.owner_scopes.first().and_then(|globals| {
                        globals
                            .iter()
                            .find(|(_, global)| **global == owner)
                            .map(|(name, _)| TemplateOwner::Global(name.clone()))
                    })
                })
                .ok_or(IncompleteReason::ExternalBinding)
        };
        let keyed = |lookup: &dyn Fn(&SourceSpan) -> bool| -> Vec<OccurrenceId> {
            occurrences
                .iter()
                .filter(|occurrence| lookup(&occurrence.span))
                .map(|occurrence| occurrence.id)
                .collect()
        };
        let owned = |table: &HashMap<SourceSpan, OwnerId>| {
            values(&occurrences, table)
                .into_iter()
                .map(|(id, owner)| local_owner(owner).map(|owner| (id, owner)))
                .collect::<Result<Vec<_>, _>>()
        };
        let mut effect_free_callees: Vec<String> = effect_queries
            .iter()
            .map(|(callee, _)| callee.clone())
            .collect();
        effect_free_callees.sort();
        effect_free_callees.dedup();
        Ok(CheckedBodyFacts {
            expression_types: values(&occurrences, &self.expression_types.borrow()),
            expression_place_types: values(&occurrences, &self.expression_place_types.borrow()),
            binding_types: values(&occurrences, &self.binding_types.borrow()),
            expression_bindings: owned(&self.expression_bindings.borrow())?,
            statement_bindings: owned(&self.statement_bindings.borrow())?,
            expression_effects: values(&occurrences, &self.expression_effects.borrow()),
            operation_adjustments: values(&occurrences, &self.operation_adjustments.borrow()),
            generic_instantiations: values(&occurrences, &self.generic_instantiations.borrow()),
            overload_targets: values(&occurrences, &self.overload_targets.borrow()),
            call_parameters: values(&occurrences, &self.call_parameters.borrow())
                .into_iter()
                .map(|(id, parameters)| {
                    (
                        id,
                        parameters
                            .into_iter()
                            .map(|parameter| CallParameterFact {
                                name: parameter.name,
                                convention: parameter.convention,
                                ty: parameter.ty,
                            })
                            .collect(),
                    )
                })
                .collect(),
            borrowed_read_call_places: keyed(&|span| {
                self.borrowed_read_call_places.borrow().contains(span)
            }),
            read_temporary_arguments: keyed(&|span| {
                self.read_temporary_arguments.borrow().contains(span)
            }),
            effect_free_callees,
            builtin_len_calls: occurrences
                .iter()
                .filter(|occurrence| {
                    occurrence.callee.as_deref() == Some("len") && self.lookup("len").is_none()
                })
                .map(|occurrence| occurrence.id)
                .collect(),
            rebind_assertions: values(&occurrences, &self.rebind_assertions.borrow()),
            copy_place_value_uses: keyed(&|span| {
                self.copy_place_value_uses.borrow().contains(span)
            }),
            interior_invalidations: values(&occurrences, &self.interior_invalidations.borrow())
                .into_iter()
                .map(|(id, invalidations)| {
                    invalidations
                        .into_iter()
                        .map(|invalidation| {
                            Ok(TemplateInvalidation {
                                root: local_owner(invalidation.base.root)?,
                                path: invalidation.base.path,
                                except: invalidation.except.map(&local_owner).transpose()?,
                                include_base_generation: invalidation.include_base_generation,
                            })
                        })
                        .collect::<Result<Vec<_>, IncompleteReason>>()
                        .map(|invalidations| (id, invalidations))
                })
                .collect::<Result<Vec<_>, _>>()?,
            unconsumed_temporaries: keyed(&|span| {
                self.unconsumed_temporaries.borrow().contains(span)
            }),
            deletable_bindings: keyed(&|span| {
                self.explicit_destroy_deletability
                    .borrow()
                    .bindings
                    .contains(span)
            }),
            locals: owner_end - baseline.owner_start,
            occurrences: occurrences
                .into_iter()
                .map(|occurrence| occurrence.id)
                .collect(),
        })
    }

    /// Install a realized bundle as if the body had been inferred: each fact
    /// at the occurrence that kept the template occurrence's identity, each
    /// binding at the body's own parameter, a fresh local, or the named
    /// module-scope declaration, and each callee summary the body depends on
    /// observed as empty so the transfer fixpoint re-runs if it grows.
    fn install_body_facts(
        &self,
        facts: &CheckedBodyFacts,
        spans: &HashMap<OccurrenceId, SourceSpan>,
        param_owners: &BodyParams,
    ) -> Result<(), TypeError> {
        let corrupt =
            |what: &str| TypeError::InvariantViolation(format!("template derivation lost {what}"));
        let local_start = self.next_owner.get();
        let local_end = local_start
            .checked_add(facts.locals)
            .ok_or_else(|| corrupt("its binding identity range"))?;
        self.next_owner.set(local_end);
        let owner = |owner: &TemplateOwner| match owner {
            TemplateOwner::Param(index) => param_owners
                .runtime
                .get(*index)
                .copied()
                .flatten()
                .ok_or_else(|| corrupt("a parameter binding")),
            TemplateOwner::CompileTimeParam(_) => {
                Err(corrupt("the fold of a compile-time parameter"))
            }
            TemplateOwner::Local(index) => Ok(OwnerId(local_start + index)),
            TemplateOwner::Global(name) => self
                .owner_scopes
                .first()
                .and_then(|globals| globals.get(name))
                .copied()
                .ok_or_else(|| corrupt("a module-scope binding")),
        };
        let span = |id: &OccurrenceId| {
            spans
                .get(id)
                .cloned()
                .ok_or_else(|| corrupt("a body occurrence"))
        };
        for (id, ty) in &facts.expression_types {
            self.expression_types
                .borrow_mut()
                .insert(span(id)?, ty.clone());
        }
        for (id, ty) in &facts.expression_place_types {
            self.expression_place_types
                .borrow_mut()
                .insert(span(id)?, ty.clone());
        }
        for (id, ty) in &facts.binding_types {
            self.binding_types
                .borrow_mut()
                .insert(span(id)?, ty.clone());
        }
        for (id, binding) in &facts.expression_bindings {
            self.expression_bindings
                .borrow_mut()
                .insert(span(id)?, owner(binding)?);
        }
        for (id, binding) in &facts.statement_bindings {
            self.statement_bindings
                .borrow_mut()
                .insert(span(id)?, owner(binding)?);
        }
        for (id, effects) in &facts.expression_effects {
            self.expression_effects
                .borrow_mut()
                .insert(span(id)?, effects.clone());
        }
        for (id, adjustment) in &facts.operation_adjustments {
            self.operation_adjustments
                .borrow_mut()
                .insert(span(id)?, adjustment.clone());
        }
        for (id, instantiation) in &facts.generic_instantiations {
            self.generic_instantiations
                .borrow_mut()
                .insert(span(id)?, instantiation.clone());
        }
        for (id, target) in &facts.overload_targets {
            self.overload_targets
                .borrow_mut()
                .insert(span(id)?, target.clone());
        }
        for (id, parameters) in &facts.call_parameters {
            self.call_parameters.borrow_mut().insert(
                span(id)?,
                parameters
                    .iter()
                    .map(|parameter| super::CallParameter {
                        name: parameter.name.clone(),
                        convention: parameter.convention,
                        ty: parameter.ty.clone(),
                    })
                    .collect(),
            );
        }
        for id in &facts.borrowed_read_call_places {
            self.borrowed_read_call_places
                .borrow_mut()
                .insert(span(id)?);
        }
        for id in &facts.read_temporary_arguments {
            self.read_temporary_arguments.borrow_mut().insert(span(id)?);
        }
        for (id, assertion) in &facts.rebind_assertions {
            self.rebind_assertions
                .borrow_mut()
                .insert(span(id)?, assertion.clone());
        }
        for id in &facts.copy_place_value_uses {
            self.copy_place_value_uses.borrow_mut().insert(span(id)?);
        }
        for (id, invalidations) in &facts.interior_invalidations {
            let invalidations = invalidations
                .iter()
                .map(|invalidation| {
                    Ok(mojito_checked::checked::InteriorInvalidation {
                        base: mojito_types::origin::OriginPlace {
                            root: owner(&invalidation.root)?,
                            path: invalidation.path.clone(),
                        },
                        except: invalidation.except.as_ref().map(&owner).transpose()?,
                        include_base_generation: invalidation.include_base_generation,
                    })
                })
                .collect::<Result<Vec<_>, TypeError>>()?;
            self.interior_invalidations
                .borrow_mut()
                .insert(span(id)?, invalidations);
        }
        for id in &facts.unconsumed_temporaries {
            self.unconsumed_temporaries.borrow_mut().insert(span(id)?);
        }
        for id in &facts.deletable_bindings {
            self.explicit_destroy_deletability
                .borrow_mut()
                .bindings
                .insert(span(id)?);
        }
        for callee in &facts.effect_free_callees {
            self.effect_observations
                .borrow_mut()
                .entry(callee.clone())
                .or_default();
            self.call_through_observations
                .borrow_mut()
                .entry(callee.clone())
                .or_default();
        }
        Ok(())
    }

    /// Discard what inferring `body` recorded and install `facts` in its
    /// place.
    fn replace_body_facts(
        &self,
        body: &[Stmt],
        facts: &CheckedBodyFacts,
        param_owners: &BodyParams,
    ) -> Result<(), TypeError> {
        let occurrences = self.body_occurrences(body);
        for occurrence in &occurrences {
            self.remove_occurrence_facts(&occurrence.span);
        }
        if let Some(table) = FactTable::ALL.into_iter().find(|table| {
            let entries = self.span_table(*table);
            occurrences
                .iter()
                .any(|occurrence| entries.has(&occurrence.span))
        }) {
            return Err(TypeError::InvariantViolation(format!(
                "template derivation: {table:?} is not cleared by remove_occurrence_facts"
            )));
        }
        self.install_body_facts(
            facts,
            &occurrences
                .into_iter()
                .map(|occurrence| (occurrence.id, occurrence.span))
                .collect(),
            param_owners,
        )
    }

    /// Remove one occurrence's entry from every occurrence-keyed fact table.
    /// `replace_body_facts` checks the list against [`FactTable::ALL`].
    fn remove_occurrence_facts(&self, span: &SourceSpan) {
        self.overload_targets.borrow_mut().remove(span);
        self.contextual_bases.borrow_mut().remove(span);
        self.generic_instantiations.borrow_mut().remove(span);
        self.method_instantiations.borrow_mut().remove(span);
        self.call_transfers.borrow_mut().remove(span);
        self.implicit_conversions.borrow_mut().remove(span);
        self.implicit_conversion_types.borrow_mut().remove(span);
        self.implicit_conversion_raises.borrow_mut().remove(span);
        self.conversion_source_borrows.borrow_mut().remove(span);
        self.simd_constructions.borrow_mut().remove(span);
        self.parameterized_method_calls.borrow_mut().remove(span);
        self.operation_adjustments.borrow_mut().remove(span);
        self.construction_immutable_binders
            .borrow_mut()
            .remove(span);
        self.call_result_origins.borrow_mut().remove(span);
        self.tuple_unpack_plans.borrow_mut().remove(span);
        self.interior_references.borrow_mut().remove(span);
        self.view_result_interiors.borrow_mut().remove(span);
        self.call_parameters.borrow_mut().remove(span);
        self.interior_invalidations.borrow_mut().remove(span);
        self.expression_types.borrow_mut().remove(span);
        self.expression_bindings.borrow_mut().remove(span);
        self.statement_bindings.borrow_mut().remove(span);
        self.with_desugars.borrow_mut().remove(span);
        self.declaration_captures.borrow_mut().remove(span);
        self.comprehension_bindings.borrow_mut().remove(span);
        self.expression_place_types.borrow_mut().remove(span);
        self.binding_types.borrow_mut().remove(span);
        self.expression_effects.borrow_mut().remove(span);
        self.selected_calls.borrow_mut().remove(span);
        self.subscript_descriptors.borrow_mut().remove(span);
        self.iteration_protocols.borrow_mut().remove(span);
        self.explicit_destroy_calls.borrow_mut().remove(span);
        self.reference_value_uses.borrow_mut().remove(span);
        self.copyable_reference_result_reads
            .borrow_mut()
            .remove(span);
        self.discarded_reference_results.borrow_mut().remove(span);
        self.borrowed_reference_receivers.borrow_mut().remove(span);
        self.copy_place_value_uses.borrow_mut().remove(span);
        self.call_place_uses.borrow_mut().remove(span);
        self.borrowed_read_call_places.borrow_mut().remove(span);
        self.read_temporary_arguments.borrow_mut().remove(span);
        self.unconsumed_temporaries.borrow_mut().remove(span);
        self.linear_temporaries.borrow_mut().remove(span);
        self.implicitly_copied_consuming_receivers
            .borrow_mut()
            .remove(span);
        self.truthiness_conditions.borrow_mut().remove(span);
        let mut deletability = self.explicit_destroy_deletability.borrow_mut();
        deletability.bindings.remove(span);
        deletability.linear_bindings.remove(span);
        self.rebind_assertions.borrow_mut().remove(span);
    }

    /// Entries in the fact stores a body inference can grow that are not
    /// keyed by one of its occurrences. A derivation has no recipe for any
    /// of them yet, so growth refuses the body and names the store.
    fn unkeyed_fact_entries(&self) -> [(&'static str, usize); UNKEYED_STORES] {
        let deletability = self.explicit_destroy_deletability.borrow();
        [
            (
                "struct instantiations",
                self.struct_instantiations.borrow().len(),
            ),
            ("hash leaf types", self.hash_leaf_types.borrow().len()),
            ("declaration types", self.declaration_types.borrow().len()),
            ("generic parameters", self.generic_parameters.borrow().len()),
            (
                "declaration effects",
                self.declaration_effects.borrow().len(),
            ),
            (
                "transferred origins",
                self.transferred_origins.borrow().len(),
            ),
            ("deletable declarations", deletability.declarations.len()),
            (
                "linear declarations",
                deletability.linear_declarations.len(),
            ),
        ]
    }

    /// The checker's storage for one occurrence-keyed fact table.
    fn span_table(&self, table: FactTable) -> std::cell::Ref<'_, dyn SpanKeyed> {
        fn keyed<T: SpanKeyed + 'static>(table: &RefCell<T>) -> std::cell::Ref<'_, dyn SpanKeyed> {
            std::cell::Ref::map(table.borrow(), |table| table as &dyn SpanKeyed)
        }
        match table {
            FactTable::OverloadTargets => keyed(&self.overload_targets),
            FactTable::ContextualBases => keyed(&self.contextual_bases),
            FactTable::GenericInstantiations => keyed(&self.generic_instantiations),
            FactTable::MethodInstantiations => keyed(&self.method_instantiations),
            FactTable::CallTransfers => keyed(&self.call_transfers),
            FactTable::ImplicitConversions => keyed(&self.implicit_conversions),
            FactTable::ImplicitConversionTypes => keyed(&self.implicit_conversion_types),
            FactTable::ImplicitConversionRaises => keyed(&self.implicit_conversion_raises),
            FactTable::ConversionSourceBorrows => keyed(&self.conversion_source_borrows),
            FactTable::SimdConstructions => keyed(&self.simd_constructions),
            FactTable::ParameterizedMethodCalls => keyed(&self.parameterized_method_calls),
            FactTable::OperationAdjustments => keyed(&self.operation_adjustments),
            FactTable::ConstructionImmutableBinders => keyed(&self.construction_immutable_binders),
            FactTable::CallResultOrigins => keyed(&self.call_result_origins),
            FactTable::TupleUnpackPlans => keyed(&self.tuple_unpack_plans),
            FactTable::InteriorReferences => keyed(&self.interior_references),
            FactTable::ViewResultInteriors => keyed(&self.view_result_interiors),
            FactTable::CallParameters => keyed(&self.call_parameters),
            FactTable::InteriorInvalidations => keyed(&self.interior_invalidations),
            FactTable::ExpressionTypes => keyed(&self.expression_types),
            FactTable::ExpressionBindings => keyed(&self.expression_bindings),
            FactTable::StatementBindings => keyed(&self.statement_bindings),
            FactTable::WithDesugars => keyed(&self.with_desugars),
            FactTable::DeclarationCaptures => keyed(&self.declaration_captures),
            FactTable::ComprehensionBindings => keyed(&self.comprehension_bindings),
            FactTable::ExpressionPlaceTypes => keyed(&self.expression_place_types),
            FactTable::BindingTypes => keyed(&self.binding_types),
            FactTable::ExpressionEffects => keyed(&self.expression_effects),
            FactTable::SelectedCalls => keyed(&self.selected_calls),
            FactTable::SubscriptDescriptors => keyed(&self.subscript_descriptors),
            FactTable::IterationProtocols => keyed(&self.iteration_protocols),
            FactTable::ExplicitDestroyCalls => keyed(&self.explicit_destroy_calls),
            FactTable::ReferenceValueUses => keyed(&self.reference_value_uses),
            FactTable::CopyableReferenceResultReads => keyed(&self.copyable_reference_result_reads),
            FactTable::DiscardedReferenceResults => keyed(&self.discarded_reference_results),
            FactTable::BorrowedReferenceReceivers => keyed(&self.borrowed_reference_receivers),
            FactTable::CopyPlaceValueUses => keyed(&self.copy_place_value_uses),
            FactTable::CallPlaceUses => keyed(&self.call_place_uses),
            FactTable::BorrowedReadCallPlaces => keyed(&self.borrowed_read_call_places),
            FactTable::ReadTemporaryArguments => keyed(&self.read_temporary_arguments),
            FactTable::UnconsumedTemporaries => keyed(&self.unconsumed_temporaries),
            FactTable::LinearTemporaries => keyed(&self.linear_temporaries),
            FactTable::ImplicitlyCopiedConsumingReceivers => {
                keyed(&self.implicitly_copied_consuming_receivers)
            }
            FactTable::TruthinessConditions => keyed(&self.truthiness_conditions),
            FactTable::DeletableBindings => {
                std::cell::Ref::map(self.explicit_destroy_deletability.borrow(), |facts| {
                    &facts.bindings as &dyn SpanKeyed
                })
            }
            FactTable::RebindAssertions => keyed(&self.rebind_assertions),
            FactTable::LinearBindings => {
                std::cell::Ref::map(self.explicit_destroy_deletability.borrow(), |facts| {
                    &facts.linear_bindings as &dyn SpanKeyed
                })
            }
        }
    }
}

/// How many fact stores `unkeyed_fact_entries` watches.
const UNKEYED_STORES: usize = 8;

/// An occurrence-keyed fact table, whatever it stores per occurrence.
trait SpanKeyed {
    fn entries(&self) -> usize;
    fn has(&self, span: &SourceSpan) -> bool;
    /// The fact at `span`, rendered for a timing note.
    fn describe(&self, span: &SourceSpan) -> Option<String>;
}

impl<V: std::fmt::Debug> SpanKeyed for HashMap<SourceSpan, V> {
    fn entries(&self) -> usize {
        self.len()
    }

    fn has(&self, span: &SourceSpan) -> bool {
        self.contains_key(span)
    }

    fn describe(&self, span: &SourceSpan) -> Option<String> {
        self.get(span).map(|fact| format!("{fact:?}"))
    }
}

impl SpanKeyed for HashSet<SourceSpan> {
    fn entries(&self) -> usize {
        self.len()
    }

    fn has(&self, span: &SourceSpan) -> bool {
        self.contains(span)
    }

    fn describe(&self, span: &SourceSpan) -> Option<String> {
        self.contains(span).then(|| "set".to_string())
    }
}

/// Whether [`CheckedBodyFacts`] carries a table's entries. Every other table
/// refuses a body that recorded into it.
const fn derivable_table(table: FactTable) -> bool {
    match table {
        FactTable::ExpressionTypes
        | FactTable::ExpressionPlaceTypes
        | FactTable::BindingTypes
        | FactTable::ExpressionBindings
        | FactTable::StatementBindings
        | FactTable::ExpressionEffects
        | FactTable::OperationAdjustments
        | FactTable::GenericInstantiations
        | FactTable::OverloadTargets
        | FactTable::CallParameters
        | FactTable::BorrowedReadCallPlaces
        | FactTable::ReadTemporaryArguments
        | FactTable::UnconsumedTemporaries
        | FactTable::RebindAssertions
        | FactTable::CopyPlaceValueUses
        | FactTable::InteriorInvalidations
        | FactTable::DeletableBindings => true,
        FactTable::ContextualBases
        | FactTable::MethodInstantiations
        | FactTable::CallTransfers
        | FactTable::ImplicitConversions
        | FactTable::ImplicitConversionTypes
        | FactTable::ImplicitConversionRaises
        | FactTable::ConversionSourceBorrows
        | FactTable::SimdConstructions
        | FactTable::ParameterizedMethodCalls
        | FactTable::ConstructionImmutableBinders
        | FactTable::CallResultOrigins
        | FactTable::TupleUnpackPlans
        | FactTable::InteriorReferences
        | FactTable::ViewResultInteriors
        | FactTable::WithDesugars
        | FactTable::DeclarationCaptures
        | FactTable::ComprehensionBindings
        | FactTable::SelectedCalls
        | FactTable::SubscriptDescriptors
        | FactTable::IterationProtocols
        | FactTable::ExplicitDestroyCalls
        | FactTable::ReferenceValueUses
        | FactTable::CopyableReferenceResultReads
        | FactTable::DiscardedReferenceResults
        | FactTable::BorrowedReferenceReceivers
        | FactTable::CallPlaceUses
        | FactTable::LinearTemporaries
        | FactTable::ImplicitlyCopiedConsumingReceivers
        | FactTable::TruthinessConditions
        | FactTable::LinearBindings => false,
    }
}

/// The entries of one fact table at a body's occurrences, in pre-order.
fn values<V: Clone>(
    occurrences: &[Occurrence],
    table: &HashMap<SourceSpan, V>,
) -> Vec<(OccurrenceId, V)> {
    occurrences
        .iter()
        .filter_map(|occurrence| {
            table
                .get(&occurrence.span)
                .map(|value| (occurrence.id, value.clone()))
        })
        .collect()
}

/// A bundle with every rebind's by-value selection cleared, for comparing a
/// derived bundle with a clone check's.
fn without_rebind_selection(facts: &CheckedBodyFacts) -> CheckedBodyFacts {
    let mut facts = facts.clone();
    for (_, assertion) in &mut facts.rebind_assertions {
        assertion.by_value = false;
    }
    facts
}

/// Whether two bundles differ only where a clone check re-ranked an overload
/// set the template had already selected from.
///
/// That is the single difference verification expects, and it is confined to
/// the selection facts of calls the derived bundle resolves through an
/// overload target. Every other table, and every other occurrence, must
/// still agree.
fn overload_rebinding_only(derived: &CheckedBodyFacts, inferred: &CheckedBodyFacts) -> bool {
    let selected: Vec<OccurrenceId> = derived.overload_targets.iter().map(|(id, _)| *id).collect();
    if selected.is_empty() {
        return false;
    }
    let without_selection = |facts: &CheckedBodyFacts| {
        let mut facts = facts.clone();
        facts
            .overload_targets
            .retain(|(id, _)| !selected.contains(id));
        facts
            .generic_instantiations
            .retain(|(id, _)| !selected.contains(id));
        facts
            .call_parameters
            .retain(|(id, _)| !selected.contains(id));
        facts
    };
    without_selection(derived) == without_selection(inferred)
}

/// Number the locals an instance keeps as its own check would mint them.
///
/// A template numbers every local its body declares, in checking order. An
/// instance declares only those in the arms the elaborator selected, and never
/// a `comptime for` variable, so the survivors are renumbered densely in
/// declaration order. A local no retained declaration introduces is left
/// alone; installing it then fails as a lost binding.
fn renumber_locals(facts: &mut CheckedBodyFacts) {
    let declared: Vec<u32> = facts
        .statement_bindings
        .iter()
        .filter_map(|(_, owner)| match owner {
            TemplateOwner::Local(local) => Some(*local),
            _ => None,
        })
        .collect();
    let renumber = |owner: &mut TemplateOwner| {
        if let TemplateOwner::Local(local) = owner
            && let Some(dense) = declared.iter().position(|kept| kept == local)
        {
            *local = u32::try_from(dense).unwrap_or(u32::MAX);
        }
    };
    for (_, owner) in facts
        .statement_bindings
        .iter_mut()
        .chain(&mut facts.expression_bindings)
    {
        renumber(owner);
    }
    for invalidation in facts
        .interior_invalidations
        .iter_mut()
        .flat_map(|(_, invalidations)| invalidations)
    {
        renumber(&mut invalidation.root);
        if let Some(except) = &mut invalidation.except {
            renumber(except);
        }
    }
    facts.locals = u32::try_from(declared.len()).unwrap_or(u32::MAX);
}

/// The module-scope declaration a template's direct call selected: the
/// binding its call occurrence resolved to.
fn template_callee(facts: &CheckedBodyFacts, call: OccurrenceId) -> Option<&str> {
    facts
        .expression_bindings
        .iter()
        .find(|(id, _)| *id == call)
        .and_then(|(_, owner)| match owner {
            TemplateOwner::Global(name) => Some(name.as_str()),
            TemplateOwner::Param(_)
            | TemplateOwner::Local(_)
            | TemplateOwner::CompileTimeParam(_) => None,
        })
}

/// Replace the fact at `id`, keeping the table's occurrence order.
fn set_fact<V>(table: &mut [(OccurrenceId, V)], id: OccurrenceId, value: V) {
    if let Some(entry) = table.iter_mut().find(|(site, _)| *site == id) {
        entry.1 = value;
    }
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

/// The syntax a certified body may hold, judged against what its one
/// inference recorded.
struct BodyShape<'a> {
    origins: &'a mojito_ast::ast::SyntaxOrigins,
    facts: &'a CheckedBodyFacts,
    params: Vec<&'a str>,
    /// Whether source validation produced the facts: only a body it checks
    /// may hold compile-time control flow, locals, and assignments.
    keyed: bool,
    /// The scalar locals declared so far.
    locals: RefCell<Vec<String>>,
}

impl BodyShape<'_> {
    fn statement(&self, statement: &Stmt, in_loop: bool) -> bool {
        match &statement.kind {
            StmtKind::Return(Some(value)) => self.expression(value) && self.scalar(value),
            StmtKind::Pass => true,
            // A condition is typed, never evaluated, by the check that
            // produced these facts, and no instance keeps its occurrences.
            StmtKind::ComptimeIf { branches, orelse } if self.keyed => branches
                .iter()
                .map(|(_, arm)| arm)
                .chain(orelse)
                .all(|arm| self.block(arm, in_loop)),
            // An unrolled body is copied once per iteration, every copy
            // sharing the bindings around the loop. A local declared inside
            // would need one binding per copy, which no recipe mints yet, and
            // the loop variable folds to a literal wherever it survives, so
            // neither is admitted: the variable may only key a condition.
            StmtKind::ComptimeFor { body, .. } if self.keyed => self.block(body, true),
            StmtKind::VarDecl { name, value, .. } if self.keyed && !in_loop => {
                let scalar = self.expression(value) && self.scalar(value);
                self.locals.borrow_mut().push(name.clone());
                scalar
            }
            StmtKind::Assign { name, value } if self.keyed => {
                self.local(name) && self.expression(value) && self.scalar(value)
            }
            StmtKind::AugAssign { place, value, .. } if self.keyed => {
                matches!(&place.kind, ExprKind::Identifier(name) if self.local(name))
                    && self.scalar(place)
                    && self.expression(value)
                    && self.scalar(value)
            }
            _ => false,
        }
    }

    /// A nested block: its locals go out of scope with it.
    fn block(&self, statements: &[Stmt], in_loop: bool) -> bool {
        let outer = self.locals.borrow().len();
        let admitted = statements
            .iter()
            .all(|statement| self.statement(statement, in_loop));
        self.locals.borrow_mut().truncate(outer);
        admitted
    }

    fn local(&self, name: &str) -> bool {
        self.locals.borrow().iter().any(|local| local == name)
    }

    fn expression(&self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) => true,
            ExprKind::Identifier(name) => self.params.contains(&name.as_str()) || self.local(name),
            ExprKind::Prefix(_, value) => self.expression(value) && self.scalar(value),
            ExprKind::Infix(_, left, right) => {
                self.expression(left)
                    && self.expression(right)
                    && self.scalar(left)
                    && self.scalar(right)
            }
            ExprKind::Call {
                param_args,
                args,
                kwargs,
                ..
            } => {
                let id = self.occurrence(expr);
                let direct = self
                    .facts
                    .call_parameters
                    .iter()
                    .any(|(call, _)| *call == id);
                let builtin_len = self.facts.builtin_len_calls.contains(&id) && args.len() == 1;
                (direct || builtin_len)
                    && param_args.is_empty()
                    && kwargs.is_empty()
                    && args.iter().all(|argument| self.expression(argument))
            }
            _ => false,
        }
    }

    /// A template occurrence is always copy zero.
    fn occurrence(&self, expr: &Expr) -> OccurrenceId {
        OccurrenceId {
            syntax: self.origins.origin(expr.syntax_id),
            copy: 0,
        }
    }

    /// Whether the recorded type of `expr` is a closed scalar.
    fn scalar(&self, expr: &Expr) -> bool {
        let id = self.occurrence(expr);
        self.facts
            .expression_types
            .iter()
            .any(|(site, ty)| *site == id && closed_scalar_or_literal(ty))
    }
}

const fn closed_scalar(ty: &Ty) -> bool {
    matches!(ty, Ty::Int | Ty::UInt | Ty::Bool | Ty::Float64)
}

const fn closed_scalar_or_literal(ty: &Ty) -> bool {
    closed_scalar(ty) || matches!(ty, Ty::IntLiteral | Ty::FloatLiteral)
}

const fn holds_comptime_if(statement: &Stmt) -> bool {
    matches!(statement.kind, StmtKind::ComptimeIf { .. })
}

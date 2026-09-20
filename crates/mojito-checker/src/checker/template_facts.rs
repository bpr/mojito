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
    InstanceName, InstanceTrace, MethodFeatures, OccurrenceId, TemplateArgumentBoundary,
    TemplateCallContract, TemplateClass, TemplateCoverage, TemplateId, TemplateInvalidation,
    TemplateObligation, TemplateOrigin, TemplateOwner, TemplatePlace, TemplateProducer,
    TemplateReference,
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
    /// A method's `self`.
    receiver: Option<OwnerId>,
    /// Runtime parameters, in declaration order.
    runtime: Vec<Option<OwnerId>>,
    /// Value parameters, bound as locals while the body checks symbolically.
    compile_time: Vec<(String, OwnerId)>,
}

/// What one body inference read or reached that no occurrence-keyed table
/// holds.
#[derive(Default)]
struct BodyReads {
    /// Each callee whose transfer or call-through summary was read, and
    /// whether it was empty.
    effect_queries: Vec<(String, bool)>,
    /// Each generic-struct application reached, before any filter.
    struct_applications: Vec<(String, Vec<mojito_types::types::TyArg>)>,
}

/// One declaration body as the template mechanism sees it.
struct BodySite<'a> {
    /// The name diagnostics, counters, and statistics use.
    display: String,
    body: &'a [Stmt],
    /// Its identity as a template.
    template_id: TemplateId,
    /// Its identity as a clone, which a trace may name.
    instance: InstanceName,
    role: BodyRole,
    /// The declaration's compile-time binders: a method's are its struct's
    /// followed by its own.
    decls: &'a [ParamDecl],
    ret_ty: &'a Ty,
    /// Whether the mechanism sees it at all: a nested `def` does not.
    participates: bool,
    /// Whether a clone still declares a compile-time parameter of its own.
    residual_binders: bool,
    declaration: BodyDeclaration<'a>,
    /// The arguments of the type `self` has here, for a method of a struct.
    receiver_arguments: Option<Vec<mojito_types::types::TyArg>>,
}

/// What a body is to the template mechanism.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BodyRole {
    /// A generic declaration whose facts may be retained and served back: it
    /// is neither generated nor a validated template's stub.
    Template,
    /// A declaration the elaborator generated, which a trace may tie to a
    /// template.
    Generated,
    /// Anything else, which is simply inferred.
    Other,
}

impl BodyRole {
    const fn of(generic: bool, generated: bool, stub: bool) -> Self {
        if generated {
            Self::Generated
        } else if generic && !stub {
            Self::Template
        } else {
            Self::Other
        }
    }
}

enum BodyDeclaration<'a> {
    Def(&'a Stmt),
    Method(&'a mojito_ast::ast::Method),
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
    /// A method call's receiver occurrence and method name.
    method_call: Option<(SyntaxId, String)>,
    /// Whether this is a `^` transfer.
    transfer: bool,
}

impl Checker {
    pub(super) fn count_body_inference(class: BodyClass, name: impl FnOnce() -> String) {
        timing::count(class.counter(), 1);
        timing::note(class.counter(), name);
    }

    /// Check a module-level or nested `def` body, or serve it from a checked
    /// template.
    pub(super) fn check_def_body(
        &mut self,
        stmt: &Stmt,
        decls: &[ParamDecl],
        ret_ty: &Ty,
        module_level: bool,
    ) -> Result<(), TypeError> {
        let StmtKind::Def {
            name,
            params,
            type_params,
            body,
            ..
        } = &stmt.kind
        else {
            return Err(TypeError::InvariantViolation(
                "check_def_body requires a function declaration".to_string(),
            ));
        };
        let generated = self.template_catalog.borrow().generated_def(name);
        let template_id = TemplateId {
            module: stmt.module.clone(),
            owner: None,
            name: name.clone(),
            declaration: stmt.span,
        };
        // In an elaborated program, a declaration source validation already
        // recorded is that template's trapping stub, not a template.
        let stub =
            !self.source_validation && self.template_catalog.borrow().validated(&template_id);
        let site = BodySite {
            display: name.clone(),
            body,
            template_id,
            instance: InstanceName {
                module: stmt.module.clone(),
                owner: None,
                name: name.clone(),
                body: None,
            },
            role: BodyRole::of(module_level && !decls.is_empty(), generated, stub),
            decls,
            ret_ty,
            participates: module_level,
            residual_binders: !type_params.is_empty(),
            declaration: BodyDeclaration::Def(stmt),
            receiver_arguments: None,
        };
        let params = BodyParams {
            receiver: None,
            runtime: params
                .iter()
                .map(|param| self.lookup_owner(&param.name))
                .collect(),
            compile_time: self.compile_time_owners(decls),
        };
        self.check_body(&site, &params)
    }

    /// Check a struct method's body, or serve it from a checked template.
    ///
    /// `owner` and `module` name the declaring struct, `receiver` is the type
    /// `self` has in this body (the instance, for a per-instantiation clone),
    /// and `decls` are the struct's binders followed by the method's own.
    pub(super) fn check_method_body(
        &mut self,
        owner: &str,
        module: Option<&String>,
        receiver: &Ty,
        m: &mojito_ast::ast::Method,
        ret_ty: &Ty,
    ) -> Result<(), TypeError> {
        let Some(first) = m.body.first() else {
            return self.check_block(&m.body, Some(ret_ty), false);
        };
        let method_decls = self.classify_params(&m.type_params)?;
        let mut decls = self.self_decls.clone();
        decls.extend(method_decls.iter().cloned());
        // A per-instantiation clone carries its receiver type. Every other
        // generated method is one the elaborator listed: a per-call clone, or
        // a member of a struct it specialized whole (`Tuple$…`).
        let generated = m.self_ty.is_some()
            || self
                .template_catalog
                .borrow()
                .generated_method(owner, &m.name);
        if m.self_ty.is_some() {
            timing::count("body_sites.instance_clone", 1);
        }
        let template_id = TemplateId {
            module: module.cloned(),
            owner: Some(owner.to_string()),
            name: m.name.clone(),
            declaration: first.span,
        };
        let stub =
            !self.source_validation && self.template_catalog.borrow().validated(&template_id);
        let site = BodySite {
            display: format!("{owner}.{}", m.name),
            body: &m.body,
            template_id,
            instance: InstanceName {
                module: first.module.clone(),
                owner: Some(owner.to_string()),
                name: m.name.clone(),
                body: Some(first.span),
            },
            role: BodyRole::of(!decls.is_empty(), generated, stub),
            decls: &decls,
            ret_ty,
            participates: true,
            residual_binders: !m.type_params.is_empty(),
            declaration: BodyDeclaration::Method(m),
            receiver_arguments: match receiver {
                Ty::Struct(_, arguments) => Some(arguments.clone()),
                _ => None,
            },
        };
        let params = BodyParams {
            receiver: m.has_self.then(|| self.lookup_owner("self")).flatten(),
            runtime: m
                .params
                .iter()
                .map(|param| self.lookup_owner(&param.name))
                .collect(),
            compile_time: self.compile_time_owners(&method_decls),
        };
        self.check_body(&site, &params)
    }

    /// The binding identities of a declaration's own value parameters.
    fn compile_time_owners(&self, decls: &[ParamDecl]) -> Vec<(String, OwnerId)> {
        decls
            .iter()
            .filter_map(|decl| match decl {
                ParamDecl::Value { name, .. } => {
                    let name = name.trim_start_matches('*');
                    self.lookup_owner(name)
                        .map(|owner| (name.to_string(), owner))
                }
                ParamDecl::Type { .. } => None,
            })
            .collect()
    }

    /// Infer one body, or serve it from a checked template.
    ///
    /// A clone the elaborator traced to a certified template has the
    /// template's facts installed and is not inferred. Any other body is
    /// inferred by `check_block`; a generic one then has its facts retained
    /// for its instances. With fact verification on, a derivable body is
    /// inferred as well and the two fact bundles must agree.
    fn check_body(
        &mut self,
        site: &BodySite<'_>,
        param_owners: &BodyParams,
    ) -> Result<(), TypeError> {
        let name = &site.display;
        let body = site.body;
        let (decls, ret_ty) = (site.decls, site.ret_ty);
        let template = site.role == BodyRole::Template;
        let generated = site.role == BodyRole::Generated;
        let derived = if site.participates && !self.source_validation {
            self.derivable_facts(site, param_owners)
        } else {
            None
        };
        let verify = self.template_catalog.borrow().verify();
        if let Some((facts, spans)) = &derived
            && !verify
        {
            self.install_body_facts(facts, spans, param_owners)?;
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
        // Capturing a body walks every fact table for every occurrence, so
        // the syntax is judged first: a generic declaration outside every
        // class is retained as such without being captured.
        let shape = template.then(|| self.certificate(site, None));
        let admitted = matches!(shape, Some(TemplateCoverage::Certified(_)));
        let census = site.participates && !decls.is_empty() && timing::enabled();
        let baseline = (derived.is_some() || admitted || census || timing::notes_enabled())
            .then(|| self.body_fact_baseline());
        Self::count_body_inference(BodyClass::of(generated, !decls.is_empty()), || name.clone());
        self.effect_query_frames
            .borrow_mut()
            .push(baseline.is_some().then(Vec::new));
        self.struct_application_frames
            .borrow_mut()
            .push(baseline.is_some().then(Vec::new));
        let inferred = self.check_block(body, Some(ret_ty), false);
        let reads = BodyReads {
            effect_queries: self
                .effect_query_frames
                .borrow_mut()
                .pop()
                .flatten()
                .unwrap_or_default(),
            struct_applications: self
                .struct_application_frames
                .borrow_mut()
                .pop()
                .flatten()
                .unwrap_or_default(),
        };
        // An application a nested body reached is the enclosing body's too.
        if let Some(Some(outer)) = self.struct_application_frames.borrow_mut().last_mut() {
            outer.extend(reads.struct_applications.iter().cloned());
        }
        inferred?;
        let traced = !template && self.instance_trace(site).is_some();
        if traced {
            self.template_catalog
                .borrow_mut()
                .stats_mut()
                .inferred_clones
                .push(name.clone());
        }
        let Some(baseline) = baseline else {
            if let Some(outside) = shape {
                self.retain_template(site, CheckedBodyFacts::default(), outside);
            }
            return Ok(());
        };
        if let Some((facts, _)) = &derived {
            let inferred = self
                .capture_body_facts(body, param_owners, &baseline, &reads)
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
                self.replace_body_facts(body, facts, param_owners)?;
                timing::count("template_derivations.overload_rebinding", 1);
            } else if inferred != *facts {
                return Err(TypeError::InvariantViolation(format!(
                    "template fact verification: derived facts for '{name}' differ from its own \
                     check:\n{}",
                    facts.difference(&inferred)
                )));
            }
            timing::count("template_derivations.verified", 1);
            self.template_catalog
                .borrow_mut()
                .stats_mut()
                .verified
                .push(name.clone());
        }
        if census {
            self.census(site, param_owners, &baseline, &reads);
        }
        if timing::notes_enabled() && matches!(shape, Some(TemplateCoverage::Incomplete(_))) {
            timing::note("template_capture.body", || name.clone());
            let _ = self.capture_body_facts(body, param_owners, &baseline, &reads);
        }
        if derived.is_none()
            && let Some(shape) = shape
        {
            self.record_template(site, param_owners, &baseline, &reads, shape);
        } else if (traced || template) && timing::notes_enabled() {
            // Capture emits the `template_capture.tables` note: what this
            // body recorded, for comparing a clone with its template.
            timing::note("template_capture.body", || name.clone());
            let _ = self.capture_body_facts(body, param_owners, &baseline, &reads);
        }
        Ok(())
    }

    /// Record that the body being inferred read `callee`'s transfer or
    /// call-through summary, and whether it was empty.
    pub(super) fn note_effect_query(&self, callee: &str, empty: bool) {
        if let Some(Some(frame)) = self.effect_query_frames.borrow_mut().last_mut() {
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
    fn instance_trace(&self, site: &BodySite<'_>) -> Option<InstanceTrace> {
        self.template_catalog
            .borrow()
            .trace(&site.instance)
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
        site: &BodySite<'_>,
        param_owners: &BodyParams,
    ) -> Option<(CheckedBodyFacts, HashMap<OccurrenceId, SourceSpan>)> {
        let name = &site.display;
        let template = site.role == BodyRole::Template;
        let trace = if template {
            InstanceTrace {
                template: site.template_id.clone(),
                type_bindings: Vec::new(),
                value_bindings: Vec::new(),
                residual: Vec::new(),
            }
        } else {
            self.instance_trace(site)?
        };
        let refusals = RefCell::new(Vec::new());
        let refuse = |reason: &'static str| {
            if !template {
                timing::count("template_derivations.ineligible", 1);
                timing::note("template_derivations.ineligible", || {
                    format!("{name}: {reason}")
                });
                refusals
                    .borrow_mut()
                    .push((name.clone(), reason.to_string()));
            }
            None
        };
        let derived = self.derive(site, param_owners, &trace, template, &refuse);
        self.template_catalog
            .borrow_mut()
            .stats_mut()
            .refused
            .extend(refusals.into_inner());
        derived
    }

    /// [`Self::derivable_facts`] once the body's trace is known.
    fn derive(
        &self,
        site: &BodySite<'_>,
        param_owners: &BodyParams,
        trace: &InstanceTrace,
        template: bool,
        refuse: &dyn Fn(
            &'static str,
        ) -> Option<(CheckedBodyFacts, HashMap<OccurrenceId, SourceSpan>)>,
    ) -> Option<(CheckedBodyFacts, HashMap<OccurrenceId, SourceSpan>)> {
        let name = &site.display;
        let body = site.body;
        let catalog = self.template_catalog.borrow();
        let Some(checked) = catalog.template(&trace.template) else {
            return refuse("its template has no retained facts");
        };
        let TemplateCoverage::Certified(class) = &checked.coverage else {
            return refuse("its template is not certified");
        };
        // Both enabled classes bake every parameter: a clone that keeps a
        // binder, or folds a value, is outside them.
        let baked = trace.residual.is_empty()
            && !site.residual_binders
            && match class {
                TemplateClass::ClosedScalarBody
                | TemplateClass::FixedCalls
                | TemplateClass::BoundedOperations
                | TemplateClass::MethodScalarBody
                | TemplateClass::MethodBody(_) => trace.value_bindings.is_empty(),
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
        let Ok(substitution) = self.instance_substitution(site, checked, trace) else {
            return refuse("an instance argument does not resolve");
        };
        if matches!(class, TemplateClass::MethodBody(_))
            && !substitution.values().all(|ty| self.plain_data(ty))
        {
            return refuse("an instance argument carries a loan, a reference, or a callable");
        }
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

    /// [`TemplateObligation::PlainDataArguments`] for one instance argument.
    fn plain_data(&self, ty: &Ty) -> bool {
        !self.type_may_carry_loans(ty)
            && !self.type_contains_reference(ty)
            && !mentions_callable(ty)
    }

    /// The checked type each baked type parameter stands for in a clone.
    ///
    /// A `def` clone's are the source types the elaborator wrote, resolved as
    /// the clone's own annotations are. A method clone's are the arguments of
    /// the receiver type the checker already resolved for it, in the struct's
    /// binder order: the raw request passes through origin erasure and
    /// literal defaulting before the clone is minted, so only the resolved
    /// receiver says what `Self.T` is inside the clone.
    fn instance_substitution(
        &self,
        site: &BodySite<'_>,
        template: &CheckedTemplate,
        trace: &InstanceTrace,
    ) -> Result<HashMap<String, Ty>, TypeError> {
        let Some(arguments) = &site.receiver_arguments else {
            return trace
                .type_bindings
                .iter()
                .map(|(name, source)| Ok((name.clone(), self.ty_from_anno(source)?)))
                .collect();
        };
        if site.role == BodyRole::Template {
            return Ok(HashMap::new());
        }
        let unresolved = || {
            TypeError::InvariantViolation(
                "a method clone's receiver does not bind its struct's parameters".to_string(),
            )
        };
        if arguments.len() != template.param_decls.len() {
            return Err(unresolved());
        }
        template
            .param_decls
            .iter()
            .zip(arguments)
            .map(|(decl, argument)| match (decl, argument) {
                (ParamDecl::Type { name, .. }, mojito_types::types::TyArg::Ty(ty)) => {
                    Ok((name.trim_start_matches('*').to_string(), ty.clone()))
                }
                _ => Err(unresolved()),
            })
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
        let substituted_reference = |(id, reference): &(OccurrenceId, TemplateReference)| {
            (
                *id,
                TemplateReference {
                    referent: substitute(&reference.referent),
                    ..reference.clone()
                },
            )
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
            reference_results: template
                .reference_results
                .iter()
                .map(substituted_reference)
                .collect(),
            reference_binding_types: template
                .reference_binding_types
                .iter()
                .map(substituted_reference)
                .collect(),
            reference_place_types: template
                .reference_place_types
                .iter()
                .map(substituted_reference)
                .collect(),
            effect_free_callees: Vec::new(),
            ..template.clone()
        };
        // Inference marks a reference result a copyable read where its
        // referent is implicitly copyable, whatever reads it. A mark the
        // template made is one a by-value read may rest on.
        facts.copyable_reference_result_reads = facts
            .reference_results
            .iter()
            .filter(|(_, reference)| self.is_implicitly_copyable(&reference.referent))
            .map(|(id, _)| *id)
            .collect();
        if !template
            .copyable_reference_result_reads
            .iter()
            .all(|read| facts.copyable_reference_result_reads.contains(read))
        {
            return Err("a reference result is not implicitly copyable for the instance");
        }
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
        // `Movable`, which a symbolic parameter always is.
        let movable = template.transfers.iter().all(|transfer| {
            fact_at(&template.expression_types, *transfer)
                .filter(|ty| mojito_types::types::is_symbolic(ty))
                .is_none_or(|ty| self.is_movable(&substitute(ty)))
        });
        if !movable {
            return Err("a transferred value is not movable for the instance");
        }
        // The declaration's own judgment of each binding whose type was a
        // parameter, at the instance's type. One built over a parameter has
        // no entry to judge again.
        for (id, declared) in &template.binding_types {
            if !mojito_types::types::is_symbolic(declared) {
                continue;
            }
            if !matches!(declared, Ty::Param { .. }) {
                return Err("a binding's type is built over a parameter");
            }
            let realized = substitute(declared);
            facts.deletable_bindings.retain(|binding| binding != id);
            facts.linear_bindings.retain(|binding| binding != id);
            if self.is_deinitable(&realized) {
                facts.deletable_bindings.push(*id);
            } else if matches!(realized, Ty::Param { .. }) {
                facts.linear_bindings.push(*id);
            }
        }
        let order = |id: &OccurrenceId| occurrences.iter().position(|found| found.id == *id);
        facts.deletable_bindings.sort_by_key(order);
        facts.linear_bindings.sort_by_key(order);
        facts.linear_temporaries.retain(|temporary| {
            fact_at(&facts.expression_types, *temporary)
                .is_some_and(|ty| matches!(ty, Ty::Param { .. }))
        });
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
            // A method call records its (empty) parameters here too. Its
            // callee is realized from its contract below.
            if template.selected_calls.iter().any(|(call, _)| call == id) {
                continue;
            }
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
        facts.struct_applications = template
            .struct_applications
            .iter()
            .map(|(name, arguments)| {
                (
                    name.clone(),
                    mojito_types::types::map_tyargs(arguments, &substitute),
                )
            })
            .collect();
        for index in 0..facts.selected_calls.len() {
            self.realize_method_call(&mut facts, index, occurrences, substitution)?;
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
    fn realize_method_call(
        &self,
        facts: &mut CheckedBodyFacts,
        index: usize,
        occurrences: &[Occurrence],
        substitution: &HashMap<String, Ty>,
    ) -> Result<(), &'static str> {
        let id = facts.selected_calls[index].0;
        let (receiver, method) = occurrences
            .iter()
            .find(|occurrence| occurrence.id == id)
            .and_then(|occurrence| occurrence.method_call.clone())
            .ok_or("a selected call is not a method call in the instance")?;
        let receiver = OccurrenceId {
            syntax: receiver,
            copy: id.copy,
        };
        let Some(Ty::Struct(owner, arguments)) =
            fact_at(&facts.expression_types, receiver).cloned()
        else {
            return Err("a method call's receiver is not a nominal struct");
        };
        let info = self
            .structs
            .get(&owner)
            .ok_or("a method call's receiver struct is not declared")?;
        let family = info
            .methods
            .get(&method)
            .ok_or("a called method is missing")?;
        let self_ty = self.self_instance_ty(&owner);
        let selected = &facts.selected_calls[index].1.contract.target;
        let clone_name =
            mojito_symbol::symbol::instance_method_clone_name(&method, &info.decls, &arguments);
        // A receiver whose type was already closed in the template (`List[Pair]`)
        // selected its clone there, on the arguments a clone check ranks too.
        let selected_clone = clone_name.as_deref().is_some_and(|clone| {
            selected
                .strip_prefix(owner.as_str())
                .and_then(|rest| rest.strip_prefix('.'))
                .and_then(|rest| rest.strip_prefix(clone))
                .is_some_and(|overload| overload.is_empty() || overload.starts_with('$'))
        });
        if selected_clone {
            let target = selected.clone();
            if !facts.effect_free_callees.contains(&target) {
                facts.effect_free_callees.push(target);
            }
            return Ok(());
        }
        let declared = match family.as_slice() {
            [only] => only,
            members => members
                .iter()
                .find(|member| {
                    super::overload_support::method_lowered_name(
                        &owner,
                        &method,
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
        if !closed_family {
            return Err("an overloaded callee declares a parameter of a parameter type");
        }
        let target = if self
            .instance_method_clone(&owner, &method, &arguments)
            .is_some()
        {
            self.method_clone_target(&owner, &method, &arguments, declared, substitution)
                .ok_or("a called method's clone family has no member for the selected overload")?
        } else {
            // No clone of this method. If the instance has clones of others,
            // this one was withheld from it or collapsed.
            let suffix = clone_name
                .as_deref()
                .and_then(|clone| clone.strip_prefix(method.as_str()));
            if suffix.is_some_and(|suffix| info.methods.keys().any(|name| name.ends_with(suffix))) {
                return Err("the instance has clones, but not of a called method");
            }
            if !declared.availability.is_empty() && !substitution.is_empty() {
                return Err("a called method has an availability condition and no clone");
            }
            selected.clone()
        };
        let contract = &mut facts.selected_calls[index].1.contract;
        contract.target.clone_from(&target);
        contract.result_ty = mojito_types::types::substitute(&contract.result_ty, substitution);
        if let Some(reference) = &mut facts.selected_calls[index].1.reference_result {
            reference.referent = mojito_types::types::substitute(&reference.referent, substitution);
        }
        if let Some(entry) = facts
            .overload_targets
            .iter_mut()
            .find(|(site, _)| *site == id)
        {
            entry.1.clone_from(&target);
        }
        if !facts.effect_free_callees.contains(&target) {
            facts.effect_free_callees.push(target);
        }
        Ok(())
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
        let named = occurrences
            .iter()
            .any(|occurrence| occurrence.id == argument && occurrence.identifier);
        let in_place =
            named && matches!(&ty, Ty::Struct(name, _) if self.structs.contains_key(name));
        // The template's own entry stands: a reference-valued operand is
        // read through its handle whatever the instance, and a nominal type
        // stays nominal under substitution. Only the nominal-place rule can
        // newly hold for an instance.
        if in_place && !facts.borrowed_read_call_places.contains(&argument) {
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
        site: &BodySite<'_>,
        param_owners: &BodyParams,
        baseline: &BodyFactBaseline,
        reads: &BodyReads,
        shape: TemplateCoverage,
    ) {
        let (facts, coverage) = match shape {
            outside @ TemplateCoverage::Incomplete(_) => (CheckedBodyFacts::default(), outside),
            TemplateCoverage::Certified(_) => {
                let captured = self
                    .capture_body_facts(site.body, param_owners, baseline, reads)
                    .and_then(|facts| {
                        // Two template occurrences sharing one identity could
                        // not be told apart from an instance's loop copies.
                        if facts.occurrences.iter().all(|id| id.copy == 0) {
                            Ok(facts)
                        } else {
                            Err(IncompleteReason::AmbiguousOccurrence)
                        }
                    });
                match captured {
                    Ok(facts) => {
                        let coverage = self.certificate(site, Some(&facts));
                        (facts, coverage)
                    }
                    Err(reason) => (
                        CheckedBodyFacts::default(),
                        TemplateCoverage::Incomplete(reason),
                    ),
                }
            }
        };
        self.retain_template(site, facts, coverage);
    }

    /// The certificate of a body's class. With no facts it judges the
    /// declaration's syntax alone: `Certified` then means only that the body
    /// is worth capturing.
    fn certificate(
        &self,
        site: &BodySite<'_>,
        facts: Option<&CheckedBodyFacts>,
    ) -> TemplateCoverage {
        let (decls, ret_ty) = (site.decls, site.ret_ty);
        match site.declaration {
            BodyDeclaration::Def(stmt) => self.template_certificate(stmt, decls, ret_ty, facts),
            BodyDeclaration::Method(method) => {
                self.method_certificate(method, decls, ret_ty, facts)
            }
        }
    }

    /// Put a template in the catalog, counted by its coverage.
    fn retain_template(
        &self,
        site: &BodySite<'_>,
        facts: CheckedBodyFacts,
        coverage: TemplateCoverage,
    ) {
        let name = &site.display;
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
        let method_body = matches!(
            coverage,
            TemplateCoverage::Certified(TemplateClass::MethodBody(_))
        );
        self.template_catalog.borrow_mut().record(CheckedTemplate {
            id: site.template_id.clone(),
            producer: if self.source_validation {
                TemplateProducer::SourceValidation
            } else {
                TemplateProducer::ExecutableCheck
            },
            param_decls: site.decls.to_vec(),
            facts,
            coverage,
            obligations: [
                TemplateObligation::DeclarationConstraints,
                TemplateObligation::RebindEqualities,
                TemplateObligation::ImplicitCopies,
            ]
            .into_iter()
            .chain([
                TemplateObligation::Movable,
                TemplateObligation::Deletability,
                TemplateObligation::ReferenceResultReads,
            ])
            .chain(method_body.then_some(TemplateObligation::PlainDataArguments))
            .collect(),
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
        facts: Option<&CheckedBodyFacts>,
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
            borrowed_params: Vec::new(),
            mut_params: Vec::new(),
            keyed,
            receiver: false,
            self_writable: false,
            moved_result: None,
            reference_result: None,
            features: std::cell::Cell::default(),
            locals: RefCell::new(Vec::new()),
            handles: RefCell::new(Vec::new()),
            references: RefCell::new(Vec::new()),
            receivers: RefCell::new(Vec::new()),
        };
        if !shape.block(body, false) {
            return outside("the body is not scalar returns over direct calls and 'len'");
        }
        let Some(facts) = facts else {
            return TemplateCoverage::Certified(TemplateClass::ClosedScalarBody);
        };
        if !shape.references_recorded(facts) {
            return outside("an expression yields or keeps a reference");
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

    /// The certificate a freshly captured method of a generic struct earns.
    ///
    /// [`TemplateClass::MethodScalarBody`]: a method with a plain read `self`
    /// and no binders of its own, on a struct whose binders are plain type
    /// parameters, returning a concrete scalar over closed scalars, runtime
    /// parameters, and reads of `self`'s scalar fields. It calls nothing.
    ///
    /// What a clone check would repeat is covered as follows.
    ///
    /// - Types: a field read is the field's declared type under the struct's
    ///   arguments in a template and a clone alike, and the class admits the
    ///   read only when that type is already a closed scalar. `self` itself
    ///   substitutes to the clone's resolved receiver.
    /// - Copies: a scalar field copied out is implicitly copyable under every
    ///   instance; realization still re-judges each copied place.
    /// - The generated-declaration leniency a clone's name switches on bears
    ///   on origin-bearing return annotations only, and the result is a
    ///   scalar.
    /// - Nothing outside the fact tables: the body wrote through no origin
    ///   parameter, transferred nothing, and recorded no request.
    ///
    /// [`TemplateClass::MethodBody`] widens that along eight independent
    /// [`MethodFeatures`]. The receiver may be `mut`, `var`, `deinit`, a bare
    /// `ref`, the `out` of an `__init__`, or absent (`@staticmethod`), and
    /// the method may carry a `where` clause: the elaborator mints a clone
    /// only where the clause holds, and a clone's signature no longer states
    /// it. A parameter may be `var`, `mut`, or a bare `ref`: it is bound from
    /// its declared convention and rooted at its own binding under every
    /// instance, its loan state is decided by a property
    /// [`TemplateObligation::PlainDataArguments`] rules out, and what its
    /// caller owes lives in the signature, which is checked per clone. A `mut`
    /// parameter may be stored to; a bare `ref` one has parametric
    /// mutability, and a write through it is refused below. The copy and move
    /// initializers, a receiver or parameter origin, binders, and `raises`
    /// stay outside.
    ///
    /// - `STATEMENTS`: a runtime statement is checked once whatever runs it,
    ///   so `if`, `while`, `break`, `continue`, and a bare `return` neither
    ///   drop nor copy an occurrence. A condition's recorded type is exactly
    ///   `Bool`, which `expect_bool` accepts without a truthiness fact. A
    ///   stored field is a closed scalar, so the store is never an in-place
    ///   operator of the field's type. Invalidations name `self` and locals
    ///   by template owner.
    /// - `OPAQUE_MOVES`: a value of any type is moved or copied whole between
    ///   a parameter, a local, a field of `self`, and the result, and is
    ///   never an operand, a receiver, a condition, or an argument, so
    ///   nothing dispatches on its type. A store or a result has the value's
    ///   own recorded type, which stays equal under substitution, so neither
    ///   check converts. What a clone check still decides from the type is
    ///   owed per instance: the copy of a place (admitted only where the
    ///   template recorded it), `Movable` at a transfer, whether a local can
    ///   be destroyed, and that an argument is plain data
    ///   ([`TemplateObligation`]).
    /// - `POINTER_SLOTS`: a pointer field with no tracked provenance holds no
    ///   loan, names no place, and is a pointer under every instance, so its
    ///   methods are the built-in ones and record an adjustment naming at
    ///   most the pointee (`derive_adjustment`). Every other judgment there
    ///   only produces an error, and the template's is at least as strict.
    /// - `SIBLING_CALLS`: see [`Self::realize_method_call`] and
    ///   `closed_method_contract`. Arguments are closed scalars in both
    ///   checks; a callee summary that is not empty refuses.
    /// - `REFERENCE_RESULT`: see [`BodyShape::returned_place`]. The handle a
    ///   `return` keeps is decided by the declaration and the statement's
    ///   syntax, and the declared origin is checked on the place's path and
    ///   the signature, which the clone's own signature check repeats.
    /// - `REFERENCE_CALLS`: see [`BodyShape::reference_call`] and
    ///   `closed_reference_contract`. The reference a call yields names the
    ///   receiver, so it is kept by template owner
    ///   ([`TemplateReference`]), and an instance marks its copyable reads
    ///   again at its own referent.
    /// - `REFERENCE_LOCALS`: see [`BodyShape::bound_place`]. A `ref`
    ///   binding's type is a reference naming a binding, kept by template
    ///   owner like a call's; every use records the referent.
    /// - `REFERENCE_RECEIVERS`: see [`BodyShape::reference_receiver`]. A call
    ///   borrows a receiver reached through a reference because of what the
    ///   receiver is, and the callee is realized as a sibling call's is.
    ///
    /// Any other handle, borrowed receiver, reference result, interior
    /// reference, or copyable read in the body refuses it
    /// ([`BodyShape::references_recorded`]).
    fn method_certificate(
        &self,
        method: &mojito_ast::ast::Method,
        decls: &[ParamDecl],
        ret_ty: &Ty,
        facts: Option<&CheckedBodyFacts>,
    ) -> TemplateCoverage {
        use mojito_ast::ast::ArgConvention;
        let outside =
            |what| TemplateCoverage::Incomplete(IncompleteReason::OutsideEnabledClass(what));
        if self.source_validation {
            return outside("a compile-time-keyed method has no class yet");
        }
        // `__init__(out self, …)` is an ordinary owned receiver here: which
        // fields a body initializes is its syntax, and definite initialization
        // is judged outside the body check. The copy and move initializers
        // take `existing`, whose conventions this class does not admit.
        let lifecycle = mojito_symbol::symbol::lifecycle_method_name(method);
        let initializer = matches!(lifecycle, "__copyinit__" | "__moveinit__");
        let constructs =
            lifecycle == "__init__" && method.self_convention == Some(ArgConvention::Out);
        let is_static = !method.has_self
            && matches!(method.decorators.as_slice(), [decorator]
                if decorator.path == ["staticmethod"]
                    && decorator.args.is_empty()
                    && decorator.kwargs.is_empty());
        let plain_read =
            method.has_self && matches!(method.self_convention, None | Some(ArgConvention::Imm));
        // A bare `ref self` has parametric mutability, so the body may not
        // write through it, and one binding identity names it under every
        // instance as it does any other receiver.
        let owned_receiver = method.has_self
            && matches!(
                method.self_convention,
                Some(
                    ArgConvention::Mut
                        | ArgConvention::Var
                        | ArgConvention::Deinit
                        | ArgConvention::Ref
                )
            );
        if !(plain_read || owned_receiver || is_static || constructs)
            || method.self_origin.is_some()
            || initializer
        {
            return outside("the receiver carries an origin, or is a copy or move initializer's");
        }
        // A `where` clause is the declaration's constraint: the elaborator
        // mints a clone only where it evaluates true, and a trace exists only
        // for a minted clone (`TemplateObligation::DeclarationConstraints`).
        if !method.type_params.is_empty()
            || !(method.decorators.is_empty() || is_static)
            || method.raises
            || method.raises_type.is_some()
        {
            return outside("the method has binders, decorators, or raises");
        }
        let plain_struct = decls.iter().all(|decl| {
            matches!(
                decl,
                ParamDecl::Type {
                    variadic: false,
                    callable_bound: None,
                    ..
                }
            )
        });
        if !plain_struct {
            return outside("a struct parameter is not a plain type parameter");
        }
        // A `mut` or bare `ref` parameter is bound from its declared
        // convention alone, and is rooted at its own binding under every
        // instance. What its caller owes lives in the signature, which is
        // checked per clone.
        let plain_params = method.params.iter().all(|parameter| {
            parameter.kind == mojito_ast::ast::ParamKind::Regular
                && matches!(
                    parameter.convention,
                    None | Some(ArgConvention::Var | ArgConvention::Mut | ArgConvention::Ref)
                )
                && parameter.default.is_none()
                && parameter.origin.is_none()
        });
        if !plain_params {
            return outside(
                "a parameter has a default, an origin, or an 'out' or 'deinit' convention",
            );
        }
        let params_passed = |conventions: &[ArgConvention]| {
            method
                .params
                .iter()
                .filter(|parameter| {
                    parameter
                        .convention
                        .is_some_and(|convention| conventions.contains(&convention))
                })
                .map(|parameter| parameter.name.as_str())
                .collect::<Vec<_>>()
        };
        let returns_reference = self
            .return_ref_contracts
            .last()
            .is_some_and(Option::is_some);
        let owned_parameter = method
            .params
            .iter()
            .any(|parameter| parameter.convention.is_some());
        let shape = BodyShape {
            origins: &self.syntax_origins,
            facts,
            params: method
                .params
                .iter()
                .map(|parameter| parameter.name.as_str())
                .collect(),
            borrowed_params: params_passed(&[ArgConvention::Mut, ArgConvention::Ref]),
            mut_params: params_passed(&[ArgConvention::Mut]),
            keyed: false,
            receiver: method.has_self,
            self_writable: matches!(
                method.self_convention,
                Some(ArgConvention::Mut | ArgConvention::Var | ArgConvention::Out)
            ),
            moved_result: (!returns_reference).then_some(ret_ty),
            reference_result: returns_reference.then_some(ret_ty),
            features: std::cell::Cell::new(
                if plain_read && closed_scalar(ret_ty) && !owned_parameter {
                    MethodFeatures::default()
                } else {
                    MethodFeatures::STATEMENTS
                },
            ),
            locals: RefCell::new(Vec::new()),
            handles: RefCell::new(Vec::new()),
            references: RefCell::new(Vec::new()),
            receivers: RefCell::new(Vec::new()),
        };
        if !shape.block(&method.body, false) {
            return outside("the body is outside the method grammar");
        }
        let class = || {
            let features = shape.features.get();
            TemplateCoverage::Certified(if features.is_empty() {
                TemplateClass::MethodScalarBody
            } else {
                TemplateClass::MethodBody(features)
            })
        };
        let Some(facts) = facts else {
            return class();
        };
        if !shape.references_recorded(facts) {
            return outside("a reference is yielded or kept outside the method grammar");
        }
        // The grammar admitted every method call it judged closed. Nothing
        // else may have selected a callee or read an effect summary.
        let targets: Vec<&str> = facts
            .selected_calls
            .iter()
            .map(|(_, call)| call.contract.target.as_str())
            .collect();
        let at_method_call =
            |id: &OccurrenceId| facts.selected_calls.iter().any(|(call, _)| call == id);
        let stray_call = facts
            .call_parameters
            .iter()
            .any(|(id, _)| !at_method_call(id))
            || !facts.generic_instantiations.is_empty()
            || !facts
                .overload_targets
                .iter()
                .all(|(id, _)| at_method_call(id))
            || !facts
                .effect_free_callees
                .iter()
                .all(|callee| targets.contains(&callee.as_str()));
        if stray_call {
            return outside("the body calls something other than a trivial method");
        }
        let effects_closed = facts
            .expression_effects
            .iter()
            .all(|(_, effects)| *effects == mojito_checked::checked::EffectFacts::default());
        let adjustments_derive = facts.operation_adjustments.iter().all(|(_, adjustment)| {
            mojito_checked::templates::derive_adjustment(adjustment, &Ty::clone).is_some()
        });
        if !effects_closed || !adjustments_derive {
            return outside("an expression has an effect or an adjustment with no recipe");
        }
        let wrote_through_origin = self
            .parametric_write_frames
            .borrow()
            .last()
            .is_some_and(|frame| !frame.is_empty());
        if wrote_through_origin {
            return outside("the body writes through an origin parameter");
        }
        class()
    }

    /// Report, for one generic body, everything that keeps it from being
    /// captured: the census that orders which recipes to write next.
    ///
    /// Counters are `template_census.<class>.bodies`, `.capturable`,
    /// `.blocked_by.<reason>` (the body has that reason),
    /// `.sole_blocker.<reason>` (it has no other), and, for a method,
    /// `.grammar.<construct>` for each construct its declaration and body
    /// hold, whatever class admits it (`grammar_features`).
    fn census(
        &self,
        site: &BodySite<'_>,
        param_owners: &BodyParams,
        baseline: &BodyFactBaseline,
        reads: &BodyReads,
    ) {
        if !timing::enabled() {
            return;
        }
        let class = if site.role == BodyRole::Generated {
            "clone"
        } else {
            "template"
        };
        let occurrences = self.body_occurrences(site.body);
        let mut reasons: Vec<String> = self
            .unkeyed_fact_entries()
            .into_iter()
            .zip(baseline.unkeyed)
            .filter(|(now, before)| now != before)
            .map(|((store, _), _)| format!("store:{store}"))
            .collect();
        if reads.effect_queries.iter().any(|(_, empty)| !empty)
            || self
                .transfer_frames
                .borrow()
                .last()
                .is_some_and(|frame| !frame.effects.is_empty() || !frame.call_throughs.is_empty())
        {
            reasons.push("effects".to_string());
        }
        for (index, table) in FactTable::ALL.into_iter().enumerate() {
            let entries = self.span_table(table);
            let recorded = occurrences
                .iter()
                .filter(|occurrence| entries.has(&occurrence.span))
                .count();
            if entries.entries() != baseline.tables[index] + recorded {
                reasons.push(format!("outside:{table:?}"));
            } else if recorded > 0 && !derivable_table(table) {
                reasons.push(format!("table:{table:?}"));
            }
        }
        // The adjustment table has a recipe per variant, not per table.
        let mut adjustments: Vec<String> = occurrences
            .iter()
            .filter_map(|occurrence| {
                let adjustments = self.operation_adjustments.borrow();
                let adjustment = adjustments.get(&occurrence.span)?;
                let kept_apart = matches!(
                    adjustment,
                    mojito_checked::checked::SemanticAdjustment::ReferenceResult { .. }
                );
                (!kept_apart
                    && mojito_checked::templates::derive_adjustment(adjustment, &Ty::clone)
                        .is_none())
                .then(|| {
                    let spelled = format!("{adjustment:?}");
                    let variant = spelled
                        .split(|c: char| !c.is_alphanumeric())
                        .next()
                        .unwrap_or_default();
                    format!("adjustment:{variant}")
                })
            })
            .collect();
        adjustments.sort();
        adjustments.dedup();
        reasons.extend(adjustments);
        if reasons.is_empty()
            && self
                .capture_body_facts(site.body, param_owners, baseline, reads)
                .is_err()
        {
            reasons.push("binding".to_string());
        }
        timing::note("template_census.body", || {
            format!("{class} {}: {}", site.display, reasons.join(" "))
        });
        if let BodyDeclaration::Method(method) = site.declaration {
            let features = grammar_features(method, site.ret_ty);
            timing::note("template_census.grammar", || {
                format!("{class} {}: {}", site.display, features.join(" "))
            });
            for feature in &features {
                timing::count_named(|| format!("template_census.{class}.grammar.{feature}"), 1);
            }
        }
        timing::count_named(|| format!("template_census.{class}.bodies"), 1);
        if reasons.is_empty() {
            timing::count_named(|| format!("template_census.{class}.capturable"), 1);
        }
        for reason in &reasons {
            timing::count_named(|| format!("template_census.{class}.blocked_by.{reason}"), 1);
        }
        if let [only] = reasons.as_slice() {
            timing::count_named(|| format!("template_census.{class}.sole_blocker.{only}"), 1);
        }
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
                    method_call: None,
                    transfer: false,
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
                    method_call: match &expr.kind {
                        ExprKind::MethodCall { object, method, .. } => {
                            Some((self.origins.origin(object.syntax_id), method.clone()))
                        }
                        ExprKind::Index { object, .. } => Some((
                            self.origins.origin(object.syntax_id),
                            "__getitem__".to_string(),
                        )),
                        _ => None,
                    },
                    transfer: matches!(expr.kind, ExprKind::Transfer(_)),
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
        reads: &BodyReads,
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
        self.capturable(&occurrences, baseline, reads)?;
        let owner_end = self.next_owner.get();
        let local_owner = |owner: OwnerId| {
            param_owners
                .runtime
                .iter()
                .position(|param| *param == Some(owner))
                .map(TemplateOwner::Param)
                .or_else(|| {
                    (param_owners.receiver == Some(owner)).then_some(TemplateOwner::Receiver)
                })
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
        // A reference names the receiver, a parameter, or a local, which
        // every instance has (`for_each_owner` renumbers the last).
        let local_place =
            |place: &mojito_types::origin::OriginPlace| match local_owner(place.root)? {
                root @ (TemplateOwner::Receiver
                | TemplateOwner::Param(_)
                | TemplateOwner::Local(_)) => Ok(TemplatePlace {
                    root,
                    path: place.path.clone(),
                }),
                TemplateOwner::Global(_) | TemplateOwner::CompileTimeParam(_) => {
                    Err(IncompleteReason::ExternalBinding)
                }
            };
        let local_reference = |reference: &mojito_types::origin::RefTy| {
            if names_place(&reference.referent) {
                return Err(IncompleteReason::ExternalBinding);
            }
            Ok(TemplateReference {
                referent: (*reference.referent).clone(),
                origin: template_origin(&reference.origin, &local_place)?,
                mutability: reference.mutability,
            })
        };
        let local_invalidations =
            |invalidations: Vec<mojito_checked::checked::InteriorInvalidation>| {
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
        let mut effect_free_callees: Vec<String> = reads
            .effect_queries
            .iter()
            .map(|(callee, _)| callee.clone())
            .collect();
        effect_free_callees.sort();
        effect_free_callees.dedup();
        // A `ref` binding's type is a reference whose origin names a binding,
        // so it is kept by template owner, apart from the closed types.
        let apart = |types: Vec<(OccurrenceId, Ty)>| {
            let (references, plain): (Vec<_>, Vec<_>) = types
                .into_iter()
                .partition(|(_, ty)| rooted_reference(ty).is_some());
            let references = references
                .iter()
                .filter_map(|(id, ty)| Some((*id, rooted_reference(ty)?)))
                .map(|(id, reference)| local_reference(reference).map(|kept| (id, kept)))
                .collect::<Result<Vec<_>, _>>()?;
            Ok::<_, IncompleteReason>((plain, references))
        };
        let (expression_place_types, reference_place_types) =
            apart(values(&occurrences, &self.expression_place_types.borrow()))?;
        let (binding_types, reference_binding_types) =
            apart(values(&occurrences, &self.binding_types.borrow()))?;
        Ok(CheckedBodyFacts {
            expression_types: values(&occurrences, &self.expression_types.borrow()),
            expression_place_types,
            binding_types,
            reference_binding_types,
            reference_place_types,
            expression_bindings: owned(&self.expression_bindings.borrow())?,
            statement_bindings: owned(&self.statement_bindings.borrow())?,
            expression_effects: values(&occurrences, &self.expression_effects.borrow()),
            operation_adjustments: values(&occurrences, &self.operation_adjustments.borrow())
                .into_iter()
                .filter(|(_, adjustment)| {
                    !matches!(
                        adjustment,
                        mojito_checked::checked::SemanticAdjustment::ReferenceResult { .. }
                    )
                })
                .collect(),
            reference_results: values(&occurrences, &self.operation_adjustments.borrow())
                .into_iter()
                .filter_map(|(id, adjustment)| match adjustment {
                    mojito_checked::checked::SemanticAdjustment::ReferenceResult { reference } => {
                        Some(local_reference(&reference).map(|reference| (id, reference)))
                    }
                    _ => None,
                })
                .collect::<Result<Vec<_>, _>>()?,
            interior_references: values(&occurrences, &self.interior_references.borrow())
                .into_iter()
                .map(|(id, place)| local_place(&place).map(|place| (id, place)))
                .collect::<Result<Vec<_>, _>>()?,
            copyable_reference_result_reads: keyed(&|span| {
                self.copyable_reference_result_reads.borrow().contains(span)
            }),
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
            borrowed_reference_receivers: keyed(&|span| {
                self.borrowed_reference_receivers.borrow().contains(span)
            }),
            read_temporary_arguments: keyed(&|span| {
                self.read_temporary_arguments.borrow().contains(span)
            }),
            effect_free_callees,
            selected_calls: values(&occurrences, &self.selected_calls.borrow())
                .into_iter()
                .map(|(id, mut contract)| {
                    let boundary = std::mem::take(&mut contract.boundary);
                    // The reference a call yields is its result type too;
                    // both name the receiver's binding, so the referent
                    // stands in for the result until an instance installs it.
                    let reference_result = contract
                        .reference_result
                        .take()
                        .map(|reference| {
                            if contract.result_ty != Ty::Ref(reference.clone()) {
                                return Err(IncompleteReason::ExternalBinding);
                            }
                            contract.result_ty = (*reference.referent).clone();
                            local_reference(&reference)
                        })
                        .transpose()?;
                    let arguments = boundary
                        .arguments
                        .into_iter()
                        .map(|argument| {
                            // An argument the call synthesized is no
                            // occurrence of the body.
                            let value = occurrences
                                .iter()
                                .find(|occurrence| occurrence.span == argument.value_source)
                                .map(|occurrence| occurrence.id)
                                .ok_or(IncompleteReason::FactOutsideBody(
                                    FactTable::SelectedCalls,
                                ))?;
                            Ok(TemplateArgumentBoundary {
                                source: argument.source,
                                value,
                                adjustments: argument.adjustments,
                                invalidations: local_invalidations(argument.invalidations)?,
                            })
                        })
                        .collect::<Result<Vec<_>, IncompleteReason>>()?;
                    Ok((
                        id,
                        TemplateCallContract {
                            contract,
                            reference_result,
                            arguments,
                            invalidations: local_invalidations(boundary.invalidations)?,
                        },
                    ))
                })
                .collect::<Result<Vec<_>, IncompleteReason>>()?,
            // Recording is idempotent, and a clone check reaches a retargeted
            // receiver's application twice, so only the set matters.
            struct_applications: reads.struct_applications.iter().fold(
                Vec::new(),
                |mut distinct, application| {
                    if !distinct.contains(application) {
                        distinct.push(application.clone());
                    }
                    distinct
                },
            ),
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
                    local_invalidations(invalidations).map(|invalidations| (id, invalidations))
                })
                .collect::<Result<Vec<_>, _>>()?,
            unconsumed_temporaries: keyed(&|span| {
                self.unconsumed_temporaries.borrow().contains(span)
            }),
            discarded_reference_results: keyed(&|span| {
                self.discarded_reference_results.borrow().contains(span)
            }),
            reference_value_uses: values(&occurrences, &self.reference_value_uses.borrow()),
            deletable_bindings: keyed(&|span| {
                self.explicit_destroy_deletability
                    .borrow()
                    .bindings
                    .contains(span)
            }),
            linear_bindings: keyed(&|span| {
                self.explicit_destroy_deletability
                    .borrow()
                    .linear_bindings
                    .contains(span)
            }),
            linear_temporaries: keyed(&|span| self.linear_temporaries.borrow().contains(span)),
            transfers: occurrences
                .iter()
                .filter(|occurrence| occurrence.transfer)
                .map(|occurrence| occurrence.id)
                .collect(),
            locals: owner_end - baseline.owner_start,
            occurrences: occurrences
                .into_iter()
                .map(|occurrence| occurrence.id)
                .collect(),
        })
    }

    /// Whether one body inference recorded only what capture can keep: no
    /// growth in a store not keyed by occurrence, no callee effect summary
    /// that was not empty, no entry keyed outside the body or in a table
    /// without a recipe, and no retained type naming a place.
    fn capturable(
        &self,
        occurrences: &[Occurrence],
        baseline: &BodyFactBaseline,
        reads: &BodyReads,
    ) -> Result<(), IncompleteReason> {
        if let Some((store, _)) = self
            .unkeyed_fact_entries()
            .into_iter()
            .zip(baseline.unkeyed)
            .find_map(|(now, before)| (now != before).then_some(now))
        {
            return Err(IncompleteReason::UnkeyedFact(store));
        }
        if reads.effect_queries.iter().any(|(_, empty)| !empty)
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
        // A type is retained as written, and a binding identity inside one
        // would never be remapped for an instance. The type of a `ref`
        // binding is the exception: a reference at the top of a place or
        // binding type is kept by template owner (`rooted_reference`).
        let expression_types = self.expression_types.borrow();
        let kept_apart = [
            &*self.expression_place_types.borrow(),
            &*self.binding_types.borrow(),
        ];
        if occurrences.iter().any(|occurrence| {
            expression_types
                .get(&occurrence.span)
                .is_some_and(names_place)
                || kept_apart
                    .iter()
                    .filter_map(|table| table.get(&occurrence.span))
                    .any(|ty| rooted_reference(ty).is_none() && names_place(ty))
        }) {
            return Err(IncompleteReason::ExternalBinding);
        }
        Ok(())
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
            TemplateOwner::Receiver => param_owners
                .receiver
                .ok_or_else(|| corrupt("the receiver binding")),
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
        let rooted = |place: &TemplatePlace| {
            Ok::<_, TypeError>(mojito_types::origin::OriginPlace {
                root: owner(&place.root)?,
                path: place.path.clone(),
            })
        };
        let referenced = |reference: &TemplateReference| {
            Ok::<_, TypeError>(mojito_types::origin::RefTy {
                referent: Box::new(reference.referent.clone()),
                origin: checked_origin(&reference.origin, &rooted)?,
                mutability: reference.mutability,
            })
        };
        for (id, reference) in &facts.reference_results {
            self.operation_adjustments.borrow_mut().insert(
                span(id)?,
                mojito_checked::checked::SemanticAdjustment::ReferenceResult {
                    reference: referenced(reference)?,
                },
            );
        }
        for (id, place) in &facts.interior_references {
            self.interior_references
                .borrow_mut()
                .insert(span(id)?, rooted(place)?);
        }
        for (id, reference) in &facts.reference_binding_types {
            self.binding_types
                .borrow_mut()
                .insert(span(id)?, Ty::Ref(referenced(reference)?));
        }
        for (id, reference) in &facts.reference_place_types {
            self.expression_place_types
                .borrow_mut()
                .insert(span(id)?, Ty::Ref(referenced(reference)?));
        }
        for id in &facts.copyable_reference_result_reads {
            self.copyable_reference_result_reads
                .borrow_mut()
                .insert(span(id)?);
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
        for id in &facts.borrowed_reference_receivers {
            self.borrowed_reference_receivers
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
        let placed = |invalidations: &[TemplateInvalidation]| {
            invalidations
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
                .collect::<Result<Vec<_>, TypeError>>()
        };
        for (id, invalidations) in &facts.interior_invalidations {
            self.interior_invalidations
                .borrow_mut()
                .insert(span(id)?, placed(invalidations)?);
        }
        for id in &facts.unconsumed_temporaries {
            self.unconsumed_temporaries.borrow_mut().insert(span(id)?);
        }
        for id in &facts.discarded_reference_results {
            self.discarded_reference_results
                .borrow_mut()
                .insert(span(id)?);
        }
        for (id, writable) in &facts.reference_value_uses {
            self.reference_value_uses
                .borrow_mut()
                .insert(span(id)?, *writable);
        }
        for id in &facts.deletable_bindings {
            self.explicit_destroy_deletability
                .borrow_mut()
                .bindings
                .insert(span(id)?);
        }
        for id in &facts.linear_bindings {
            self.explicit_destroy_deletability
                .borrow_mut()
                .linear_bindings
                .insert(span(id)?);
        }
        for id in &facts.linear_temporaries {
            self.linear_temporaries.borrow_mut().insert(span(id)?);
        }
        for (id, call) in &facts.selected_calls {
            let arguments = call
                .arguments
                .iter()
                .map(|argument| {
                    Ok(mojito_checked::checked::CheckedCallArgumentBoundary {
                        source: argument.source,
                        value_source: span(&argument.value)?,
                        adjustments: argument.adjustments.clone(),
                        invalidations: placed(&argument.invalidations)?,
                    })
                })
                .collect::<Result<Vec<_>, TypeError>>()?;
            let reference_result = call
                .reference_result
                .as_ref()
                .map(&referenced)
                .transpose()?;
            self.selected_calls.borrow_mut().insert(
                span(id)?,
                mojito_checked::checked::CheckedCallContract {
                    result_ty: reference_result
                        .clone()
                        .map_or_else(|| call.contract.result_ty.clone(), Ty::Ref),
                    reference_result,
                    boundary: mojito_checked::checked::CheckedCallBoundary {
                        arguments,
                        invalidations: placed(&call.invalidations)?,
                    },
                    ..call.contract.clone()
                },
            );
        }
        // The body's own source decides, exactly as it does for an inferred
        // body, whether an application it reaches is user-reachable.
        let source = spans.values().next().and_then(|span| span.source.clone());
        for (template, arguments) in &facts.struct_applications {
            self.record_struct_instantiation(template, arguments, source.as_deref());
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
const UNKEYED_STORES: usize = 7;

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
        | FactTable::SelectedCalls
        | FactTable::InteriorInvalidations
        | FactTable::DiscardedReferenceResults
        | FactTable::ReferenceValueUses
        | FactTable::InteriorReferences
        | FactTable::CopyableReferenceResultReads
        | FactTable::BorrowedReferenceReceivers
        | FactTable::DeletableBindings
        | FactTable::LinearBindings
        | FactTable::LinearTemporaries => true,
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
        | FactTable::ViewResultInteriors
        | FactTable::WithDesugars
        | FactTable::DeclarationCaptures
        | FactTable::ComprehensionBindings
        | FactTable::SubscriptDescriptors
        | FactTable::IterationProtocols
        | FactTable::ExplicitDestroyCalls
        | FactTable::CallPlaceUses
        | FactTable::ImplicitlyCopiedConsumingReceivers
        | FactTable::TruthinessConditions => false,
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
    for_each_owner(facts, &renumber);
    facts.locals = u32::try_from(declared.len()).unwrap_or(u32::MAX);
}

/// Visit every binding a bundle names by template owner: the binding tables,
/// each invalidation's root and exception, and the root of every place a
/// reference's origin holds.
fn for_each_owner(facts: &mut CheckedBodyFacts, visit: &dyn Fn(&mut TemplateOwner)) {
    fn origin_owners(origin: &mut TemplateOrigin, visit: &dyn Fn(&mut TemplateOwner)) {
        match origin {
            TemplateOrigin::Place(place) => visit(&mut place.root),
            TemplateOrigin::Union(members) => members
                .iter_mut()
                .for_each(|member| origin_owners(member, visit)),
            TemplateOrigin::Unrooted(_) => {}
        }
    }
    for (_, owner) in facts
        .statement_bindings
        .iter_mut()
        .chain(&mut facts.expression_bindings)
    {
        visit(owner);
    }
    for (_, place) in &mut facts.interior_references {
        visit(&mut place.root);
    }
    for (_, reference) in facts
        .reference_results
        .iter_mut()
        .chain(&mut facts.reference_binding_types)
        .chain(&mut facts.reference_place_types)
    {
        origin_owners(&mut reference.origin, visit);
    }
    for (_, call) in &mut facts.selected_calls {
        if let Some(reference) = &mut call.reference_result {
            origin_owners(&mut reference.origin, visit);
        }
    }
    let call_invalidations = facts.selected_calls.iter_mut().flat_map(|(_, call)| {
        call.arguments
            .iter_mut()
            .flat_map(|argument| &mut argument.invalidations)
            .chain(&mut call.invalidations)
    });
    for invalidation in facts
        .interior_invalidations
        .iter_mut()
        .flat_map(|(_, invalidations)| invalidations)
        .chain(call_invalidations)
    {
        visit(&mut invalidation.root);
        if let Some(except) = &mut invalidation.except {
            visit(except);
        }
    }
}

/// The constructs a method's declaration and body hold, by name and without
/// judging any of them: what the census reports so the next class can be
/// chosen from what bodies are made of rather than from their first refusal.
fn grammar_features(method: &mojito_ast::ast::Method, ret_ty: &Ty) -> Vec<String> {
    #[derive(Default)]
    struct Constructs(Vec<String>);

    /// The variant name a `Debug` rendering leads with. Rendering stops at
    /// the first character past it, so a subtree is never formatted.
    fn variant(node: &dyn std::fmt::Debug) -> String {
        struct Leading(String);

        impl std::fmt::Write for Leading {
            fn write_str(&mut self, rendered: &str) -> std::fmt::Result {
                let name = rendered
                    .find(|c: char| !c.is_alphanumeric())
                    .unwrap_or(rendered.len());
                self.0.push_str(&rendered[..name]);
                if name == rendered.len() {
                    Ok(())
                } else {
                    Err(std::fmt::Error)
                }
            }
        }

        let mut leading = Leading(String::new());
        // The error is how rendering is cut short.
        let _ = std::fmt::write(&mut leading, format_args!("{node:?}"));
        leading.0
    }

    impl mojito_ast::visit::Visitor for Constructs {
        fn visit_stmt(&mut self, statement: &Stmt) {
            self.0.push(format!("stmt:{}", variant(&statement.kind)));
        }

        fn visit_expr(&mut self, expr: &Expr) {
            let arguments = match &expr.kind {
                ExprKind::MethodCall { args, kwargs, .. } | ExprKind::Call { args, kwargs, .. } => {
                    args.len() + kwargs.len()
                }
                _ => 0,
            };
            let suffix = if arguments > 0 { "+args" } else { "" };
            self.0.push(format!("expr:{}{suffix}", variant(&expr.kind)));
        }
    }

    let receiver = match (method.has_self, method.self_convention) {
        (false, _) => "static".to_string(),
        (true, None) => "read".to_string(),
        (true, Some(convention)) => format!("{convention:?}").to_lowercase(),
    };
    let result = if matches!(method.ret, Some(mojito_ast::ast::Type::Ref { .. })) {
        "ref"
    } else if closed_scalar(ret_ty) {
        "scalar"
    } else if *ret_ty == Ty::None {
        "none"
    } else if mojito_types::types::is_symbolic(ret_ty) {
        "symbolic"
    } else {
        "closed"
    };
    let mut features = vec![format!("self:{receiver}"), format!("result:{result}")];
    let declared = [
        (method.self_origin.is_some(), "self:origin"),
        (!method.where_clauses.is_empty(), "where"),
        (method.raises || method.raises_type.is_some(), "raises"),
        (!method.type_params.is_empty(), "binders"),
        (!method.decorators.is_empty(), "decorated"),
    ];
    features.extend(
        declared
            .into_iter()
            .filter(|(held, _)| *held)
            .map(|(_, name)| name.to_string()),
    );
    for parameter in &method.params {
        if let Some(convention) = parameter.convention {
            features.push(format!("param:{convention:?}").to_lowercase());
        }
        if parameter.kind != mojito_ast::ast::ParamKind::Regular {
            features.push("param:variadic".to_string());
        }
        if parameter.default.is_some() {
            features.push("param:default".to_string());
        }
        if parameter.origin.is_some() {
            features.push("param:origin".to_string());
        }
    }
    let mut constructs = Constructs::default();
    mojito_ast::visit::walk_block(&mut constructs, &method.body);
    features.extend(constructs.0);
    features.sort();
    features.dedup();
    features
}

/// Whether a type names a checker-local place: an origin rooted at a binding
/// identity, in a pointer, a reference, or a struct's origin argument.
/// The reference at the top of a `ref` binding's type, when its origin names
/// a binding: what a bundle keeps by template owner instead of as written.
fn rooted_reference(ty: &Ty) -> Option<&mojito_types::origin::RefTy> {
    match ty {
        Ty::Ref(reference) if names_place(ty) && !names_place(&reference.referent) => {
            Some(reference)
        }
        _ => None,
    }
}

fn names_place(ty: &Ty) -> bool {
    use mojito_types::origin::{Origin, PointerOrigin};
    fn rooted(origin: &Origin) -> bool {
        match origin {
            Origin::Place(_) => true,
            Origin::Union(members) => members.iter().any(rooted),
            Origin::Param(_)
            | Origin::SelfParam
            | Origin::Static
            | Origin::Untracked { .. }
            | Origin::Unbound => false,
        }
    }
    mojito_types::types::mentions(ty, &|ty| {
        match ty {
        Ty::Pointer { origin, .. } => matches!(origin, PointerOrigin::Place { .. }),
        Ty::Ref(reference) => rooted(&reference.origin) || names_place(&reference.referent),
        Ty::Struct(_, arguments) => arguments.iter().any(|argument| {
            matches!(argument, mojito_types::types::TyArg::Origin(origin) if rooted(origin))
        }),
        _ => false,
    }
    })
}

/// An origin with each place it is rooted at mapped by `place`.
fn template_origin(
    origin: &mojito_types::origin::Origin,
    place: &dyn Fn(&mojito_types::origin::OriginPlace) -> Result<TemplatePlace, IncompleteReason>,
) -> Result<TemplateOrigin, IncompleteReason> {
    use mojito_types::origin::Origin;
    match origin {
        Origin::Place(rooted) => place(rooted).map(TemplateOrigin::Place),
        Origin::Union(members) => members
            .iter()
            .map(|member| template_origin(member, place))
            .collect::<Result<_, _>>()
            .map(TemplateOrigin::Union),
        Origin::Param(_)
        | Origin::SelfParam
        | Origin::Static
        | Origin::Untracked { .. }
        | Origin::Unbound => Ok(TemplateOrigin::Unrooted(origin.clone())),
    }
}

/// The inverse of [`template_origin`], for an instance's own bindings.
fn checked_origin(
    origin: &TemplateOrigin,
    rooted: &dyn Fn(&TemplatePlace) -> Result<mojito_types::origin::OriginPlace, TypeError>,
) -> Result<mojito_types::origin::Origin, TypeError> {
    use mojito_types::origin::Origin;
    match origin {
        TemplateOrigin::Place(place) => rooted(place).map(Origin::Place),
        TemplateOrigin::Union(members) => members
            .iter()
            .map(|member| checked_origin(member, rooted))
            .collect::<Result<_, _>>()
            .map(Origin::Union),
        TemplateOrigin::Unrooted(origin) => Ok(origin.clone()),
    }
}

/// Whether a type is, holds, or is applied to a callable.
fn mentions_callable(ty: &Ty) -> bool {
    match ty {
        Ty::Func { .. } | Ty::GenericFunc { .. } | Ty::Overload(_) => true,
        Ty::Struct(_, arguments) => arguments.iter().any(|argument| {
            matches!(argument, mojito_types::types::TyArg::Ty(element) if mentions_callable(element))
        }),
        Ty::Tuple(elements) | Ty::RuntimePack(elements) | Ty::Variant(elements) => {
            elements.iter().any(mentions_callable)
        }
        _ => false,
    }
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
            | TemplateOwner::Receiver
            | TemplateOwner::Local(_)
            | TemplateOwner::CompileTimeParam(_) => None,
        })
}

/// The fact a table holds at `id`.
fn fact_at<V>(table: &[(OccurrenceId, V)], id: OccurrenceId) -> Option<&V> {
    table
        .iter()
        .find(|(site, _)| *site == id)
        .map(|(_, fact)| fact)
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
    /// `None` judges the syntax alone, before anything is captured.
    facts: Option<&'a CheckedBodyFacts>,
    params: Vec<&'a str>,
    /// The `mut` and `ref` parameters among them: a place the body borrows,
    /// so never the source of a `^` transfer.
    borrowed_params: Vec<&'a str>,
    /// The `mut` parameters, which the body may store to.
    mut_params: Vec<&'a str>,
    /// Whether source validation produced the facts. A body it checks may
    /// hold compile-time control flow over scalar locals and assignments,
    /// under that check's own rules; any other body may hold runtime
    /// statements instead: scalar locals and assignments, `if`, `while`, and
    /// a bare `return`, each checked once.
    keyed: bool,
    /// Whether the body is a method's, which may read `self`'s fields.
    receiver: bool,
    /// Whether `self` is a `mut` or `var` receiver, whose scalar fields the
    /// body may write.
    self_writable: bool,
    /// The declared result, when the body may move a whole value of any type
    /// into it: a method that returns no reference.
    moved_result: Option<&'a Ty>,
    /// The declared referent, when the method returns a reference: every
    /// `return` then hands out a place of `self` as a handle.
    reference_result: Option<&'a Ty>,
    /// What the body held beyond scalar `return`s.
    features: std::cell::Cell<MethodFeatures>,
    /// The locals declared so far.
    locals: RefCell<Vec<(String, LocalKind)>>,
    /// The occurrences admitted as reference handles.
    handles: RefCell<Vec<OccurrenceId>>,
    /// The calls admitted as reference-returning.
    references: RefCell<Vec<OccurrenceId>>,
    /// The references admitted as a method call's receiver.
    receivers: RefCell<Vec<OccurrenceId>>,
}

/// What a local of a certified body is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LocalKind {
    /// A `var` of a closed scalar type.
    Scalar,
    /// A `var` holding a whole value of any other type.
    Value,
    /// A `ref` binding: a handle on a place, never a value to move.
    Reference,
}

impl BodyShape<'_> {
    fn statement(&self, statement: &Stmt, in_loop: bool) -> bool {
        match &statement.kind {
            StmtKind::Return(value) if self.reference_result.is_some() => value
                .as_ref()
                .is_some_and(|value| self.returned_place(value)),
            StmtKind::Return(Some(value)) => {
                (self.expression(value) && self.scalar(value))
                    || self.moved_result.is_some_and(|result| {
                        (self.whole_value(value) || self.reference_read(value))
                            && self.typed(value, result)
                    })
            }
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
                self.locals
                    .borrow_mut()
                    .push((name.clone(), LocalKind::Scalar));
                scalar
            }
            // A runtime statement is checked once, whatever runs it, so it
            // neither drops nor copies an occurrence. A scalar local is one
            // binding wherever it is declared.
            StmtKind::VarDecl { name, ty, value } if !self.keyed => {
                let scalar = self.expression(value) && self.scalar(value);
                let moved = !scalar
                    && ty.is_none()
                    && self.moved_result.is_some()
                    && (self.whole_value(value) || self.reference_read(value))
                    && self.judged_binding(value);
                let kind = if scalar {
                    LocalKind::Scalar
                } else {
                    LocalKind::Value
                };
                self.locals.borrow_mut().push((name.clone(), kind));
                (scalar || moved) && self.holds(MethodFeatures::STATEMENTS)
            }
            StmtKind::RefDecl { name, value } if !self.keyed => {
                let bound = self.bound_place(statement, value);
                self.locals
                    .borrow_mut()
                    .push((name.clone(), LocalKind::Reference));
                bound && self.holds(MethodFeatures::REFERENCE_LOCALS)
            }
            StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue if !self.keyed => {
                self.holds(MethodFeatures::STATEMENTS)
            }
            StmtKind::If { branches, orelse } if !self.keyed => {
                branches
                    .iter()
                    .all(|(condition, arm)| self.condition(condition) && self.block(arm, in_loop))
                    && orelse.as_ref().is_none_or(|arm| self.block(arm, in_loop))
                    && self.holds(MethodFeatures::STATEMENTS)
            }
            StmtKind::While {
                cond,
                body,
                orelse: None,
            } if !self.keyed => {
                self.condition(cond)
                    && self.block(body, in_loop)
                    && self.holds(MethodFeatures::STATEMENTS)
            }
            // A scalar field of a writable `self`: the store is a plain
            // scalar write, never an in-place operator of the field's type.
            StmtKind::SetPlace { place, value } if !self.keyed => {
                let scalar =
                    self.scalar_field_place(place) && self.expression(value) && self.scalar(value);
                (scalar || self.whole_store(place, value)) && self.holds(MethodFeatures::STATEMENTS)
            }
            // A discarded value. That it is not read is the statement's
            // syntax; what the call itself recorded is the call's to answer.
            StmtKind::Expr(value) => {
                let call = matches!(
                    value.kind,
                    ExprKind::Call { .. } | ExprKind::MethodCall { .. }
                ) && self.expression(value)
                    && self.closed(value);
                call || (self.moved_result.is_some() && self.pointer_statement(value))
                    || (!self.keyed && self.abort(value))
            }
            StmtKind::Assign { name, value } if name == "_" => {
                self.expression(value) && self.scalar(value)
            }
            StmtKind::Assign { name, value } if self.local(name) => {
                self.expression(value)
                    && self.scalar(value)
                    && (self.keyed || self.holds(MethodFeatures::STATEMENTS))
            }
            // A whole store to a `mut` parameter, of the parameter's own type.
            StmtKind::Assign { name, value } => {
                self.mut_params.contains(&name.as_str())
                    && self.parameter_store(value)
                    && self.holds(MethodFeatures::STATEMENTS)
            }
            StmtKind::AugAssign { place, value, .. } => {
                let local = matches!(&place.kind, ExprKind::Identifier(name)
                    if self.local(name) || self.mut_params.contains(&name.as_str()));
                (local || (!self.keyed && self.scalar_field_place(place)))
                    && self.scalar(place)
                    && self.expression(value)
                    && self.scalar(value)
                    && (self.keyed || self.holds(MethodFeatures::STATEMENTS))
            }
            _ => false,
        }
    }

    /// The value of a `return` in a method that returns a reference: a field
    /// of `self`, a pointer slot, or a reference a call on a field yields, of
    /// exactly the declared referent type, so neither check converts it.
    ///
    /// The `return` keeps the place as a handle because the declaration
    /// returns a reference, whatever the place's type, and demands neither a
    /// copy nor a move of it. Whether the place lies within the declared
    /// origin is judged on its path and the signature, which no instance
    /// changes.
    fn returned_place(&self, value: &Expr) -> bool {
        let id = self.occurrence(value);
        let forwarded =
            matches!(&value.kind, ExprKind::Identifier(name) if self.reference_local(name));
        let admitted = (forwarded
            || self.receiver_field(value)
            || self.slot(value)
            || self.reference_call(value))
            && self
                .reference_result
                .is_some_and(|referent| self.typed(value, referent))
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.reference_value_uses, id) == Some(&false)
                    && !facts.copy_place_value_uses.contains(&id)
            });
        if admitted {
            self.handle(id);
        }
        admitted && self.holds(MethodFeatures::REFERENCE_RESULT)
    }

    /// A reference-returning call on a field of `self`, passing scalars:
    /// a subscript or a named accessor whose recorded contract is a
    /// `closed_reference_contract`. It is admitted as a returned place, a
    /// whole value read, a `ref` declaration's value, or the reference a
    /// field is read or a method called through ([`Self::through`]), never as
    /// an operand or an argument, each of which records a borrow of its own.
    /// An iterator's `__next__` marks its copyable read by another rule.
    fn reference_call(&self, expr: &Expr) -> bool {
        let (object, method, arguments) = match &expr.kind {
            ExprKind::Index { object, index } => {
                (object, "__getitem__", std::slice::from_ref(&**index))
            }
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } if kwargs.is_empty() && method != "__next__" => {
                (object, method.as_str(), args.as_slice())
            }
            _ => return false,
        };
        let admitted = !self.keyed
            && self.receiver_field(object)
            && arguments
                .iter()
                .all(|argument| self.expression(argument) && self.scalar(argument))
            && self.facts.is_none_or(|facts| {
                self.named_contract(facts, expr, object, method)
                    .is_some_and(mojito_checked::templates::closed_reference_contract)
            });
        if admitted {
            self.references.borrow_mut().push(self.occurrence(expr));
        }
        admitted && self.holds(MethodFeatures::REFERENCE_CALLS)
    }

    /// A reference call read by value, which the template marked a copyable
    /// read: a referent that is not implicitly copyable records nothing
    /// there, and its clone check would refuse the read.
    fn reference_read(&self, expr: &Expr) -> bool {
        let id = self.occurrence(expr);
        self.reference_call(expr)
            && self
                .facts
                .is_none_or(|facts| facts.copyable_reference_result_reads.contains(&id))
    }

    /// Whether the references the check recorded are exactly the ones the
    /// grammar admitted: each handle kept at a returned place, a `ref`
    /// declaration's value, or the base of a field read through a reference,
    /// each receiver borrowed through a reference, and each reference result,
    /// interior generation, and copyable read at an admitted reference call.
    /// Every other writer of those tables decides on a type or on a
    /// binding's declaration.
    fn references_recorded(&self, facts: &CheckedBodyFacts) -> bool {
        let handles = self.handles.borrow();
        let references = self.references.borrow();
        let receivers = self.receivers.borrow();
        facts.borrowed_reference_receivers.len() == receivers.len()
            && facts
                .borrowed_reference_receivers
                .iter()
                .all(|id| receivers.contains(id))
            && facts.reference_value_uses.len() == handles.len()
            && facts
                .reference_value_uses
                .iter()
                .all(|(id, _)| handles.contains(id))
            && references
                .iter()
                .all(|id| fact_at(&facts.reference_results, *id).is_some())
            && facts
                .reference_results
                .iter()
                .map(|(id, _)| id)
                .chain(facts.interior_references.iter().map(|(id, _)| id))
                .chain(&facts.copyable_reference_result_reads)
                .all(|id| references.contains(id))
    }

    /// The place a `ref` declaration binds: `self`, a field of it, a
    /// parameter, a `var` local, or a reference call on a field.
    ///
    /// The declaration decides the binding's mutability from the value's own
    /// reference and the binding it names, re-stamps an origin path computed
    /// upstream, and runs no check of its own (`StmtKind::RefDecl`). Its type
    /// is that reference, kept by template owner, and a later use resolves
    /// through it and records the referent. So an instance substitutes the
    /// referent and gets its own bindings back in the origin.
    fn bound_place(&self, statement: &Stmt, value: &Expr) -> bool {
        let named = match &value.kind {
            ExprKind::Identifier(name) => {
                (self.receiver && name == "self")
                    || self.params.contains(&name.as_str())
                    || self.local(name)
                    || self.declared(name)
            }
            _ => self.receiver_field(value),
        };
        let declaration = OccurrenceId {
            syntax: self.origins.origin(statement.syntax_id),
            copy: 0,
        };
        let id = self.occurrence(value);
        let admitted = (named || self.reference_call(value))
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.reference_binding_types, declaration).is_some_and(|reference| {
                    fact_at(&facts.expression_types, id) == Some(&reference.referent)
                })
            });
        if admitted {
            self.handle(id);
        }
        admitted
    }

    /// A field read through a reference, which keeps the reference as a
    /// handle: a field has its declared type under the referent's arguments
    /// in a template and a clone alike.
    fn reference_member(&self, expr: &Expr) -> bool {
        let ExprKind::Member { object, .. } = &expr.kind else {
            return false;
        };
        let admitted = self.through(object);
        if admitted {
            self.handle(self.occurrence(object));
        }
        admitted
    }

    /// A reference the body reads a field or calls a method through: a `ref`
    /// local, or a reference call's result.
    fn through(&self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Identifier(name) => {
                self.reference_local(name) && self.holds(MethodFeatures::REFERENCE_LOCALS)
            }
            _ => self.reference_call(expr) && self.holds(MethodFeatures::REFERENCE_RECEIVERS),
        }
    }

    /// The receiver of a method call made through a reference. The call
    /// borrows it, which is decided by what the receiver is and never by its
    /// type. `named_contract` then demands a nominal struct there, so a
    /// method of a bare parameter, which a clone selects again, stays out.
    fn reference_receiver(&self, object: &Expr) -> bool {
        let admitted = self.through(object);
        if admitted {
            let id = self.occurrence(object);
            let mut receivers = self.receivers.borrow_mut();
            if !receivers.contains(&id) {
                receivers.push(id);
            }
        }
        admitted && self.holds(MethodFeatures::REFERENCE_RECEIVERS)
    }

    /// Note that `id` is kept as a reference handle.
    fn handle(&self, id: OccurrenceId) {
        let mut handles = self.handles.borrow_mut();
        if !handles.contains(&id) {
            handles.push(id);
        }
    }

    /// The compiler-private trap `_mojito_abort("message")`, as a statement.
    /// The built-in types its literal and selects nothing, so it records no
    /// parameters and no binding; a declaration of that name would record
    /// both, and is not this.
    fn abort(&self, expr: &Expr) -> bool {
        let id = self.occurrence(expr);
        let admitted = matches!(&expr.kind, ExprKind::Call { name, param_args, args, kwargs }
            if name == "_mojito_abort"
                && param_args.is_empty()
                && kwargs.is_empty()
                && matches!(args.as_slice(), [message] if matches!(message.kind, ExprKind::Str(_))))
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.call_parameters, id).is_none()
                    && fact_at(&facts.expression_bindings, id).is_none()
            });
        admitted && self.holds(MethodFeatures::STATEMENTS)
    }

    /// Note that the body holds `feature`.
    fn holds(&self, feature: MethodFeatures) -> bool {
        self.features.set(self.features.get().union(feature));
        true
    }

    /// A runtime condition: a scalar expression whose recorded type is
    /// exactly `Bool`, which `expect_bool` accepts without a truthiness fact.
    fn condition(&self, expr: &Expr) -> bool {
        let id = self.occurrence(expr);
        self.expression(expr)
            && self.facts.is_none_or(|facts| {
                facts
                    .expression_types
                    .iter()
                    .any(|(site, ty)| *site == id && *ty == Ty::Bool)
            })
    }

    /// A whole value of any type, moved or copied out of a parameter, a
    /// local, or a field of `self`. It is never an operand, a receiver, a
    /// condition, or an argument, so nothing dispatches on its type.
    ///
    /// A `^` transfer owes `Movable` at the instance's type. A bare place is
    /// admitted only where the template recorded the copy, which the instance
    /// then owes: a type that is not copyable records nothing there, and its
    /// clone check would.
    fn whole_value(&self, expr: &Expr) -> bool {
        let source = |place: &Expr| match &place.kind {
            ExprKind::Identifier(name) => {
                self.params.contains(&name.as_str())
                    || self.declared(name)
                    || self.reference_local(name)
            }
            ExprKind::Member { .. } => self.receiver_field(place) || self.reference_member(place),
            _ => false,
        };
        // A place the body only borrows is copied, never moved out of.
        let borrowed = |place: &Expr| match &place.kind {
            ExprKind::Identifier(name) => {
                self.borrowed_params.contains(&name.as_str()) || self.reference_local(name)
            }
            ExprKind::Member { .. } => !self.receiver_field(place),
            _ => false,
        };
        let admitted = match &expr.kind {
            ExprKind::Transfer(inner) => {
                (source(inner) && !borrowed(inner)) || self.call_result(inner)
            }
            _ if self.call_result(expr) => true,
            // The pointee, taken out of its slot: a temporary.
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => {
                method == "unsafe_take_pointee"
                    && args.is_empty()
                    && kwargs.is_empty()
                    && self.pointer(object)
            }
            _ => {
                let id = self.occurrence(expr);
                (source(expr) || self.slot(expr))
                    && self
                        .facts
                        .is_none_or(|facts| facts.copy_place_value_uses.contains(&id))
            }
        };
        admitted && self.holds(MethodFeatures::OPAQUE_MOVES)
    }

    /// The result of a sibling call, of any type: a temporary, whose type is
    /// the contract's substituted result.
    fn call_result(&self, expr: &Expr) -> bool {
        matches!(&expr.kind, ExprKind::MethodCall { method, .. }
            if !matches!(method.as_str(), "unsafe_take_pointee" | "unsafe_offset"))
            && self.expression(expr)
    }

    /// A pointer into storage `self` owns: a field of `self` whose recorded
    /// type is a pointer with no tracked provenance, or an element offset
    /// from one. Such a pointer holds no loan and names no place, and it is a
    /// pointer under every instance, so its methods are the built-in ones.
    fn pointer(&self, expr: &Expr) -> bool {
        let admitted = match &expr.kind {
            ExprKind::Member { .. } => {
                self.receiver_field(expr)
                    && self.facts.is_none_or(|facts| {
                        fact_at(&facts.expression_types, self.occurrence(expr)).is_some_and(|ty| {
                            matches!(ty, Ty::Pointer { origin, .. } if origin.as_origin().is_none())
                        })
                    })
            }
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => {
                method == "unsafe_offset"
                    && kwargs.is_empty()
                    && matches!(args.as_slice(), [offset]
                        if self.expression(offset) && self.scalar(offset))
                    && self.pointer(object)
            }
            _ => false,
        };
        admitted && self.holds(MethodFeatures::POINTER_SLOTS)
    }

    /// One element slot of such a pointer, `pointer[scalar]`.
    fn slot(&self, expr: &Expr) -> bool {
        matches!(&expr.kind, ExprKind::Index { object, index }
            if self.pointer(object) && self.expression(index) && self.scalar(index))
    }

    /// A statement-level pointer operation that yields nothing: destroying
    /// the pointee, or freeing the allocation.
    fn pointer_statement(&self, expr: &Expr) -> bool {
        matches!(&expr.kind, ExprKind::MethodCall { object, method, args, kwargs }
            if matches!(method.as_str(), "unsafe_deinit_pointee" | "unsafe_free" | "free")
                && args.is_empty()
                && kwargs.is_empty()
                && self.pointer(object))
    }

    /// A whole value stored to a field of a writable `self` whose declared
    /// type is the value's own, so neither check converts it.
    fn whole_store(&self, place: &Expr, value: &Expr) -> bool {
        let scalar = || self.expression(value) && self.scalar(value);
        self.moved_result.is_some()
            && ((self.self_writable && self.receiver_field(place)) || self.slot(place))
            && (self.whole_value(value) || scalar())
            && self.facts.is_none_or(|facts| {
                let stored = fact_at(&facts.expression_place_types, self.occurrence(place));
                stored.is_some()
                    && stored == fact_at(&facts.expression_types, self.occurrence(value))
            })
    }

    /// A whole value stored to a `mut` parameter. The check accepts the
    /// store only where the value has the parameter's type or converts to it,
    /// and a conversion is a fact no derivation carries, so the two types are
    /// equal in the template and stay equal under substitution.
    fn parameter_store(&self, value: &Expr) -> bool {
        let scalar = self.expression(value) && self.scalar(value);
        scalar || self.moved_result.is_some_and(|_| self.whole_value(value))
    }

    /// Whether the recorded type of `expr` is exactly `ty`.
    fn typed(&self, expr: &Expr, ty: &Ty) -> bool {
        self.facts
            .is_none_or(|facts| fact_at(&facts.expression_types, self.occurrence(expr)) == Some(ty))
    }

    /// Whether the binding `value` initializes has a type an instance can
    /// judge again: a bare parameter, whose deletability realization decides
    /// at the instance's type, or a closed type, which both checks judge
    /// alike. A type built over a parameter leaves no such entry.
    fn judged_binding(&self, value: &Expr) -> bool {
        self.facts.is_none_or(|facts| {
            fact_at(&facts.binding_types, self.occurrence(value)).is_some_and(|ty| {
                matches!(ty, Ty::Param { .. }) || !mojito_types::types::is_symbolic(ty)
            })
        })
    }

    /// A closed scalar field of a writable `self`, as the target of a store.
    fn scalar_field_place(&self, place: &Expr) -> bool {
        ((self.self_writable && self.receiver_field(place)) || self.reference_member(place))
            && self.scalar(place)
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

    /// Whether the call at `expr` recorded a closed contract naming `method`
    /// on the receiver's own struct, and nothing a derivation lacks. A call
    /// that is not trivial is a sibling call, which only a method body holds.
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
                        && mojito_checked::templates::closed_method_contract(call)
                        && self.holds(MethodFeatures::SIBLING_CALLS))
            })
    }

    /// The contract the call at `expr` recorded, when it names `method` on
    /// the struct `object` has.
    fn named_contract<'f>(
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
        fact_at(&facts.selected_calls, self.occurrence(expr)).filter(|call| {
            call.contract
                .target
                .strip_prefix(owner.as_str())
                .and_then(|rest| rest.strip_prefix('.'))
                .and_then(|rest| rest.strip_prefix(method))
                .is_some_and(|overload| overload.is_empty() || overload.starts_with('$'))
        })
    }

    /// Whether `expr` is `self.<field>` in a method body.
    fn receiver_field(&self, expr: &Expr) -> bool {
        self.receiver
            && matches!(&expr.kind, ExprKind::Member { object, .. }
                if matches!(&object.kind, ExprKind::Identifier(name) if name == "self"))
    }

    /// Whether `name` is a scalar local.
    fn local(&self, name: &str) -> bool {
        self.local_kind(name) == Some(LocalKind::Scalar)
    }

    /// Whether `name` is a `var` local of any type.
    fn declared(&self, name: &str) -> bool {
        matches!(
            self.local_kind(name),
            Some(LocalKind::Scalar | LocalKind::Value)
        )
    }

    /// Whether `name` is a `ref` local.
    fn reference_local(&self, name: &str) -> bool {
        self.local_kind(name) == Some(LocalKind::Reference)
    }

    /// The innermost local of that name.
    fn local_kind(&self, name: &str) -> Option<LocalKind> {
        self.locals
            .borrow()
            .iter()
            .rev()
            .find(|(local, _)| local == name)
            .map(|(_, kind)| *kind)
    }

    fn expression(&self, expr: &Expr) -> bool {
        match &expr.kind {
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Bool(_) => true,
            // A `ref` local is read through its handle, as the scalar every
            // use site of `expression` also demands.
            ExprKind::Identifier(name) => match self.local_kind(name) {
                Some(kind) => kind != LocalKind::Value,
                None => self.params.contains(&name.as_str()),
            },
            // A field of `self`, admitted where its recorded type is a closed
            // scalar: every use site of `expression` also demands `scalar`.
            ExprKind::Member { .. } => {
                (self.receiver_field(expr) || self.reference_member(expr)) && self.scalar(expr)
            }
            // A call of a method on `self` or on one of its fields, passing
            // scalars, whose recorded contract changes per instance only in
            // its target and its substituted result (`closed_method_contract`).
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => {
                let on_self = matches!(&object.kind, ExprKind::Identifier(name) if name == "self");
                ((self.receiver && (on_self || self.receiver_field(object)))
                    || (!self.keyed && self.reference_receiver(object)))
                    && (!self.keyed || (args.is_empty() && kwargs.is_empty()))
                    && args
                        .iter()
                        .chain(kwargs.iter().map(|keyword| &keyword.value))
                        .all(|argument| self.expression(argument) && self.scalar(argument))
                    && self
                        .facts
                        .is_none_or(|facts| self.sibling_call(facts, expr, object, method))
            }
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
                let known = self.facts.is_none_or(|facts| {
                    facts.call_parameters.iter().any(|(call, _)| *call == id)
                        || (facts.builtin_len_calls.contains(&id) && args.len() == 1)
                });
                // The built-in `len` reads its operand in place and realizes
                // its witness per instance, so a method may hand it a field of
                // `self` of any type, not only a scalar one.
                let builtin_len = self
                    .facts
                    .is_none_or(|facts| facts.builtin_len_calls.contains(&id));
                known
                    && param_args.is_empty()
                    && kwargs.is_empty()
                    && args.iter().all(|argument| {
                        let held = matches!(&argument.kind, ExprKind::Identifier(name)
                            if self.reference_local(name));
                        self.expression(argument)
                            || (builtin_len && (held || self.receiver_field(argument)))
                    })
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

    /// Whether the recorded type of `expr` mentions no parameter.
    fn closed(&self, expr: &Expr) -> bool {
        let id = self.occurrence(expr);
        self.facts.is_none_or(|facts| {
            facts
                .expression_types
                .iter()
                .any(|(site, ty)| *site == id && !mojito_types::types::is_symbolic(ty))
        })
    }

    /// Whether the recorded type of `expr` is a closed scalar. With no facts
    /// yet, the syntax alone never rules a type out.
    fn scalar(&self, expr: &Expr) -> bool {
        let id = self.occurrence(expr);
        self.facts.is_none_or(|facts| {
            facts
                .expression_types
                .iter()
                .any(|(site, ty)| *site == id && closed_scalar_or_literal(ty))
        })
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

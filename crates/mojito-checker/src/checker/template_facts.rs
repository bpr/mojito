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

use super::{
    Checker, EffectRead, callable_contract_target, callable_lowered_name, method_binder_owner,
};
use mojito_ast::ast::{Expr, ExprKind, Stmt, StmtKind};
use mojito_checked::templates::{
    BoundBuiltin, CallParameterFact, CheckedBodyFacts, CheckedTemplate, FactTable,
    IncompleteReason, InstanceName, InstanceTrace, MethodFeatures, OccurrenceId,
    TemplateArgumentBoundary, TemplateAugmentedSubscript, TemplateCallContract,
    TemplateCallResultOrigin, TemplateClass, TemplateCoverage, TemplateId, TemplateInvalidation,
    TemplateObligation, TemplateOrigin, TemplateOwner, TemplatePlace, TemplateProducer,
    TemplateReference, TypedOrigins, TypedTable,
};
use mojito_common::error::TypeError;
use mojito_common::timing;
use mojito_common::token::SourceSpan;
use mojito_common::token::SyntaxId;
use mojito_types::origin::OwnerId;
use mojito_types::types::{ParamDecl, Ty, TySubst};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

mod bound_dispatch;
mod constructions;

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
    /// what the read found.
    effect_queries: Vec<(String, EffectRead)>,
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
    /// A direct or method call's positional arguments.
    arguments: Vec<SyntaxId>,
    /// A direct or method call's keyword arguments, each with its value.
    keywords: Vec<(String, SyntaxId)>,
    /// Whether this is a bare identifier.
    identifier: bool,
    /// A method call's receiver occurrence and method name.
    method_call: Option<(SyntaxId, String)>,
    /// An admitted operator's kind and operand occurrences.
    operator: Option<(mojito_ast::ast::InfixOp, SyntaxId, SyntaxId)>,
    /// Whether this is a `^` transfer.
    transfer: bool,
    /// Whether this is an integer literal.
    literal: bool,
    /// A subscript's index, when the elaborator folded it to an integer
    /// literal: the iteration a pack element was copied for.
    folded_index: Option<i64>,
}

/// What an instance's arguments stand for in its template's facts.
struct InstanceSubstitution {
    /// Each baked type binder's checked type.
    types: TySubst,
    /// Each baked type pack's element types.
    packs: HashMap<mojito_types::param_expr::ParamId, Vec<Ty>>,
}

/// The loop index each pack-element occurrence of an instance was copied
/// for: the loop's own binder, bound to the literal the elaborator folded
/// there.
type ElementIndices =
    HashMap<OccurrenceId, (mojito_types::param_expr::ParamId, mojito_types::ct::CtValue)>;

/// What the grammar names that the template's facts do not: the occurrences
/// an instance must dispatch or prove itself.
#[derive(Debug, Default)]
struct GrammarNotes {
    operators: Vec<OccurrenceId>,
    bound_builtins: Vec<(OccurrenceId, BoundBuiltin)>,
    constructions: Vec<OccurrenceId>,
    callable_calls: Vec<OccurrenceId>,
    repr_calls: Vec<OccurrenceId>,
    print_calls: Vec<OccurrenceId>,
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
        self.mark_symbolic_selection(&site);
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
        let method_decls =
            self.classify_params(&method_binder_owner(owner, &m.name), &m.type_params)?;
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
        self.mark_symbolic_selection(&site);
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

    /// Record on the body's transfer frame (pushed by its inner checker
    /// before the site is entered) whether selections made under source
    /// validation stand (`TransferFrame::keeps_symbolic_selection`): its
    /// trace names a validated template, or, for a body no trace covers (a
    /// per-call clone, a seam without the elaborator's traces), a clone's
    /// name marks it.
    fn mark_symbolic_selection(&self, site: &BodySite<'_>) {
        let keeps = {
            let catalog = self.template_catalog.borrow();
            match catalog.trace(&site.instance) {
                Some(trace) => catalog.validated(&trace.template),
                None => site.display.contains('$'),
            }
        };
        if let Some(frame) = self.transfer_frames.borrow_mut().last_mut() {
            frame.keeps_symbolic_selection = keeps;
        }
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
            // An inference re-resolves the return annotation over the body's
            // own places at each `return` (`reconcile_return_origin_tails`),
            // recording the receiver's facts at an `origin_of(self)` there;
            // the instance does so once, under its own bindings.
            if let Some(Some((annotation, generated))) = self.return_annotations.last() {
                let _ = if *generated {
                    self.resolve_generated_return_annotation(annotation)
                } else {
                    self.resolve_return_annotation(annotation)
                };
            }
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
        let shape = template.then(|| self.certificate(site, None).0);
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
        if let Some((realized, _)) = &derived {
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
            let facts = &comparable(realized);
            let inferred = comparable(&inferred);
            if inferred != *facts && overload_rebinding_only(facts, &inferred) {
                // The one expected difference: the clone check ranked an
                // overload set again on concrete arguments, which the
                // template's selection forbids. The derived facts stand.
                self.replace_body_facts(body, realized, param_owners)?;
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
    /// call-through summary, and what the read found.
    pub(super) fn note_effect_query(&self, callee: &str, read: EffectRead) {
        if let Some(Some(frame)) = self.effect_query_frames.borrow_mut().last_mut() {
            frame.push((callee.to_string(), read));
        }
    }

    /// Remove one occurrence's entry from every occurrence-keyed fact table.
    /// `replace_body_facts` checks the list against [`FactTable::ALL`]; the
    /// augmented store drops what it recorded at a synthesized receiver.
    pub(super) fn remove_occurrence_facts(&self, span: &SourceSpan) {
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

    /// The transfer effects one body inference replayed or published, as
    /// `(residue, transfers)`.
    ///
    /// `residue` is an effect no derivation accounts for: a named callable's
    /// effects behind a call-through residue, a function value's baked
    /// effects, or a destination a captured binding names. `transfers` is the
    /// rest: a callee's transfer summary replayed on the call's own receiver
    /// and arguments, the origins it merged, and the effect the body's own
    /// frame then publishes. Those exist only while a value may carry a loan
    /// ([`TemplateObligation::PlainDataTransfers`]). A call-through residue
    /// the body reads or publishes is neither: capture keeps it
    /// ([`TemplateObligation::CallThroughResidue`]).
    fn body_transfer_effects(
        &self,
        occurrences: &[Occurrence],
        baseline: &BodyFactBaseline,
        reads: &BodyReads,
    ) -> (bool, bool) {
        use mojito_types::origin::SigOrigin;
        fn bound(origin: &SigOrigin) -> bool {
            match origin {
                SigOrigin::Bound(_) => true,
                SigOrigin::Projected(base, _) => bound(base),
                SigOrigin::Union(members) => members.iter().any(bound),
                _ => false,
            }
        }
        let frames = self.transfer_frames.borrow();
        let frame = frames.last();
        let residue = reads
            .effect_queries
            .iter()
            .any(|(_, read)| matches!(read, EffectRead::Residue))
            || frame.is_some_and(|frame| {
                frame
                    .effects
                    .iter()
                    .any(|effect| bound(&effect.dest) || bound(&effect.src))
            });
        let merged = self
            .unkeyed_fact_entries()
            .into_iter()
            .zip(baseline.unkeyed)
            .any(|(now, before)| now.0 == TRANSFERRED_ORIGINS && now != before);
        let recorded = {
            let entries = self.span_table(FactTable::CallTransfers);
            occurrences
                .iter()
                .any(|occurrence| entries.has(&occurrence.span))
        };
        let transfers = merged
            || recorded
            || reads
                .effect_queries
                .iter()
                .any(|(_, read)| matches!(read, EffectRead::Transfers))
            || frame.is_some_and(|frame| !frame.effects.is_empty());
        (residue, transfers)
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
                pack_bindings: Vec::new(),
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
        // An origin binder and a trait-bounded type binder are the
        // exceptions: a clone keeps each, bound symbolically as the
        // template's is, and no fact substitutes it.
        let kept_binders = matches!(class, TemplateClass::MethodBody(features)
                if features.contains(MethodFeatures::ORIGIN_PARAMETERS)
                    || features.contains(MethodFeatures::BOUND_BINDERS))
            && matches!(site.declaration, BodyDeclaration::Method(method)
            if method
                .type_params
                .iter()
                .all(|binder| origin_binder(binder) || bound_binder(binder))
                && trace.residual.iter().all(|name| {
                    method.type_params.iter().any(|binder| binder.name == *name)
                }));
        let baked = ((trace.residual.is_empty() && !site.residual_binders) || kept_binders)
            && match class {
                TemplateClass::ClosedScalarBody
                | TemplateClass::FixedCalls
                | TemplateClass::BoundedOperations
                | TemplateClass::MethodScalarBody
                | TemplateClass::MethodBody(_) => trace.value_bindings.is_empty(),
                // The folded values selected the arms; no retained
                // occurrence names one. A folded loop index is read back
                // from the copy it fixed.
                TemplateClass::ScalarBranches | TemplateClass::PackElements => true,
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
            && !substitution.types.values().all(|ty| self.plain_data(ty))
        {
            return refuse("an instance argument carries a loan, a reference, or a callable");
        }
        let occurrences = self.body_occurrences(body);
        // Every occurrence of the body is one the template checked. A class
        // without compile-time control flow keeps them all, once each; a
        // keyed one keeps the arms the elaborator selected, once per loop
        // iteration it unrolled, and drops the rest, facts and all.
        let keyed = matches!(
            class,
            TemplateClass::ScalarBranches | TemplateClass::PackElements
        );
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
        // A loop variable the elaborator folded: the identifier's facts are
        // not the literal's. The literal at a pack element's index says
        // which element the copy is.
        let folded: Vec<OccurrenceId> = occurrences
            .iter()
            .filter(|occurrence| {
                occurrence.literal
                    && checked
                        .facts
                        .expression_bindings
                        .iter()
                        .any(|(id, _)| id.syntax == occurrence.id.syntax)
            })
            .map(|occurrence| occurrence.id)
            .collect();
        let indices = occurrences
            .iter()
            .filter_map(|occurrence| {
                let index = occurrence.folded_index?;
                let (_, binder) =
                    checked
                        .facts
                        .expression_types
                        .iter()
                        .find_map(|(id, ty)| match ty {
                            Ty::Dependent(dependent) if id.syntax == occurrence.id.syntax => {
                                dependent.pack_element()
                            }
                            _ => None,
                        })?;
                match binder.kind() {
                    mojito_types::param_expr::ParamKind::DeclRef(reference) => Some((
                        occurrence.id,
                        (reference.id.clone(), mojito_types::ct::CtValue::Int(index)),
                    )),
                    _ => None,
                }
            })
            .collect();
        let selected = checked.facts.selected(&ids, &folded);
        match self.realize_instance_facts(&selected, &substitution, &indices, &occurrences) {
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

    /// [`TemplateObligation::PlainDataTransfers`] for one retained type: a
    /// closed type whose storage, with its fields at their own arguments,
    /// holds no loan, no reference, and no callable. `plain_data` judges an
    /// instance argument, which may be symbolic.
    fn loan_free(&self, ty: &Ty) -> bool {
        let callable_field = matches!(ty, Ty::Struct(name, _)
        if self.structs.get(name).is_some_and(|info| {
            info.fields.iter().any(|(_, field)| mentions_callable(field))
        }));
        !mojito_types::types::is_symbolic(ty)
            && !self.type_carries_loans(ty)
            && !self.type_contains_reference(ty)
            && !mentions_callable(ty)
            && !callable_field
    }

    /// [`TemplateObligation::CallThroughResidue`] for one substituted type:
    /// closed, and holding no loan and no reference in its storage, its
    /// fields at their own arguments. A callable is not refused as
    /// `loan_free` refuses one: what a callable's storage carries is its
    /// environment's, which no substitution changes, so a `thin` one carries
    /// nothing in either check and a `capturing` one the same open set.
    fn residue_plain(&self, ty: &Ty) -> bool {
        !mojito_types::types::is_symbolic(ty)
            && !self.type_carries_loans(ty)
            && !self.type_contains_reference(ty)
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
    ) -> Result<InstanceSubstitution, TypeError> {
        let Some(arguments) = &site.receiver_arguments else {
            // A binding names the template's own binder; the declaration
            // carries its identity. The source type is resolved as the
            // clone's own signature resolved it: a generated declaration's
            // spelling of an already-checked type (`StringLiteral`) is
            // admitted where a user-spelled one is not.
            let decl_named = |name: &str| {
                template
                    .param_decls
                    .iter()
                    .find(|decl| decl.name().trim_start_matches('*') == name)
            };
            let resolve = |source: &mojito_ast::ast::Type| {
                let generated = self.generated_declaration.replace(true);
                self.bare_string_literal_parameter
                    .set(super::declarations::is_string_literal_annotation(source));
                let ty = self.ty_from_anno(source);
                self.bare_string_literal_parameter.set(false);
                self.generated_declaration.set(generated);
                ty
            };
            let types = trace
                .type_bindings
                .iter()
                .filter_map(|(name, source)| {
                    decl_named(name).map(|decl| Ok((decl.id().clone(), resolve(source)?)))
                })
                .collect::<Result<_, _>>()?;
            let packs = trace
                .pack_bindings
                .iter()
                .filter_map(|(name, sources)| {
                    let elements = sources.iter().map(resolve);
                    decl_named(name)
                        .map(|decl| Ok((decl.id().clone(), elements.collect::<Result<_, _>>()?)))
                })
                .collect::<Result<_, _>>()?;
            return Ok(InstanceSubstitution { types, packs });
        };
        if site.role == BodyRole::Template {
            return Ok(InstanceSubstitution {
                types: HashMap::new(),
                packs: HashMap::new(),
            });
        }
        let unresolved = || {
            TypeError::InvariantViolation(
                "a method clone's receiver does not bind its struct's parameters".to_string(),
            )
        };
        // The declarations are the struct's binders followed by the method's
        // own, which a clone keeps symbolic (`BOUND_BINDERS`) and the
        // receiver does not bind.
        let (struct_decls, own) = template
            .param_decls
            .split_at_checked(arguments.len())
            .ok_or_else(unresolved)?;
        if !own
            .iter()
            .all(|decl| matches!(decl, ParamDecl::Type { bounds, .. } if !bounds.is_empty()))
        {
            return Err(unresolved());
        }
        struct_decls
            .iter()
            .zip(arguments)
            .map(|(decl, argument)| match (decl, argument) {
                (ParamDecl::Type { id, .. }, mojito_types::types::TyArg::Ty(ty)) => {
                    Ok((id.clone(), ty.clone()))
                }
                _ => Err(unresolved()),
            })
            .collect::<Result<_, _>>()
            .map(|types| InstanceSubstitution {
                types,
                packs: HashMap::new(),
            })
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
    ///
    /// `indices` fixes, per occurrence, the loop index a pack element was
    /// copied for: a type keyed by that occurrence substitutes under it, so
    /// the dependent `Ts[i]` folds to the copy's own element.
    fn realize_instance_facts(
        &self,
        template: &CheckedBodyFacts,
        instance: &InstanceSubstitution,
        indices: &ElementIndices,
        occurrences: &[Occurrence],
    ) -> Result<CheckedBodyFacts, &'static str> {
        let InstanceSubstitution {
            types: substitution,
            packs,
        } = instance;
        let substitute =
            |ty: &Ty| mojito_types::types::substitute_packs(ty, substitution, packs, &[]);
        let mut facts = substituted_facts(template, instance, indices)?;
        // A per-call request the template recorded names the caller's own
        // binders; an instance that closed it would retarget the call in the
        // clone check, which no recipe repeats.
        for (_, instantiation) in &mut facts.method_instantiations {
            let arguments = mojito_types::types::map_tyargs(&instantiation.arguments, &substitute);
            if arguments != instantiation.arguments {
                return Err("an instance closes a per-call clone request");
            }
            instantiation.owner_arguments =
                mojito_types::types::map_tyargs(&instantiation.owner_arguments, &substitute);
        }
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
        // A transfer moves the loans its source carries, so a body of
        // plain-data values replays none. Every retained type is judged: the
        // template's own parameter may carry one, and so may a closed view.
        if template.vanishing_transfers
            && !facts
                .expression_types
                .iter()
                .chain(&facts.binding_types)
                .all(|(_, ty)| self.loan_free(ty))
        {
            return Err("a value may carry a loan where the body replays a transfer");
        }
        facts.vanishing_transfers = false;
        // A call-through residue is republished verbatim on the same ground:
        // an argument carries an origin only while its binding's type carries
        // a loan. A type the substitution left alone carries in the instance
        // what it carried in the template, so only the substituted types are
        // judged, and a callable among them is judged by its environment
        // alone, which never substitutes (`residue_plain`).
        let residue = !template.call_throughs.is_empty() || !template.call_through_reads.is_empty();
        let substituted_plain = facts
            .expression_types
            .iter()
            .zip(&template.expression_types)
            .chain(facts.binding_types.iter().zip(&template.binding_types))
            .filter(|(_, (_, declared))| mojito_types::types::is_symbolic(declared))
            .all(|((_, ty), _)| self.residue_plain(ty));
        if residue && !substituted_plain {
            return Err("a substituted value may carry a loan where the body keeps a residue");
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
        // The declaration's own judgment of each binding whose type mentions
        // a parameter, at the instance's type: deletable where the type is
        // `Deinitable`, linear where it is still a bare parameter. A type
        // built over a parameter answers from its own conformance under the
        // instance's arguments.
        for (id, declared) in &template.binding_types {
            if !mojito_types::types::is_symbolic(declared) {
                continue;
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
            // callee is realized from its contract below, and a call through
            // a callable parameter from the instance's own binding of it.
            if template.selected_calls.iter().any(|(call, _)| call == id)
                || template.callable_calls.contains(id)
            {
                continue;
            }
            self.realize_direct_call(&mut facts, template, *id, occurrences)?;
        }
        for call in &template.callable_calls {
            self.realize_callable_call(&mut facts, *call, occurrences)?;
        }
        facts.callable_calls.clear();
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
        // A call through a bound is realized first: a built-in instance
        // drops it from the calls, and a struct instance gives it a nominal
        // target the closed-call recipe then leaves as it stands.
        let bound_dispatches: Vec<OccurrenceId> = facts
            .selected_calls
            .iter()
            .filter(|(_, call)| {
                mojito_symbol::symbol::is_trait_dispatch_symbol(&call.contract.target)
            })
            .map(|(id, _)| *id)
            .collect();
        for call in &bound_dispatches {
            self.realize_bound_dispatch(&mut facts, *call, occurrences)?;
        }
        // An element store's value getter and in-place dunder stand as the
        // template selected them (`substituted_element_stores`).
        let embedded: Vec<String> = facts
            .augmented_subscripts
            .iter_mut()
            .flat_map(|(_, store)| store.contracts_mut())
            .map(|call| call.contract.target.clone())
            .collect();
        for target in &embedded {
            note_realized_callee(&mut facts, target, target);
        }
        let inverted_writes = self.realize_inverted_writes(&mut facts, occurrences)?;
        for index in 0..facts.selected_calls.len() {
            let id = facts.selected_calls[index].0;
            if bound_dispatches.contains(&id) || inverted_writes.contains(&id) {
                continue;
            }
            self.realize_method_call(&mut facts, index, occurrences, substitution)?;
        }
        // A construction selected its constructor from the arguments' types
        // and named the instance's clone of it, where one exists.
        for construction in &template.constructions {
            self.realize_construction(&mut facts, *construction, occurrences, substitution)?;
        }
        for operator in &template.operators {
            self.realize_operator(&mut facts, *operator, occurrences)?;
        }
        facts.operators.clear();
        for (call, builtin) in &template.bound_builtins {
            self.realize_bound_builtin(&facts, *call, *builtin, occurrences)?;
        }
        facts.bound_builtins.clear();
        for call in &template.repr_calls {
            self.realize_repr_call(&facts, *call, occurrences)?;
        }
        facts.repr_calls.clear();
        for call in &template.print_calls {
            self.realize_print_call(&facts, *call, occurrences)?;
        }
        facts.print_calls.clear();
        // A conversion is selected last: its source type is one the call and
        // construction recipes may have realized.
        for index in 0..facts.conversions.len() {
            self.realize_conversion(&mut facts, index, substitution)?;
        }
        realize_boundary_conversions(&mut facts)?;
        facts.struct_applications =
            sorted_applications(std::mem::take(&mut facts.struct_applications));
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
        // Every residue read was rekeyed to a realized callee, which must
        // publish the residue the template read.
        let realized_targets: Vec<&str> = facts
            .selected_calls
            .iter()
            .map(|(_, call)| call.contract.target.as_str())
            .chain(
                facts
                    .overload_targets
                    .iter()
                    .map(|(_, target)| target.as_str()),
            )
            .collect();
        for (callee, residue) in &facts.call_through_reads {
            if !realized_targets.contains(&callee.as_str()) {
                return Err("a call-through residue was read from a callee no admitted call names");
            }
            let published = self.call_through_effects.borrow();
            if published.get(callee).map(Vec::as_slice) != Some(residue.as_slice()) {
                return Err("a callee's call-through residue is not the template's");
            }
        }
        let position = |id: &OccurrenceId| {
            occurrences
                .iter()
                .position(|occurrence| occurrence.id == *id)
        };
        facts.overload_targets.sort_by_key(|(id, _)| position(id));
        // An operator's copy, conversion, and adjustment are appended where
        // its recipe ran; the inferred bundle holds each in occurrence order.
        facts.copy_place_value_uses.sort_by_key(position);
        facts.conversions.sort_by_key(|(id, _)| position(id));
        facts
            .operation_adjustments
            .sort_by_key(|(id, _)| position(id));
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
        substitution: &TySubst,
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
        let selected = &facts.selected_calls[index].1.contract.target;
        // A subscript that is the target of a store selected the setter.
        let method = if method == "__getitem__" && names_method(selected, &owner, "__setitem__") {
            "__setitem__".to_string()
        } else {
            method
        };
        let family = info
            .methods
            .get(&method)
            .ok_or("a called method is missing")?;
        let self_ty = self.self_instance_ty(&owner);
        let clone_name =
            mojito_symbol::symbol::instance_method_clone_name(&method, &info.decls, &arguments);
        // A receiver whose type was already closed in the template (`List[Pair]`)
        // selected its clone there, on the arguments a clone check ranks too.
        let selected_clone = clone_name
            .as_deref()
            .is_some_and(|clone| names_method(selected, &owner, clone));
        if selected_clone {
            let target = selected.clone();
            note_realized_callee(facts, &target, &target);
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
        // An argument whose own type is its parameter's matches it exactly
        // under every instance, which no other member can outrank; two
        // members an instance makes identical collapse, and
        // `method_clone_target` finds no single clone for them.
        let call = &facts.selected_calls[index].1;
        let exact = call.contract.arguments.iter().all(|parameter| {
            call.arguments
                .iter()
                .find(|bound| bound.source == parameter.source)
                .and_then(|bound| fact_at(&facts.expression_types, bound.value))
                .is_some_and(|ty| {
                    *ty == mojito_types::types::substitute(&parameter.parameter_ty, substitution)
                })
        });
        if !closed_family && !exact {
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
        // The callee has no binders of its own, so its parameter types were
        // recorded at the receiver's arguments: in the caller's binder scope,
        // whether the receiver is `self` or a field of another struct.
        let selected = selected.clone();
        let contract = &mut facts.selected_calls[index].1.contract;
        contract.target.clone_from(&target);
        contract.result_ty = mojito_types::types::substitute(&contract.result_ty, substitution);
        for argument in &mut contract.arguments {
            argument.parameter_ty =
                mojito_types::types::substitute(&argument.parameter_ty, substitution);
        }
        if let Some((_, parameters)) = facts
            .call_parameters
            .iter_mut()
            .find(|(site, _)| *site == id)
        {
            for parameter in parameters {
                parameter.ty = mojito_types::types::substitute(&parameter.ty, substitution);
            }
        }
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
        note_realized_callee(facts, &selected, &target);
        Ok(())
    }

    /// Realize one call through a callable parameter for an instance.
    ///
    /// The call recorded the parameter's own contract symbol and parameters,
    /// in the caller's binder scope: the instance takes both from its own
    /// binding of the parameter, which the elaborator already substituted.
    fn realize_callable_call(
        &self,
        facts: &mut CheckedBodyFacts,
        id: OccurrenceId,
        occurrences: &[Occurrence],
    ) -> Result<(), &'static str> {
        let name = occurrences
            .iter()
            .find(|occurrence| occurrence.id == id)
            .and_then(|occurrence| occurrence.callee.as_deref())
            .ok_or("a callable call occurrence is not a direct call in the instance")?;
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
    fn realize_direct_call(
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

    /// Realize one operator over two places of one type for an instance, as
    /// `infer_infix` decides it on the substituted operand type.
    ///
    /// A closed scalar operates natively and records nothing, as the template
    /// did; it owes only that the primitive path has the operator and gives
    /// the type the template kept (`scalar_operator_result`), which a bound
    /// alone does not promise. A nominal struct dispatches the operator's
    /// dunder, whose selection `struct_infix_dispatch` makes from the types
    /// alone: the instance records the target it names, reaches the struct's
    /// application, and writes the three facts that dispatch carries and the
    /// symbolic template could not — the implicit copy of a consumed operand,
    /// the conversion of an adapted one, and the `NegatedEquality` adjustment
    /// of a `!=` served by `__eq__`. The dunder's result must still be the
    /// type the template kept, which is what an arithmetic operator's bound
    /// promised. Anything else (a tuple, a vector, a pointer) is the clone
    /// check's to judge.
    ///
    /// The reflected dunder has no arm because no admitted operator reaches
    /// it: a comparison has no reflected form, and the left operand of an
    /// arithmetic one has the forward dunder its bound required.
    fn realize_operator(
        &self,
        facts: &mut CheckedBodyFacts,
        id: OccurrenceId,
        occurrences: &[Occurrence],
    ) -> Result<(), &'static str> {
        let (op, left, right) = occurrences
            .iter()
            .find(|occurrence| occurrence.id == id)
            .and_then(|occurrence| occurrence.operator)
            .ok_or("an operator is not one in the instance")?;
        let operand = |syntax| OccurrenceId {
            syntax,
            copy: id.copy,
        };
        let (left, right) = (operand(left), operand(right));
        let operand_ty = fact_at(&facts.expression_types, left)
            .ok_or("an operand has no retained type")?
            .clone();
        if fact_at(&facts.expression_types, right) != Some(&operand_ty) {
            return Err("an operator's operands differ for the instance");
        }
        let result = fact_at(&facts.expression_types, id)
            .ok_or("an operator has no retained result type")?
            .clone();
        if closed_scalar(&operand_ty) {
            return if super::operators::scalar_operator_result(op, &operand_ty) == Some(result) {
                Ok(())
            } else {
                Err("the operator is not the instance's scalar operation")
            };
        }
        let Ty::Struct(name, arguments) = &operand_ty else {
            return Err("an operand is neither a scalar nor a struct");
        };
        if !self.structs.contains_key(name) {
            return Err("an operand is a built-in aggregate");
        }
        let dispatch = self
            .struct_infix_dispatch(op, &operand_ty, &operand_ty)
            .map_err(|_| "the operator is undefined for the instance's type")?
            .ok_or("the instance's type has no dunder for the operator")?;
        let dunder = if dispatch.negated_equality {
            "__eq__"
        } else {
            op.dunder().ok_or("the operator dispatches no dunder")?
        };
        if self.struct_dunder(&operand_ty, dunder, &[&dispatch.operand_ty]) != Some(Ok(result)) {
            return Err("the dunder's result is not the type the template kept");
        }
        // `check_consuming_as` on a place operand: the copy, at the operand's
        // own type rather than the converted one, and the demand the
        // bundle-wide check makes of every copy the template kept.
        if dispatch.consumes {
            if !(self.is_copyable(&operand_ty) && self.is_implicitly_copyable(&operand_ty)) {
                return Err("a consumed operand is not implicitly copyable for the instance");
            }
            facts.copy_place_value_uses.push(right);
        }
        // The conversion `record_implicit_conversion` installs. Its
        // constructor is selected by [`Self::realize_conversion`], which runs
        // after every operator and inherits its refusals.
        if dispatch.converted {
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
        let application = (name.clone(), arguments.clone());
        if source.is_some()
            && !super::overload_support::is_bundled_module_source(source)
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
    fn realize_repr_call(
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
    /// argument only what its syntax decides.
    fn realize_print_call(
        &self,
        facts: &CheckedBodyFacts,
        id: OccurrenceId,
        occurrences: &[Occurrence],
    ) -> Result<(), &'static str> {
        let arguments = occurrences
            .iter()
            .find(|occurrence| occurrence.id == id)
            .map(|occurrence| occurrence.arguments.as_slice())
            .ok_or("a print call has no occurrence in the instance")?;
        arguments.iter().try_for_each(|syntax| {
            let argument = OccurrenceId {
                syntax: *syntax,
                copy: id.copy,
            };
            let ty = fact_at(&facts.expression_types, argument)
                .ok_or("a print call's argument has no retained type")?;
            if self.printable_argument(ty) {
                Ok(())
            } else {
                Err("a print call's argument is not Writable for the instance")
            }
        })
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
    /// implicit copy the template did not. A conversion that kept no target
    /// type is the nominal-string wrap, whose literal constructor is the same
    /// under every instance.
    ///
    /// A conversion at a selected call's argument is also carried by the
    /// contract's own boundary, which capture and installation copy verbatim.
    /// [`realize_boundary_conversions`] writes the target selected here back
    /// into that second copy, once every conversion has been re-selected.
    fn realize_conversion(
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
        // a consuming constructor copies its source, a raising one records a
        // call effect, and a view one materializes a borrow owner and records
        // an adjustment with no recipe.
        if selected.consumes_source || selected.error.is_some() || selected.source_borrow.is_some()
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
                    Ok(mut facts) => {
                        let (coverage, notes) = self.certificate(site, Some(&facts));
                        // The template recorded nothing at an operator the
                        // grammar admitted, nothing that names a bound
                        // builtin's call, no contract at a construction, and
                        // its own parameter's contract at a call through it,
                        // and nothing that names `repr`'s callee, so only the
                        // grammar names them.
                        facts.operators = notes.operators;
                        facts.bound_builtins = notes.bound_builtins;
                        facts.constructions = notes.constructions;
                        facts.callable_calls = notes.callable_calls;
                        facts.repr_calls = notes.repr_calls;
                        facts.print_calls = notes.print_calls;
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

    /// The certificate of a body's class, with the operators the grammar
    /// admitted over parameter-typed operands. With no facts it judges the
    /// declaration's syntax alone: `Certified` then means only that the body
    /// is worth capturing.
    fn certificate(
        &self,
        site: &BodySite<'_>,
        facts: Option<&CheckedBodyFacts>,
    ) -> (TemplateCoverage, GrammarNotes) {
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
            .chain(method_body.then_some(TemplateObligation::PlainDataTransfers))
            .chain(method_body.then_some(TemplateObligation::ConstructorSelection))
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
    /// - Argument typing: the template recorded no copy, move, or adjustment
    ///   at the call (any such table refuses the capture), so each argument
    ///   either matched its parameter exactly with `T` symbolic, and matches
    ///   exactly after substitution, or converts through an `@implicit`
    ///   constructor, which the instance selects again from its own source
    ///   and target types (`realize_conversion`) and which never re-ranks the
    ///   callee. A keyed body converts in an arm the same way: the instance
    ///   re-selects only in the arms the elaborator kept.
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
    ) -> (TemplateCoverage, GrammarNotes) {
        let outside = |what| {
            (
                TemplateCoverage::Incomplete(IncompleteReason::OutsideEnabledClass(what)),
                GrammarNotes::default(),
            )
        };
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
        // A type pack is fixed per instance as a type binder is: the
        // elaborator writes its elements into the clone's signature, and an
        // element the body reads by loop index is fixed by the unrolling
        // (`TemplateClass::PackElements`).
        let pack_binders: Vec<&str> = decls
            .iter()
            .filter_map(|decl| match decl {
                ParamDecl::Type {
                    name,
                    variadic: true,
                    ..
                } => Some(name.trim_start_matches('*')),
                ParamDecl::Type { .. } | ParamDecl::Value { .. } => None,
            })
            .collect();
        let plain_binders = type_params.iter().all(|parameter| {
            parameter.callable_bound.is_none()
                && parameter.default.is_none()
                && parameter.origin_mutability.is_none()
        }) && decls.iter().all(|decl| match decl {
            // A binder's constraints are the declaration's `where` clauses:
            // the requesting call and the elaborator discharge them before an
            // instance exists (`TemplateObligation::DeclarationConstraints`).
            ParamDecl::Type {
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
        // A reflection query over a parameter is validated as a node; the
        // instance's field facts are the elaborator's, so no fact of this
        // body derives.
        if super::comptime_validation::reads_reflection(body) {
            return outside("a body reading a reflection handle keeps its clone check");
        }
        // One producer per body: source validation owns every body it
        // checks (keyed by compile-time control flow or a `rebind`), the
        // executable check the surviving ones.
        let keyed = self.source_validation;
        if !keyed
            && (!pack_binders.is_empty()
                || decls
                    .iter()
                    .any(|decl| matches!(decl, ParamDecl::Value { .. }))
                || body.iter().any(holds_comptime_if))
        {
            return outside("a compile-time-keyed body is source validation's to certify");
        }
        // A variadic parameter is admitted only as the collector of one of
        // the declaration's own packs, which an instance binds as the tuple
        // of the elements written in its signature.
        let pack_collector = |parameter: &mojito_ast::ast::FnParam| {
            parameter.kind == mojito_ast::ast::ParamKind::Variadic
                && matches!(&parameter.ty, mojito_ast::ast::Type::Named(name, arguments)
                    if arguments.is_empty()
                        && pack_binders.contains(&name.trim_start_matches('*')))
        };
        let plain_params = params.iter().all(|parameter| {
            (parameter.kind == mojito_ast::ast::ParamKind::Regular || pack_collector(parameter))
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
        // A body returning nothing falls off its end: the grammar admits no
        // value `return` for it, and a bare `return` only in a runtime body.
        if !closed_scalar(ret_ty) && *ret_ty != Ty::None {
            return outside("the return type is not a concrete scalar");
        }
        let packs: Vec<&str> = params
            .iter()
            .filter(|parameter| pack_collector(parameter))
            .map(|parameter| parameter.name.as_str())
            .collect();
        let shape = BodyShape {
            origins: &self.syntax_origins,
            facts,
            structs: &self.structs,
            params: params
                .iter()
                .map(|parameter| parameter.name.as_str())
                .collect(),
            packs,
            loop_vars: RefCell::new(Vec::new()),
            print_calls: RefCell::new(Vec::new()),
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
            subscripts: RefCell::new(Vec::new()),
            places: RefCell::new(Vec::new()),
            operators: RefCell::new(Vec::new()),
            bound_builtins: RefCell::new(Vec::new()),
            constructions: RefCell::new(Vec::new()),
            callable_params: Vec::new(),
            callable_calls: RefCell::new(Vec::new()),
            repr_calls: RefCell::new(Vec::new()),
        };
        if !shape.block(body, false)
            || !shape.operators.borrow().is_empty()
            || !shape.bound_builtins.borrow().is_empty()
            || !shape.constructions.borrow().is_empty()
            || !shape.repr_calls.borrow().is_empty()
        {
            return outside("the body is not scalar returns over direct calls and 'len'");
        }
        let class = |class| {
            (
                TemplateCoverage::Certified(class),
                GrammarNotes {
                    print_calls: shape.print_calls.borrow().clone(),
                    ..GrammarNotes::default()
                },
            )
        };
        let Some(facts) = facts else {
            return class(TemplateClass::ClosedScalarBody);
        };
        if !shape.references_recorded(facts) {
            return outside("an expression yields or keeps a reference");
        }
        if !facts.call_throughs.is_empty() || !facts.call_through_reads.is_empty() {
            return outside("the body calls or forwards a callable parameter");
        }
        let effects_closed = facts
            .expression_effects
            .iter()
            .all(|(_, effects)| *effects == mojito_checked::checked::EffectFacts::default());
        if !effects_closed {
            return outside("a call has an effect");
        }
        let adjustments_derive = facts
            .operation_adjustments
            .iter()
            .all(|(_, adjustment)| adjustment_derives(adjustment));
        if !adjustments_derive {
            return (
                TemplateCoverage::Incomplete(IncompleteReason::UnsupportedTable(
                    FactTable::OperationAdjustments,
                )),
                GrammarNotes::default(),
            );
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
        if facts.vanishing_transfers {
            return outside("the body replays a transfer summary");
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
        class(if !shape.packs.is_empty() {
            TemplateClass::PackElements
        } else if keyed {
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
    /// initializers, a receiver or parameter origin, and binders stay
    /// outside.
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
    ///   `closed_method_contract`. Arguments are closed scalars or kept
    ///   places in both checks; a call-through residue refuses.
    /// - `VALUE_ARGUMENTS`: see [`BodyShape::argument`] and
    ///   `value_method_contract`. A whole value bound by value has exactly
    ///   its parameter's type, so nothing converts it under any instance,
    ///   and the borrow, temporary, transfer, and copy facts a call records
    ///   for it are decided by its syntax and the callee's conventions; the
    ///   copy and the transfer are owed again per instance. An overloaded
    ///   family with symbolic parameter types is admitted only for exact
    ///   arguments, which no member outranks.
    /// - `VANISHING_TRANSFERS`: see [`Self::body_transfer_effects`]. A
    ///   replayed transfer moves the loans its source carries; a plain-data
    ///   value carries none, so an instance whose every retained type is
    ///   loan-free records no transfer, merges no origin, and publishes no
    ///   effect ([`TemplateObligation::PlainDataTransfers`]).
    /// - `OPERATOR_DISPATCH`: see [`BodyShape::operator`] and
    ///   [`Self::realize_operator`]. The template records nothing at an
    ///   operator its bound proves, and both operands are places read where
    ///   they lie; the instance repeats `infer_infix`'s type-driven selection
    ///   and records the target it names, together with the copy, conversion,
    ///   and negated-equality adjustment that selection carries. An
    ///   arithmetic operator's result is the operand's type, so it is a
    ///   temporary of that type wherever the body puts it
    ///   ([`BodyShape::operator_value`]).
    /// - `BOUND_DISPATCH`: see [`BodyShape::bound_dispatch`] and
    ///   [`Self::realize_bound_dispatch`]. A method call through a bound
    ///   records the abstract contract, or the inverted write, and nothing
    ///   the receiver's type decides; the instance repeats
    ///   `infer_method_call`'s type-driven choice between a place read, a
    ///   hashed leaf, and the struct's own method (`bound_witness`).
    /// - `BOUND_BUILTINS`: see [`BodyShape::bound_builtin`] and
    ///   [`Self::realize_bound_builtin`]. `hasher.update(x)` and
    ///   `writer.write(x)` select no callee and record at an argument only
    ///   what its syntax decides; the instance proves the argument's bound
    ///   again at its own type.
    /// - `BOUND_BINDERS`: a trait-bounded binder of the method's own
    ///   (`[H: Hasher]`) is kept by every clone, bound symbolically as the
    ///   template binds it, and substituted by nothing.
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
    /// - `REFERENCE_ARGUMENTS`: see [`BodyShape::reference_argument`]. An
    ///   argument reached through a reference is read, copied, kept, or lent
    ///   as a named place is, by its syntax and the callee's conventions; a
    ///   constructor's `BorrowRefArguments` names the lending positions and
    ///   each loan's mutability, neither of which an instance changes
    ///   (`derive_adjustment`).
    /// - `RAISES`: see [`BodyShape::raised`]. A `raise` records nothing of
    ///   its own, and the judgment an instance repeats there holds under
    ///   every substitution the template's holds under.
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
    ) -> (TemplateCoverage, GrammarNotes) {
        use mojito_ast::ast::ArgConvention;
        let outside = |what| {
            (
                TemplateCoverage::Incomplete(IncompleteReason::OutsideEnabledClass(what)),
                GrammarNotes::default(),
            )
        };
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
        // An origin binder and a trait-bounded type binder (`[H: Hasher]`)
        // are kept by every clone and bound symbolically as the template
        // binds them, so no fact reads either.
        if !method
            .type_params
            .iter()
            .all(|binder| origin_binder(binder) || bound_binder(binder))
            || !(method.decorators.is_empty() || is_static)
        {
            return outside(
                "the method has binders other than origins and bounded types, or decorators",
            );
        }
        let raises = method.raises || method.raises_type.is_some();
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
        // A `mut` or `ref` parameter is bound from its declared convention
        // alone, and is rooted at its own binding under every instance. What
        // its caller owes lives in the signature, which is checked per clone:
        // that holds an origin clause too, which names a binder, `self`, or
        // another parameter and never a struct parameter's type. A struct
        // that declares an origin parameter is outside `plain_struct`, and no
        // clone is minted for one.
        let plain_params = method.params.iter().all(|parameter| {
            parameter.kind == mojito_ast::ast::ParamKind::Regular
                && matches!(
                    parameter.convention,
                    None | Some(ArgConvention::Var | ArgConvention::Mut | ArgConvention::Ref)
                )
                && parameter.default.is_none()
                && (parameter.origin.is_none() || parameter.convention == Some(ArgConvention::Ref))
        });
        if !plain_params {
            return outside(
                "a parameter has a default, an origin on a convention other than 'ref', or an \
                 'out' or 'deinit' convention",
            );
        }
        let origin_parameter = method.type_params.iter().any(origin_binder)
            || method
                .params
                .iter()
                .any(|parameter| parameter.origin.is_some());
        let bound_binders = method.type_params.iter().any(bound_binder);
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
            structs: &self.structs,
            params: method
                .params
                .iter()
                .map(|parameter| parameter.name.as_str())
                .collect(),
            callable_params: method
                .params
                .iter()
                .filter(|parameter| matches!(parameter.ty, mojito_ast::ast::Type::Func { .. }))
                .map(|parameter| parameter.name.as_str())
                .collect(),
            callable_calls: RefCell::new(Vec::new()),
            packs: Vec::new(),
            loop_vars: RefCell::new(Vec::new()),
            print_calls: RefCell::new(Vec::new()),
            borrowed_params: params_passed(&[ArgConvention::Mut, ArgConvention::Ref]),
            mut_params: params_passed(&[ArgConvention::Mut]),
            keyed: false,
            receiver: method.has_self,
            self_writable: matches!(
                method.self_convention,
                Some(
                    ArgConvention::Mut
                        | ArgConvention::Var
                        | ArgConvention::Out
                        | ArgConvention::Deinit
                )
            ),
            moved_result: (!returns_reference).then_some(ret_ty),
            reference_result: returns_reference.then_some(ret_ty),
            features: std::cell::Cell::new({
                let mut features = if plain_read && closed_scalar(ret_ty) && !owned_parameter {
                    MethodFeatures::default()
                } else {
                    MethodFeatures::STATEMENTS
                };
                if origin_parameter {
                    features = features.union(MethodFeatures::ORIGIN_PARAMETERS);
                }
                if bound_binders {
                    features = features
                        .union(MethodFeatures::STATEMENTS)
                        .union(MethodFeatures::BOUND_BINDERS);
                }
                if raises {
                    features = features
                        .union(MethodFeatures::STATEMENTS)
                        .union(MethodFeatures::RAISES);
                }
                features
            }),
            locals: RefCell::new(Vec::new()),
            handles: RefCell::new(Vec::new()),
            references: RefCell::new(Vec::new()),
            receivers: RefCell::new(Vec::new()),
            subscripts: RefCell::new(Vec::new()),
            places: RefCell::new(Vec::new()),
            operators: RefCell::new(Vec::new()),
            bound_builtins: RefCell::new(Vec::new()),
            constructions: RefCell::new(Vec::new()),
            repr_calls: RefCell::new(Vec::new()),
        };
        if !shape.block(&method.body, false) {
            return outside("the body is outside the method grammar");
        }
        let class = || {
            let features = shape.features.get();
            let coverage = TemplateCoverage::Certified(if features.is_empty() {
                TemplateClass::MethodScalarBody
            } else {
                TemplateClass::MethodBody(features)
            });
            (
                coverage,
                GrammarNotes {
                    operators: shape.operators.borrow().clone(),
                    bound_builtins: shape.bound_builtins.borrow().clone(),
                    constructions: shape.constructions.borrow().clone(),
                    callable_calls: shape.callable_calls.borrow().clone(),
                    repr_calls: shape.repr_calls.borrow().clone(),
                    print_calls: Vec::new(),
                },
            )
        };
        let Some(facts) = facts else {
            return class();
        };
        if !shape.references_recorded(facts) {
            return outside("a reference is yielded or kept outside the method grammar");
        }
        // The grammar admitted every method call it judged closed, every
        // call an element store embeds, and every construction. Nothing
        // else may have selected a callee or read an effect summary.
        let targets: Vec<&str> = facts
            .selected_calls
            .iter()
            .map(|(_, call)| call.contract.target.as_str())
            .chain(facts.augmented_subscripts.iter().flat_map(|(_, store)| {
                store
                    .getter
                    .iter()
                    .chain(&store.inplace)
                    .map(|call| call.contract.target.as_str())
            }))
            .collect();
        let constructions = shape.constructions.borrow();
        let callable_calls = shape.callable_calls.borrow();
        let admitted_call = |id: &OccurrenceId| {
            facts.selected_calls.iter().any(|(call, _)| call == id)
                || constructions.contains(id)
                || callable_calls.contains(id)
        };
        // A call through a bound reads the summaries of every conformer's
        // method of that name, one key per conformer (`Struct.method`, or the
        // overload symbol `Struct.method$ov$…`), none of them a target.
        let dispatched: Vec<&str> = facts
            .selected_calls
            .iter()
            .filter(|(_, call)| {
                mojito_symbol::symbol::is_trait_dispatch_symbol(&call.contract.target)
            })
            .filter_map(|(_, call)| {
                let target = &call.contract.target;
                let member = target
                    .rsplit_once('.')
                    .map_or(target.as_str(), |(_, member)| member);
                member.split('$').next()
            })
            .collect();
        let conformer_copy = |callee: &str| {
            callee.rsplit_once('.').is_some_and(|(_, member)| {
                let member = member.split('$').next().unwrap_or(member);
                dispatched.contains(&member)
            })
        };
        let stray_call = facts
            .call_parameters
            .iter()
            .any(|(id, _)| !admitted_call(id))
            || !facts.generic_instantiations.is_empty()
            || !facts
                .overload_targets
                .iter()
                .all(|(id, _)| admitted_call(id))
            || !facts.effect_free_callees.iter().all(|callee| {
                targets.contains(&callee.as_str())
                    || conformer_copy(callee)
                    || shape.callable_params.contains(&callee.as_str())
            });
        if stray_call {
            return outside("the body calls something other than a trivial method");
        }
        // A residue the body publishes or reads is republished for an
        // instance, but only a body that calls or forwards its own callable
        // parameter records one the recipe covers.
        let residue = !facts.call_throughs.is_empty() || !facts.call_through_reads.is_empty();
        if residue && !shape.holds(MethodFeatures::CALLABLE_PARAMETERS) {
            return outside("a keyed body publishes or reads a call-through residue");
        }
        if facts.vanishing_transfers
            && (shape.keyed || !shape.holds(MethodFeatures::VANISHING_TRANSFERS))
        {
            return outside("a keyed body replays a transfer summary");
        }
        let effects_closed = facts
            .expression_effects
            .iter()
            .all(|(_, effects)| *effects == mojito_checked::checked::EffectFacts::default());
        let adjustments_derive = facts
            .operation_adjustments
            .iter()
            .all(|(_, adjustment)| adjustment_derives(adjustment));
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

    /// The reference each reference call at the body's occurrences yields,
    /// and each element store made through one or through a setter.
    ///
    /// A store through a mutable reference getter overwrote the getter's
    /// reference at the site; the contract the store embeds still holds it.
    fn captured_reference_stores(
        &self,
        occurrences: &[Occurrence],
        local_reference: &dyn Fn(
            &mojito_types::origin::RefTy,
        ) -> Result<TemplateReference, IncompleteReason>,
        local_contract: &dyn Fn(
            mojito_checked::checked::CheckedCallContract,
        ) -> Result<TemplateCallContract, IncompleteReason>,
    ) -> Result<ReferenceStores, IncompleteReason> {
        let adjustments = values(occurrences, &self.operation_adjustments.borrow());
        let references = adjustments
            .iter()
            .filter_map(|(id, adjustment)| {
                let reference = match adjustment {
                    mojito_checked::checked::SemanticAdjustment::ReferenceResult { reference } => {
                        reference
                    }
                    mojito_checked::checked::SemanticAdjustment::AugmentedSubscript(plan)
                        if plan.setter.is_none()
                            && self.kept_element_store_at(occurrences, *id, adjustment) =>
                    {
                        plan.getter.reference_result.as_ref()?
                    }
                    _ => return None,
                };
                Some(local_reference(reference).map(|reference| (*id, reference)))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let stores = adjustments
            .iter()
            .filter_map(|(id, adjustment)| match adjustment {
                mojito_checked::checked::SemanticAdjustment::AugmentedSubscript(plan)
                    if self.kept_element_store_at(occurrences, *id, adjustment) =>
                {
                    Some((*id, plan))
                }
                _ => None,
            })
            .map(|(id, plan)| {
                Ok((
                    id,
                    TemplateAugmentedSubscript {
                        operand_ty: plan.operand_ty.clone(),
                        result_ty: plan.result_ty.clone(),
                        getter: plan
                            .setter
                            .is_some()
                            .then(|| local_contract(plan.getter.clone()))
                            .transpose()?,
                        inplace: plan.inplace.clone().map(local_contract).transpose()?,
                    },
                ))
            })
            .collect::<Result<Vec<_>, IncompleteReason>>()?;
        Ok(ReferenceStores { references, stores })
    }

    /// [`Self::kept_element_store`] at the occurrence `id`.
    fn kept_element_store_at(
        &self,
        occurrences: &[Occurrence],
        id: OccurrenceId,
        adjustment: &mojito_checked::checked::SemanticAdjustment,
    ) -> bool {
        occurrences
            .iter()
            .find(|occurrence| occurrence.id == id)
            .is_some_and(|occurrence| self.kept_element_store(&occurrence.span, adjustment))
    }

    /// Whether the adjustment at `site` is an element store the call
    /// selected at the site stands for: one through the mutable reference
    /// its getter yields, with no synthesized value, or one read through a
    /// value getter and written back through the setter selected there, with
    /// its computed value keyed at the site. Such a store is kept apart from
    /// the adjustment table (`augmented_subscripts`) with the value getter
    /// and in-place dunder it embeds beside that call, and rebuilt from the
    /// realized call.
    fn kept_element_store(
        &self,
        site: &SourceSpan,
        adjustment: &mojito_checked::checked::SemanticAdjustment,
    ) -> bool {
        let mojito_checked::checked::SemanticAdjustment::AugmentedSubscript(plan) = adjustment
        else {
            return false;
        };
        let selected = self.selected_calls.borrow();
        let selected = selected.get(site);
        match &plan.setter {
            None => plan.value_source.is_none() && selected == Some(&plan.getter),
            Some(setter) => plan.value_source.as_ref() == Some(site) && selected == Some(setter),
        }
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
            .filter(|(now, before)| now != before && now.0 != TRANSFERRED_ORIGINS)
            .map(|((store, _), _)| format!("store:{store}"))
            .collect();
        if self.body_transfer_effects(&occurrences, baseline, reads).0 {
            reasons.push("effects".to_string());
        }
        let annotation = self.return_annotation_spans();
        for (index, table) in FactTable::ALL.into_iter().enumerate() {
            let entries = self.span_table(table);
            let recorded = occurrences
                .iter()
                .filter(|occurrence| entries.has(&occurrence.span))
                .count();
            if self.grew_outside_body(table, &occurrences, baseline.tables[index], &annotation) {
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
                ) || self.kept_element_store(&occurrence.span, adjustment);
                (!kept_apart && !adjustment_derives(adjustment)).then(|| {
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
                    keywords: Vec::new(),
                    identifier: false,
                    method_call: None,
                    operator: None,
                    transfer: false,
                    literal: false,
                    folded_index: None,
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
                        ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } => args
                            .iter()
                            .map(|argument| self.origins.origin(argument.syntax_id))
                            .collect(),
                        _ => Vec::new(),
                    },
                    keywords: match &expr.kind {
                        ExprKind::Call { kwargs, .. } | ExprKind::MethodCall { kwargs, .. } => {
                            kwargs
                                .iter()
                                .map(|keyword| {
                                    (
                                        keyword.name.clone(),
                                        self.origins.origin(keyword.value.syntax_id),
                                    )
                                })
                                .collect()
                        }
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
                    operator: match &expr.kind {
                        ExprKind::Infix(op, left, right) if operator_dispatch(*op) => Some((
                            *op,
                            self.origins.origin(left.syntax_id),
                            self.origins.origin(right.syntax_id),
                        )),
                        _ => None,
                    },
                    transfer: matches!(expr.kind, ExprKind::Transfer(_)),
                    literal: matches!(expr.kind, ExprKind::Int(_)),
                    folded_index: match &expr.kind {
                        ExprKind::Index { index, .. } => match &index.kind {
                            ExprKind::Int(value) => value.to_i64(),
                            _ => None,
                        },
                        _ => None,
                    },
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
        let vanishing_transfers = self.body_transfer_effects(&occurrences, baseline, reads).1;
        let owner_end = self.next_owner.get();
        let local_owner = |owner: OwnerId| {
            self.template_owner(owner, param_owners, baseline.owner_start, owner_end)
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
        let template_contract = |contract| {
            local_contract(
                contract,
                &occurrences,
                &local_place,
                &local_reference,
                &local_invalidations,
            )
        };
        let ReferenceStores {
            references: reference_results,
            stores: augmented_subscripts,
        } = self.captured_reference_stores(&occurrences, &local_reference, &template_contract)?;
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
        let (effect_free_callees, value_callees, call_through_reads) = callee_reads(reads);
        let call_throughs = self
            .transfer_frames
            .borrow()
            .last()
            .map(|frame| frame.call_throughs.clone())
            .unwrap_or_default();
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
        let mut typed_origins = Vec::new();
        let expression_types = unbound_typed(
            TypedTable::Expression,
            values(&occurrences, &self.expression_types.borrow()),
            &local_place,
            &mut typed_origins,
        )?;
        let expression_place_types = unbound_typed(
            TypedTable::Place,
            expression_place_types,
            &local_place,
            &mut typed_origins,
        )?;
        let binding_types = unbound_typed(
            TypedTable::Binding,
            binding_types,
            &local_place,
            &mut typed_origins,
        )?;
        let call_result_origins = values(&occurrences, &self.call_result_origins.borrow())
            .into_iter()
            .map(|(id, slots)| {
                slots
                    .iter()
                    .map(|(slot, origin, mutability)| {
                        Ok(TemplateCallResultOrigin {
                            slot: *slot,
                            origin: template_origin(origin, &local_place)?,
                            mutability: *mutability,
                        })
                    })
                    .collect::<Result<Vec<_>, IncompleteReason>>()
                    .map(|slots| (id, slots))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CheckedBodyFacts {
            expression_types,
            expression_place_types,
            binding_types,
            typed_origins,
            call_result_origins,
            reference_binding_types,
            reference_place_types,
            expression_bindings: owned(&self.expression_bindings.borrow())?,
            statement_bindings: owned(&self.statement_bindings.borrow())?,
            expression_effects: values(&occurrences, &self.expression_effects.borrow()),
            operation_adjustments: values(&occurrences, &self.operation_adjustments.borrow())
                .into_iter()
                .filter(|(id, adjustment)| {
                    !matches!(
                        adjustment,
                        mojito_checked::checked::SemanticAdjustment::ReferenceResult { .. }
                    ) && !self.kept_element_store_at(&occurrences, *id, adjustment)
                })
                .collect(),
            reference_results,
            augmented_subscripts,
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
            value_callees,
            selected_calls: values(&occurrences, &self.selected_calls.borrow())
                .into_iter()
                .map(|(id, call)| Ok((id, template_contract(call)?)))
                .collect::<Result<Vec<_>, IncompleteReason>>()?,
            // An application's origin arguments are erased where it is
            // recorded (`materialized_instantiation_argument`), so one is kept
            // with its struct origin slots unbound, like a retained type.
            struct_applications: sorted_applications(
                reads
                    .struct_applications
                    .iter()
                    .map(|(name, arguments)| {
                        match without_struct_origins(&Ty::Struct(name.clone(), arguments.clone())) {
                            Ty::Struct(name, arguments) => (name, arguments),
                            _ => (name.clone(), arguments.clone()),
                        }
                    })
                    .collect(),
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
            subscript_descriptors: values(&occurrences, &self.subscript_descriptors.borrow()),
            call_place_uses: keyed(&|span| self.call_place_uses.borrow().contains(span)),
            transfers: occurrences
                .iter()
                .filter(|occurrence| occurrence.transfer)
                .map(|occurrence| occurrence.id)
                .collect(),
            vanishing_transfers,
            call_throughs,
            call_through_reads,
            conversions: self.body_conversions(&occurrences),
            // The certificate fills these from the grammar.
            operators: Vec::new(),
            bound_builtins: Vec::new(),
            constructions: Vec::new(),
            callable_calls: Vec::new(),
            repr_calls: Vec::new(),
            print_calls: Vec::new(),
            method_instantiations: values(&occurrences, &self.method_instantiations.borrow()),
            locals: owner_end - baseline.owner_start,
            occurrences: occurrences
                .into_iter()
                .map(|occurrence| occurrence.id)
                .collect(),
        })
    }

    /// Write each realized conversion back into the four conversion tables,
    /// as `record_selected_conversion` writes one: the converted-to type, the
    /// error type, and the source borrow only where the selection has them.
    fn install_conversions(
        &self,
        conversions: &[(SourceSpan, &mojito_checked::templates::TemplateConversion)],
    ) {
        for (site, conversion) in conversions {
            self.implicit_conversions
                .borrow_mut()
                .insert(site.clone(), conversion.target.clone());
            if let Some(result) = &conversion.result {
                self.implicit_conversion_types
                    .borrow_mut()
                    .insert(site.clone(), result.clone());
            }
            if let Some(raises) = &conversion.raises {
                self.implicit_conversion_raises
                    .borrow_mut()
                    .insert(site.clone(), raises.clone());
            }
            if let Some(mutable) = conversion.source_borrow {
                self.conversion_source_borrows
                    .borrow_mut()
                    .insert(site.clone(), mutable);
            }
        }
    }

    /// The implicit conversion recorded at each of `occurrences`, in
    /// occurrence order: what the four conversion tables hold at one span.
    fn body_conversions(
        &self,
        occurrences: &[Occurrence],
    ) -> Vec<(OccurrenceId, mojito_checked::templates::TemplateConversion)> {
        let targets = self.implicit_conversions.borrow();
        let types = self.implicit_conversion_types.borrow();
        let raises = self.implicit_conversion_raises.borrow();
        let borrows = self.conversion_source_borrows.borrow();
        occurrences
            .iter()
            .filter_map(|occurrence| {
                let target = targets.get(&occurrence.span)?;
                Some((
                    occurrence.id,
                    mojito_checked::templates::TemplateConversion {
                        target: target.clone(),
                        result: types.get(&occurrence.span).cloned(),
                        raises: raises.get(&occurrence.span).cloned(),
                        source_borrow: borrows.get(&occurrence.span).copied(),
                    },
                ))
            })
            .collect()
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
            .find_map(|(now, before)| {
                (now != before && now.0 != TRANSFERRED_ORIGINS).then_some(now)
            })
        {
            return Err(IncompleteReason::UnkeyedFact(store));
        }
        if self.body_transfer_effects(occurrences, baseline, reads).0 {
            return Err(IncompleteReason::UnkeyedFact("transfer effects"));
        }
        // A residue is republished verbatim, so it may name only slots and
        // signature places: a compile-time callable is folded per instance,
        // and a carried origin exists only while the argument's type carries
        // a loan (`TemplateObligation::CallThroughResidue`).
        let republishable = self.transfer_frames.borrow().last().is_none_or(|frame| {
            frame.call_throughs.iter().all(|residue| {
                matches!(
                    residue.callee,
                    mojito_checked::checked::CallThroughCallee::RuntimeParam(_)
                ) && residue.args.iter().all(|arg| arg.carried.is_empty())
            })
        });
        if !republishable {
            return Err(IncompleteReason::UnkeyedFact("call-through residue"));
        }
        let annotation = self.return_annotation_spans();
        for (index, table) in FactTable::ALL.into_iter().enumerate() {
            if self.grew_outside_body(table, occurrences, baseline.tables[index], &annotation) {
                return Err(IncompleteReason::FactOutsideBody(table));
            }
            let entries = self.span_table(table);
            let recorded = occurrences
                .iter()
                .filter(|occurrence| entries.has(&occurrence.span))
                .count();
            // A recorded transfer is kept as the bundle's `vanishing_transfers`.
            if recorded > 0 && !derivable_table(table) && table != FactTable::CallTransfers {
                return Err(IncompleteReason::UnsupportedTable(table));
            }
        }
        // A conversion is selected again at the instance's types, so only one
        // the recipe repeats is kept: an `@implicit` constructor, which
        // records the converted-to type beside its target, or the
        // nominal-string wrap, whose constructor no instance changes. An
        // index normalization records neither, and is refused here; the two
        // other writers of a bare literal-constructor target (`String(x)`'s
        // retarget, a literal `for` iterable) are refused by the grammar
        // instead, one at its overload target and one as a statement.
        let targets = self.implicit_conversions.borrow();
        let types = self.implicit_conversion_types.borrow();
        let wrap = mojito_symbol::symbol::nominal_string_literal_ctor_symbol();
        if occurrences.iter().any(|occurrence| {
            targets
                .get(&occurrence.span)
                .is_some_and(|target| !types.contains_key(&occurrence.span) && *target != wrap)
        }) {
            return Err(IncompleteReason::UnkeyedFact("implicit conversion"));
        }
        drop(targets);
        drop(types);
        // A construction's immutable-binder record is kept only as the fact
        // that it is empty, which installation writes again: the grammar
        // admits no `ImmOrigin` argument, so it is never otherwise.
        let immutable_binders = self.construction_immutable_binders.borrow();
        if occurrences.iter().any(|occurrence| {
            immutable_binders
                .get(&occurrence.span)
                .is_some_and(|binders| !binders.is_empty())
        }) {
            return Err(IncompleteReason::ImmutableBinder);
        }
        // A type is retained as written, and a binding identity inside one
        // would never be remapped for an instance. Two exceptions are kept by
        // template owner: a reference at the top of a place or binding type
        // (`rooted_reference`), and a struct's origin arguments
        // (`unbound_struct_origins`); a pointer's own provenance and a
        // reference below the top stay refused.
        let expression_types = self.expression_types.borrow();
        let kept_apart = [
            &*self.expression_place_types.borrow(),
            &*self.binding_types.borrow(),
        ];
        let abstracted = |ty: &Ty| {
            unbound_struct_origins(ty, &|place| {
                Ok(TemplatePlace {
                    root: TemplateOwner::Receiver,
                    path: place.path.clone(),
                })
            })
            .is_ok()
        };
        if occurrences.iter().any(|occurrence| {
            expression_types
                .get(&occurrence.span)
                .is_some_and(|ty| !abstracted(ty))
                || kept_apart
                    .iter()
                    .filter_map(|table| table.get(&occurrence.span))
                    .any(|ty| rooted_reference(ty).is_none() && !abstracted(ty))
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
        let rooted = |place: &TemplatePlace| {
            Ok::<_, TypeError>(mojito_types::origin::OriginPlace {
                root: owner(&place.root)?,
                path: place.path.clone(),
            })
        };
        // A struct type kept with its origin slots unbound gets the
        // instance's own bindings back in them.
        let typed = |table: TypedTable, id: &OccurrenceId, ty: &Ty| {
            facts
                .typed_origins
                .iter()
                .find(|typed| typed.table == table && typed.occurrence == *id)
                .map_or_else(
                    || Ok(ty.clone()),
                    |typed| bind_struct_origins(ty, &typed.origins, &rooted),
                )
        };
        for (id, ty) in &facts.expression_types {
            self.expression_types
                .borrow_mut()
                .insert(span(id)?, typed(TypedTable::Expression, id, ty)?);
        }
        for (id, ty) in &facts.expression_place_types {
            self.expression_place_types
                .borrow_mut()
                .insert(span(id)?, typed(TypedTable::Place, id, ty)?);
        }
        for (id, ty) in &facts.binding_types {
            self.binding_types
                .borrow_mut()
                .insert(span(id)?, typed(TypedTable::Binding, id, ty)?);
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
        for (id, slots) in &facts.call_result_origins {
            let resolved = slots
                .iter()
                .map(|resolved| {
                    Ok((
                        resolved.slot,
                        checked_origin(&resolved.origin, &rooted)?,
                        resolved.mutability,
                    ))
                })
                .collect::<Result<Vec<_>, TypeError>>()?;
            self.call_result_origins
                .borrow_mut()
                .insert(span(id)?, resolved);
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
        self.install_conversions(
            &facts
                .conversions
                .iter()
                .map(|(id, conversion)| Ok((span(id)?, conversion)))
                .collect::<Result<Vec<_>, TypeError>>()?,
        );
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
        for (id, descriptors) in &facts.subscript_descriptors {
            self.subscript_descriptors
                .borrow_mut()
                .insert(span(id)?, descriptors.clone());
        }
        for id in &facts.call_place_uses {
            self.call_place_uses.borrow_mut().insert(span(id)?);
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
        for (id, instantiation) in &facts.method_instantiations {
            self.method_instantiations
                .borrow_mut()
                .insert(span(id)?, instantiation.clone());
        }
        // A construction's immutable-binder record, empty as the grammar
        // requires: what `infer_construction` writes at every construction.
        for id in &facts.constructions {
            self.construction_immutable_binders
                .borrow_mut()
                .insert(span(id)?, Vec::new());
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
        let checked_contract = |call: &TemplateCallContract| {
            checked_contract(call, &span, &placed, &rooted, &referenced)
        };
        for (id, call) in &facts.selected_calls {
            self.selected_calls
                .borrow_mut()
                .insert(span(id)?, checked_contract(call)?);
        }
        self.install_element_stores(facts, &span, &checked_contract)?;
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
        for (callee, residue) in &facts.call_through_reads {
            self.call_through_observations
                .borrow_mut()
                .entry(callee.clone())
                .or_insert_with(|| residue.clone());
        }
        // The residue goes on the body's own frame, which publishes it under
        // the body's key when it is popped, as an inferred body's would be.
        if let Some(frame) = self.transfer_frames.borrow_mut().last_mut() {
            for residue in &facts.call_throughs {
                if !frame.call_throughs.contains(residue) {
                    frame.call_throughs.push(residue.clone());
                }
            }
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

    /// Install each element store: the call installed at its site is its
    /// getter for a store through a reference and its setter otherwise, and
    /// the adjustment overwrites that call's reference as the checker's own
    /// insert does. A store through a setter binds its computed value at
    /// the site.
    fn install_element_stores(
        &self,
        facts: &CheckedBodyFacts,
        span: &dyn Fn(&OccurrenceId) -> Result<SourceSpan, TypeError>,
        checked_contract: &dyn Fn(
            &TemplateCallContract,
        ) -> Result<
            mojito_checked::checked::CheckedCallContract,
            TypeError,
        >,
    ) -> Result<(), TypeError> {
        for (id, store) in &facts.augmented_subscripts {
            let site = span(id)?;
            let installed = self
                .selected_calls
                .borrow()
                .get(&site)
                .cloned()
                .ok_or_else(|| {
                    TypeError::InvariantViolation(
                        "a derived element store has no call at its site".to_string(),
                    )
                })?;
            let (getter, setter) = match &store.getter {
                Some(getter) => (checked_contract(getter)?, Some(installed)),
                None => (installed, None),
            };
            let value_source = setter.is_some().then(|| site.clone());
            self.operation_adjustments.borrow_mut().insert(
                site,
                mojito_checked::checked::SemanticAdjustment::AugmentedSubscript(Box::new(
                    mojito_checked::checked::CheckedAugmentedSubscript {
                        getter,
                        setter,
                        inplace: store.inplace.as_ref().map(checked_contract).transpose()?,
                        operand_ty: store.operand_ty.clone(),
                        result_ty: store.result_ty.clone(),
                        value_source,
                    },
                )),
            );
        }
        Ok(())
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
            (TRANSFERRED_ORIGINS, self.transferred_origins.borrow().len()),
            ("deletable declarations", deletability.declarations.len()),
            (
                "linear declarations",
                deletability.linear_declarations.len(),
            ),
        ]
    }

    /// The template-local identity of a binding a body's fact names: one of
    /// the declaration's parameters, its receiver, a compile-time parameter,
    /// a local the body declared (its identity in `locals..owner_end`), or a
    /// module-scope binding by name. Anything else is outside the body.
    fn template_owner(
        &self,
        owner: OwnerId,
        param_owners: &BodyParams,
        locals: u32,
        owner_end: u32,
    ) -> Result<TemplateOwner, IncompleteReason> {
        param_owners
            .runtime
            .iter()
            .position(|param| *param == Some(owner))
            .map(TemplateOwner::Param)
            .or_else(|| (param_owners.receiver == Some(owner)).then_some(TemplateOwner::Receiver))
            .or_else(|| {
                param_owners
                    .compile_time
                    .iter()
                    .find(|(_, param)| *param == owner)
                    .map(|(name, _)| TemplateOwner::CompileTimeParam(name.clone()))
            })
            .or_else(|| {
                (locals..owner_end)
                    .contains(&owner.0)
                    .then(|| TemplateOwner::Local(owner.0 - locals))
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
    }

    /// Whether a table grew outside the body's occurrences during its check,
    /// from `before` entries. Growth at the enclosing return annotation's own
    /// expressions is the `return`'s re-resolution of that annotation, which
    /// an instance repeats itself (`annotation_spans`), so it is not outside.
    fn grew_outside_body(
        &self,
        table: FactTable,
        occurrences: &[Occurrence],
        before: usize,
        annotation: &HashSet<SourceSpan>,
    ) -> bool {
        let entries = self.span_table(table);
        let recorded = occurrences
            .iter()
            .filter(|occurrence| entries.has(&occurrence.span))
            .count();
        let annotated = annotation.iter().filter(|span| entries.has(span)).count();
        let Some(growth) = entries.entries().checked_sub(before) else {
            return true;
        };
        growth < recorded || growth > recorded + annotated
    }

    /// The expression spans of the return annotation the body being checked
    /// re-resolves at each `return`, or none.
    fn return_annotation_spans(&self) -> HashSet<SourceSpan> {
        self.return_annotations
            .last()
            .and_then(Option::as_ref)
            .map(|(annotation, _)| annotation_spans(annotation))
            .unwrap_or_default()
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

/// The unkeyed store a replayed transfer merges origins into.
const TRANSFERRED_ORIGINS: &str = "transferred origins";

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
        | FactTable::SubscriptDescriptors
        | FactTable::CallPlaceUses
        | FactTable::DeletableBindings
        | FactTable::LinearBindings
        | FactTable::LinearTemporaries
        | FactTable::MethodInstantiations
        | FactTable::ConstructionImmutableBinders
        | FactTable::ImplicitConversions
        | FactTable::ImplicitConversionTypes
        | FactTable::ImplicitConversionRaises
        | FactTable::ConversionSourceBorrows
        | FactTable::CallResultOrigins => true,
        FactTable::ContextualBases
        | FactTable::CallTransfers
        | FactTable::SimdConstructions
        | FactTable::ParameterizedMethodCalls
        | FactTable::TupleUnpackPlans
        | FactTable::ViewResultInteriors
        | FactTable::WithDesugars
        | FactTable::DeclarationCaptures
        | FactTable::ComprehensionBindings
        | FactTable::IterationProtocols
        | FactTable::ExplicitDestroyCalls
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

/// A bundle with every rebind's by-value selection cleared and the grammar's
/// construction notes dropped, for comparing a derived bundle with a clone
/// check's: a raw capture names no construction, and a realized bundle keeps
/// them for installation.
fn comparable(facts: &CheckedBodyFacts) -> CheckedBodyFacts {
    let mut facts = facts.clone();
    for (_, assertion) in &mut facts.rebind_assertions {
        assertion.by_value = false;
    }
    facts.constructions.clear();
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
    /// Every call contract a bundle keeps: the selected calls and those an
    /// element store embeds.
    fn calls<'f>(
        selected: &'f mut [(OccurrenceId, TemplateCallContract)],
        stores: &'f mut [(OccurrenceId, TemplateAugmentedSubscript)],
    ) -> impl Iterator<Item = &'f mut TemplateCallContract> {
        selected.iter_mut().map(|(_, call)| call).chain(
            stores
                .iter_mut()
                .flat_map(|(_, store)| store.contracts_mut()),
        )
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
    for call in calls(&mut facts.selected_calls, &mut facts.augmented_subscripts) {
        if let Some(reference) = &mut call.reference_result {
            origin_owners(&mut reference.origin, visit);
        }
        for origin in &mut call.result_origins {
            origin_owners(origin, visit);
        }
    }
    for origin in facts
        .typed_origins
        .iter_mut()
        .flat_map(|typed| &mut typed.origins)
    {
        origin_owners(origin, visit);
    }
    for resolved in facts
        .call_result_origins
        .iter_mut()
        .flat_map(|(_, slots)| slots)
    {
        origin_owners(&mut resolved.origin, visit);
    }
    let call_invalidations = calls(&mut facts.selected_calls, &mut facts.augmented_subscripts)
        .flat_map(|call| {
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

/// Whether a compile-time argument of a construction is a type: no origin
/// (`ImmOrigin(o)` would bind a slot immutably) and no value.
fn type_argument(argument: &mojito_ast::ast::ParamArg) -> bool {
    use mojito_ast::ast::ParamArg;
    match argument {
        ParamArg::Type(_) => true,
        ParamArg::Named { value, .. } => type_argument(value),
        ParamArg::Value(_) => false,
    }
}

/// The spans of the expressions a return annotation embeds (`origin_of(self)`
/// in `Self.IteratorType[origin_of(self)]`). A body's `return` re-resolves
/// the annotation over the body's own places
/// (`reconcile_return_origin_tails`), so an inference records the receiver's
/// facts there, outside the body; a derived instance resolves the annotation
/// once itself.
fn annotation_spans(annotation: &mojito_ast::ast::SourceType) -> HashSet<SourceSpan> {
    struct Spans(HashSet<SourceSpan>);
    impl mojito_ast::visit::Visitor for Spans {
        fn visit_expr(&mut self, expr: &Expr) {
            self.0.insert(expr.source_span());
        }
    }
    let mut spans = Spans(HashSet::new());
    mojito_ast::visit::walk_type(&mut spans, annotation);
    spans.0
}

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

/// A type with every struct origin argument mapped by `origin`, in pre-order:
/// the one traversal behind keeping a struct's origin tail by template owner
/// (`unbound_struct_origins`) and giving it an instance's bindings back
/// (`bind_struct_origins`). A pointer's or a reference's own origin is not a
/// struct argument and passes through.
fn map_struct_origins<E>(
    ty: &Ty,
    origin: &mut dyn FnMut(
        &mojito_types::origin::Origin,
    ) -> Result<mojito_types::origin::Origin, E>,
) -> Result<Ty, E> {
    use mojito_types::origin::Origin;
    use mojito_types::types::TyArg;
    fn all<E>(
        types: &[Ty],
        origin: &mut dyn FnMut(&Origin) -> Result<Origin, E>,
    ) -> Result<Vec<Ty>, E> {
        types
            .iter()
            .map(|ty| map_struct_origins(ty, origin))
            .collect()
    }
    Ok(match ty {
        Ty::Struct(name, arguments) => Ty::Struct(
            name.clone(),
            arguments
                .iter()
                .map(|argument| {
                    Ok(match argument {
                        TyArg::Ty(ty) => TyArg::Ty(map_struct_origins(ty, origin)?),
                        TyArg::Origin(slot) => TyArg::Origin(origin(slot)?),
                        TyArg::Val(value) => TyArg::Val(value.clone()),
                    })
                })
                .collect::<Result<Vec<_>, E>>()?,
        ),
        Ty::Tuple(elements) => Ty::Tuple(all(elements, origin)?),
        Ty::RuntimePack(elements) => Ty::RuntimePack(all(elements, origin)?),
        Ty::Variant(alternatives) => Ty::Variant(all(alternatives, origin)?),
        Ty::VariadicPack(element) => {
            Ty::VariadicPack(Box::new(map_struct_origins(element, origin)?))
        }
        Ty::ComptimeList(element) => {
            Ty::ComptimeList(Box::new(map_struct_origins(element, origin)?))
        }
        Ty::Pointer {
            element,
            origin: provenance,
        } => Ty::Pointer {
            element: Box::new(map_struct_origins(element, origin)?),
            origin: provenance.clone(),
        },
        Ty::Ref(reference) => {
            let mut reference = reference.clone();
            reference.referent = Box::new(map_struct_origins(&reference.referent, origin)?);
            Ty::Ref(reference)
        }
        _ => ty.clone(),
    })
}

/// A struct-typed value's type with its origin tails unbound, and those
/// origins by template owner, for a type that names a binding only there.
/// `None` when the type names no binding at all, so it is kept as written.
fn unbound_struct_origins(
    ty: &Ty,
    place: &dyn Fn(&mojito_types::origin::OriginPlace) -> Result<TemplatePlace, IncompleteReason>,
) -> Result<Option<(Ty, Vec<TemplateOrigin>)>, IncompleteReason> {
    if !names_place(ty) {
        return Ok(None);
    }
    let mut origins = Vec::new();
    let unbound = map_struct_origins(ty, &mut |origin| {
        origins.push(template_origin(origin, place)?);
        Ok(mojito_types::origin::Origin::Unbound)
    })?;
    if names_place(&unbound) {
        return Err(IncompleteReason::ExternalBinding);
    }
    Ok(Some((unbound, origins)))
}

/// One retained type table with every struct type that names a binding in an
/// origin argument (`_ListIter[T, origin_of(self)]`) kept with the slot
/// unbound, its origins appended to `typed_origins` by template owner.
fn unbound_typed(
    table: TypedTable,
    types: Vec<(OccurrenceId, Ty)>,
    place: &dyn Fn(&mojito_types::origin::OriginPlace) -> Result<TemplatePlace, IncompleteReason>,
    typed_origins: &mut Vec<TypedOrigins>,
) -> Result<Vec<(OccurrenceId, Ty)>, IncompleteReason> {
    types
        .into_iter()
        .map(|(id, ty)| match unbound_struct_origins(&ty, place)? {
            Some((ty, origins)) => {
                typed_origins.push(TypedOrigins {
                    table,
                    occurrence: id,
                    origins,
                });
                Ok((id, ty))
            }
            None => Ok((id, ty)),
        })
        .collect()
}

/// A type with every struct origin argument unbound, for comparing two types
/// but for their origin tails.
fn without_struct_origins(ty: &Ty) -> Ty {
    let Ok(erased) = map_struct_origins(ty, &mut |_| {
        Ok::<_, std::convert::Infallible>(mojito_types::origin::Origin::Unbound)
    });
    erased
}

/// The inverse of [`unbound_struct_origins`]: the kept origins, rooted at an
/// instance's own bindings, written back into the type's struct origin slots
/// in the order they were taken out.
fn bind_struct_origins(
    ty: &Ty,
    origins: &[TemplateOrigin],
    rooted: &dyn Fn(&TemplatePlace) -> Result<mojito_types::origin::OriginPlace, TypeError>,
) -> Result<Ty, TypeError> {
    let mut slots = origins.iter();
    map_struct_origins(ty, &mut |_| {
        let origin = slots.next().ok_or_else(|| {
            TypeError::InvariantViolation("template derivation lost a struct origin".to_string())
        })?;
        checked_origin(origin, rooted)
    })
}

/// Whether a type names a checker-local place: an origin rooted at a binding
/// identity, in a pointer, a reference, or a struct's origin argument.
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

/// Whether a method's binder is an origin the body may only read through.
///
/// It names where a `ref` parameter's referent lives. It is inferred at every
/// call, erased before execution, and no clone is minted per origin, so a
/// clone's check binds it symbolically as the template's does. A `mut` one
/// lets the body write through it, which is judged per instantiation.
/// A trait-bounded type binder of a method's own (`[H: Hasher]`), which a
/// clone keeps and binds symbolically as the template does.
fn bound_binder(binder: &mojito_ast::ast::TypeParam) -> bool {
    !binder.bounds.is_empty()
        && !binder
            .bounds
            .iter()
            .any(|bound| bound == "Origin" || bound == "OriginSet")
        && binder.origin_mutability.is_none()
        && binder.value_type.is_none()
        && binder.callable_bound.is_none()
        && binder.default.is_none()
        && !binder.infer_only
}

fn origin_binder(binder: &mojito_ast::ast::TypeParam) -> bool {
    matches!(binder.bounds.as_slice(), [bound] if bound == "Origin")
        && binder
            .origin_mutability
            .as_ref()
            .is_none_or(|mutability| matches!(mutability.kind, ExprKind::Bool(false)))
        && binder.value_type.is_none()
        && binder.callable_bound.is_none()
        && binder.default.is_none()
        && !binder.infer_only
}

/// Whether the lowered callee `target` is `owner`'s `method`, or a clone or
/// an overload of it.
fn names_method(target: &str, owner: &str, method: &str) -> bool {
    target
        .strip_prefix(owner)
        .and_then(|rest| rest.strip_prefix('.'))
        .and_then(|rest| rest.strip_prefix(method))
        .is_some_and(|overload| overload.is_empty() || overload.starts_with('$'))
}

/// The fact a table holds at `id`.
/// What one body's effect reads become in its bundle: the callees whose
/// summaries were empty, those among them read where a callable's name
/// stands as a value, and each call-through residue read, by callee. A
/// callee read with a residue is not effect-free, whatever its transfer
/// summary said: the residue is kept apart, and an instance owes it again.
fn callee_reads(reads: &BodyReads) -> (Vec<String>, Vec<String>, CallThroughReads) {
    let mut call_through_reads: CallThroughReads = Vec::new();
    for (callee, read) in &reads.effect_queries {
        if let EffectRead::CallThrough(residue) = read
            && !call_through_reads.iter().any(|(read, _)| read == callee)
        {
            call_through_reads.push((callee.clone(), residue.clone()));
        }
    }
    call_through_reads.sort_by(|(left, _), (right, _)| left.cmp(right));
    let names = |value: bool| {
        let mut names: Vec<String> = reads
            .effect_queries
            .iter()
            .filter(|(callee, read)| {
                (!value || matches!(read, EffectRead::Value))
                    && !call_through_reads.iter().any(|(read, _)| read == callee)
            })
            .map(|(callee, _)| callee.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    };
    (names(false), names(true), call_through_reads)
}

/// Each callee whose call-through residue one body read, with the residue.
type CallThroughReads = Vec<(String, Vec<mojito_checked::checked::CallThroughEffect>)>;

/// Record that realization resolved the template's callee `selected` to the
/// instance's `target`: the target's summaries are read as the template read
/// them, empty or with the residue the template kept.
fn note_realized_callee(facts: &mut CheckedBodyFacts, selected: &str, target: &str) {
    // A second call on the same callee finds the read already rekeyed.
    let residue = facts
        .call_through_reads
        .iter_mut()
        .find(|(callee, _)| callee == selected || callee == target);
    match residue {
        Some(read) => target.clone_into(&mut read.0),
        None if !facts
            .effect_free_callees
            .iter()
            .any(|callee| callee == target) =>
        {
            facts.effect_free_callees.push(target.to_string());
        }
        None => {}
    }
}

/// Write each realized conversion back into the call boundary that carries
/// it a second time.
///
/// A converting argument records its conversion twice: in the four
/// conversion tables, which [`Checker::realize_conversion`] has just
/// re-selected at the instance's types, and in the selected call's own
/// boundary, which lowering reads to build the argument's register. The
/// boundary names the constructor the template found, so an instance takes
/// the target back from the conversion at the same occurrence; a boundary
/// conversion the tables no longer hold is one the recipes did not reach.
fn realize_boundary_conversions(facts: &mut CheckedBodyFacts) -> Result<(), &'static str> {
    let realized: Vec<(OccurrenceId, String)> = facts
        .conversions
        .iter()
        .map(|(id, conversion)| (*id, conversion.target.clone()))
        .collect();
    for (_, call) in &mut facts.selected_calls {
        for argument in &mut call.arguments {
            for adjustment in &mut argument.adjustments {
                let mojito_checked::checked::CheckedCallValueAdjustment::ImplicitConversion {
                    target,
                } = adjustment
                else {
                    continue;
                };
                let selected = realized
                    .iter()
                    .find(|(id, _)| *id == argument.value)
                    .map(|(_, target)| target)
                    .ok_or("a converted argument kept no conversion of its own")?;
                selected.clone_into(target);
            }
        }
    }
    Ok(())
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

/// Whether one adjustment has a derivation recipe, as a template's own
/// symbolic facts can be judged.
///
/// `derive_adjustment` answers for one instance, under that instance's
/// substitution. A template applies it under the identity, where an
/// adjustment naming a type still names a parameter: a type name is the one
/// recipe that re-renders such a type, so only an instance's substitution
/// decides it, and the derivation refuses there if the type stays symbolic.
fn adjustment_derives(adjustment: &mojito_checked::checked::SemanticAdjustment) -> bool {
    matches!(
        adjustment,
        mojito_checked::checked::SemanticAdjustment::TypeName { .. }
    ) || mojito_checked::templates::derive_adjustment(adjustment, &Ty::clone).is_some()
}

/// One kept call's contract under an instance's own spans and bindings: the
/// inverse of [`local_contract`].
fn checked_contract(
    call: &TemplateCallContract,
    span: &dyn Fn(&OccurrenceId) -> Result<SourceSpan, TypeError>,
    placed: &PlacedInvalidations<'_>,
    rooted: &dyn Fn(&TemplatePlace) -> Result<mojito_types::origin::OriginPlace, TypeError>,
    referenced: &dyn Fn(&TemplateReference) -> Result<mojito_types::origin::RefTy, TypeError>,
) -> Result<mojito_checked::checked::CheckedCallContract, TypeError> {
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
    let reference_result = call.reference_result.as_ref().map(referenced).transpose()?;
    Ok(mojito_checked::checked::CheckedCallContract {
        result_ty: match reference_result.clone() {
            Some(reference) => Ty::Ref(reference),
            None if call.result_origins.is_empty() => call.contract.result_ty.clone(),
            None => bind_struct_origins(&call.contract.result_ty, &call.result_origins, rooted)?,
        },
        reference_result,
        boundary: mojito_checked::checked::CheckedCallBoundary {
            arguments,
            invalidations: placed(&call.invalidations)?,
        },
        ..call.contract.clone()
    })
}

/// Kept invalidations under an instance's own bindings.
type PlacedInvalidations<'a> = dyn Fn(
        &[TemplateInvalidation],
    ) -> Result<Vec<mojito_checked::checked::InteriorInvalidation>, TypeError>
    + 'a;

/// One call's contract in template-local terms: its boundary kept by
/// occurrence and template owner, and its reference result by template
/// owner.
fn local_contract(
    mut contract: mojito_checked::checked::CheckedCallContract,
    occurrences: &[Occurrence],
    local_place: &dyn Fn(
        &mojito_types::origin::OriginPlace,
    ) -> Result<TemplatePlace, IncompleteReason>,
    local_reference: &dyn Fn(
        &mojito_types::origin::RefTy,
    ) -> Result<TemplateReference, IncompleteReason>,
    local_invalidations: &dyn Fn(
        Vec<mojito_checked::checked::InteriorInvalidation>,
    ) -> Result<Vec<TemplateInvalidation>, IncompleteReason>,
) -> Result<TemplateCallContract, IncompleteReason> {
    let boundary = std::mem::take(&mut contract.boundary);
    // The reference a call yields is its result type too; both name the
    // receiver's binding, so the referent stands in for the result until an
    // instance installs it.
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
    let result_origins = match unbound_struct_origins(&contract.result_ty, local_place)? {
        Some((unbound, origins)) => {
            contract.result_ty = unbound;
            origins
        }
        None => Vec::new(),
    };
    let arguments = boundary
        .arguments
        .into_iter()
        .map(|argument| {
            // An argument the call synthesized is no occurrence of the body.
            let value = occurrences
                .iter()
                .find(|occurrence| occurrence.span == argument.value_source)
                .map(|occurrence| occurrence.id)
                .ok_or(IncompleteReason::FactOutsideBody(FactTable::SelectedCalls))?;
            Ok(TemplateArgumentBoundary {
                source: argument.source,
                value,
                adjustments: argument.adjustments,
                invalidations: local_invalidations(argument.invalidations)?,
            })
        })
        .collect::<Result<Vec<_>, IncompleteReason>>()?;
    Ok(TemplateCallContract {
        contract,
        reference_result,
        result_origins,
        arguments,
        invalidations: local_invalidations(boundary.invalidations)?,
    })
}

/// What `captured_reference_stores` reads off the adjustment table: the
/// reference each reference call yields, and each element store through one.
struct ReferenceStores {
    references: Vec<(OccurrenceId, TemplateReference)>,
    stores: Vec<(OccurrenceId, TemplateAugmentedSubscript)>,
}

/// The template's facts with every retained type substituted for an
/// instance, before any call is realized: an adjustment through its recipe,
/// a type keyed by a pack-element occurrence under that copy's loop index,
/// every other type under the instance's arguments alone.
fn substituted_facts(
    template: &CheckedBodyFacts,
    InstanceSubstitution {
        types: substitution,
        packs,
    }: &InstanceSubstitution,
    indices: &ElementIndices,
) -> Result<CheckedBodyFacts, &'static str> {
    let substitute = |ty: &Ty| mojito_types::types::substitute_packs(ty, substitution, packs, &[]);
    let typed = |entries: &[(OccurrenceId, Ty)]| -> Vec<(OccurrenceId, Ty)> {
        entries
            .iter()
            .map(|(id, ty)| {
                let values: Vec<_> = indices.get(id).cloned().into_iter().collect();
                (
                    *id,
                    mojito_types::types::substitute_packs(ty, substitution, packs, &values),
                )
            })
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
    Ok(CheckedBodyFacts {
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
        augmented_subscripts: substituted_element_stores(
            &template.augmented_subscripts,
            &substitute,
        )?,
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
        // A value-position read is under the same name in the instance;
        // every call's read is under the callee realized for it.
        effect_free_callees: template.value_callees.clone(),
        ..template.clone()
    })
}

/// The element stores of an instance: the template's, their types
/// substituted. The value getter and in-place dunder a store embeds are
/// kept as they stand, so each must name only closed types: an instance
/// realizes no call but the one at the site.
fn substituted_element_stores(
    stores: &[(OccurrenceId, TemplateAugmentedSubscript)],
    substitute: &dyn Fn(&Ty) -> Ty,
) -> Result<Vec<(OccurrenceId, TemplateAugmentedSubscript)>, &'static str> {
    stores
        .iter()
        .map(|(id, store)| {
            let mut store = store.clone();
            let closed =
                store.contracts_mut().all(|call| {
                    !mojito_types::types::is_symbolic(&call.contract.result_ty)
                        && call.contract.arguments.iter().all(|argument| {
                            !mojito_types::types::is_symbolic(&argument.parameter_ty)
                        })
                });
            if !closed {
                return Err("an element store embeds a call of a parameter type");
            }
            store.operand_ty = substitute(&store.operand_ty);
            store.result_ty = substitute(&store.result_ty);
            Ok((*id, store))
        })
        .collect()
}

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
    /// The declared structs, which a call may construct.
    structs: &'a HashMap<String, super::StructInfo>,
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
    /// Whether `self` is a `mut`, `var`, or `deinit` receiver, whose scalar
    /// fields the body may write.
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
    /// The subscripts admitted as the base of a store.
    subscripts: RefCell<Vec<OccurrenceId>>,
    /// The arguments admitted as a place a call keeps.
    places: RefCell<Vec<OccurrenceId>>,
    /// The operators admitted over operands of one parameter-typed type.
    operators: RefCell<Vec<OccurrenceId>>,
    /// The checker builtins called on a bounded parameter, which an instance
    /// proves again at its own type.
    bound_builtins: RefCell<Vec<(OccurrenceId, BoundBuiltin)>>,
    /// The struct constructions admitted, whose constructor an instance
    /// re-selects on its own arguments.
    constructions: RefCell<Vec<OccurrenceId>>,
    /// The parameters declared with a `def(...)` type, which the body may
    /// call or forward.
    callable_params: Vec<&'a str>,
    /// The calls through such a parameter admitted, whose contract an
    /// instance takes from its own parameter binding.
    callable_calls: RefCell<Vec<OccurrenceId>>,
    /// The `repr(value)` calls admitted, whose argument an instance proves
    /// `Writable` at its own type.
    repr_calls: RefCell<Vec<OccurrenceId>>,
    /// The variadic parameters collecting a type pack of the declaration's
    /// own, whose elements the body may read by loop index.
    packs: Vec<&'a str>,
    /// The `comptime for` variables in scope, innermost last.
    loop_vars: RefCell<Vec<String>>,
    /// The `print(...)` calls admitted, whose arguments an instance proves
    /// `Writable` at its own types.
    print_calls: RefCell<Vec<OccurrenceId>>,
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
            StmtKind::ComptimeFor { var, body, .. } if self.keyed => {
                self.loop_vars.borrow_mut().push(var.clone());
                let admitted = self.block(body, true);
                self.loop_vars.borrow_mut().pop();
                admitted
            }
            StmtKind::VarDecl { name, value, .. } if self.keyed && !in_loop => {
                let scalar = self.expression(value) && self.scalar(value);
                self.locals
                    .borrow_mut()
                    .push((name.clone(), LocalKind::Scalar));
                scalar
            }
            // A runtime statement is checked once, whatever runs it, so it
            // neither drops nor copies an occurrence. A scalar local is one
            // binding wherever it is declared. An annotated local holds its
            // declared type, which the value may convert to.
            StmtKind::VarDecl { name, ty, value } if !self.keyed => {
                let closed = self.expression(value) && self.scalar(value);
                let scalar = closed && (ty.is_none() || self.scalar_binding(value));
                let moved = !scalar
                    && self.moved_result.is_some()
                    && (closed
                        || self.whole_value(value)
                        || self.reference_read(value)
                        || (ty.is_some() && self.converted_place(value)))
                    && (ty.is_none() || self.annotated_binding(value));
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
            StmtKind::Raise(value) if !self.keyed => {
                self.raised(value) && self.holds(MethodFeatures::RAISES)
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
                let scalar = (self.scalar_field_place(place) || self.reference_element(place))
                    && self.expression(value)
                    && self.scalar(value);
                (scalar || self.whole_store(place, value) || self.element_store(place, value))
                    && self.holds(MethodFeatures::STATEMENTS)
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
                    || (self.keyed && self.print_call(value))
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
                let scalar = (local
                    || (!self.keyed
                        && (self.scalar_field_place(place)
                            || self.reference_element(place)
                            || self.setter_element(place))))
                    && self.scalar(place)
                    && self.expression(value)
                    && self.scalar(value)
                    && (self.keyed || self.holds(MethodFeatures::STATEMENTS));
                scalar
                    || (!self.keyed
                        && self.inplace_element(place, value)
                        && self.holds(MethodFeatures::STATEMENTS))
            }
            _ => false,
        }
    }

    /// The value of a `return` in a method that returns a reference: a field
    /// of `self`, a pointer slot, a `ref` local, a `mut` or `ref` parameter, or
    /// a reference a call on a field yields, of exactly the declared referent
    /// type, so neither check converts it.
    ///
    /// The `return` keeps the place as a handle because the declaration
    /// returns a reference, whatever the place's type, and demands neither a
    /// copy nor a move of it. Whether the place lies within the declared
    /// origin is judged on its path and the signature, which no instance
    /// changes.
    fn returned_place(&self, value: &Expr) -> bool {
        let id = self.occurrence(value);
        let forwarded = matches!(&value.kind, ExprKind::Identifier(name)
            if self.reference_local(name) || self.borrowed_params.contains(&name.as_str()));
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
    /// field is read or a method called through ([`Self::through`]), or an
    /// argument ([`Self::reference_argument`]), never as an operand.
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
        let on_self =
            self.receiver && matches!(&object.kind, ExprKind::Identifier(name) if name == "self");
        let admitted = !self.keyed
            && (self.receiver_field(object) || on_self)
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
    /// binding's declaration. Likewise each subscript descriptor sits at a
    /// subscript admitted as a store's base, and each kept call place at an
    /// argument admitted as one.
    fn references_recorded(&self, facts: &CheckedBodyFacts) -> bool {
        let handles = self.handles.borrow();
        let references = self.references.borrow();
        let receivers = self.receivers.borrow();
        let subscripts = self.subscripts.borrow();
        let places = self.places.borrow();
        facts.subscript_descriptors.len() == subscripts.len()
            && facts
                .subscript_descriptors
                .iter()
                .all(|(id, _)| subscripts.contains(id))
            && facts.call_place_uses.len() == places.len()
            && facts.call_place_uses.iter().all(|id| places.contains(id))
            && facts.borrowed_reference_receivers.len() == receivers.len()
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

    /// The operand of a `raise`: a construction, or `Error` made from a
    /// string literal.
    ///
    /// `require_error` asks whether the operand is a string, which a
    /// constructed struct's name and the builtin `Error` settle, and whether
    /// its type is the declared error type. Both types are functions of the
    /// same parameters, so the instance's answer is the template's.
    fn raised(&self, value: &Expr) -> bool {
        let id = self.occurrence(value);
        let error = matches!(&value.kind, ExprKind::Call { name, param_args, args, kwargs }
            if name == "Error"
                && !self.structs.contains_key(name)
                && param_args.is_empty()
                && kwargs.is_empty()
                && matches!(args.as_slice(), [message] if matches!(message.kind, ExprKind::Str(_))))
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.expression_types, id) == Some(&Ty::Error)
                    && fact_at(&facts.call_parameters, id).is_none()
                    && fact_at(&facts.expression_bindings, id).is_none()
            });
        error || self.construction(value)
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
    /// local, or a field of `self`. It is never an operand, a receiver, or a
    /// condition, and as an argument it binds a parameter of its own type
    /// ([`Self::argument`]), so nothing dispatches on its type.
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
            _ if self.call_result(expr) || self.construction(expr) || self.operator_value(expr) => {
                true
            }
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

    /// A parameter, a local, or a field of `self` that an `@implicit`
    /// constructor reads in place: no copy is recorded, and an instance
    /// selecting a constructor that consumes its source refuses
    /// ([`Checker::realize_conversion`]).
    fn converted_place(&self, expr: &Expr) -> bool {
        let named = match &expr.kind {
            ExprKind::Identifier(name) => {
                self.params.contains(&name.as_str()) || self.declared(name)
            }
            ExprKind::Member { .. } => self.receiver_field(expr),
            _ => false,
        };
        named
            && self
                .facts
                .is_none_or(|facts| fact_at(&facts.conversions, self.occurrence(expr)).is_some())
            && self.holds(MethodFeatures::OPAQUE_MOVES)
    }

    /// The result of an admitted operator ([`Self::operator`]) whose operands
    /// are not closed scalars: a temporary of the operand's own type, which an
    /// instance's dunder yields exactly as a sibling call's result does.
    fn operator_value(&self, expr: &Expr) -> bool {
        matches!(&expr.kind, ExprKind::Infix(op, left, right)
            if self.operator(expr, *op, left, right))
    }

    /// The result of a sibling call, of any type: a temporary, whose type is
    /// the contract's substituted result.
    fn call_result(&self, expr: &Expr) -> bool {
        matches!(&expr.kind, ExprKind::MethodCall { method, .. }
            if !matches!(method.as_str(), "unsafe_take_pointee" | "unsafe_offset"))
            && self.expression(expr)
    }

    /// A construction of a declared struct: a temporary of the constructed
    /// type, whose constructor an instance re-selects on its own arguments.
    ///
    /// Its compile-time arguments are types, so it binds no origin
    /// immutably and folds no value. Each argument is a closed scalar, a
    /// whole value, or the `copy:` of a named place, so what the call records
    /// at an argument is decided by the argument's syntax and the
    /// constructor's conventions, and the template's selection binds every
    /// argument exactly under every instance. A hand-written constructor's
    /// `ref` parameter takes a named place or a reference, which it lends.
    fn construction(&self, expr: &Expr) -> bool {
        let ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } = &expr.kind
        else {
            return false;
        };
        if self.keyed || !self.structs.contains_key(name) {
            return false;
        }
        // A binding of that name would shadow the struct: the recorded type
        // says the call constructed it.
        let id = self.occurrence(expr);
        let constructed = self.facts.is_none_or(|facts| {
            matches!(fact_at(&facts.expression_types, id),
                Some(Ty::Struct(constructed, _)) if constructed == name)
        });
        let named_place = |place: &Expr| match &place.kind {
            ExprKind::Identifier(name) => {
                (self.receiver && name == "self")
                    || self.params.contains(&name.as_str())
                    || self.declared(name)
            }
            ExprKind::Member { .. } => self.receiver_field(place),
            _ => false,
        };
        let copied = args.is_empty()
            && matches!(kwargs.as_slice(), [copy] if copy.name == "copy" && named_place(&copy.value));
        // A fieldwise struct's reference field takes a `ref` local as the
        // handle it is: the argument records the referent's read-through
        // facts and that it is kept as a handle, both decided by the field's
        // declaration and the binding's kind.
        let info = &self.structs[name];
        let reference_field = |field: Option<&(String, Ty)>, argument: &Expr| {
            info.fieldwise_init
                && !info.methods.contains_key("__init__")
                && field.is_some_and(|(_, ty)| matches!(ty, Ty::Ref(_)))
                && matches!(&argument.kind, ExprKind::Identifier(name) if self.reference_local(name))
        };
        // A hand-written constructor's `ref` parameter borrows the place it is
        // handed, a named place or one reached through a reference. Which
        // positions lend is the selected constructor's declaration, and the
        // loan's mutability the place's own, so neither changes per instance.
        let lent = |position: usize| {
            self.facts.is_none_or(|facts| {
                matches!(fact_at(&facts.operation_adjustments, id),
                    Some(mojito_checked::checked::SemanticAdjustment::BorrowRefArguments {
                        arguments,
                        materialized: None,
                    }) if arguments.iter().any(|(lent, _)| *lent == position))
            })
        };
        let lent_place = |position: usize, argument: &Expr| {
            lent(position) && (named_place(argument) || self.reference_argument(argument))
        };
        let value = |field: Option<&(String, Ty)>, argument: &Expr| {
            (self.expression(argument) && self.scalar(argument))
                || self.whole_value(argument)
                || reference_field(field, argument)
        };
        let admitted = constructed
            && param_args.iter().all(type_argument)
            && (copied
                || (args.iter().enumerate().all(|(position, argument)| {
                    value(info.fields.get(position), argument) || lent_place(position, argument)
                }) && kwargs.iter().all(|keyword| {
                    let field = info.fields.iter().find(|(name, _)| *name == keyword.name);
                    value(field, &keyword.value)
                })));
        if admitted {
            for (position, argument) in args.iter().enumerate() {
                if !value(info.fields.get(position), argument) && lent_place(position, argument) {
                    let mut places = self.places.borrow_mut();
                    let id = self.occurrence(argument);
                    if !places.contains(&id) {
                        places.push(id);
                    }
                }
            }
            let fields = args
                .iter()
                .enumerate()
                .map(|(position, argument)| (info.fields.get(position), argument));
            let keyed = kwargs.iter().map(|keyword| {
                let field = info.fields.iter().find(|(name, _)| *name == keyword.name);
                (field, &keyword.value)
            });
            for (field, argument) in fields.chain(keyed) {
                if reference_field(field, argument) {
                    self.handle(self.occurrence(argument));
                }
            }
            let mut constructions = self.constructions.borrow_mut();
            if !constructions.contains(&id) {
                constructions.push(id);
            }
        }
        admitted && self.holds(MethodFeatures::CONSTRUCTIONS)
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

    /// Whether the local an annotated `var` binds to `value` holds a closed
    /// scalar.
    fn scalar_binding(&self, value: &Expr) -> bool {
        self.facts.is_none_or(|facts| {
            fact_at(&facts.binding_types, self.occurrence(value)).is_some_and(closed_scalar)
        })
    }

    /// Whether an annotated `var`'s value reaches the declared type either
    /// as it is or through a recorded conversion, which an instance selects
    /// again at its own types. Any other relation the check accepted — an
    /// annotation left to inference, a view borrowing its source — is a
    /// fact no derivation carries.
    fn annotated_binding(&self, value: &Expr) -> bool {
        let id = self.occurrence(value);
        self.facts.is_none_or(|facts| {
            fact_at(&facts.binding_types, id).is_some_and(|declared| {
                self.typed(value, declared) || fact_at(&facts.conversions, id).is_some()
            })
        })
    }

    /// Whether the recorded type of `expr` is exactly `ty`, but for the
    /// origin arguments of a struct: a retained type keeps those slots
    /// unbound, and a `return` reconciles a value's origin tail with the
    /// declared one on the places alone (`reconcile_return_origin_tails`),
    /// which no instance changes.
    fn typed(&self, expr: &Expr, ty: &Ty) -> bool {
        self.facts.is_none_or(|facts| {
            fact_at(&facts.expression_types, self.occurrence(expr)).is_some_and(|recorded| {
                without_struct_origins(recorded) == without_struct_origins(ty)
            })
        })
    }

    /// A closed scalar field of a writable `self`, as the target of a store.
    ///
    /// A field reached through a subscript (`self.entries[i].hits`) makes the
    /// subscript a place, which records its index shape: one plain index, and
    /// whether the struct's setter takes the value by keyword. Both are read
    /// off the syntax and the setter's declaration, so an instance inherits
    /// the entry.
    fn scalar_field_place(&self, place: &Expr) -> bool {
        let admitted = ((self.self_writable && self.receiver_field(place))
            || self.reference_member(place))
            && self.scalar(place);
        let through = match &place.kind {
            ExprKind::Member { object, .. } if matches!(object.kind, ExprKind::Index { .. }) => {
                Some(object)
            }
            _ => None,
        };
        if admitted && let Some(object) = through {
            self.subscript(self.occurrence(object));
        }
        admitted && (through.is_none() || self.holds(MethodFeatures::SUBSCRIPT_STORES))
    }

    /// A value stored to an element of a writable `self` or of one of its
    /// fields, where the subscripted struct declares a setter
    /// (`self.counts[i] = n`, `self.index[b] = entries^`, `self[k] = v^`).
    ///
    /// The store is a call of `__setitem__` recorded at the subscript, under
    /// the contract a sibling call has: the index and the value are its
    /// arguments ([`Self::argument`]), a closed scalar or a whole value bound
    /// by value to a parameter of exactly its own type, so an instance changes
    /// the target and, by substitution, the parameter types alone.
    fn element_store(&self, place: &Expr, value: &Expr) -> bool {
        let ExprKind::Index { object, index } = &place.kind else {
            return false;
        };
        let admitted = self.self_writable
            && (self.receiver_field(object) || self.receiver_itself(object))
            && self.argument(place, index)
            && self.argument(place, value)
            && self.facts.is_none_or(|facts| {
                self.named_contract(facts, place, object, "__setitem__")
                    .is_some_and(mojito_checked::templates::value_method_contract)
            });
        if admitted {
            self.subscript(self.occurrence(place));
        }
        admitted
            && self.holds(MethodFeatures::SIBLING_CALLS)
            && self.holds(MethodFeatures::SUBSCRIPT_STORES)
    }

    /// A closed scalar element of a writable `self` or of one of its fields,
    /// stored through the mutable reference its getter yields
    /// (`self.counts[i] += 1`, or `self.cells[i] = n` on a struct that
    /// declares no setter).
    ///
    /// The getter is a reference call, and the store records no second
    /// contract: the checker writes the computed value back through the
    /// reference. Its record is the getter's contract beside the element's
    /// type, kept apart as `augmented_subscripts`, and the reference's
    /// mutability is the receiver binding's, which no instance changes.
    fn reference_element(&self, place: &Expr) -> bool {
        self.through_reference(place)
            && self.scalar(place)
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.augmented_subscripts, self.occurrence(place))
                    .is_some_and(|store| store.inplace.is_none())
            })
    }

    /// A closed scalar element of a writable `self` or of one of its fields
    /// whose struct declares a value getter and a setter, stored augmented
    /// (`self.table[i] += 1`).
    ///
    /// The setter is the call recorded at the subscript, binding the index
    /// and the computed value, which the check keys at the subscript too.
    /// The getter is kept beside it in `augmented_subscripts` as it stands,
    /// so the subscripted value's type is closed: an instance realizes the
    /// setter alone, and reads the element through the template's getter.
    fn setter_element(&self, place: &Expr) -> bool {
        self.through_setter(place)
            && self.scalar(place)
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.augmented_subscripts, self.occurrence(place))
                    .is_some_and(|store| store.inplace.is_none())
            })
    }

    /// An element of a closed struct type stored augmented through its
    /// in-place dunder (`self.counters[i] += 3` → `__iadd__`), read through a
    /// mutable reference getter or through a value getter and a setter.
    ///
    /// The dunder is selected on the element's type, which is closed, so the
    /// contract is kept in `augmented_subscripts` as it stands. It binds the
    /// operand, a closed scalar, by value.
    fn inplace_element(&self, place: &Expr, value: &Expr) -> bool {
        (self.through_reference(place) || self.through_setter(place))
            && self.expression(value)
            && self.scalar(value)
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.augmented_subscripts, self.occurrence(place)).is_some_and(|store| {
                    !mojito_types::types::is_symbolic(&store.operand_ty)
                        && store
                            .inplace
                            .as_ref()
                            .is_some_and(mojito_checked::templates::closed_method_contract)
                })
            })
    }

    /// The subscript `place` of a writable `self` or of one of its fields,
    /// stored through the mutable reference its getter yields.
    fn through_reference(&self, place: &Expr) -> bool {
        let ExprKind::Index { object, .. } = &place.kind else {
            return false;
        };
        let id = self.occurrence(place);
        let admitted = self.self_writable
            && (self.receiver_field(object) || self.receiver_itself(object))
            && self.reference_call(place)
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.augmented_subscripts, id).is_some_and(|store| store.getter.is_none())
                    && fact_at(&facts.selected_calls, id)
                        .and_then(|call| call.reference_result.as_ref())
                        .is_some_and(|reference| {
                            reference.mutability == mojito_types::origin::Mutability::Mutable
                        })
            });
        if admitted {
            self.subscript(id);
        }
        admitted && self.holds(MethodFeatures::SUBSCRIPT_STORES)
    }

    /// The subscript `place` of a writable `self` or of one of its fields,
    /// of a closed type, read through a closed value getter and written back
    /// through a setter that takes the element by value.
    fn through_setter(&self, place: &Expr) -> bool {
        let ExprKind::Index { object, index } = &place.kind else {
            return false;
        };
        let id = self.occurrence(place);
        let admitted = self.self_writable
            && (self.receiver_field(object) || self.receiver_itself(object))
            && self.argument(place, index)
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.expression_types, self.occurrence(object))
                    .is_some_and(|ty| !mojito_types::types::is_symbolic(ty))
                    && self
                        .named_contract(facts, place, object, "__setitem__")
                        .is_some_and(mojito_checked::templates::value_method_contract)
                    && fact_at(&facts.augmented_subscripts, id)
                        .and_then(|store| store.getter.as_ref())
                        .is_some_and(mojito_checked::templates::closed_method_contract)
            });
        if admitted {
            self.subscript(id);
        }
        admitted
            && self.holds(MethodFeatures::SIBLING_CALLS)
            && self.holds(MethodFeatures::SUBSCRIPT_STORES)
    }

    /// Note that the subscript `id` is the base of a store.
    fn subscript(&self, id: OccurrenceId) {
        let mut subscripts = self.subscripts.borrow_mut();
        if !subscripts.contains(&id) {
            subscripts.push(id);
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
                        && mojito_checked::templates::value_method_contract(call)
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
        fact_at(&facts.selected_calls, self.occurrence(expr))
            .filter(|call| names_method(&call.contract.target, owner, method))
    }

    /// One argument of a method call: a closed scalar bound by value, a whole
    /// value of any type bound by value, or a place the call keeps for a
    /// `mut` or bare `ref` parameter.
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
    fn argument(&self, call: &Expr, argument: &Expr) -> bool {
        let named = match &argument.kind {
            ExprKind::Identifier(name) => {
                self.declared(name) || self.params.contains(&name.as_str())
            }
            _ => self.receiver_field(argument),
        };
        let Some(facts) = self.facts else {
            return self.expression(argument)
                || (!self.keyed && (named || self.whole_value(argument)));
        };
        let id = self.occurrence(argument);
        let contract = fact_at(&facts.selected_calls, self.occurrence(call));
        let parameter = contract.and_then(|call| {
            let bound = call.arguments.iter().find(|bound| bound.value == id)?;
            call.contract
                .arguments
                .iter()
                .find(|parameter| parameter.source == bound.source)
        });
        if !facts.call_place_uses.contains(&id) {
            let by_value = parameter.is_none_or(|parameter| !parameter.requires_place);
            if self.expression(argument) && self.scalar(argument) {
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
            return !self.keyed
                && by_value
                && parameter.is_some_and(|parameter| {
                    converted
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
    fn reference_argument(&self, argument: &Expr) -> bool {
        let admitted = match &argument.kind {
            ExprKind::Identifier(name) => self.reference_local(name),
            ExprKind::Member { .. } => self.reference_member(argument),
            _ => self.reference_call(argument),
        };
        admitted && self.holds(MethodFeatures::REFERENCE_ARGUMENTS)
    }

    /// Whether `expr` is `self.<field>` in a method body.
    fn receiver_field(&self, expr: &Expr) -> bool {
        self.receiver
            && matches!(&expr.kind, ExprKind::Member { object, .. }
                if matches!(&object.kind, ExprKind::Identifier(name) if name == "self"))
    }

    /// Whether `expr` is `self` itself in a method body.
    fn receiver_itself(&self, expr: &Expr) -> bool {
        self.receiver && matches!(&expr.kind, ExprKind::Identifier(name) if name == "self")
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

    /// Whether `expr` names a `var` local holding a whole value, which a
    /// method call or `len` reads or writes in place.
    fn value_local(&self, expr: &Expr) -> bool {
        matches!(&expr.kind, ExprKind::Identifier(name)
            if self.local_kind(name) == Some(LocalKind::Value))
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
            // A call of a method on `self`, on one of its fields, or on a
            // `var` local, passing scalars, whose recorded contract changes
            // per instance only in its target and its substituted result
            // (`closed_method_contract`).
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => {
                let on_self = matches!(&object.kind, ExprKind::Identifier(name) if name == "self");
                let sibling = ((self.receiver && (on_self || self.receiver_field(object)))
                    || (!self.keyed
                        && (self.reference_receiver(object) || self.value_local(object))))
                    && (!self.keyed || (args.is_empty() && kwargs.is_empty()))
                    && args
                        .iter()
                        .chain(kwargs.iter().map(|keyword| &keyword.value))
                        .all(|argument| self.argument(expr, argument))
                    && self
                        .facts
                        .is_none_or(|facts| self.sibling_call(facts, expr, object, method));
                sibling
                    || self.bound_dispatch(expr, object, args, kwargs)
                    || self.bound_builtin(expr, object, method, args, kwargs)
            }
            ExprKind::Prefix(_, value) => self.expression(value) && self.scalar(value),
            ExprKind::Infix(op, left, right) => {
                (self.expression(left)
                    && self.expression(right)
                    && self.scalar(left)
                    && self.scalar(right))
                    || self.operator(expr, *op, left, right)
            }
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } => {
                let id = self.occurrence(expr);
                let known = self.facts.is_none_or(|facts| {
                    facts.call_parameters.iter().any(|(call, _)| *call == id)
                        || (facts.builtin_len_calls.contains(&id) && args.len() == 1)
                });
                if self.callable_params.contains(&name.as_str()) {
                    return self.callable_call(id, param_args, args, kwargs, known);
                }
                if name == "_unqualified_type_name" || name == "repr" {
                    return self.string_builtin(id, name, param_args, args, kwargs);
                }
                // The built-in `len` reads its operand in place and realizes
                // its witness per instance, so a method may hand it a field of
                // `self` or a `var` local of any type, not only a scalar one.
                let builtin_len = self
                    .facts
                    .is_none_or(|facts| facts.builtin_len_calls.contains(&id));
                known && param_args.is_empty() && kwargs.is_empty() && args.iter().all(|argument| {
                    let held = matches!(&argument.kind, ExprKind::Identifier(name)
                            if self.reference_local(name));
                    let on_self = self.receiver
                        && matches!(&argument.kind, ExprKind::Identifier(name) if name == "self");
                    self.expression(argument)
                        || (builtin_len
                            && (held
                                || on_self
                                || self.receiver_field(argument)
                                || self.value_local(argument)))
                })
            }
            _ => false,
        }
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
    fn callable_call(
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
    fn string_builtin(
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

    /// `print(...)` as a statement of a keyed body: a checker builtin that
    /// selects no callee, over closed scalars and pack elements.
    ///
    /// What the builtin records at an argument its syntax decides (an
    /// unconsumed temporary, a literal's materialization); what it proves,
    /// that the argument is `Writable`, the instance proves again at its own
    /// type ([`Checker::realize_print_call`]). A declaration of that name
    /// would record call parameters and a binding, and is not this.
    fn print_call(&self, expr: &Expr) -> bool {
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
        let admitted = name == "print"
            && !declared
            && param_args.is_empty()
            && kwargs.is_empty()
            && args.iter().all(|argument| {
                (self.expression(argument) && self.scalar(argument)) || self.pack_element(argument)
            });
        if admitted {
            let mut calls = self.print_calls.borrow_mut();
            if !calls.contains(&id) {
                calls.push(id);
            }
        }
        admitted
    }

    /// `pack[i]`: an element of a pack-typed parameter at the innermost
    /// `comptime for` variable.
    ///
    /// The template typed the element once, as the dependent `Ts[i]` over
    /// the loop's own binder, and recorded nothing else there: no place, no
    /// adjustment, no borrow. The elaborator folds `i` to the iteration's
    /// literal in each unrolled copy, which the instance reads back to fix
    /// the element ([`Checker::realize_instance_facts`]).
    fn pack_element(&self, expr: &Expr) -> bool {
        let ExprKind::Index { object, index } = &expr.kind else {
            return false;
        };
        let named = matches!(&object.kind, ExprKind::Identifier(name)
                if self.packs.contains(&name.as_str()))
            && matches!(&index.kind, ExprKind::Identifier(name)
                if self.loop_vars.borrow().last() == Some(name));
        named
            && self.facts.is_none_or(|facts| {
                let id = self.occurrence(expr);
                let ty = fact_at(&facts.expression_types, id);
                let element = matches!(
                    ty,
                    Some(Ty::Dependent(dependent))
                        if dependent.pack_element().is_some_and(|(_, index)| {
                            matches!(index.kind(), mojito_types::param_expr::ParamKind::DeclRef(_))
                        })
                );
                // The element is a place of the collector, read where it
                // lies.
                element
                    && fact_at(&facts.expression_place_types, id)
                        .is_none_or(|place| Some(place) == ty)
                    && fact_at(&facts.operation_adjustments, id).is_none()
                    && !facts.call_place_uses.contains(&id)
                    && !facts.borrowed_read_call_places.contains(&id)
            })
    }

    /// A method call on a place whose type is a bare struct parameter, which
    /// the template proves through the parameter's bound and an instance
    /// re-selects on its own type ([`Checker::realize_bound_dispatch`]).
    ///
    /// The receiver is a place, so the template recorded at the call either
    /// the abstract contract (`__trait_dispatch.…`) or, for `write_to`, the
    /// inverted write, and nothing that depends on the receiver's type. Each
    /// argument is a closed scalar or a named place of a bare parameter type
    /// handed to a bounded `mut`/`ref` parameter of the requirement, whose
    /// facts (a kept place, its generation refresh) the convention decides.
    /// An instance's own check reads a built-in receiver's place, feeds a
    /// leaf to the hasher, or selects the struct's own method, each from the
    /// type alone.
    fn bound_dispatch(
        &self,
        expr: &Expr,
        object: &Expr,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> bool {
        // A receiver a `var self` requirement consumes is a named place's
        // `^` transfer, which records the move at itself.
        let consumed = matches!(&object.kind, ExprKind::Transfer(inner)
            if matches!(inner.kind, ExprKind::Identifier(_)))
            && self.whole_value(object);
        let place = match &object.kind {
            ExprKind::Identifier(name) => {
                self.params.contains(&name.as_str()) || self.local_kind(name).is_some()
            }
            ExprKind::Member { .. } => self.receiver_field(object) || self.reference_member(object),
            ExprKind::Index { .. } => self.slot(object),
            _ => consumed,
        };
        let named = |argument: &Expr| {
            matches!(&argument.kind, ExprKind::Identifier(name)
                if self.params.contains(&name.as_str()) || self.declared(name))
        };
        let shape = !self.keyed
            && place
            && kwargs.is_empty()
            && args.iter().all(|argument| {
                (self.expression(argument) && self.scalar(argument)) || named(argument)
            });
        let admitted = shape
            && self.facts.is_none_or(|facts| {
                let id = self.occurrence(expr);
                let Some(receiver @ Ty::Param { .. }) =
                    fact_at(&facts.expression_types, self.occurrence(object))
                else {
                    return false;
                };
                // A named place handed to a bounded parameter of the
                // requirement: its own bounds prove the parameter's, or its
                // type is closed, the same in every instance, and the
                // template's check proved it against the bound.
                let bounded = |argument: &Expr, parameter: &Ty| {
                    let ty = fact_at(&facts.expression_types, self.occurrence(argument));
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
                    let Some(bound) = call
                        .arguments
                        .iter()
                        .find(|bound| bound.source == parameter.source)
                    else {
                        return false;
                    };
                    let Some(argument) = args
                        .iter()
                        .find(|argument| self.occurrence(argument) == bound.value)
                    else {
                        return false;
                    };
                    if !parameter.requires_place {
                        return self.expression(argument) && self.scalar(argument);
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
                let contract = if consumed {
                    mojito_checked::templates::consuming_method_contract(call)
                } else {
                    mojito_checked::templates::closed_method_contract(call)
                };
                mojito_symbol::symbol::is_trait_dispatch_symbol(&call.contract.target)
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
                        Some(mojito_ast::ast::ArgConvention::Var) => {
                            consumed && call.invalidations.is_empty()
                        }
                        Some(_) => false,
                    }
                    && (call.contract.result_ty == *receiver
                        || !mojito_types::types::is_symbolic(&call.contract.result_ty))
                    && call.contract.arguments.len() == args.len()
                    && call.contract.arguments.iter().all(kept_argument)
            });
        admitted && self.holds(MethodFeatures::BOUND_DISPATCH)
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
    fn bound_builtin(
        &self,
        expr: &Expr,
        object: &Expr,
        method: &str,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> bool {
        let receiver = matches!(&object.kind, ExprKind::Identifier(name)
            if self.params.contains(&name.as_str()) || self.local_kind(name).is_some());
        let (builtin, bound) = match (method, args.len()) {
            ("update", 1) => (BoundBuiltin::Update, "Hasher"),
            ("_update_with_simd", 1) => (BoundBuiltin::UpdateSimd, "Hasher"),
            ("write", 1..) => (BoundBuiltin::Write, "Writer"),
            _ => return false,
        };
        let id = self.occurrence(expr);
        let admitted = !self.keyed
            && receiver
            && kwargs.is_empty()
            && args.iter().all(|argument| self.sink_argument(argument))
            && self.facts.is_none_or(|facts| {
                matches!(fact_at(&facts.expression_types, self.occurrence(object)),
                    Some(Ty::Param { bounds, .. }) if bounds.iter().any(|carried| carried == bound))
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
                self.expression(argument)
                    && (self.scalar(argument) || name == "repr" || name == "_unqualified_type_name")
            }
            ExprKind::Identifier(name) => {
                self.params.contains(&name.as_str()) || self.local_kind(name).is_some()
            }
            ExprKind::Member { .. } => {
                self.receiver_field(argument) || self.reference_member(argument)
            }
            ExprKind::Index { .. } => self.slot(argument) || self.reference_call(argument),
            ExprKind::MethodCall { .. } => {
                self.reference_call(argument)
                    || (self.expression(argument) && self.scalar(argument))
            }
            _ => self.expression(argument) && self.scalar(argument),
        }
    }

    /// An operator over two places of one type that mentions a struct
    /// parameter, which dispatches on the type alone.
    ///
    /// The template, whose type is symbolic, recorded nothing at it: a bound
    /// (or a `where` assumption) proves the operator and the operands are
    /// read where they lie. An instance decides the same operator on its
    /// substituted type ([`Checker::realize_operator`]), and each operand
    /// is a place, so the instance's check reads it where it lies too.
    ///
    /// The result is the operand's own type for an arithmetic, bitwise, or
    /// shift operator, so it is not a scalar under every instance;
    /// [`Self::operator_value`] is what admits it where a temporary may go.
    fn operator(
        &self,
        expr: &Expr,
        op: mojito_ast::ast::InfixOp,
        left: &Expr,
        right: &Expr,
    ) -> bool {
        let place = |operand: &Expr| match &operand.kind {
            ExprKind::Identifier(name) => {
                self.params.contains(&name.as_str()) || self.local_kind(name).is_some()
            }
            ExprKind::Member { .. } => {
                self.receiver_field(operand) || self.reference_member(operand)
            }
            ExprKind::Index { .. } => self.slot(operand),
            _ => false,
        };
        let id = self.occurrence(expr);
        let admitted = !self.keyed
            && operator_dispatch(op)
            && place(left)
            && place(right)
            && self.facts.is_none_or(|facts| {
                let same = fact_at(&facts.expression_types, self.occurrence(left))
                    .zip(fact_at(&facts.expression_types, self.occurrence(right)))
                    .is_some_and(|(left, right)| {
                        left == right && mojito_types::types::is_symbolic(left)
                    });
                same && !facts.overload_targets.iter().any(|(site, _)| *site == id)
                    && !facts
                        .operation_adjustments
                        .iter()
                        .any(|(site, _)| *site == id)
                    && !facts
                        .copy_place_value_uses
                        .contains(&self.occurrence(right))
            });
        if admitted {
            let mut operators = self.operators.borrow_mut();
            if !operators.contains(&id) {
                operators.push(id);
            }
        }
        admitted && self.holds(MethodFeatures::OPERATOR_DISPATCH)
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

/// A body's struct applications as one canonical ordered set, since a derived
/// bundle is compared with an inferred one.
///
/// Recording is idempotent, and a clone check reaches a retargeted receiver's
/// application twice. Substitution collapses applications too: `Bag[T]` and
/// `Bag[Int]` are two of a template's and one of the instance's, so a derived
/// bundle must reduce exactly as its own check would.
fn sorted_applications(
    mut applications: Vec<(String, Vec<mojito_types::types::TyArg>)>,
) -> Vec<(String, Vec<mojito_types::types::TyArg>)> {
    applications.sort_by_cached_key(|(name, arguments)| {
        let arguments: Vec<String> = arguments.iter().map(ToString::to_string).collect();
        (name.clone(), arguments)
    });
    applications.dedup();
    applications
}

/// The operators [`BodyShape::operator`] admits: every one a trait names, so
/// that a bound on the operand's parameter proves it — equality and ordering,
/// which yield `Bool`, and the arithmetic, bitwise, and shift operators, whose
/// result is the operand's own type (`Float64` for `/`).
const fn operator_dispatch(op: mojito_ast::ast::InfixOp) -> bool {
    super::builtins::infix_operation_trait(op).is_some()
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

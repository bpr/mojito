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

use super::annotations::{existential_binder, simd_binder_slots, simd_binder_view};
use super::body_carry::ObservedEffects;
use super::builtins::{SIMD_WILDCARD_BOUND, simd_wildcard_binder};
use super::{Checker, EffectRead, callable_contract_target, callable_lowered_name};
use mojito_ast::ast::{CaptureKind, Expr, ExprKind, Stmt, StmtKind};
use mojito_checked::templates::{
    BoundBuiltin, CallParameterFact, CheckedBodyFacts, CheckedTemplate, FactTable, FoldedLiteral,
    IncompleteReason, InstanceName, InstanceTrace, MethodFeatures, OccurrenceId,
    TemplateArgumentBoundary, TemplateAugmentedSubscript, TemplateCallContract,
    TemplateCallResultOrigin, TemplateCallTransfer, TemplateClass, TemplateCoverage,
    TemplateEffectSource, TemplateId, TemplateInvalidation, TemplateObligation, TemplateOrigin,
    TemplateOwner, TemplatePlace, TemplateProducer, TemplateReference, TemplateTransferDest,
    TemplateTransferEffect, TemplateTransferSource, TypedOrigins, TypedTable, WithForm,
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
mod comprehensions;
mod constructions;
mod iterations;
mod nested_defs;
mod tuple_unpacks;

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
    /// How many of those unkeyed entries the body's nested `def` statements
    /// key, which a recipe accounts for (`nested_def_entries`).
    nested_defs: [usize; UNKEYED_STORES],
    /// The length of the hash-leaf demand log.
    hash_leaf_demands: usize,
    /// The transferred-origin store as the body found it: what a replay
    /// merges is keyed by owner, so growth is told entry by entry.
    transferred: HashMap<OwnerId, Vec<mojito_types::origin::Origin>>,
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

/// The transfers one body inference replayed, in template-local terms
/// (`CheckedBodyFacts::{call_transfers, transferred_origins,
/// transfer_effects, transfer_reads}`).
struct BodyTransfers {
    call_transfers: Vec<(OccurrenceId, Vec<TemplateCallTransfer>)>,
    transferred_origins: Vec<(TemplateOwner, Vec<TemplateTransferSource>)>,
    transfer_effects: Vec<TemplateTransferEffect>,
    transfer_reads: Vec<(String, Vec<mojito_types::types::TransferEffect>)>,
}

/// One declaration body as the template mechanism sees it.
#[derive(Clone)]
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

#[derive(Clone, Copy)]
enum BodyDeclaration<'a> {
    Def(&'a Stmt),
    Method(&'a mojito_ast::ast::Method),
}

impl BodyDeclaration<'_> {
    /// The declaration's own compile-time parameters.
    fn type_params(&self) -> Option<&[mojito_ast::ast::TypeParam]> {
        match self {
            Self::Def(Stmt {
                kind: StmtKind::Def { type_params, .. },
                ..
            }) => Some(type_params),
            Self::Def(_) => None,
            Self::Method(method) => Some(&method.type_params),
        }
    }
}

/// The facts a body takes instead of being inferred, with its occurrence
/// spans by the identity each kept from the template, and the desugar of
/// each of its `with` statements, built from the template's form.
struct DerivedBody {
    facts: CheckedBodyFacts,
    spans: HashMap<OccurrenceId, SourceSpan>,
    desugars: HashMap<SourceSpan, super::with_stmt::WithDesugar>,
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
    /// The struct and `[...]` arguments a method call's receiver applies,
    /// where it is a type application (`Pair[Self.T].pick(v, 1)`).
    type_receiver: Option<(String, Vec<mojito_ast::ast::ParamArg>)>,
    /// An admitted operator's kind, its operand occurrences, and whether
    /// its right operand is a place, which a consuming dunder copies.
    operator: Option<(mojito_ast::ast::InfixOp, SyntaxId, SyntaxId, bool)>,
    /// A unary `-` or `~` and its operand occurrence.
    prefix: Option<(mojito_ast::ast::PrefixOp, SyntaxId)>,
    /// An augmented assignment's place and value occurrences.
    augmented: Option<(SyntaxId, SyntaxId)>,
    /// Whether this is a `^` transfer.
    transfer: bool,
    /// How the clone check ranks an overload member it is handed to.
    ranking: ArgumentRanking,
    /// The value of an integer or `Bool` literal, which may be a folded
    /// compile-time value.
    literal: Option<mojito_types::ct::CtValue>,
    /// A subscript's index occurrence and value, when the elaborator folded
    /// it to an integer literal: the iteration a pack element was copied for.
    folded_index: Option<(SyntaxId, i64)>,
    /// A `SIMD[…](…)` construction's integer dimension occurrences.
    dimensions: Vec<SyntaxId>,
    /// A direct call's compile-time arguments, each the value of an integer
    /// or `Bool` literal the elaborator folded there, or `None`.
    compile_time_literals: Vec<Option<mojito_types::ct::CtValue>>,
    /// A literal's kind.
    literal_kind: Option<LiteralKind>,
    /// What the elaborator wrote here for a vector the template spelled
    /// otherwise ([`fold_vector_values`]).
    vector_fold: Option<VectorFold>,
}

/// Syntax the elaborator writes for a vector the template spelled otherwise.
enum VectorFold {
    /// A dimension of a vector alias's construction (`U256(…)` as
    /// `SIMD[DType.uint64, 4](…)`), which no check types.
    Dimension,
    /// The construction a struct's vector value binder folded to
    /// (`Self.key`), under the name's identity.
    Construction,
    /// One lane of such a construction, a literal the template never had.
    Lane(LiteralKind),
    /// The `DType.<member>` constant a method's own `DType` binder folded to
    /// (`Scalar[dt]`) under the name's identity, which the instance's own
    /// check skips as it skips any constant it wrote.
    Constant,
}

/// The kind of a literal, which decides its own type.
#[derive(Clone, Copy)]
enum LiteralKind {
    Int,
    Float,
    Bool,
}

impl LiteralKind {
    const fn ty(self) -> Ty {
        match self {
            Self::Int => Ty::IntLiteral,
            Self::Float => Ty::FloatLiteral,
            Self::Bool => Ty::Bool,
        }
    }
}

/// What the clone check's overload ranking reads of an argument's
/// expression, beside its type.
#[derive(Clone, Copy, Default)]
struct ArgumentRanking {
    /// Whether its checked type is the same under any expected type
    /// ([`context_free`]).
    context_free: bool,
    /// Whether it hands over a value the callee may own rather than a place
    /// the callee borrows or copies.
    owned: bool,
}

/// What an instance's arguments stand for in its template's facts.
struct InstanceSubstitution {
    /// Each baked type binder's checked type.
    types: TySubst,
    /// Each baked type pack's element types.
    packs: HashMap<mojito_types::param_expr::ParamId, Vec<Ty>>,
    /// Each wildcard vector binder's lane-shaped view (`simd_binder_view`)
    /// with the closed vector type the clone bakes the binder to, which a
    /// retained type equal to the whole view takes as it stands
    /// (`fold_binder_views`): the native `UInt` has no lane form, so
    /// folding its slots alone would spell it `SIMD[DType.uint64, 1]`.
    views: Vec<(Ty, Ty)>,
    /// Each folded value binder's value, which a lane dtype or width the
    /// template left open takes (`SIMD[DType.int32, w]`).
    values: Vec<(mojito_types::param_expr::ParamId, mojito_types::ct::CtValue)>,
    /// The method's own value binders a per-instantiation clone keeps
    /// symbolic (`VALUE_BINDERS`): an occurrence reading one is bound to
    /// the clone's own compile-time parameter of that name.
    kept_values: Vec<String>,
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
    simd_to_bits: Vec<(OccurrenceId, OccurrenceId)>,
    simd_casts: Vec<(OccurrenceId, OccurrenceId)>,
    simd_lengths: Vec<(OccurrenceId, OccurrenceId)>,
    pack_relocations: Vec<mojito_checked::templates::PackRelocation>,
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
            residual_binders: !type_params.iter().all(clone_origin_binder),
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
            self.classify_params(&self.method_binder_owner(owner, m), &m.type_params)?;
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
            residual_binders: !m.type_params.iter().all(clone_origin_binder),
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
    /// validation stand (`TransferFrame::keeps_symbolic_selection`): a
    /// member of a struct specialized whole always keeps them, as does a
    /// body no trace covers (a seam without the elaborator's traces) whose
    /// name marks it a clone; any other traced body keeps them when its
    /// template was validated.
    fn mark_symbolic_selection(&self, site: &BodySite<'_>) {
        let keeps = {
            let catalog = self.template_catalog.borrow();
            let whole_struct = site
                .instance
                .owner
                .as_deref()
                .is_some_and(|owner| catalog.generated_struct(owner));
            match catalog.trace(&site.instance) {
                Some(trace) if !whole_struct => catalog.validated(&trace.template),
                _ => site.display.contains('$'),
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
        if let Some(DerivedBody {
            facts,
            spans,
            desugars,
        }) = &derived
            && !verify
        {
            self.with_desugars.borrow_mut().extend(
                desugars
                    .iter()
                    .map(|(span, desugar)| (span.clone(), desugar.clone())),
            );
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
        // The first copy of a shared stub checked is that stub's template.
        let stub_template = (generated && derived.is_none())
            .then(|| self.instance_trace(site))
            .flatten()
            .filter(|trace| {
                trace.shared_stub
                    && self
                        .template_catalog
                        .borrow()
                        .template(&trace.template)
                        .is_none()
            })
            .map(|trace| trace.template);
        let census = site.participates && !decls.is_empty() && timing::enabled();
        let baseline = (derived.is_some()
            || admitted
            || stub_template.is_some()
            || census
            || timing::notes_enabled())
        .then(|| self.body_fact_baseline(body));
        Self::count_body_inference(BodyClass::of(generated, !decls.is_empty()), || name.clone());
        // A nested body records what it reads for its enclosing body too.
        let queries =
            baseline.is_some() || matches!(self.effect_query_frames.borrow().last(), Some(Some(_)));
        let applications = baseline.is_some()
            || matches!(
                self.struct_application_frames.borrow().last(),
                Some(Some(_))
            );
        self.effect_query_frames
            .borrow_mut()
            .push(queries.then(Vec::new));
        self.struct_application_frames
            .borrow_mut()
            .push(applications.then(Vec::new));
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
        // What a nested body read and reached is the enclosing body's too.
        if let Some(Some(outer)) = self.effect_query_frames.borrow_mut().last_mut() {
            outer.extend(reads.effect_queries.iter().cloned());
        }
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
        if let Some(DerivedBody {
            facts: realized, ..
        }) = &derived
        {
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
        } else if let Some(template_id) = stub_template {
            let shape = self.certificate(site, None).0;
            let stub = BodySite {
                template_id,
                ..site.clone()
            };
            self.record_template(&stub, param_owners, &baseline, &reads, shape);
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
        self.tuple_unpack_sources.borrow_mut().remove(span);
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
        self.comprehension_iterables.borrow_mut().remove(span);
        self.nested_def_params.borrow_mut().remove(span);
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

    /// Whether one body inference replayed or published a transfer residue
    /// no derivation accounts for: a named callable's effects behind a
    /// call-through residue, a function value's baked effects, or an effect
    /// or a replayed summary naming a captured binding. A callee's summary
    /// replayed on the call's own receiver and arguments is retained instead
    /// ([`TemplateObligation::ReplayedTransfers`]), and a call-through residue
    /// the body reads or publishes is kept too
    /// ([`TemplateObligation::CallThroughResidue`]).
    fn transfer_residue(&self, reads: &BodyReads) -> bool {
        use mojito_types::origin::SigOrigin;
        fn bound(origin: &SigOrigin) -> bool {
            match origin {
                SigOrigin::Bound(_) => true,
                SigOrigin::Projected(base, _) => bound(base),
                SigOrigin::Union(members) => members.iter().any(bound),
                _ => false,
            }
        }
        let bound_effect = |effect: &mojito_types::types::TransferEffect| {
            bound(&effect.dest) || bound(&effect.src)
        };
        reads.effect_queries.iter().any(|(_, read)| match read {
            EffectRead::Residue => true,
            EffectRead::Transfers(effects) => effects.iter().any(bound_effect),
            _ => false,
        }) || self
            .transfer_frames
            .borrow()
            .last()
            .is_some_and(|frame| frame.effects.iter().any(bound_effect))
    }

    /// The transfers one body inference replayed, in template-local terms:
    /// the call transfers at its occurrences, the origins the replays merged
    /// (the store's growth over the baseline), the effects on the body's own
    /// frame, and the summaries read. Each source is judged by the type of
    /// the binding it is rooted at, a frame effect's union source member by
    /// member; an effect with a source member that is not a parameter or the
    /// receiver, and a merge into a binding outside the body, have no such
    /// judgment and refuse.
    fn body_transfers(
        &self,
        occurrences: &[Occurrence],
        baseline: &BodyFactBaseline,
        reads: &BodyReads,
        param_owners: &BodyParams,
        local_owner: &dyn Fn(OwnerId) -> Result<TemplateOwner, IncompleteReason>,
        local_place: &dyn Fn(
            &mojito_types::origin::OriginPlace,
        ) -> Result<TemplatePlace, IncompleteReason>,
    ) -> Result<BodyTransfers, IncompleteReason> {
        use mojito_checked::checked::CheckedTransferDest;
        use mojito_types::origin::{Origin, SigOrigin};
        let refuse = IncompleteReason::UnkeyedFact("transfer effects");
        let source = |origin: &Origin| {
            Ok::<_, IncompleteReason>(TemplateTransferSource {
                origin: template_origin(origin, local_place)?,
                root_ty: match origin {
                    Origin::Place(place) => self.owner_binding_type(place.root),
                    _ => None,
                },
            })
        };
        let recorded = self.call_transfers.borrow();
        let call_transfers = occurrences
            .iter()
            .filter_map(|occurrence| {
                recorded
                    .get(&occurrence.span)
                    .map(|transfers| (occurrence.id, transfers))
            })
            .map(|(id, transfers)| {
                transfers
                    .iter()
                    .map(|transfer| {
                        let dest = match transfer.dest {
                            CheckedTransferDest::Receiver => TemplateTransferDest::Receiver,
                            CheckedTransferDest::Argument(index) => {
                                TemplateTransferDest::Argument(index)
                            }
                            CheckedTransferDest::Owner(_) => return Err(refuse.clone()),
                        };
                        Ok(TemplateCallTransfer {
                            dest,
                            dest_path: transfer.dest_path.clone(),
                            sources: transfer
                                .sources
                                .iter()
                                .map(source)
                                .collect::<Result<_, _>>()?,
                            mutable: transfer.mutable,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(|transfers| (id, transfers))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let merged = self.transferred_origins.borrow();
        let mut transferred_origins = merged
            .iter()
            .filter_map(|(owner, origins)| {
                let before = baseline.transferred.get(owner);
                let grown: Vec<&Origin> = origins
                    .iter()
                    .filter(|origin| before.is_none_or(|before| !before.contains(origin)))
                    .collect();
                (!grown.is_empty()).then_some((*owner, grown))
            })
            .map(|(owner, grown)| {
                let dest = match local_owner(owner)? {
                    dest @ (TemplateOwner::Param(_)
                    | TemplateOwner::Receiver
                    | TemplateOwner::Local(_)) => dest,
                    TemplateOwner::Global(_) | TemplateOwner::CompileTimeParam(_) => {
                        return Err(IncompleteReason::ExternalBinding);
                    }
                };
                let sources = grown
                    .into_iter()
                    .map(source)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok((dest, sources))
            })
            .collect::<Result<Vec<_>, _>>()?;
        transferred_origins
            .sort_by(|(left, _), (right, _)| format!("{left:?}").cmp(&format!("{right:?}")));
        let frames = self.transfer_frames.borrow();
        if frames.last().is_some_and(|frame| frame.latent_escapes) {
            return Err(refuse);
        }
        let param_borrowed = frames
            .last()
            .map(|frame| frame.param_borrowed.as_slice())
            .unwrap_or_default();
        let transfer_effects = frames
            .last()
            .map(|frame| frame.recorded.as_slice())
            .unwrap_or_default()
            .iter()
            .map(|(effect, latent)| {
                let sources = sig_origin_members(&effect.src)
                    .iter()
                    .map(|member| {
                        let (owner, is_place) = match member {
                            SigOrigin::Self_ => (param_owners.receiver, true),
                            SigOrigin::Param(index) => (
                                param_owners.runtime.get(*index).copied().flatten(),
                                param_borrowed.get(*index).copied().unwrap_or(false),
                            ),
                            _ => (None, false),
                        };
                        owner
                            .and_then(|owner| self.owner_binding_type(owner))
                            .map(|ty| TemplateEffectSource { ty, is_place })
                            .ok_or_else(|| refuse.clone())
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(TemplateTransferEffect {
                    effect: effect.clone(),
                    sources,
                    latent: latent.clone(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut transfer_reads: Vec<(String, Vec<mojito_types::types::TransferEffect>)> =
            Vec::new();
        for (callee, read) in &reads.effect_queries {
            if let EffectRead::Transfers(effects) = read
                && !transfer_reads.iter().any(|(read, _)| read == callee)
            {
                transfer_reads.push((callee.clone(), effects.clone()));
            }
        }
        transfer_reads.sort_by(|(left, _), (right, _)| left.cmp(right));
        Ok(BodyTransfers {
            call_transfers,
            transferred_origins,
            transfer_effects,
            transfer_reads,
        })
    }

    /// The literals a body materialized at, or combined with, a struct's
    /// symbolic lane type (`clamped = 0`, `remaining - 1` over a
    /// `Scalar[Self.dtype]`), realized at the instance's folded lane.
    ///
    /// A materialization the template recorded at the symbolic lane has its
    /// target substituted; the instance repeats the fit the template left to
    /// it. An operator over a lane-typed operand and a literal records
    /// nothing while the lane is a vector, and so does an augmented
    /// assignment of one to a lane-typed place, but a lane folding to a native
    /// scalar (`Scalar[DType.int]` is `Int`) takes the numeric path, which
    /// materializes the literal at that scalar, as the clone check does.
    fn realize_lane_literals(
        &self,
        template: &CheckedBodyFacts,
        facts: &mut CheckedBodyFacts,
        occurrences: &[Occurrence],
    ) -> Result<(), &'static str> {
        use mojito_checked::checked::SemanticAdjustment;
        let fits = |id: OccurrenceId, target: &Ty| {
            occurrences
                .iter()
                .find(|occurrence| occurrence.id == id)
                .and_then(|occurrence| occurrence.literal.clone())
                .map(|value| match value {
                    mojito_types::ct::CtValue::Int(value) => {
                        mojito_types::ct::CtValue::IntLiteral(value.into())
                    }
                    value => value,
                })
                .is_some_and(|value| self.literal_value_fits_target(&value, target))
        };
        for (id, adjustment) in &template.operation_adjustments {
            if let SemanticAdjustment::MaterializeLiteral(target) = adjustment
                && mojito_types::types::is_symbolic(target)
            {
                let Some(SemanticAdjustment::MaterializeLiteral(realized)) =
                    fact_at(&facts.operation_adjustments, *id)
                else {
                    return Err("a lane literal's materialization is not realized");
                };
                if !fits(*id, realized) {
                    return Err("a literal does not fit the instance's lane");
                }
            }
        }
        let open_lane = |id: OccurrenceId| {
            fact_at(&template.expression_types, id).is_some_and(|ty| {
                matches!(ty, Ty::Simd { .. }) && mojito_types::types::is_symbolic(ty)
            })
        };
        let mut realized = Vec::new();
        for occurrence in occurrences {
            let Some((left, right)) = occurrence
                .operator
                .map(|(_, left, right, _)| (left, right))
                .or(occurrence.augmented)
            else {
                continue;
            };
            let operand = |syntax| OccurrenceId {
                syntax,
                copy: occurrence.id.copy,
            };
            for (lane, literal) in [(left, right), (right, left)] {
                let (lane, literal) = (operand(lane), operand(literal));
                let literal_typed = fact_at(&template.expression_types, literal)
                    .is_some_and(|ty| matches!(ty, Ty::IntLiteral | Ty::FloatLiteral));
                if !open_lane(lane)
                    || !literal_typed
                    || fact_at(&template.operation_adjustments, literal).is_some()
                {
                    continue;
                }
                let Some(native) = fact_at(&facts.expression_types, lane)
                    .filter(|ty| matches!(ty, Ty::Int | Ty::UInt | Ty::Float64))
                else {
                    continue;
                };
                if !fits(literal, native) {
                    return Err("a literal does not fit the instance's lane");
                }
                realized.push((
                    literal,
                    SemanticAdjustment::MaterializeLiteral(native.clone()),
                ));
            }
        }
        if !realized.is_empty() {
            facts.operation_adjustments.extend(realized);
            let order = |id: OccurrenceId| occurrences.iter().position(|found| found.id == id);
            facts
                .operation_adjustments
                .sort_by_key(|(id, _)| order(*id));
        }
        Ok(())
    }

    /// [`TemplateObligation::ReplayedTransfers`]: replay the template's
    /// transfers for the instance.
    ///
    /// A source stands where its binding's substituted type may still carry
    /// a loan, and vanishes where it is plain data, which is what the
    /// instance's own check records for such a binding. A call whose realized
    /// callee publishes an empty summary records nothing, so every source
    /// there must vanish; one whose summary is the one the template read
    /// replays the kept sources; any other summary refuses. A read whose
    /// realized summary is empty is observed empty, as a plain-data clone's
    /// check observes it; any other read must find the summary it recorded.
    fn realize_transfers(
        &self,
        facts: &mut CheckedBodyFacts,
        template: &CheckedBodyFacts,
        substitute: &dyn Fn(&Ty) -> Ty,
    ) -> Result<(), &'static str> {
        let kept = |source: &TemplateTransferSource| {
            source
                .root_ty
                .as_ref()
                .is_none_or(|ty| !self.loan_free(&substitute(ty)))
        };
        let realized_source = |source: &TemplateTransferSource| TemplateTransferSource {
            origin: source.origin.clone(),
            root_ty: source.root_ty.as_ref().map(substitute),
        };
        let summaries = self.transfer_effects.borrow();
        let mut call_transfers = Vec::new();
        for (id, transfers) in &template.call_transfers {
            let callee = facts
                .selected_calls
                .iter()
                .find(|(call, _)| call == id)
                .map(|(_, call)| call.contract.target.as_str())
                .or_else(|| {
                    facts
                        .overload_targets
                        .iter()
                        .find(|(call, _)| call == id)
                        .map(|(_, target)| target.as_str())
                })
                .ok_or("a replayed transfer's call names no realized callee")?;
            let read = facts
                .transfer_reads
                .iter()
                .find(|(read, _)| read == callee)
                .map(|(_, effects)| effects)
                .ok_or("a replayed transfer's callee summary was not read")?;
            let realized: Vec<TemplateCallTransfer> = transfers
                .iter()
                .filter_map(|transfer| {
                    let sources: Vec<_> = transfer
                        .sources
                        .iter()
                        .filter(|source| kept(source))
                        .map(realized_source)
                        .collect();
                    (!sources.is_empty()).then(|| TemplateCallTransfer {
                        sources,
                        ..transfer.clone()
                    })
                })
                .collect();
            let summary = summaries.get(callee).map(Vec::as_slice).unwrap_or_default();
            if summary.is_empty() {
                if !realized.is_empty() {
                    return Err("a transfer survives a callee whose summary is empty");
                }
            } else if summary != read.as_slice() {
                return Err("a callee's transfer summary is no longer the one the template read");
            }
            if !realized.is_empty() {
                call_transfers.push((*id, realized));
            }
        }
        facts.call_transfers = call_transfers;
        facts.transferred_origins = template
            .transferred_origins
            .iter()
            .filter_map(|(dest, sources)| {
                let sources: Vec<_> = sources
                    .iter()
                    .filter(|source| kept(source))
                    .map(realized_source)
                    .collect();
                (!sources.is_empty()).then(|| (dest.clone(), sources))
            })
            .collect();
        // A source member whose binding is plain data vanishes, as the
        // instance's own check abstracts only the origins a loan-carrying
        // value holds: an effect left with no member vanishes, and one left
        // with a single member takes that member's own place flag, or
        // vanishes where it is the destination. A latent effect is published
        // where its stored type now carries a loan, stays latent while that
        // type is still symbolic, and vanishes otherwise, as the instance's
        // own check of the store decides.
        facts.transfer_effects = template
            .transfer_effects
            .iter()
            .filter_map(|effect| {
                let (members, sources): (Vec<_>, Vec<_>) = sig_origin_members(&effect.effect.src)
                    .into_iter()
                    .zip(effect.sources.iter().map(|source| TemplateEffectSource {
                        ty: substitute(&source.ty),
                        is_place: source.is_place,
                    }))
                    .filter(|(_, source)| !self.loan_free(&source.ty))
                    .unzip();
                let src_is_place = match sources.as_slice() {
                    [only] => only.is_place,
                    _ => false,
                };
                let src = mojito_types::origin::SigOrigin::union(members);
                (!sources.is_empty() && src != effect.effect.dest).then(|| TemplateTransferEffect {
                    effect: mojito_types::types::TransferEffect {
                        src,
                        src_is_place,
                        ..effect.effect.clone()
                    },
                    sources,
                    latent: effect.latent.clone(),
                })
            })
            .filter_map(|effect| {
                let latent = match effect.latent.as_ref().map(substitute) {
                    None => None,
                    Some(stored) if self.type_carries_loans(&stored) => None,
                    Some(stored)
                        if mojito_types::types::is_symbolic(&stored)
                            && self.type_may_carry_loans(&stored) =>
                    {
                        Some(stored)
                    }
                    Some(_) => return None,
                };
                Some(TemplateTransferEffect { latent, ..effect })
            })
            .collect();
        let (empty, replayed): (Vec<_>, Vec<_>) = std::mem::take(&mut facts.transfer_reads)
            .into_iter()
            .partition(|(callee, _)| summaries.get(callee).is_none_or(Vec::is_empty));
        if replayed
            .iter()
            .any(|(callee, read)| summaries.get(callee) != Some(read))
        {
            return Err("a callee's transfer summary is no longer the one the template read");
        }
        facts.transfer_reads = replayed;
        for (callee, _) in empty {
            if !facts.effect_free_callees.contains(&callee) {
                facts.effect_free_callees.push(callee);
            }
        }
        Ok(())
    }

    /// The type of the binding an owner identifies, from the scope that
    /// registered it.
    fn owner_binding_type(&self, owner: OwnerId) -> Option<Ty> {
        self.owner_scopes
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, scope)| {
                scope
                    .iter()
                    .find(|(_, candidate)| **candidate == owner)
                    .and_then(|(name, _)| self.scopes.get(index)?.get(name).cloned())
            })
    }

    fn body_fact_baseline(&self, body: &[Stmt]) -> BodyFactBaseline {
        BodyFactBaseline {
            tables: FactTable::ALL.map(|table| self.span_table(table).entries()),
            unkeyed: self.unkeyed_fact_entries(),
            nested_defs: self.nested_def_entries(body),
            hash_leaf_demands: self.hash_leaf_demands.borrow().len(),
            transferred: (**self.transferred_origins.borrow()).clone(),
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
    ) -> Option<DerivedBody> {
        let name = &site.display;
        let template = site.role == BodyRole::Template;
        let trace = if template {
            InstanceTrace {
                template: site.template_id.clone(),
                type_bindings: Vec::new(),
                value_bindings: Vec::new(),
                pack_bindings: Vec::new(),
                residual: Vec::new(),
                shared_stub: false,
            }
        } else {
            self.instance_trace(site)?
        };
        let refusals = RefCell::new(Vec::new());
        let refuse = |reason: &'static str| {
            if template {
                timing::count("template_bodies.reuse_ineligible", 1);
                timing::note("template_bodies.reuse_ineligible", || {
                    format!("{name}: {reason}")
                });
            } else {
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
        refuse: &dyn Fn(&'static str) -> Option<DerivedBody>,
    ) -> Option<DerivedBody> {
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
        // template's is, and no fact substitutes it. So is a method's own
        // scalar value binder (`VALUE_BINDERS`), which the parser spells as
        // a bound (`n: Int`): a per-instantiation clone keeps it, and an
        // occurrence reading it is bound to the clone's own parameter. A
        // compile-time callable binder (`CALLABLE_BINDERS`) is kept by every
        // clone the same way.
        let kept_binders = matches!(class, TemplateClass::MethodBody(features)
                if features.contains(MethodFeatures::ORIGIN_PARAMETERS)
                    || features.contains(MethodFeatures::BOUND_BINDERS)
                    || features.contains(MethodFeatures::VALUE_BINDERS)
                    || features.contains(MethodFeatures::CALLABLE_BINDERS))
            && matches!(site.declaration, BodyDeclaration::Method(method)
            if method.type_params.iter().all(|binder| {
                origin_binder(binder)
                    || bound_binder(binder)
                    || (callable_binder(binder)
                        && matches!(class, TemplateClass::MethodBody(features)
                            if features.contains(MethodFeatures::CALLABLE_BINDERS)))
            })
                && trace.residual.iter().all(|name| {
                    method.type_params.iter().any(|binder| binder.name == *name)
                }));
        let baked = ((trace.residual.is_empty() && !site.residual_binders) || kept_binders)
            && match class {
                // A value-keyed struct specialized whole folds its own
                // values, and a per-call clone its method's own, which
                // `instance_substitution` binds and the occurrences read as
                // the literals they folded to (`folded_literals`).
                TemplateClass::MethodBody(features)
                    if features.contains(MethodFeatures::VALUE_BINDERS) =>
                {
                    true
                }
                TemplateClass::MethodScalarBody | TemplateClass::MethodBody(_) => {
                    trace.value_bindings.is_empty()
                        || (matches!(&site.receiver_arguments, Some(arguments) if arguments.is_empty())
                            && matches!(site.declaration, BodyDeclaration::Method(method)
                            if trace.value_bindings.iter().all(|(name, _)| {
                                method.type_params.iter().all(|binder| binder.name != *name)
                            })))
                }
                // The folded values selected the arms, or are read as the
                // literals each occurrence folded to (`folded_literals`). A
                // folded loop index is read back from the copy it fixed.
                TemplateClass::ClosedScalarBody
                | TemplateClass::FixedCalls
                | TemplateClass::BoundedOperations
                | TemplateClass::ScalarBranches
                | TemplateClass::PackElements
                | TemplateClass::FunctionBody(_) => true,
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
        // A clone whose only binders are the elaborator's origin binders
        // (`Span[Int, __clone_origin0]`) carries exactly those binders' loans
        // in its arguments; the transfer recipe judges each replayed source
        // by its binding's substituted type.
        let binder_clone = site
            .declaration
            .type_params()
            .is_some_and(|binders| !binders.is_empty() && binders.iter().all(clone_origin_binder));
        if matches!(
            class,
            TemplateClass::MethodBody(_) | TemplateClass::FunctionBody(_)
        ) && !substitution
            .types
            .values()
            .chain(substitution.packs.values().flatten())
            .all(|ty| {
                self.plain_data(ty)
                    || (binder_clone
                        && binder_tail_loans(ty)
                        && !self.type_contains_reference(ty)
                        && !mentions_callable(ty))
            })
        {
            return refuse("an instance argument carries a loan, a reference, or a callable");
        }
        let Some(desugars) = self.instance_with_desugars(body, &checked.facts.with_forms) else {
            return refuse("a `with` statement has no desugar form in its template");
        };
        let mut occurrences =
            self.occurrences_over(body, &desugars, Some(&checked.facts.occurrences));
        fold_vector_values(&mut occurrences, &checked.facts.occurrences);
        // Every occurrence of the body is one the template checked. A class
        // without compile-time control flow keeps them all, once each; a
        // keyed one keeps the arms the elaborator selected, once per loop
        // iteration it unrolled, and drops the rest, facts and all.
        let keyed = class.keyed();
        // A relocated pack's spread and transfer have no instance occurrence.
        let relocated: Vec<SyntaxId> = checked
            .facts
            .pack_relocations
            .iter()
            .flat_map(|relocation| [relocation.spread.syntax, relocation.transfer.syntax])
            .collect();
        // A folded struct value (`Self.key`, `Self.n`) drops the `Self` its
        // name was read on, the template occurrence right after the name's
        // in pre-order, which the instance no longer holds; a vector adds
        // its lanes, which no template occurrence has.
        let instance_syntax: HashSet<SyntaxId> = occurrences
            .iter()
            .map(|occurrence| occurrence.id.syntax)
            .collect();
        let folded_selves: Vec<SyntaxId> = occurrences
            .iter()
            .filter_map(|occurrence| {
                let order = &checked.facts.occurrences;
                let at = order
                    .iter()
                    .position(|id| id.syntax == occurrence.id.syntax)?;
                let next = order.get(at + 1)?.syntax;
                match occurrence.vector_fold {
                    Some(VectorFold::Construction) => Some(next),
                    None if occurrence.literal.is_some() && !instance_syntax.contains(&next) => {
                        Some(next)
                    }
                    _ => None,
                }
            })
            .collect();
        let template_occurrences = checked
            .facts
            .occurrences
            .iter()
            .filter(|id| !relocated.contains(&id.syntax) && !folded_selves.contains(&id.syntax))
            .count();
        let elaborated = |occurrence: &Occurrence| {
            matches!(
                occurrence.vector_fold,
                Some(VectorFold::Lane(_) | VectorFold::Dimension)
            )
        };
        let lanes = occurrences
            .iter()
            .filter(|occurrence| elaborated(occurrence))
            .count();
        let traced = occurrences.iter().all(|occurrence| {
            elaborated(occurrence)
                || ((keyed || occurrence.id.copy == 0)
                    && !folded_selves.contains(&occurrence.id.syntax)
                    && checked
                        .facts
                        .occurrences
                        .iter()
                        .any(|id| id.syntax == occurrence.id.syntax))
        }) && (keyed || occurrences.len() - lanes == template_occurrences);
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
        occurrences
            .retain(|occurrence| !matches!(occurrence.vector_fold, Some(VectorFold::Constant)));
        let ids: Vec<OccurrenceId> = occurrences.iter().map(|occurrence| occurrence.id).collect();
        let Some(folded) = folded_literals(&checked.facts, &occurrences) else {
            return refuse("a folded compile-time value does not fit its template type");
        };
        let indices = occurrences
            .iter()
            .filter_map(|occurrence| {
                let (_, index) = occurrence.folded_index?;
                let (_, binder) = checked
                    .facts
                    .expression_types
                    .iter()
                    .map(|(id, ty)| (id, ty))
                    .chain(
                        checked
                            .facts
                            .rebind_assertions
                            .iter()
                            .map(|(id, assertion)| (id, &assertion.operand)),
                    )
                    .filter(|(id, _)| id.syntax == occurrence.id.syntax)
                    .find_map(|(_, ty)| match ty {
                        Ty::Dependent(dependent) => dependent.pack_element(),
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
        let indices = transferred_element_indices(indices, &occurrences);
        let mut selected = checked.facts.selected(&ids, &folded);
        if !relocate_packs(&mut selected, &ids) {
            return refuse("a relocated pack is not the instance's transfer");
        }
        if !construct_folded_vectors(&mut selected, &occurrences) {
            return refuse("a folded vector value is not the template's closed vector");
        }
        // A reference accessor's value twin (`__getitem_param_value__$k`)
        // returns by value what its template handed out as a reference.
        let returns_reference = self
            .return_ref_contracts
            .last()
            .is_some_and(Option::is_some);
        if matches!(class, TemplateClass::MethodBody(features)
            if features.contains(MethodFeatures::REFERENCE_RESULT))
            && !returns_reference
        {
            let returned: Vec<SyntaxId> = returned_values(body)
                .into_iter()
                .map(|syntax| self.syntax_origins.origin(syntax))
                .collect();
            read_returns_by_value(&mut selected, &returned);
        }
        match self.realize_instance_facts(&selected, &substitution, &indices, &occurrences) {
            Ok(facts) => Some(DerivedBody {
                facts,
                spans: occurrences
                    .into_iter()
                    .map(|occurrence| (occurrence.id, occurrence.span))
                    .collect(),
                desugars,
            }),
            Err(reason) => refuse(reason),
        }
    }

    /// [`TemplateObligation::PlainDataArguments`] for one instance argument.
    fn plain_data(&self, ty: &Ty) -> bool {
        !self.type_may_carry_loans(ty)
            && !self.type_contains_reference(ty)
            && !mentions_callable(ty)
    }

    /// [`TemplateObligation::ReplayedTransfers`] for one source's binding type: a
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
    /// receiver says what `Self.T` is inside the clone. A per-call clone's
    /// own binders are the source types and packs its trace names, resolved
    /// as a `def` clone's are.
    fn instance_substitution(
        &self,
        site: &BodySite<'_>,
        template: &CheckedTemplate,
        trace: &InstanceTrace,
    ) -> Result<InstanceSubstitution, TypeError> {
        // A binding names the template's own binder; the declaration
        // carries its identity. The source type is resolved as the clone's
        // own signature resolved it: a generated declaration's spelling of
        // an already-checked type (`StringLiteral`) is admitted where a
        // user-spelled one is not.
        let resolve = |source: &mojito_ast::ast::Type| {
            let generated = self.generated_declaration.replace(true);
            self.bare_string_literal_parameter
                .set(super::declarations::is_string_literal_annotation(source));
            let ty = self.ty_from_anno(source);
            self.bare_string_literal_parameter.set(false);
            self.generated_declaration.set(generated);
            ty
        };
        let Some(arguments) = &site.receiver_arguments else {
            let decl_named = |name: &str| {
                template
                    .param_decls
                    .iter()
                    .find(|decl| decl.name().trim_start_matches('*') == name)
            };
            let mut values = Vec::new();
            let mut views = Vec::new();
            let types = trace
                .type_bindings
                .iter()
                .filter_map(|(name, source)| {
                    decl_named(name).map(|decl| {
                        let ty = resolve(source)?;
                        values.extend(simd_binder_values(decl, &ty, &mut views)?);
                        Ok((decl.id().clone(), ty))
                    })
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
            values.extend(trace.value_bindings.iter().filter_map(|(name, value)| {
                decl_named(name).map(|decl| (decl.id().clone(), value.clone()))
            }));
            return Ok(InstanceSubstitution {
                types,
                packs,
                views,
                values,
                kept_values: Vec::new(),
            });
        };
        // A shared stub names nothing its receiver binds.
        if site.role == BodyRole::Template || trace.shared_stub {
            return Ok(InstanceSubstitution {
                types: HashMap::new(),
                packs: HashMap::new(),
                views: Vec::new(),
                values: Vec::new(),
                kept_values: Vec::new(),
            });
        }
        let unresolved = || {
            TypeError::InvariantViolation(
                "a method clone's receiver does not bind its struct's parameters".to_string(),
            )
        };
        // The declarations are the struct's binders followed by the method's
        // own. A per-instantiation clone keeps its own symbolic
        // (`BOUND_BINDERS`); a per-call clone bakes them, and its trace
        // names the source type, element list, or value written for each. A member
        // of a variadic struct specialized whole (`Tuple$t2[String, Int]`)
        // has the pack's elements as its receiver's arguments, as many as its
        // trace names; a specialization whose receiver carries none (a user
        // struct's `Pair$t2[…]`, `TString$…`) binds the element sources its
        // trace names, resolved as a `def` clone's are.
        let struct_owner = template
            .id
            .owner
            .as_deref()
            .map(super::annotations::binder_owner);
        let struct_count = template
            .param_decls
            .iter()
            .take_while(|decl| struct_owner.as_deref() == Some(&*decl.id().owner))
            .count();
        let (struct_decls, own) = template.param_decls.split_at(struct_count);
        let resolved_arguments = || {
            arguments
                .iter()
                .map(|argument| match argument {
                    mojito_types::types::TyArg::Ty(ty) => Ok(ty.clone()),
                    _ => Err(unresolved()),
                })
                .collect::<Result<Vec<_>, _>>()
        };
        let mut types = TySubst::new();
        let mut packs = HashMap::new();
        let mut views = Vec::new();
        let mut values = Vec::new();
        let mut kept_values = Vec::new();
        match struct_decls {
            [
                ParamDecl::Type {
                    id,
                    name,
                    variadic: true,
                    ..
                },
            ] => {
                let traced = trace
                    .pack_bindings
                    .iter()
                    .find(|(bound, _)| bound == name.trim_start_matches('*'))
                    .map(|(_, elements)| elements)
                    .ok_or_else(unresolved)?;
                let elements = if arguments.is_empty() {
                    let elements = traced.iter().map(resolve).collect::<Result<Vec<_>, _>>()?;
                    // The template's `Self` names this instance only when the
                    // resolved pack mangles back to it.
                    let own = template.id.owner.as_deref().map(|owner| {
                        self.specialized_value_structs(
                            &self.canonicalize_public_tuple_types(Ty::Struct(
                                owner.to_string(),
                                elements
                                    .iter()
                                    .cloned()
                                    .map(mojito_types::types::TyArg::Ty)
                                    .collect(),
                            )),
                        )
                    });
                    if !matches!(own, Some(Ty::Struct(name, arguments))
                        if arguments.is_empty() && site.instance.owner.as_deref() == Some(&*name))
                    {
                        return Err(unresolved());
                    }
                    elements
                } else if traced.len() == arguments.len() {
                    resolved_arguments()?
                } else {
                    return Err(unresolved());
                };
                if elements.is_empty() {
                    return Err(unresolved());
                }
                packs.insert(id.clone(), elements);
            }
            _ if struct_decls.len() == arguments.len() => {
                for (decl, ty) in struct_decls.iter().zip(resolved_arguments()?) {
                    match decl {
                        ParamDecl::Type {
                            id,
                            variadic: false,
                            ..
                        } => {
                            types.insert(id.clone(), ty);
                        }
                        _ => return Err(unresolved()),
                    }
                }
            }
            // A value-keyed struct specialized whole folds its values, which
            // its members' traces name.
            _ if arguments.is_empty() => {
                for decl in struct_decls {
                    let value = match decl {
                        ParamDecl::Value {
                            variadic: false, ..
                        } => trace
                            .value_bindings
                            .iter()
                            .find(|(name, _)| name == decl.name())
                            .map(|(_, value)| value.clone()),
                        _ => None,
                    };
                    values.push((decl.id().clone(), value.ok_or_else(unresolved)?));
                }
            }
            _ => return Err(unresolved()),
        }
        for decl in own {
            let name = decl.name().trim_start_matches('*');
            if let Some((_, source)) = trace.type_bindings.iter().find(|(bound, _)| bound == name) {
                let ty = resolve(source)?;
                values.extend(simd_binder_values(decl, &ty, &mut views)?);
                types.insert(decl.id().clone(), ty);
            } else if let Some((_, sources)) =
                trace.pack_bindings.iter().find(|(bound, _)| bound == name)
            {
                let elements = sources.iter().map(resolve).collect::<Result<_, _>>()?;
                packs.insert(decl.id().clone(), elements);
            } else if let Some((_, value)) = trace
                .value_bindings
                .iter()
                .find(|(bound, _)| bound == name)
                .filter(|_| matches!(decl, ParamDecl::Value { .. }))
            {
                values.push((decl.id().clone(), value.clone()));
            } else if matches!(decl, ParamDecl::Value { .. })
                && site
                    .declaration
                    .type_params()
                    .is_some_and(|binders| binders.iter().any(|binder| binder.name == name))
            {
                kept_values.push(name.to_string());
            } else if !matches!(decl, ParamDecl::Type { bounds, .. } if !bounds.is_empty()) {
                return Err(unresolved());
            }
        }
        Ok(InstanceSubstitution {
            types,
            packs,
            views,
            values,
            kept_values,
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
            views,
            values,
            kept_values,
        } = instance;
        // A closed public `Tuple` names the specialization the clone check
        // selects for it (`canonicalize_public_tuple_types`), and so does a
        // value-keyed or variadic struct at closed arguments
        // (`specialized_value_structs`).
        let canonical =
            |ty: Ty| self.specialized_value_structs(&self.canonicalize_public_tuple_types(ty));
        let substitute = |ty: &Ty| {
            canonical(mojito_types::types::substitute_packs(
                &fold_binder_views(ty, views),
                substitution,
                packs,
                values,
            ))
        };
        let demands = self.hash_leaf_demands.borrow().len();
        let mut facts = substituted_facts(template, instance, indices, &canonical)?;
        realize_value_shaped_constructions(template, &mut facts, occurrences)?;
        realize_simd_intrinsics(template, &mut facts, occurrences)?;
        self.realize_lane_literals(template, &mut facts, occurrences)?;
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
        if !self.copied_places_hold(&facts, occurrences) {
            return Err("a copied place is not implicitly copyable for the instance");
        }
        // `infer_method_call`'s demand on a place a consuming call copies,
        // at the instance's type.
        let copied_receivers = facts
            .implicitly_copied_consuming_receivers
            .iter()
            .all(|call| {
                occurrences
                    .iter()
                    .find(|occurrence| occurrence.id == *call)
                    .and_then(|occurrence| occurrence.method_call.as_ref())
                    .and_then(|(receiver, _)| {
                        let receiver = OccurrenceId {
                            syntax: *receiver,
                            copy: call.copy,
                        };
                        fact_at(&facts.expression_types, receiver)
                    })
                    .is_some_and(|ty| self.is_implicitly_copyable(ty))
            });
        if !copied_receivers {
            return Err("a copied consuming receiver is not implicitly copyable for the instance");
        }
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
        // A residue naming a compile-time callable names it by the binder's
        // name, which only an instance keeping that binder still declares.
        let kept_callables = template.call_throughs.iter().all(|residue| {
            !matches!(&residue.callee,
                mojito_checked::checked::CallThroughCallee::ValueParam(name)
                    if !instance.kept_values.contains(name))
        });
        if !kept_callables {
            return Err("a residue names a compile-time callable the instance does not keep");
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
        // `check_capture_capability`'s demand on a nested `def`'s owned
        // capture, at the instance's type.
        let capturable = template
            .nested_defs
            .iter()
            .flat_map(|(_, recipe)| &recipe.captures)
            .filter(|capture| mojito_types::types::is_symbolic(&capture.ty))
            .all(|capture| match capture.kind {
                CaptureKind::Copy => self.is_implicitly_copyable(&substitute(&capture.ty)),
                CaptureKind::Move => self.is_movable(&substitute(&capture.ty)),
                CaptureKind::Imm | CaptureKind::Mut | CaptureKind::Ref => true,
            });
        if !capturable {
            return Err("a nested def's owned capture is not capturable for the instance");
        }
        // The declaration's own judgment of each binding whose type mentions
        // a parameter, at the instance's type: deletable where the type is
        // `Deinitable`, linear where it is still a bare parameter. A type
        // built over a parameter answers from its own conformance under the
        // instance's arguments. A tuple unpacking's target is judged nowhere.
        let unpacked: Vec<OccurrenceId> = template
            .tuple_unpacks
            .iter()
            .flat_map(|(_, unpack)| unpack.targets.iter().copied())
            .collect();
        for (id, declared) in &template.binding_types {
            if !mojito_types::types::is_symbolic(declared) || unpacked.contains(id) {
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
        // `expect_bool`'s judgment of each condition the template tested
        // through `Bool(x)`, at the instance's type: a `Bool` is read as it
        // stands. A condition the template read as a `Bool` stays one.
        let mut truthiness = Vec::new();
        for condition in &facts.truthiness_conditions {
            let converts = fact_at(&facts.expression_types, *condition)
                .ok_or("a truthiness condition has no recorded type")
                .and_then(|ty| {
                    self.condition_truthiness(ty, "condition")
                        .map_err(|_| "a condition is not boolable for the instance")
                })?;
            if converts {
                truthiness.push(*condition);
            }
        }
        facts.truthiness_conditions = truthiness;
        renumber_locals(&mut facts)?;
        let folded = |owner: &TemplateOwner| matches!(owner, TemplateOwner::CompileTimeParam(name) if !kept_values.contains(name));
        if facts
            .expression_bindings
            .iter()
            .chain(&facts.statement_bindings)
            .any(|(_, owner)| folded(owner))
        {
            return Err("an occurrence still names a folded compile-time parameter");
        }
        // A nested `def`'s call selects the declaration the body itself
        // introduces, whose signature no instance changes, and reads its
        // summaries under the same name.
        let nested_calls = nested_def_calls(template);
        for (_, callee) in &nested_calls {
            note_realized_callee(&mut facts, callee, callee);
        }
        for (id, _) in &template.call_parameters {
            // A method call records its (empty) parameters here too. Its
            // callee is realized from its contract below, and a call through
            // a callable parameter from the instance's own binding of it.
            if template.selected_calls.iter().any(|(call, _)| call == id)
                || template
                    .inplace_updates
                    .iter()
                    .any(|(update, _)| update == id)
                || template.callable_calls.contains(id)
                || nested_calls.iter().any(|(call, _)| call == id)
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
        self.realize_iterations(&mut facts, &substitute)?;
        Self::realize_comprehension_bindings(&mut facts, &substitute);
        Self::realize_nested_defs(&mut facts, &substitute);
        self.realize_tuple_unpacks(&mut facts, &substitute)?;
        facts.struct_applications =
            self.instance_struct_applications(template, instance, &substitute);
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
        // An element store's value getter is realized on the instance's
        // subscripted value, and its in-place dunder on the instance's
        // element where the template dispatched it through the element's
        // bound; a closed one stands as the template selected it
        // (`substituted_element_stores`).
        self.realize_element_getters(&mut facts, occurrences, substitution)?;
        self.realize_element_dunders(&mut facts)?;
        self.realize_inplace_updates(&mut facts, substitution)?;
        let inverted_writes = self.realize_inverted_writes(&mut facts, occurrences)?;
        for index in 0..facts.selected_calls.len() {
            let id = facts.selected_calls[index].0;
            if bound_dispatches.contains(&id) || inverted_writes.contains(&id) {
                continue;
            }
            self.realize_method_call(&mut facts, index, occurrences, substitution)?;
        }
        self.realize_tuple_elements(&mut facts, occurrences)?;
        self.realize_static_overloads(&mut facts, occurrences, substitution)?;
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
            self.realize_bound_builtin(&mut facts, *call, *builtin, occurrences)?;
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
        self.realize_transfers(&mut facts, template, &substitute)?;
        // Kept with struct origin slots unbound, as capture keeps them: a
        // loan-carrying argument substitutes a clone's origin binder there.
        facts.struct_applications = sorted_applications(
            std::mem::take(&mut facts.struct_applications)
                .into_iter()
                .map(|(name, arguments)| {
                    match without_struct_origins(&Ty::Struct(name.clone(), arguments.clone())) {
                        Ty::Struct(name, arguments) => (name, arguments),
                        _ => (name, arguments),
                    }
                })
                .collect(),
        );
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
        // The template's leaves are closed; a bound builtin or dispatch
        // realized above recorded the instance's own.
        let mut leaves = std::mem::take(&mut facts.hash_leaves);
        leaves.extend_from_slice(&self.hash_leaf_demands.borrow()[demands..]);
        facts.hash_leaves = canonical_hash_leaves(leaves);
        Ok(facts)
    }

    /// The generic-struct applications a template reached, substituted for
    /// an instance. A value-keyed struct's application at the instance's
    /// values names the specialization the elaborator minted, which the
    /// clone check records no application of.
    fn instance_struct_applications(
        &self,
        template: &CheckedBodyFacts,
        InstanceSubstitution {
            types,
            packs,
            values,
            ..
        }: &InstanceSubstitution,
        substitute: &dyn Fn(&Ty) -> Ty,
    ) -> Vec<(String, Vec<mojito_types::types::TyArg>)> {
        template
            .struct_applications
            .iter()
            .filter(|(name, arguments)| {
                let application = mojito_types::types::substitute_packs(
                    &Ty::Struct(name.clone(), arguments.clone()),
                    types,
                    packs,
                    values,
                );
                !matches!(self.specialized_value_structs(&application),
                    Ty::Struct(specialization, _) if specialization != *name)
            })
            .map(|(name, arguments)| {
                (
                    name.clone(),
                    mojito_types::types::map_tyargs(arguments, substitute),
                )
            })
            .collect()
    }

    /// `check_consuming`'s demand on each copied place, at the instance's
    /// type. The synthesized `copy`'s receiver is copied explicitly, which
    /// demands only `Copyable`.
    fn copied_places_hold(&self, facts: &CheckedBodyFacts, occurrences: &[Occurrence]) -> bool {
        let explicit_copy = |place: &OccurrenceId| {
            occurrences.iter().any(|occurrence| {
                occurrence.callee.as_deref() == Some("__mojito_fieldwise_copy")
                    && occurrence.id.copy == place.copy
                    && occurrence.arguments == [place.syntax]
            })
        };
        facts.copy_place_value_uses.iter().all(|place| {
            facts
                .expression_types
                .iter()
                .find(|(id, _)| id == place)
                .is_some_and(|(_, ty)| {
                    self.is_copyable(ty)
                        && (explicit_copy(place) || self.is_implicitly_copyable(ty))
                })
        })
    }

    /// `ty` with every application of a value-keyed struct at closed values
    /// (`_StridedRange[DType.int]`) named as the specialization the
    /// elaborator minted for it (`_StridedRange$dint;`), which is the type
    /// the clone check reads, and so is a user variadic struct at a closed
    /// pack (`Bag[Int, String]` as `Bag$t2[…]`). An application with no
    /// minted specialization is left as it is.
    fn specialized_value_structs(&self, ty: &Ty) -> Ty {
        struct Specialized<'a>(&'a Checker);

        impl mojito_types::types::TyRewrite for Specialized<'_> {
            fn whole(&mut self, ty: &Ty) -> Option<Ty> {
                let Ty::Struct(name, arguments) = ty else {
                    return None;
                };
                let values = arguments
                    .iter()
                    .map(|argument| match argument {
                        mojito_types::types::TyArg::Val(value) => Some(value.clone()),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>()
                    .filter(|values| !values.is_empty())
                    .or_else(|| closed_pack_values(arguments))?;
                let mangled = mojito_symbol::symbol::mangle(name, &values).ok()?;
                self.0
                    .structs
                    .get(&mangled)
                    .is_some_and(|info| info.fixed_arguments.is_none())
                    .then(|| Ty::Struct(mangled, Vec::new()))
            }

            fn expr(
                &mut self,
                expr: &mojito_types::param_expr::ParamExpr,
            ) -> Result<mojito_types::param_expr::ParamExpr, mojito_types::param_expr::ParamError>
            {
                Ok(expr.clone())
            }
        }

        mojito_types::types::rewrite_ty(ty, &mut Specialized(self)).unwrap_or_else(|_| ty.clone())
    }

    /// A retained type under an instance's arguments, naming the generated
    /// Tuple the clone check selects for a closed public one.
    fn instance_ty(&self, ty: &Ty, substitution: &TySubst) -> Ty {
        self.canonicalize_public_tuple_types(mojito_types::types::substitute(ty, substitution))
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
        let (receiver, method) = method_call_at(occurrences, id)
            .ok_or("a selected call is not a method call in the instance")?;
        let Some(Ty::Struct(owner, arguments)) =
            fact_at(&facts.expression_types, receiver).cloned()
        else {
            return Err("a method call's receiver is not a nominal struct");
        };
        let selected = facts.selected_calls[index].1.contract.target.clone();
        // A tuple element's accessor is realized by its own recipe
        // (`realize_tuple_elements`).
        if tuple_element_accessor(&selected).is_some()
            && fact_at(&facts.expression_types, receiver)
                .and_then(mojito_types::types::tuple_elements)
                .is_some()
        {
            return Ok(());
        }
        // A per-call clone (`Scaler.scaled$i3`) bakes the callee's binders
        // into its target. On a non-generic receiver the request names no
        // instance, and substitution left its arguments as they were, so the
        // clone check selects the same clone.
        if let Some(request) = fact_at(&facts.method_instantiations, id) {
            if !arguments.is_empty() || !request.owner_arguments.is_empty() {
                return Err("a per-call clone request is keyed by a generic receiver");
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
            substitution,
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
                parameter.ty = self.instance_ty(&parameter.ty, substitution);
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

    /// Realize each tuple element read (`BodyShape::tuple_element`) on the
    /// instance's own Tuple, as its own check selects it.
    ///
    /// A generated Tuple the instance's type names, once declared, has one
    /// place accessor per position, and the check calls the element's
    /// (`__getitem_param__$k`) as a reference call on the local, read by
    /// copy; before any such Tuple is declared, the check types the element
    /// from the tuple's arguments and records nothing else. Which of the two
    /// the template met depends on whether its own Tuple was declared yet,
    /// and on whether its type was closed, so the instance records the call
    /// afresh whatever the template recorded. The position is the literal
    /// index, which no instance changes. The read is by copy, so an element
    /// the instance cannot copy implicitly is left to the clone check.
    fn realize_tuple_elements(
        &self,
        facts: &mut CheckedBodyFacts,
        occurrences: &[Occurrence],
    ) -> Result<(), &'static str> {
        let mut realized = false;
        for occurrence in occurrences {
            let (Some((receiver, method)), Some((_, position))) =
                (&occurrence.method_call, occurrence.folded_index)
            else {
                continue;
            };
            let id = occurrence.id;
            let receiver = OccurrenceId {
                syntax: *receiver,
                copy: id.copy,
            };
            let Some(tuple @ Ty::Struct(owner, arguments)) =
                fact_at(&facts.expression_types, receiver)
            else {
                continue;
            };
            let Some(elements) = mojito_types::types::tuple_elements(tuple) else {
                continue;
            };
            if method != "__getitem__" {
                continue;
            }
            let accessor = format!("{TUPLE_ELEMENT_ACCESSOR}${position}");
            let declared = self
                .structs
                .get(owner)
                .is_some_and(|info| info.methods.contains_key(&accessor));
            if !declared {
                if self
                    .predeclared_generated_tuple_arguments
                    .contains_key(owner)
                {
                    return Err("a tuple element's Tuple is generated but not yet declared");
                }
                if fact_at(&facts.selected_calls, id).is_some() {
                    return Err("a tuple element's accessor is not declared on the instance");
                }
                continue;
            }
            let element = usize::try_from(position)
                .ok()
                .and_then(|position| elements.get(position))
                .ok_or("a tuple element's position is outside the tuple")?;
            if matches!(element, Ty::Ref(_)) {
                return Err("a tuple element holds a reference");
            }
            if !self.is_implicitly_copyable(element) {
                return Err("a tuple element read by value is not implicitly copyable");
            }
            let root = fact_at(&facts.expression_bindings, receiver)
                .cloned()
                .ok_or("a tuple element's local has no binding")?;
            let target = format!("{owner}.{accessor}");
            let (element, application) = ((*element).clone(), (owner.clone(), arguments.clone()));
            let reference = TemplateReference {
                referent: element.clone(),
                origin: mojito_checked::templates::TemplateOrigin::Place(
                    mojito_checked::templates::TemplatePlace {
                        root,
                        path: Vec::new(),
                    },
                ),
                mutability: mojito_types::origin::Mutability::Mutable,
            };
            let call = TemplateCallContract {
                contract: mojito_checked::checked::CheckedCallContract {
                    target: target.clone(),
                    raises: None,
                    result_ty: element,
                    result_adapter: None,
                    receiver_requires_place: true,
                    receiver_elided: false,
                    receiver_convention: Some(mojito_ast::ast::ArgConvention::Ref),
                    arguments: Vec::new(),
                    captures: Vec::new(),
                    reference_result: None,
                    parameter_arguments: Vec::new(),
                    param_decls: Vec::new(),
                    boundary: mojito_checked::checked::CheckedCallBoundary::default(),
                },
                reference_result: Some(reference.clone()),
                result_origins: Vec::new(),
                arguments: Vec::new(),
                invalidations: Vec::new(),
            };
            upsert(&mut facts.selected_calls, id, call);
            upsert(&mut facts.overload_targets, id, target.clone());
            upsert(&mut facts.call_parameters, id, Vec::new());
            upsert(&mut facts.reference_results, id, reference);
            if !facts.copyable_reference_result_reads.contains(&id) {
                facts.copyable_reference_result_reads.push(id);
            }
            note_realized_callee(facts, &target, &target);
            // The receiver's application is recorded only from a source
            // that records applications at all.
            let source = occurrence.span.source.as_deref();
            if source.is_some() && !super::overload_support::is_bundled_module_source(source) {
                facts.struct_applications.push(application);
            }
            realized = true;
        }
        if realized {
            let position = |id: &OccurrenceId| {
                occurrences
                    .iter()
                    .position(|occurrence| occurrence.id == *id)
            };
            facts.selected_calls.sort_by_key(|(id, _)| position(id));
            facts.call_parameters.sort_by_key(|(id, _)| position(id));
            facts.reference_results.sort_by_key(|(id, _)| position(id));
            facts.copyable_reference_result_reads.sort_by_key(position);
        }
        Ok(())
    }

    /// Realize the value getter each element store through a setter embeds,
    /// on the instance's own subscripted value, as the setter selected at
    /// the site is realized ([`Self::realize_method_contract`]).
    fn realize_element_getters(
        &self,
        facts: &mut CheckedBodyFacts,
        occurrences: &[Occurrence],
        substitution: &TySubst,
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
                    substitution,
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
    fn realize_element_dunders(&self, facts: &mut CheckedBodyFacts) -> Result<(), &'static str> {
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
    fn realize_inplace_updates(
        &self,
        facts: &mut CheckedBodyFacts,
        substitution: &TySubst,
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
                    substitution,
                )?
                .unwrap_or_else(|| selected.clone());
            if let Some((_, parameters)) = facts
                .call_parameters
                .iter_mut()
                .find(|(site, _)| site == id)
            {
                for parameter in parameters {
                    parameter.ty = self.instance_ty(&parameter.ty, substitution);
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

    /// Rewrite one method call's contract for an instance whose receiver is
    /// the struct `owner` under `arguments`: the target is the instance's
    /// clone of the selected declaration, where one exists, or the copy a
    /// struct specialized whole holds of it, and the result,
    /// raised, parameter, and referent types substitute. `None` where the template
    /// already selected the receiver's clone, whose contract stands.
    fn realize_method_contract(
        &self,
        types: &[(OccurrenceId, Ty)],
        call: &mut TemplateCallContract,
        (owner, arguments): (&str, &[mojito_types::types::TyArg]),
        method: &str,
        substitution: &TySubst,
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
        // A struct specialized whole (`AHasher$…`) holds its own copy of
        // each member the template selected on the template struct
        // (`AHasher._update`).
        let selected_owner = selected
            .split_once('.')
            .map(|(selected_owner, _)| selected_owner)
            .filter(|selected_owner| {
                *selected_owner != owner && owner.starts_with(&format!("{selected_owner}$"))
            });
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
                    super::overload_support::method_lowered_name(
                        selected_owner.unwrap_or(owner),
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
                .is_some_and(|ty| *ty == self.instance_ty(&parameter.parameter_ty, substitution))
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
            // No clone of this method. If the instance has clones of others,
            // this one was withheld from it or collapsed.
            let suffix = clone_name
                .as_deref()
                .and_then(|clone| clone.strip_prefix(method));
            if suffix.is_some_and(|suffix| info.methods.keys().any(|name| name.ends_with(suffix))) {
                return Err("the instance has clones, but not of a called method");
            }
            // The erased callee serves the instance too; its availability
            // condition is judged at the instance's receiver arguments, as
            // the clone check judges it.
            if !declared.availability.is_empty() && !substitution.is_empty() {
                let Ty::Struct(_, bound) = self.instance_ty(
                    &Ty::Struct(owner.to_string(), arguments.to_vec()),
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
            match selected_owner.and_then(|selected_owner| selected.strip_prefix(selected_owner)) {
                Some(member) => {
                    let target = format!("{owner}{member}");
                    let declared_here = match family.as_slice() {
                        [_] => target == format!("{owner}.{method}"),
                        members => members.iter().any(|candidate| {
                            super::overload_support::method_lowered_name(
                                owner,
                                method,
                                candidate,
                                self_ty.as_ref(),
                            ) == target
                        }),
                    };
                    if !declared_here {
                        return Err("a specialized struct does not declare the selected member");
                    }
                    target
                }
                None => selected.clone(),
            }
        };
        // The callee has no binders of its own, so its parameter types were
        // recorded at the receiver's arguments: in the caller's binder scope,
        // whether the receiver is `self` or a field of another struct.
        let contract = &mut call.contract;
        contract.target.clone_from(&target);
        contract.result_ty = self.instance_ty(&contract.result_ty, substitution);
        contract.raises = contract
            .raises
            .as_ref()
            .map(|raised| self.instance_ty(raised, substitution));
        for argument in &mut contract.arguments {
            argument.parameter_ty = self.instance_ty(&argument.parameter_ty, substitution);
        }
        if let Some(reference) = &mut call.reference_result {
            reference.referent = self.instance_ty(&reference.referent, substitution);
        }
        Ok(Some(target))
    }

    /// Realize each overloaded static a body calls on a generic struct's
    /// type application (`Pair[Self.T].pick(v, 1)`,
    /// [`BodyShape::static_call`]): the member the template ranked, on the
    /// instance's clone of the static where the receiver's arguments,
    /// resolved in the instance, have one.
    ///
    /// The clone check re-ranks the clone family there, and its members
    /// differ only in closed parameter types, so it selects the clone of the
    /// template's member (`method_clone_target`). A receiver with no clone
    /// keeps the erased member, as does a static on an inferred or
    /// contextual receiver, which no instance retargets.
    fn realize_static_overloads(
        &self,
        facts: &mut CheckedBodyFacts,
        occurrences: &[Occurrence],
        substitution: &TySubst,
    ) -> Result<(), &'static str> {
        for index in 0..facts.overload_targets.len() {
            let (id, selected) = &facts.overload_targets[index];
            if facts.selected_calls.iter().any(|(call, _)| call == id) {
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
                        super::overload_support::method_lowered_name(
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
        Ok(())
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
    fn realize_callable_call(
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
    fn realize_operator(
        &self,
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
        if closed_scalar(&left_ty) {
            return if right_ty == left_ty
                && super::operators::scalar_operator_result(op, &left_ty) == Some(result)
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
    /// implicit copy the template did not, or borrows it otherwise than the
    /// template's. A conversion that kept no target
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
        let to = self.instance_ty(&result, substitution);
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
                        facts.simd_to_bits = notes.simd_to_bits;
                        facts.simd_casts = notes.simd_casts;
                        facts.simd_lengths = notes.simd_lengths;
                        facts.pack_relocations = notes.pack_relocations;
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
        let whole_values = method_body
            || matches!(
                coverage,
                TemplateCoverage::Certified(TemplateClass::FunctionBody(_))
            );
        let constructs = method_body
            || matches!(
                coverage,
                TemplateCoverage::Certified(TemplateClass::FunctionBody(features))
                    if features.contains(MethodFeatures::CONSTRUCTIONS)
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
            .chain(whole_values.then_some(TemplateObligation::PlainDataArguments))
            .chain(method_body.then_some(TemplateObligation::ReplayedTransfers))
            .chain(constructs.then_some(TemplateObligation::ConstructorSelection))
            .collect(),
        });
    }

    /// The certificate a freshly captured module-level generic function earns.
    ///
    /// Every class shares a declaration shape: plain type parameters, plain
    /// regular runtime parameters (read, and in a runtime body `var` or `mut`),
    /// a concrete scalar result, and no captures, decorators, or `where`
    /// clauses, so the declaration's bounds —
    /// discharged where an instance is requested — are all an instance owes.
    /// Only a [`TemplateClass::FunctionBody`] may raise
    /// (`MethodFeatures::RAISES`).
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
    /// - Effects: the callee raises only where the body may (`effect_derives`),
    ///   and its transfer and call-through summaries were empty; a realization
    ///   re-reads them and installs the same fixpoint observation a clone
    ///   check would.
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
            // A keyed body reads a `DType` binder only as a lane dtype, which
            // the grammar admits in a construction alone
            // ([`BodyShape::simd_construction`]).
            ParamDecl::Value {
                ty,
                variadic: false,
                ..
            } => {
                matches!(**ty, Ty::Bool | Ty::Int) || (self.source_validation && **ty == Ty::Dtype)
            }
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
        // executable check the surviving ones, a value-keyed body with
        // neither among them.
        let keyed = self.source_validation;
        if !keyed
            && (!pack_binders.is_empty()
                || body.iter().any(holds_comptime_if)
                || super::rebind::body_keys_rebind(body, &self.rebind_keyed_bodies))
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
        // A `var` or `mut` parameter of a runtime body is bound from its
        // declared convention alone, and is rooted at its own binding under
        // every instance, as a method's is.
        let owned_param = |parameter: &mojito_ast::ast::FnParam| {
            !keyed
                && matches!(
                    parameter.convention,
                    Some(mojito_ast::ast::ArgConvention::Var | mojito_ast::ast::ArgConvention::Mut)
                )
        };
        let plain_params = params.iter().all(|parameter| {
            (parameter.kind == mojito_ast::ast::ParamKind::Regular || pack_collector(parameter))
                && (parameter.convention.is_none() || owned_param(parameter))
                && parameter.default.is_none()
                && parameter.origin.is_none()
        });
        if !plain_params {
            return outside("a parameter is not an immutable, 'var', or 'mut' regular parameter");
        }
        let owned_params = params.iter().any(owned_param);
        let mut_params: Vec<&str> = params
            .iter()
            .filter(|parameter| parameter.convention == Some(mojito_ast::ast::ArgConvention::Mut))
            .map(|parameter| parameter.name.as_str())
            .collect();
        if captures.is_some() || !decorators.is_empty() {
            return outside("the declaration captures or is decorated");
        }
        if keyed && (*raises || raises_type.is_some()) {
            return outside("a keyed body raises");
        }
        // A body returning nothing falls off its end: the grammar admits no
        // value `return` for it, and a bare `return` only in a runtime body.
        // A runtime body may return a whole value of any type, which every
        // `return` must move or copy at exactly the declared type.
        if !closed_scalar(ret_ty) && *ret_ty != Ty::None && keyed {
            return outside("the return type is not a concrete scalar");
        }
        let packs: Vec<&str> = params
            .iter()
            .filter(|parameter| pack_collector(parameter))
            .map(|parameter| parameter.name.as_str())
            .collect();
        let desugars = self.with_desugars.borrow();
        let shape = BodyShape {
            origins: &self.syntax_origins,
            facts,
            structs: &self.structs,
            traits: &self.traits,
            desugars: &desugars,
            desugar_depth: std::cell::Cell::new(0),
            error_binders: RefCell::new(Vec::new()),
            constants: &self.comptimes,
            vector_aliases: self.vector_aliases(),
            params: params
                .iter()
                .map(|parameter| parameter.name.as_str())
                .collect(),
            packs,
            pack_struct: None,
            loop_vars: RefCell::new(Vec::new()),
            values: decls
                .iter()
                .filter_map(|decl| match decl {
                    ParamDecl::Value { name, .. } => Some(name.as_str()),
                    ParamDecl::Type { .. } => None,
                })
                .collect(),
            struct_values: Vec::new(),
            struct_lanes: Vec::new(),
            struct_vectors: Vec::new(),
            print_calls: RefCell::new(Vec::new()),
            nested_depth: std::cell::Cell::new(0),
            borrowed_params: mut_params.clone(),
            mut_params,
            keyed,
            receiver: false,
            self_convention: None,
            // A runtime body may hold a whole value of any type in a local
            // or an argument, and iterate a place, as a method's may
            // (`FunctionBody`); a keyed body keeps source validation's rules.
            moved_result: (!keyed).then_some(ret_ty),
            reference_result: None,
            // A `var` or `mut` parameter makes the body a `FunctionBody`, whose
            // instances owe plain-data arguments.
            features: std::cell::Cell::new(
                [
                    (*raises || raises_type.is_some(), MethodFeatures::RAISES),
                    (owned_params, MethodFeatures::OWNED_PARAMETERS),
                ]
                .into_iter()
                .filter(|(held, _)| *held)
                .fold(MethodFeatures::default(), |features, (_, feature)| {
                    features.union(MethodFeatures::STATEMENTS).union(feature)
                }),
            ),
            locals: RefCell::new(Vec::new()),
            handles: RefCell::new(Vec::new()),
            references: RefCell::new(Vec::new()),
            receivers: RefCell::new(Vec::new()),
            subscripts: RefCell::new(Vec::new()),
            places: RefCell::new(Vec::new()),
            operators: RefCell::new(Vec::new()),
            bound_builtins: RefCell::new(Vec::new()),
            constructions: RefCell::new(Vec::new()),
            binders: Vec::new(),
            struct_binders: Vec::new(),
            binder_constructions: RefCell::new(Vec::new()),
            callable_params: Vec::new(),
            callable_calls: RefCell::new(Vec::new()),
            callable_binders: Vec::new(),
            static_calls: RefCell::new(Vec::new()),
            repr_calls: RefCell::new(Vec::new()),
            lane_binders: Vec::new(),
            simd_to_bits: RefCell::new(Vec::new()),
            simd_casts: RefCell::new(Vec::new()),
            simd_lengths: RefCell::new(Vec::new()),
            pack_relocations: RefCell::new(Vec::new()),
            stringified: RefCell::new(Vec::new()),
        };
        if !shape.block(body)
            || !shape.operators.borrow().is_empty()
            || !shape.bound_builtins.borrow().is_empty()
            || !shape.repr_calls.borrow().is_empty()
        {
            return outside("the body is not scalar returns over direct calls and 'len'");
        }
        // The scalar classes argue for runtime statements over closed scalars
        // (`STATEMENTS`); a body holding more is a `FunctionBody`, whose
        // certificate argues for each feature `FUNCTION_FEATURES` names. With
        // no facts the grammar reads a name by its syntax alone, so a feature
        // it holds may be one the recorded types rule out (`ptr.unsafe_free()`
        // on a local is a copied consuming call until its type names an
        // untracked pointer).
        let features = shape.features.get();
        let widened = !features.without(MethodFeatures::STATEMENTS).is_empty();
        if facts.is_some() && !FUNCTION_FEATURES.contains(features) {
            return outside("the body holds a construct outside the function classes");
        }
        let class = |class| {
            (
                TemplateCoverage::Certified(class),
                GrammarNotes {
                    print_calls: shape.print_calls.borrow().clone(),
                    constructions: shape.constructions.borrow().clone(),
                    simd_to_bits: shape.simd_to_bits.borrow().clone(),
                    simd_casts: shape.simd_casts.borrow().clone(),
                    simd_lengths: shape.simd_lengths.borrow().clone(),
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
        if !facts
            .expression_effects
            .iter()
            .all(|(_, effects)| effect_derives(effects))
        {
            return outside("a call has an effect other than raising");
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
        // Every call selected a module-scope declaration or is a method call
        // or in-place update the grammar admitted, which records its
        // parameters here too; every effect summary read belongs to one of
        // those calls, and no application carries a pack.
        let callees: Option<Vec<&str>> = facts
            .call_parameters
            .iter()
            .map(|(id, _)| {
                fact_at(&facts.selected_calls, *id)
                    .or_else(|| fact_at(&facts.inplace_updates, *id))
                    .map_or_else(
                        || template_callee(facts, *id),
                        |call| Some(call.contract.target.as_str()),
                    )
            })
            .collect();
        let Some(callees) = callees else {
            return outside("a call's callee is not a module-scope declaration");
        };
        if !facts.effect_free_callees.iter().all(|callee| {
            callees.contains(&callee.as_str()) || dispatched_conformer(&callees, callee)
        }) {
            return outside("an effect summary was read outside a direct call");
        }
        if facts.replays_transfers() {
            return outside("the body replays a transfer summary");
        }
        // A construction's `__init__` target is re-selected per instance
        // ([`Checker::realize_construction`]); it records no call parameters.
        let at_call = |id: &OccurrenceId| facts.call_parameters.iter().any(|(call, _)| call == id);
        let constructions = shape.constructions.borrow();
        let stringified = shape.stringified.borrow();
        if !facts
            .overload_targets
            .iter()
            .all(|(id, _)| at_call(id) || constructions.contains(id) || stringified.contains(id))
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
        } else if widened {
            TemplateClass::FunctionBody(features)
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
    /// mutability, and a write through it is refused below. A `var *values`
    /// collector is such a parameter too: its type is a pack of a
    /// substituted type, and the body owns its binding. A `None` default is
    /// the same value under every instance. The copy and move initializers,
    /// a receiver or parameter origin, and binders stay outside.
    ///
    /// - `STATEMENTS`: a runtime statement is checked once whatever runs it,
    ///   so `if`, `while`, `break`, `continue`, and a bare `return` neither
    ///   drop nor copy an occurrence. A condition's recorded type is exactly
    ///   `Bool`, which `expect_bool` accepts without a truthiness fact, unless
    ///   the body holds `TRUTHINESS`. A
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
    /// - `REPLAYED_TRANSFERS`: see [`Self::body_transfers`] and
    ///   [`Self::realize_transfers`]. The template records each replayed
    ///   transfer, the origins it merged, and the effect its frame derived,
    ///   every source by the binding it is rooted at; an instance replays
    ///   them again, dropping a source whose binding is plain data
    ///   ([`TemplateObligation::ReplayedTransfers`]).
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
    ///   template binds it, and substituted by nothing, so a construction
    ///   of one records the same adjustment in every clone (see
    ///   [`BodyShape::binder_construction`]).
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
    /// - `RAISES`: see [`BodyShape::raised`] and [`effect_derives`]. A
    ///   `raise` records nothing of its own, a raising call's effect and
    ///   contract carry the callee's error type, which substitutes, and the
    ///   judgment an instance repeats at either holds under every
    ///   substitution the template's holds under.
    /// - `CONSUMING_CALLS`: see [`BodyShape::consuming_call`] and
    ///   `consuming_nominal_contract`. The receiver's move is recorded at its
    ///   `^` transfer, and a named `deinit self` destructor's mark is the
    ///   struct's declaration, so an instance changes only the target, as a
    ///   sibling call's.
    /// - `COPIED_RECEIVERS`: see [`BodyShape::copied_consuming_call`]. The
    ///   call's copy of its receiver is decided by the receiver's syntax and
    ///   the callee's convention, and the instance owes it at its own type.
    /// - `DIRECT_CALLS`: see `method_direct_calls`. A callee taking closed
    ///   scalars, the member of an overload set (`range(n)`) or a generic
    ///   one applied to types (`unsafe_alloc[Self.T](n)`), is selected alike
    ///   under every instance, which realizes it as a function template's
    ///   direct call, substituting the application's arguments.
    /// - `ITERATION`: see [`BodyShape::iterable`] and `realize_iterations`.
    ///   The protocol a loop records is selected from the iterable's type
    ///   and resolved against the place it borrows, so an instance keeps the
    ///   place and selects again from its own substituted type; the loop
    ///   variable's binding is a local like any other.
    /// - `SIMD_CONSTRUCTIONS`: see [`BodyShape::simd_construction`]. Only a
    ///   closed construction records its dtype and width, which the instance
    ///   installs as they stand; its value goes to a checker builtin, a
    ///   by-value parameter, or a `var` local.
    /// - `TRUTHINESS`: see [`BodyShape::truthiness_condition`]. A condition
    ///   read whole from a place is marked for `Bool(x)` by its type alone,
    ///   so an instance repeats `expect_bool`'s judgment at its own type.
    /// - `TUPLE_UNPACKS`: see [`BodyShape::tuple_unpack`]. The element reads
    ///   are a function of the value's type and place, from which an
    ///   instance builds them again.
    /// - `PARAMETERIZED_CALLS`: see [`BodyShape::parameterized_call`]. The
    ///   declared compile-time parameters are the callee's, and on a
    ///   non-generic receiver the per-call clone the call targets is the same
    ///   under every instance (`realize_method_call`).
    /// - `COMPREHENSIONS`: see [`BodyShape::comprehension`]. Each clause's
    ///   protocol is an `ITERATION` loop's, and each binder is declared from
    ///   that protocol's binding plan, which an instance selects again
    ///   (`install_comprehension_bindings`).
    /// - `COMPTIME_CONTROL`: a `comptime if` or `comptime for` in a body
    ///   source validation checked. Every arm was checked once with the
    ///   struct's parameters symbolic; an instance keeps the occurrences of
    ///   the arms the elaborator selected, once per unrolled copy, and drops
    ///   the rest with their facts (`CheckedBodyFacts::selected`). A loop
    ///   variable read as a runtime value is the copy's literal, which takes
    ///   a literal's facts (`folded_literals`), and a local declared inside
    ///   the loop is one binding per copy (`renumber_locals`), as in a keyed
    ///   `def`. A `rebind` in such a body is erased and its equality taken on
    ///   faith, which each instance discharges at its own type
    ///   (`TemplateObligation::RebindEqualities`): one that does not hold
    ///   refuses the derivation, and the clone check reports it.
    /// - `NESTED_DEFS`: see [`BodyShape::nested_def`]. A nested `def`'s
    ///   declaration facts are its signature, keyed by its statement and
    ///   substituted, and its captures name the body's own bindings, so an
    ///   instance writes them again under its own statement and bindings
    ///   (`install_nested_defs`); a call of it selects the declaration the
    ///   body introduces, under every instance. A nested body's effect reads
    ///   are the enclosing body's too.
    /// - `STATIC_CALLS`: see [`BodyShape::static_call`]. A static records at
    ///   most its overload member and, behind a leading-dot root, the
    ///   expected type's head. An instance inherits the head, and the member
    ///   too, except that a generic struct's spelled receiver names the
    ///   instance's clone of it (`realize_static_overloads`).
    ///
    /// Any other handle, borrowed receiver, reference result, interior
    /// reference, or copyable read in the body refuses it
    /// ([`BodyShape::references_recorded`]).
    /// The struct binders `select` picks for a member only source
    /// validation checks: a struct specialized whole, whose clones fold them.
    fn validated_struct_binders(&self, select: fn(&[ParamDecl]) -> Vec<&str>) -> Vec<&str> {
        if self.source_validation {
            select(&self.self_decls)
        } else {
            Vec::new()
        }
    }

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
        // A receiver origin naming one of the method's own origin binders
        // (`ref [o] self`) is a signature fact like a `ref` parameter's
        // clause: the binder is kept by every clone (`ORIGIN_PARAMETERS`).
        let receiver_origin_kept = method.self_origin.as_ref().is_none_or(|origins| {
            matches!(origins.as_slice(), [origin]
            if matches!(&origin.kind, ExprKind::Identifier(name)
                if method.type_params.iter().any(|binder| {
                    binder.name == *name && origin_binder(binder)
                })))
        });
        if !(plain_read || owned_receiver || is_static || constructs)
            || !receiver_origin_kept
            || initializer
        {
            return outside(
                "the receiver carries an origin that is not the method's own binder, or is a copy \
                 or move initializer's",
            );
        }
        // A `where` clause is the declaration's constraint: the elaborator
        // mints a clone only where it evaluates true, and a trace exists only
        // for a minted clone (`TemplateObligation::DeclarationConstraints`).
        // An origin binder and a trait-bounded type binder (`[H: Hasher]`)
        // are kept by every clone and bound symbolically as the template
        // binds them, so no fact reads either. The wildcard vector binder is
        // baked by every clone; only source validation sees its body, with
        // the parameter viewed as a lane-shaped vector (`simd_binder_view`),
        // and the elaborated program holds a trap stub in its place.
        // A scalar or `DType` value binder of the method's own (`[n: Int]`,
        // `[dt: DType]`) is folded by every per-call clone, as a value-keyed
        // `def`'s is; a vector over it (`Scalar[dt]`) is value-shaped.
        let simd_binders = method.type_params.iter().any(simd_wildcard_binder);
        // A compile-time callable binder of the method's own is kept by every
        // clone, which the specializer never folds: it names no callable in
        // generic identity (`CALLABLE_BINDERS`).
        let own_decls = &decls[self.self_decls.len().min(decls.len())..];
        let callable_binders = method_callable_binders(method, own_decls);
        let Some(value_binders) = method_value_binders(method, own_decls) else {
            return outside("a method's own value binder is not an 'Int', 'Bool', or 'DType'");
        };
        let bound_type_binder = |binder: &mojito_ast::ast::TypeParam| {
            bound_binder(binder) && !value_binders.contains(&binder.name.as_str())
        };
        if !method.type_params.iter().all(|binder| {
            origin_binder(binder)
                || bound_binder(binder)
                || callable_binders.contains(&binder.name.as_str())
                || (self.source_validation && simd_wildcard_binder(binder))
        }) || !(method.decorators.is_empty() || is_static)
        {
            return outside(
                "the method has binders other than origins, bounded types, and scalar values, \
                 or decorators",
            );
        }
        let raises = method.raises || method.raises_type.is_some();
        // A struct's origin binder is erased from its declarations, and a
        // scalar value binder (`Array[T, length: Int]`) is read in the body
        // as `Self.length`, a runtime read of the reified parameter on the
        // erased path every such struct keeps: neither is substituted. A
        // type pack (`Tuple[*Ts]`) is fixed per instance as a type binder is:
        // the struct is specialized whole, its receiver's arguments are the
        // pack's elements, and an element the body reads by loop index is
        // fixed by the unrolling. Only source validation checks such a body.
        // A `DType` or vector value binder (`_SequentialRange[dtype]`,
        // `AHasher[key]`) keys a struct specialized whole: only source
        // validation checks its members, and each specialization's member
        // folds the values its trace names. The method's own `DType`
        // binders are among the declarations, folded as its scalar ones are.
        let plain_struct = decls.iter().all(|decl| {
            matches!(
                decl,
                ParamDecl::Type {
                    variadic: false,
                    callable_bound: None,
                    ..
                }
            ) || (self.source_validation
                && matches!(
                    decl,
                    ParamDecl::Type {
                        variadic: true,
                        callable_bound: None,
                        ..
                    }
                ))
                || matches!(decl, ParamDecl::Value { ty, variadic: false, .. } if closed_scalar(ty))
                || matches!(decl, ParamDecl::Value { name, ty, variadic: false, .. }
                    if matches!(**ty, Ty::Dtype) && value_binders.contains(&name.as_str()))
                || matches!(decl, ParamDecl::Value { name, .. }
                    if callable_binders.contains(&name.as_str()))
                || (self.source_validation
                    && matches!(decl, ParamDecl::Value { ty, variadic: false, .. }
                        if matches!(**ty, Ty::Dtype | Ty::Simd { .. })))
        });
        if !plain_struct {
            return outside("a struct parameter is not a plain type or scalar value parameter");
        }
        // A `mut` or `ref` parameter is bound from its declared convention
        // alone, and is rooted at its own binding under every instance. What
        // its caller owes lives in the signature, which is checked per clone:
        // that holds an origin clause too, which names a binder, `self`, or
        // another parameter and never a struct parameter's type. A struct
        // that declares an origin parameter is outside `plain_struct`, and no
        // clone is minted for one.
        let plain_params = method.params.iter().all(|parameter| {
            (parameter.kind == mojito_ast::ast::ParamKind::Regular
                || (parameter.kind == mojito_ast::ast::ParamKind::Variadic
                    && parameter.convention == Some(ArgConvention::Var)))
                && matches!(
                    parameter.convention,
                    None | Some(ArgConvention::Var | ArgConvention::Mut | ArgConvention::Ref)
                )
                && parameter
                    .default
                    .as_ref()
                    .is_none_or(|default| matches!(default.kind, ExprKind::None))
                && (parameter.origin.is_none() || parameter.convention == Some(ArgConvention::Ref))
        });
        if !plain_params {
            return outside(
                "a parameter has a default other than 'None', an origin on a convention other \
                 than 'ref', a keyword pack, a variadic not taken 'var', or an 'out' or 'deinit' \
                 convention",
            );
        }
        let origin_parameter = method.type_params.iter().any(origin_binder)
            || method
                .params
                .iter()
                .any(|parameter| parameter.origin.is_some());
        let bound_binders = method.type_params.iter().any(bound_type_binder);
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
        let (pack_struct, packs) = struct_pack_collectors(method, decls);
        let desugars = self.with_desugars.borrow();
        let shape = BodyShape {
            origins: &self.syntax_origins,
            facts,
            structs: &self.structs,
            traits: &self.traits,
            desugars: &desugars,
            desugar_depth: std::cell::Cell::new(0),
            error_binders: RefCell::new(Vec::new()),
            constants: &self.comptimes,
            vector_aliases: self.vector_aliases(),
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
            callable_binders: callable_binders.clone(),
            static_calls: RefCell::new(Vec::new()),
            packs,
            pack_struct,
            loop_vars: RefCell::new(Vec::new()),
            values: value_binders.clone(),
            struct_values: struct_scalar_binders(&self.self_decls),
            struct_lanes: self.validated_struct_binders(struct_lane_binders),
            struct_vectors: self.validated_struct_binders(struct_vector_binders),
            print_calls: RefCell::new(Vec::new()),
            nested_depth: std::cell::Cell::new(0),
            borrowed_params: params_passed(&[ArgConvention::Mut, ArgConvention::Ref]),
            mut_params: params_passed(&[ArgConvention::Mut]),
            keyed: false,
            receiver: method.has_self,
            self_convention: method.self_convention,
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
                if simd_binders {
                    features = features
                        .union(MethodFeatures::STATEMENTS)
                        .union(MethodFeatures::SIMD_BINDERS);
                }
                if !value_binders.is_empty() {
                    features = features
                        .union(MethodFeatures::STATEMENTS)
                        .union(MethodFeatures::VALUE_BINDERS);
                }
                if raises {
                    features = features
                        .union(MethodFeatures::STATEMENTS)
                        .union(MethodFeatures::RAISES);
                }
                if !callable_binders.is_empty() {
                    features = features
                        .union(MethodFeatures::STATEMENTS)
                        .union(MethodFeatures::CALLABLE_BINDERS);
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
            binders: method
                .type_params
                .iter()
                .filter(|binder| bound_type_binder(binder))
                .map(|binder| binder.name.as_str())
                .collect(),
            struct_binders: self.self_decls.iter().map(ParamDecl::id).collect(),
            binder_constructions: RefCell::new(Vec::new()),
            repr_calls: RefCell::new(Vec::new()),
            lane_binders: method
                .type_params
                .iter()
                .filter(|binder| simd_wildcard_binder(binder))
                .flat_map(|binder| {
                    [
                        format!("{}.dtype", binder.name),
                        format!("{}.size", binder.name),
                    ]
                })
                .collect(),
            simd_to_bits: RefCell::new(Vec::new()),
            simd_casts: RefCell::new(Vec::new()),
            simd_lengths: RefCell::new(Vec::new()),
            pack_relocations: RefCell::new(Vec::new()),
            stringified: RefCell::new(Vec::new()),
        };
        if !shape.block(&method.body) {
            return outside("the body is outside the method grammar");
        }
        let class = || {
            let features = shape.features.get();
            let coverage = TemplateCoverage::Certified(if features.is_empty() {
                TemplateClass::MethodScalarBody
            } else {
                TemplateClass::MethodBody(features)
            });
            (coverage, shape.grammar_notes())
        };
        let Some(facts) = facts else {
            return class();
        };
        if let Some(what) = self.method_facts_refusal(&shape, facts) {
            return outside(what);
        }
        class()
    }

    /// Why a method body the grammar admitted still falls outside its class,
    /// judged on the facts its check recorded; `None` when none applies.
    fn method_facts_refusal(
        &self,
        shape: &BodyShape<'_>,
        facts: &CheckedBodyFacts,
    ) -> Option<&'static str> {
        if !shape.references_recorded(facts) {
            return Some("a reference is yielded or kept outside the method grammar");
        }
        if stray_method_call(facts, shape) {
            return Some("the body calls something other than a trivial method");
        }
        // A residue the body publishes or reads is republished for an
        // instance, but only a body that calls or forwards its own callable
        // parameter records one the recipe covers.
        let residue = !facts.call_throughs.is_empty() || !facts.call_through_reads.is_empty();
        if residue
            && !shape.holds(MethodFeatures::CALLABLE_PARAMETERS)
            && !shape.holds(MethodFeatures::CALLABLE_BINDERS)
        {
            return Some("a keyed body publishes or reads a call-through residue");
        }
        // A residue naming a compile-time callable names one of the method's
        // own binders, which every clone keeps under its name.
        let own_binder_residues = facts
            .call_throughs
            .iter()
            .all(|residue| match &residue.callee {
                mojito_checked::checked::CallThroughCallee::RuntimeParam(_) => true,
                mojito_checked::checked::CallThroughCallee::ValueParam(name) => {
                    shape.callable_binders.contains(&name.as_str())
                }
            });
        if !own_binder_residues {
            return Some("a residue names a compile-time callable that is not the method's own");
        }
        if facts.replays_transfers()
            && (shape.keyed || !shape.holds(MethodFeatures::REPLAYED_TRANSFERS))
        {
            return Some("a keyed body replays a transfer summary");
        }
        let effects_closed = facts
            .expression_effects
            .iter()
            .all(|(_, effects)| effect_derives(effects));
        let binder_constructions = shape.binder_constructions.borrow();
        let adjustments_derive = facts.operation_adjustments.iter().all(|(id, adjustment)| {
            binder_constructions.contains(id)
                || adjustment_derives(adjustment)
                || matches!(adjustment,
                    mojito_checked::checked::SemanticAdjustment::MaterializeLiteral(target)
                        if shape.struct_lane_simd(target))
        });
        if !effects_closed || !adjustments_derive {
            return Some("an expression has an effect or an adjustment with no recipe");
        }
        let wrote_through_origin = self
            .parametric_write_frames
            .borrow()
            .last()
            .is_some_and(|frame| !frame.is_empty());
        if wrote_through_origin {
            return Some("the body writes through an origin parameter");
        }
        None
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

    /// The in-place dunder each augmented assignment to a place at the
    /// body's occurrences selects, keyed at the place.
    fn captured_inplace_updates(
        &self,
        occurrences: &[Occurrence],
        local_contract: &dyn Fn(
            mojito_checked::checked::CheckedCallContract,
        ) -> Result<TemplateCallContract, IncompleteReason>,
    ) -> Result<Vec<(OccurrenceId, TemplateCallContract)>, IncompleteReason> {
        values(occurrences, &self.operation_adjustments.borrow())
            .into_iter()
            .filter_map(|(id, adjustment)| match adjustment {
                mojito_checked::checked::SemanticAdjustment::AugmentedInPlace(call) => {
                    Some((id, *call))
                }
                _ => None,
            })
            .map(|(id, call)| local_contract(call).map(|call| (id, call)))
            .collect()
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

    /// Whether the adjustment at `site` constructs a binder that is not the
    /// enclosing struct's, which every clone keeps symbolic
    /// ([`BodyShape::binder_construction`]).
    fn own_binder_construction(
        &self,
        site: &SourceSpan,
        adjustment: &mojito_checked::checked::SemanticAdjustment,
    ) -> bool {
        matches!(
            adjustment,
            mojito_checked::checked::SemanticAdjustment::ConstructTypeParam { .. }
        ) && matches!(self.expression_types.borrow().get(site),
            Some(Ty::Param { binder, .. })
                if self.self_decls.iter().all(|decl| *decl.id() != binder.id))
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
            .unkeyed_growth(site.body, baseline)
            .into_iter()
            .map(|store| format!("store:{store}"))
            .collect();
        if self.symbolic_hash_leaf(baseline) {
            reasons.push(format!("store:{SYMBOLIC_HASH_LEAVES}"));
        }
        if self.transfer_residue(reads) {
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
                        | mojito_checked::checked::SemanticAdjustment::AugmentedInPlace(_)
                ) || self.kept_element_store(&occurrence.span, adjustment)
                    || self.own_binder_construction(&occurrence.span, adjustment);
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
        self.occurrences_over(body, &self.with_desugars.borrow(), None)
    }

    /// The occurrences of `body` with each `with` statement's children read
    /// from its desugar in `desugars`: the statement stays an occurrence,
    /// and the nodes the desugar synthesized are occurrences beside the ones
    /// it kept from the source. `template` holds the occurrences of the
    /// checked template an instance body is matched against.
    fn occurrences_over(
        &self,
        body: &[Stmt],
        desugars: &HashMap<SourceSpan, super::with_stmt::WithDesugar>,
        template: Option<&[OccurrenceId]>,
    ) -> Vec<Occurrence> {
        struct Occurrences<'a> {
            origins: &'a mojito_ast::ast::SyntaxOrigins,
            template: Option<&'a [OccurrenceId]>,
            found: Vec<Occurrence>,
            copies: HashMap<SyntaxId, u32>,
            /// The `DType` object of each `DType.<member>` constant met so
            /// far, which is part of the constant rather than an occurrence:
            /// the elaborator folds a `DType` binder to such a constant under
            /// the name's identity alone.
            constant_objects: HashSet<SyntaxId>,
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
                self.push_plain(id, statement.source_span());
                if let StmtKind::AugAssign { place, value, .. } = &statement.kind
                    && let Some(occurrence) = self.found.last_mut()
                {
                    occurrence.augmented = Some((
                        self.origins.origin(place.syntax_id),
                        self.origins.origin(value.syntax_id),
                    ));
                }
            }

            fn visit_expr(&mut self, expr: &Expr) {
                self.visit_expression(expr);
                // A slice argument's descriptor, which the check synthesizes
                // under an identity derived from the subscript's
                // (`synthetic_slice_descriptor`), follows its subscript.
                let descriptors: Vec<usize> = match &expr.kind {
                    ExprKind::Slice { .. } => vec![0],
                    ExprKind::MultiIndex { args, .. } => args
                        .iter()
                        .enumerate()
                        .filter(|(_, argument)| {
                            matches!(
                                argument,
                                mojito_ast::ast::SubscriptArg::Slice { .. }
                                    | mojito_ast::ast::SubscriptArg::KeywordSlice { .. }
                            )
                        })
                        .map(|(position, _)| position)
                        .collect(),
                    _ => Vec::new(),
                };
                for position in descriptors {
                    let syntax = SyntaxId::derived(
                        expr.syntax_id,
                        u32::try_from(position).unwrap_or(u32::MAX),
                    );
                    let id = self.next_copy(syntax);
                    self.push_plain(
                        id,
                        SourceSpan::syntax(expr.source.clone(), expr.span, syntax),
                    );
                }
            }
        }

        impl Occurrences<'_> {
            /// An occurrence that is neither a call, a name, nor an operator.
            fn push_plain(&mut self, id: OccurrenceId, span: SourceSpan) {
                self.found.push(Occurrence {
                    id,
                    span,
                    callee: None,
                    arguments: Vec::new(),
                    keywords: Vec::new(),
                    identifier: false,
                    method_call: None,
                    type_receiver: None,
                    operator: None,
                    prefix: None,
                    augmented: None,
                    transfer: false,
                    ranking: ArgumentRanking::default(),
                    literal: None,
                    folded_index: None,
                    dimensions: Vec::new(),
                    compile_time_literals: Vec::new(),
                    literal_kind: None,
                    vector_fold: None,
                });
            }

            fn visit_expression(&mut self, expr: &Expr) {
                if self.constant_objects.contains(&expr.syntax_id) {
                    return;
                }
                if let ExprKind::Member { object, .. } = &expr.kind
                    && matches!(&object.kind, ExprKind::Identifier(name) if name == "DType")
                {
                    self.constant_objects.insert(object.syntax_id);
                    // The constant the elaborator writes for a type-position
                    // binder (`Scalar[Self.dtype]`) takes a fresh identity:
                    // the template spelled a type there, not an occurrence.
                    // A value-position binder (`Scalar[dt]`) folds under the
                    // name's identity, which the template checked: matched
                    // against that template, the constant stands for the
                    // name's occurrence ([`VectorFold::Constant`]).
                    let origin = self.origins.origin(expr.syntax_id);
                    if origin.is_fresh() {
                        if self
                            .template
                            .is_some_and(|template| template.iter().any(|id| id.syntax == origin))
                        {
                            let id = self.next_copy(expr.syntax_id);
                            self.push_plain(id, expr.source_span());
                            if let Some(occurrence) = self.found.last_mut() {
                                occurrence.vector_fold = Some(VectorFold::Constant);
                            }
                        }
                        return;
                    }
                }
                let id = self.next_copy(expr.syntax_id);
                self.found.push(Occurrence {
                    id,
                    span: expr.source_span(),
                    callee: match &expr.kind {
                        ExprKind::Call { name, .. } => Some(name.clone()),
                        _ => None,
                    },
                    arguments: match &expr.kind {
                        ExprKind::Call { args, .. }
                        | ExprKind::MethodCall { args, .. }
                        | ExprKind::Invoke { args, .. } => args
                            .iter()
                            .map(|argument| self.origins.origin(argument.syntax_id))
                            .collect(),
                        _ => Vec::new(),
                    },
                    keywords: match &expr.kind {
                        ExprKind::Call { kwargs, .. }
                        | ExprKind::MethodCall { kwargs, .. }
                        | ExprKind::Invoke { kwargs, .. } => kwargs
                            .iter()
                            .map(|keyword| {
                                (
                                    keyword.name.clone(),
                                    self.origins.origin(keyword.value.syntax_id),
                                )
                            })
                            .collect(),
                        _ => Vec::new(),
                    },
                    identifier: matches!(expr.kind, ExprKind::Identifier(_)),
                    method_call: match &expr.kind {
                        ExprKind::MethodCall { object, method, .. } => {
                            Some((self.origins.origin(object.syntax_id), method.clone()))
                        }
                        // `receiver.method[…](…)`, a method call with
                        // explicit compile-time arguments.
                        ExprKind::Invoke { callee, .. } => match &callee.kind {
                            ExprKind::Member { object, field } => {
                                Some((self.origins.origin(object.syntax_id), field.clone()))
                            }
                            _ => None,
                        },
                        ExprKind::Index { object, .. } | ExprKind::MultiIndex { object, .. } => {
                            Some((
                                self.origins.origin(object.syntax_id),
                                "__getitem__".to_string(),
                            ))
                        }
                        _ => None,
                    },
                    type_receiver: match &expr.kind {
                        ExprKind::MethodCall { object, .. } => match &object.kind {
                            ExprKind::TypeApply { name, args } => {
                                Some((name.clone(), args.clone()))
                            }
                            _ => None,
                        },
                        _ => None,
                    },
                    operator: match &expr.kind {
                        ExprKind::Infix(op, left, right) if operator_dispatch(*op) => Some((
                            *op,
                            self.origins.origin(left.syntax_id),
                            self.origins.origin(right.syntax_id),
                            super::places::is_place_expr(right),
                        )),
                        _ => None,
                    },
                    prefix: match &expr.kind {
                        ExprKind::Prefix(
                            op @ (mojito_ast::ast::PrefixOp::Neg
                            | mojito_ast::ast::PrefixOp::Invert),
                            value,
                        ) => Some((*op, self.origins.origin(value.syntax_id))),
                        _ => None,
                    },
                    augmented: None,
                    transfer: matches!(expr.kind, ExprKind::Transfer(_)),
                    ranking: ArgumentRanking {
                        context_free: context_free(expr),
                        owned: super::overload_support::argument_is_owned(expr),
                    },
                    literal: match &expr.kind {
                        ExprKind::Int(value) => Some(value.to_i64().map_or_else(
                            || mojito_types::ct::CtValue::IntLiteral(value.clone()),
                            mojito_types::ct::CtValue::Int,
                        )),
                        ExprKind::Bool(value) => Some(mojito_types::ct::CtValue::Bool(*value)),
                        _ => None,
                    },
                    folded_index: match &expr.kind {
                        ExprKind::Index { index, .. } => match &index.kind {
                            ExprKind::Int(value) => value
                                .to_i64()
                                .map(|value| (self.origins.origin(index.syntax_id), value)),
                            _ => None,
                        },
                        _ => None,
                    },
                    dimensions: match &expr.kind {
                        ExprKind::Call {
                            name, param_args, ..
                        } if name == "SIMD" => param_args
                            .iter()
                            .filter_map(|argument| match argument {
                                mojito_ast::ast::ParamArg::Value(value)
                                    if matches!(value.kind, ExprKind::Int(_)) =>
                                {
                                    Some(self.origins.origin(value.syntax_id))
                                }
                                _ => None,
                            })
                            .collect(),
                        _ => Vec::new(),
                    },
                    compile_time_literals: match &expr.kind {
                        ExprKind::Call { param_args, .. } => param_args
                            .iter()
                            .map(|argument| match argument {
                                mojito_ast::ast::ParamArg::Value(value) => match &value.kind {
                                    ExprKind::Int(value) => {
                                        value.to_i64().map(mojito_types::ct::CtValue::Int)
                                    }
                                    ExprKind::Bool(value) => {
                                        Some(mojito_types::ct::CtValue::Bool(*value))
                                    }
                                    _ => None,
                                },
                                _ => None,
                            })
                            .collect(),
                        _ => Vec::new(),
                    },
                    literal_kind: match expr.kind {
                        ExprKind::Int(_) => Some(LiteralKind::Int),
                        ExprKind::Float(_) => Some(LiteralKind::Float),
                        ExprKind::Bool(_) => Some(LiteralKind::Bool),
                        _ => None,
                    },
                    vector_fold: None,
                });
            }
        }

        let mut occurrences = Occurrences {
            origins: &self.syntax_origins,
            template,
            found: Vec::new(),
            copies: HashMap::new(),
            constant_objects: HashSet::new(),
        };
        let expanded = expand_with_statements(body, desugars);
        mojito_ast::visit::walk_block(&mut occurrences, expanded.as_deref().unwrap_or(body));
        occurrences.found
    }

    /// The desugar of each `with` statement of an instance body, nested ones
    /// included, built from its own syntax in the form the template's
    /// statement recorded; `None` when a statement has no recorded form.
    fn instance_with_desugars(
        &self,
        body: &[Stmt],
        forms: &[(OccurrenceId, WithForm)],
    ) -> Option<HashMap<SourceSpan, super::with_stmt::WithDesugar>> {
        struct Withs(Vec<Stmt>);

        impl mojito_ast::visit::Visitor for Withs {
            fn visit_stmt(&mut self, statement: &Stmt) {
                if matches!(statement.kind, StmtKind::With { .. }) {
                    self.0.push(statement.clone());
                }
            }
        }

        let mut pending = Withs(Vec::new());
        mojito_ast::visit::walk_block(&mut pending, body);
        let mut desugars = HashMap::new();
        while let Some(statement) = pending.0.pop() {
            let span = statement.source_span();
            if desugars.contains_key(&span) {
                continue;
            }
            let syntax = self.syntax_origins.origin(statement.syntax_id);
            let form = forms
                .iter()
                .find(|(id, _)| id.syntax == syntax)
                .map(|(_, form)| *form)?;
            let statements = super::with_stmt::with_desugar(&statement, form)?;
            mojito_ast::visit::walk_block(&mut pending, &statements);
            desugars.insert(span, super::with_stmt::WithDesugar { form, statements });
        }
        Some(desugars)
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
        self.capturable(body, &occurrences, baseline, reads)?;
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
        let inplace_updates = self.captured_inplace_updates(&occurrences, &template_contract)?;
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
        let transfers = self.body_transfers(
            &occurrences,
            baseline,
            reads,
            param_owners,
            &local_owner,
            &local_place,
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
        let (nested_defs, capture_accesses) =
            self.captured_nested_defs(body, &occurrences, &local_owner, &local_place)?;
        Ok(CheckedBodyFacts {
            expression_types,
            construction_immutable_binders: self.captured_immutable_binders(&occurrences),
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
                    !kept_apart(adjustment)
                        && !self.kept_element_store_at(&occurrences, *id, adjustment)
                })
                .collect(),
            reference_results,
            augmented_subscripts,
            inplace_updates,
            interior_references: values(&occurrences, &self.interior_references.borrow())
                .into_iter()
                .map(|(id, place)| local_place(&place).map(|place| (id, place)))
                .collect::<Result<Vec<_>, _>>()?,
            copyable_reference_result_reads: keyed(&|span| {
                self.copyable_reference_result_reads.borrow().contains(span)
            }),
            generic_instantiations: values(&occurrences, &self.generic_instantiations.borrow()),
            overload_targets: values(&occurrences, &self.overload_targets.borrow()),
            call_parameters: captured_call_parameters(&occurrences, &self.call_parameters.borrow()),
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
            explicit_destroy_calls: keyed(&|span| {
                self.explicit_destroy_calls.borrow().contains(span)
            }),
            implicitly_copied_consuming_receivers: keyed(&|span| {
                self.implicitly_copied_consuming_receivers
                    .borrow()
                    .contains(span)
            }),
            truthiness_conditions: keyed(&|span| {
                self.truthiness_conditions.borrow().contains(span)
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
            simd_constructions: values(&occurrences, &self.simd_constructions.borrow()),
            contextual_bases: values(&occurrences, &self.contextual_bases.borrow()),
            parameterized_method_calls: values(
                &occurrences,
                &self.parameterized_method_calls.borrow(),
            ),
            view_result_interiors: values(&occurrences, &self.view_result_interiors.borrow()),
            iterations: self.captured_iterations(&occurrences, &local_place)?,
            comprehension_bindings: self
                .captured_comprehension_bindings(&occurrences, &local_owner)?,
            nested_defs,
            capture_accesses,
            with_forms: values(&occurrences, &self.with_desugars.borrow())
                .into_iter()
                .map(|(id, desugar)| (id, desugar.form))
                .collect(),
            tuple_unpacks: self.captured_tuple_unpacks(&occurrences, &local_reference)?,
            call_place_uses: keyed(&|span| self.call_place_uses.borrow().contains(span)),
            transfers: occurrences
                .iter()
                .filter(|occurrence| occurrence.transfer)
                .map(|occurrence| occurrence.id)
                .collect(),
            call_transfers: transfers.call_transfers,
            transferred_origins: transfers.transferred_origins,
            transfer_effects: transfers.transfer_effects,
            transfer_reads: transfers.transfer_reads,
            call_throughs,
            call_through_reads,
            conversions: self.body_conversions(&occurrences),
            method_instantiations: values(&occurrences, &self.method_instantiations.borrow()),
            hash_leaves: self.hash_leaves_since(baseline.hash_leaf_demands),
            locals: owner_end - baseline.owner_start,
            occurrences: occurrences
                .into_iter()
                .map(|occurrence| occurrence.id)
                .collect(),
            // The lists the certificate fills from the grammar (`operators`,
            // `bound_builtins`, `constructions`, `callable_calls`,
            // `repr_calls`, `print_calls`, `simd_to_bits`, `simd_casts`,
            // `simd_lengths`)
            // stay empty in a capture (`record_template`).
            ..CheckedBodyFacts::default()
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
        body: &[Stmt],
        occurrences: &[Occurrence],
        baseline: &BodyFactBaseline,
        reads: &BodyReads,
    ) -> Result<(), IncompleteReason> {
        if let Some(store) = self.unkeyed_growth(body, baseline).first() {
            return Err(IncompleteReason::UnkeyedFact(store));
        }
        // The body's locals are told from every other binding by their
        // identity range; a check whose range split (`reserve_owners`)
        // has no such range.
        if self.owner_range_split.get() {
            return Err(IncompleteReason::UnkeyedFact("binding identities"));
        }
        if self.symbolic_hash_leaf(baseline) {
            return Err(IncompleteReason::UnkeyedFact(SYMBOLIC_HASH_LEAVES));
        }
        if self.transfer_residue(reads) {
            return Err(IncompleteReason::UnkeyedFact("transfer effects"));
        }
        // A residue is republished verbatim, so it may name only slots,
        // signature places, and compile-time callables by name, which the
        // certificate admits only for the method's own kept binders; a
        // carried origin exists only while the argument's type carries a
        // loan (`TemplateObligation::CallThroughResidue`).
        let republishable = self.transfer_frames.borrow().last().is_none_or(|frame| {
            frame
                .call_throughs
                .iter()
                .all(|residue| residue.args.iter().all(|arg| arg.carried.is_empty()))
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
            if recorded > 0 && !derivable_table(table) {
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
        // A type is retained as written, and a binding identity inside one
        // would never be remapped for an instance. Two exceptions are kept by
        // template owner: a reference at the top of a place or binding type
        // (`rooted_reference`), and a struct's origin arguments and a
        // pointer's own provenance at the top of a type (`typed_origins`); a
        // reference below the top stays refused.
        let expression_types = self.expression_types.borrow();
        let kept_apart = [
            &*self.expression_place_types.borrow(),
            &*self.binding_types.borrow(),
        ];
        let abstracted = |ty: &Ty| {
            typed_origins(ty, &|place| {
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
        let local_start = self
            .reserve_owners(facts.locals)
            .map_err(|_| corrupt("its binding identity range"))?;
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
            TemplateOwner::CompileTimeParam(name) => param_owners
                .compile_time
                .iter()
                .find(|(kept, _)| kept == name)
                .map(|(_, owner)| *owner)
                .ok_or_else(|| corrupt("the fold of a compile-time parameter")),
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
                    |typed| bind_typed_origins(ty, typed, &rooted),
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
        self.install_call_results(facts, &span, &rooted)?;
        self.install_transfers(facts, &span, &rooted, &owner)?;
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
        for (id, dimensions) in &facts.simd_constructions {
            self.simd_constructions
                .borrow_mut()
                .insert(span(id)?, *dimensions);
        }
        for (id, base) in &facts.contextual_bases {
            self.contextual_bases
                .borrow_mut()
                .insert(span(id)?, base.clone());
        }
        for (id, decls) in &facts.parameterized_method_calls {
            self.parameterized_method_calls
                .borrow_mut()
                .insert(span(id)?, decls.clone());
        }
        self.install_iterations(facts, &span, &rooted)?;
        self.install_comprehension_bindings(facts, &span, &owner)?;
        self.install_nested_defs(facts, &span, &owner, &rooted)?;
        self.install_tuple_unpacks(facts, &span, &referenced)?;
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
        // A construction's immutable-binder record: the kept one where it
        // binds a slot immutably, else the empty one `infer_construction`
        // writes at every construction.
        for id in &facts.constructions {
            self.construction_immutable_binders
                .borrow_mut()
                .insert(span(id)?, Vec::new());
        }
        for (id, binders) in &facts.construction_immutable_binders {
            self.construction_immutable_binders
                .borrow_mut()
                .insert(span(id)?, binders.clone());
        }
        for (id, writable) in &facts.reference_value_uses {
            self.reference_value_uses
                .borrow_mut()
                .insert(span(id)?, *writable);
        }
        self.install_occurrence_marks(facts, &span)?;
        let checked_contract = |call: &TemplateCallContract| {
            checked_contract(call, &span, &placed, &rooted, &referenced)
        };
        for (id, call) in &facts.selected_calls {
            self.selected_calls
                .borrow_mut()
                .insert(span(id)?, checked_contract(call)?);
        }
        self.install_element_stores(facts, &span, &checked_contract)?;
        self.install_inplace_updates(facts, &span, &checked_contract)?;
        // The body's own source decides, exactly as it does for an inferred
        // body, whether an application it reaches is user-reachable.
        let source = spans.values().next().and_then(|span| span.source.clone());
        for (template, arguments) in &facts.struct_applications {
            self.record_struct_instantiation(template, arguments, source.as_deref());
        }
        for callee in &facts.effect_free_callees {
            self.note_body_effect_read(callee, ObservedEffects::Transfers(Vec::new()));
            self.note_body_effect_read(callee, ObservedEffects::CallThroughs(Vec::new()));
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
            self.note_body_effect_read(callee, ObservedEffects::CallThroughs(residue.clone()));
            self.call_through_observations
                .borrow_mut()
                .entry(callee.clone())
                .or_insert_with(|| residue.clone());
        }
        for (callee, read) in &facts.transfer_reads {
            self.note_body_effect_read(callee, ObservedEffects::Transfers(read.clone()));
            self.note_body_effect_read(callee, ObservedEffects::CallThroughs(Vec::new()));
            self.effect_observations
                .borrow_mut()
                .entry(callee.clone())
                .or_insert_with(|| read.clone());
            self.call_through_observations
                .borrow_mut()
                .entry(callee.clone())
                .or_default();
        }
        for leaf in &facts.hash_leaves {
            self.record_hash_leaf(leaf);
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

    /// The immutable-binder records a body keeps. A view-returning call's is
    /// the immutable slots of its result origins, which installation derives
    /// again, and an empty one is what installation writes at every
    /// construction; any other names slots and field paths alone.
    fn captured_immutable_binders(
        &self,
        occurrences: &[Occurrence],
    ) -> Vec<(OccurrenceId, Vec<super::ImmutableOriginBinder>)> {
        let binders = self.construction_immutable_binders.borrow();
        let result_origins = self.call_result_origins.borrow();
        occurrences
            .iter()
            .filter_map(|occurrence| {
                let record = binders.get(&occurrence.span)?;
                let derived = result_origins
                    .get(&occurrence.span)
                    .is_some_and(|slots| *record == call_result_immutable_binders(slots));
                (!record.is_empty() && !derived).then(|| (occurrence.id, record.clone()))
            })
            .collect()
    }

    /// Install what a view-returning call records about its result: the
    /// origins its contract binds, the immutable binders those imply, and
    /// the owned-interior tags its callee's return origin projects.
    fn install_call_results(
        &self,
        facts: &CheckedBodyFacts,
        span: &dyn Fn(&OccurrenceId) -> Result<SourceSpan, TypeError>,
        rooted: &dyn Fn(&TemplatePlace) -> Result<mojito_types::origin::OriginPlace, TypeError>,
    ) -> Result<(), TypeError> {
        for (id, slots) in &facts.call_result_origins {
            let resolved = slots
                .iter()
                .map(|resolved| {
                    Ok((
                        resolved.slot,
                        checked_origin(&resolved.origin, rooted)?,
                        resolved.mutability,
                    ))
                })
                .collect::<Result<Vec<_>, TypeError>>()?;
            let binders = call_result_immutable_binders(&resolved);
            if !binders.is_empty() {
                self.construction_immutable_binders
                    .borrow_mut()
                    .insert(span(id)?, binders);
            }
            self.call_result_origins
                .borrow_mut()
                .insert(span(id)?, resolved);
        }
        for (id, tags) in &facts.view_result_interiors {
            self.view_result_interiors
                .borrow_mut()
                .insert(span(id)?, tags.clone());
        }
        Ok(())
    }

    /// Install a derivation's replayed transfers: the call transfers at
    /// their spans, the merged origins at the instance's own bindings, and
    /// the effects on the body's frame, which publishes them under the body's
    /// key when it is popped, as an inferred body's would be.
    fn install_transfers(
        &self,
        facts: &CheckedBodyFacts,
        span: &dyn Fn(&OccurrenceId) -> Result<SourceSpan, TypeError>,
        rooted: &dyn Fn(&TemplatePlace) -> Result<mojito_types::origin::OriginPlace, TypeError>,
        owner: &dyn Fn(&TemplateOwner) -> Result<OwnerId, TypeError>,
    ) -> Result<(), TypeError> {
        use mojito_checked::checked::{CheckedCallTransfer, CheckedTransferDest};
        for (id, transfers) in &facts.call_transfers {
            let resolved = transfers
                .iter()
                .map(|transfer| {
                    Ok(CheckedCallTransfer {
                        dest: match transfer.dest {
                            TemplateTransferDest::Receiver => CheckedTransferDest::Receiver,
                            TemplateTransferDest::Argument(index) => {
                                CheckedTransferDest::Argument(index)
                            }
                        },
                        dest_path: transfer.dest_path.clone(),
                        sources: transfer
                            .sources
                            .iter()
                            .map(|source| checked_origin(&source.origin, rooted))
                            .collect::<Result<_, _>>()?,
                        mutable: transfer.mutable,
                    })
                })
                .collect::<Result<Vec<_>, TypeError>>()?;
            self.call_transfers.borrow_mut().insert(span(id)?, resolved);
        }
        for (dest, sources) in &facts.transferred_origins {
            let dest = owner(dest)?;
            let origins = sources
                .iter()
                .map(|source| checked_origin(&source.origin, rooted))
                .collect::<Result<Vec<_>, TypeError>>()?;
            let mut overlay = self.transferred_origins.borrow_mut();
            let merged = overlay.entry(dest).or_default();
            for origin in origins {
                if !merged.contains(&origin) {
                    merged.push(origin);
                }
            }
        }
        if let Some(frame) = self.transfer_frames.borrow_mut().last_mut() {
            for effect in &facts.transfer_effects {
                frame.record(effect.effect.clone(), effect.latent.clone());
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
        // A `with` desugar is the body's syntax, which the derived facts
        // describe; it outlives the clear.
        let desugars: Vec<_> = {
            let recorded = self.with_desugars.borrow();
            occurrences
                .iter()
                .filter_map(|occurrence| {
                    let desugar = recorded.get(&occurrence.span)?;
                    Some((occurrence.span.clone(), desugar.clone()))
                })
                .collect()
        };
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
        )?;
        self.with_desugars.borrow_mut().extend(desugars);
        Ok(())
    }

    /// Install the bare per-occurrence marks: temporaries no one consumes,
    /// discarded results, explicit-destroy calls, consuming calls on a copied
    /// receiver, truthiness conditions, and each binding's deletability.
    fn install_occurrence_marks(
        &self,
        facts: &CheckedBodyFacts,
        span: &dyn Fn(&OccurrenceId) -> Result<SourceSpan, TypeError>,
    ) -> Result<(), TypeError> {
        for id in &facts.unconsumed_temporaries {
            self.unconsumed_temporaries.borrow_mut().insert(span(id)?);
        }
        for id in &facts.discarded_reference_results {
            self.discarded_reference_results
                .borrow_mut()
                .insert(span(id)?);
        }
        for id in &facts.explicit_destroy_calls {
            self.explicit_destroy_calls.borrow_mut().insert(span(id)?);
        }
        for id in &facts.implicitly_copied_consuming_receivers {
            self.implicitly_copied_consuming_receivers
                .borrow_mut()
                .insert(span(id)?);
        }
        for id in &facts.linear_temporaries {
            self.linear_temporaries.borrow_mut().insert(span(id)?);
        }
        for id in &facts.truthiness_conditions {
            self.truthiness_conditions.borrow_mut().insert(span(id)?);
        }
        let mut deletability = self.explicit_destroy_deletability.borrow_mut();
        for id in &facts.deletable_bindings {
            deletability.bindings.insert(span(id)?);
        }
        for id in &facts.linear_bindings {
            deletability.linear_bindings.insert(span(id)?);
        }
        Ok(())
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

    /// Install each in-place update as the adjustment at its place, as the
    /// checker's own selection records it.
    fn install_inplace_updates(
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
        for (id, call) in &facts.inplace_updates {
            self.operation_adjustments.borrow_mut().insert(
                span(id)?,
                mojito_checked::checked::SemanticAdjustment::AugmentedInPlace(Box::new(
                    checked_contract(call)?,
                )),
            );
        }
        Ok(())
    }

    /// The unkeyed stores the body's check grew beyond the entries its
    /// nested `def` statements key, which their recipe carries. The
    /// transferred-origin store is told entry by entry elsewhere.
    fn unkeyed_growth(&self, body: &[Stmt], baseline: &BodyFactBaseline) -> Vec<&'static str> {
        let nested = self.nested_def_entries(body);
        self.unkeyed_fact_entries()
            .into_iter()
            .zip(baseline.unkeyed)
            .zip(nested.into_iter().zip(baseline.nested_defs))
            .filter(|(((store, now), (_, before)), (keyed, keyed_before))| {
                let grown = now.checked_sub(*before);
                let recipe = keyed.checked_sub(*keyed_before);
                *store != TRANSFERRED_ORIGINS && grown != recipe
            })
            .map(|(((store, _), _), _)| store)
            .collect()
    }

    /// Entries in the fact stores a body inference can grow that are not
    /// keyed by one of its occurrences. A derivation has a recipe only for
    /// the entries a nested `def` statement keys (`unkeyed_growth`), so any
    /// other growth refuses the body and names the store.
    fn unkeyed_fact_entries(&self) -> [(&'static str, usize); UNKEYED_STORES] {
        let deletability = self.explicit_destroy_deletability.borrow();
        [
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

    /// The leaves hashed since the demand log held `start` entries,
    /// deduplicated and in canonical order, so a template's bundle and an
    /// instance's compare whatever order their checks demanded them in.
    fn hash_leaves_since(&self, start: usize) -> Vec<Ty> {
        canonical_hash_leaves(self.hash_leaf_demands.borrow()[start..].to_vec())
    }

    /// Whether the body's check hashed a leaf whose type names a parameter.
    fn symbolic_hash_leaf(&self, baseline: &BodyFactBaseline) -> bool {
        self.hash_leaf_demands.borrow()[baseline.hash_leaf_demands..]
            .iter()
            .any(mojito_types::types::is_symbolic)
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

/// The generated Tuple accessor a subscript at a literal index selects, one
/// member per position (`__getitem_param__$k`).
const TUPLE_ELEMENT_ACCESSOR: &str = "__getitem_param__";

/// The unkeyed store a replayed transfer merges origins into.
const TRANSFERRED_ORIGINS: &str = "transferred origins";

/// The refusal a body's check that hashed a symbolic leaf names: an instance
/// could not record that leaf again, since its type is not every instance's.
const SYMBOLIC_HASH_LEAVES: &str = "symbolic hash leaf types";

/// Hash leaves deduplicated, in the order of their debug spelling.
fn canonical_hash_leaves(mut leaves: Vec<Ty>) -> Vec<Ty> {
    leaves.sort_by_cached_key(|leaf| format!("{leaf:?}"));
    leaves.dedup();
    leaves
}

/// How many fact stores `unkeyed_fact_entries` watches.
const UNKEYED_STORES: usize = 6;

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

impl<V: std::fmt::Debug> SpanKeyed for mojito_checked::fact_store::FactMap<SourceSpan, V> {
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

impl SpanKeyed for mojito_checked::fact_store::FactSet<SourceSpan> {
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

/// The features a [`TemplateClass::FunctionBody`] may hold: runtime
/// statements, whole values moved or copied between a parameter, a local, an
/// argument, and the result, a runtime `for` over a place, a condition
/// tested through `__bool__`, a `SIMD` construction or lane read, a
/// construction of a declared struct, a method call on a local or a parameter whose contract
/// is a value contract or which dispatches through the parameter's bound, a
/// keyword slice of a closed local, the stringify builtin, `external_call`,
/// an operator over a closed struct type, and a `raises` declaration. Each
/// recipe is a method body's, on a body without a receiver.
const FUNCTION_FEATURES: MethodFeatures = MethodFeatures::STATEMENTS
    .union(MethodFeatures::OPAQUE_MOVES)
    .union(MethodFeatures::VALUE_ARGUMENTS)
    .union(MethodFeatures::ITERATION)
    .union(MethodFeatures::TRUTHINESS)
    .union(MethodFeatures::RAISES)
    .union(MethodFeatures::OWNED_PARAMETERS)
    .union(MethodFeatures::CONSUMING_CALLS)
    .union(MethodFeatures::POINTER_SLOTS)
    .union(MethodFeatures::SIMD_CONSTRUCTIONS)
    .union(MethodFeatures::SIMD_INTRINSICS)
    .union(MethodFeatures::CONSTRUCTIONS)
    .union(MethodFeatures::SIBLING_CALLS)
    .union(MethodFeatures::TUPLE_UNPACKS)
    .union(MethodFeatures::TRY_STATEMENTS)
    .union(MethodFeatures::SLICE_VIEWS)
    .union(MethodFeatures::STRINGIFY)
    .union(MethodFeatures::FOREIGN_CALLS)
    .union(MethodFeatures::CLOSED_OPERATORS)
    .union(MethodFeatures::BOUND_DISPATCH);

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
        | FactTable::ExplicitDestroyCalls
        | FactTable::ImplicitlyCopiedConsumingReceivers
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
        | FactTable::CallResultOrigins
        | FactTable::IterationProtocols
        | FactTable::SimdConstructions
        | FactTable::TruthinessConditions
        | FactTable::TupleUnpackPlans
        | FactTable::ParameterizedMethodCalls
        | FactTable::ViewResultInteriors
        | FactTable::ComprehensionBindings
        | FactTable::WithDesugars
        | FactTable::CallTransfers => true,
        FactTable::DeclarationCaptures | FactTable::ContextualBases => true,
    }
}

/// `body` with each `with` statement that has a desugar in `desugars` made a
/// scope of that desugar under the statement's own identity, or `None` when
/// the body holds no such statement.
fn expand_with_statements(
    body: &[Stmt],
    desugars: &HashMap<SourceSpan, super::with_stmt::WithDesugar>,
) -> Option<Vec<Stmt>> {
    struct Finds<'a> {
        desugars: &'a HashMap<SourceSpan, super::with_stmt::WithDesugar>,
        found: bool,
    }

    impl mojito_ast::visit::Visitor for Finds<'_> {
        fn visit_stmt(&mut self, statement: &Stmt) {
            self.found |= matches!(statement.kind, StmtKind::With { .. })
                && self.desugars.contains_key(&statement.source_span());
        }
    }

    struct Expand<'a>(&'a HashMap<SourceSpan, super::with_stmt::WithDesugar>);

    impl mojito_ast::visit::MutVisitor for Expand<'_> {
        fn visit_stmt_mut(&mut self, statement: &mut Stmt) {
            if matches!(statement.kind, StmtKind::With { .. })
                && let Some(desugar) = self.0.get(&statement.source_span())
            {
                statement.kind = StmtKind::Scope(desugar.statements.clone());
            }
        }
    }

    if desugars.is_empty() {
        return None;
    }
    let mut finds = Finds {
        desugars,
        found: false,
    };
    mojito_ast::visit::walk_block(&mut finds, body);
    finds.found.then(|| {
        let mut expanded = body.to_vec();
        mojito_ast::visit::walk_block_mut(&mut Expand(desugars), &mut expanded);
        expanded
    })
}

/// The immutable binders a view-returning call records beside its result
/// origins (`record_call_result_origins`): each slot the callee fixes as
/// immutable, bound at the result itself.
fn call_result_immutable_binders(
    slots: &[super::CallResultOrigin],
) -> Vec<super::ImmutableOriginBinder> {
    slots
        .iter()
        .filter(|(_, _, mutability)| {
            *mutability == Some(mojito_types::origin::Mutability::Immutable)
        })
        .map(|(slot, _, _)| (Vec::new(), *slot))
        .collect()
}

/// The parameters each call at a body's occurrences binds, in pre-order.
fn captured_call_parameters(
    occurrences: &[Occurrence],
    table: &HashMap<SourceSpan, Vec<super::CallParameter>>,
) -> Vec<(OccurrenceId, Vec<CallParameterFact>)> {
    values(occurrences, table)
        .into_iter()
        .map(|(id, parameters)| {
            let facts = parameters
                .into_iter()
                .map(|parameter| CallParameterFact {
                    name: parameter.name,
                    convention: parameter.convention,
                    ty: parameter.ty,
                })
                .collect();
            (id, facts)
        })
        .collect()
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
/// instance declares only those in the arms the elaborator selected, once per
/// unrolled copy of their declaration, and never a `comptime for` variable.
/// A fact naming a local names the declaration in scope where the fact sits:
/// the latest copy at or before its occurrence in pre-order, since each
/// unrolled copy is a scope of its own. A fact that sits at no occurrence may
/// name only a local declared once. The declarations are renumbered densely
/// in declaration order. A local no retained declaration introduces is left
/// alone; installing it then fails as a lost binding.
fn renumber_locals(facts: &mut CheckedBodyFacts) -> Result<(), &'static str> {
    let occurrences = facts.occurrences.clone();
    let position = |id: OccurrenceId| occurrences.iter().position(|found| *found == id);
    // A tuple unpacking declares each of its targets, whose binding sits at
    // the target.
    let unpacked: Vec<OccurrenceId> = facts
        .tuple_unpacks
        .iter()
        .filter(|(_, unpack)| unpack.declares)
        .flat_map(|(_, unpack)| unpack.targets.iter().copied())
        .collect();
    let mut declared: Vec<(usize, u32)> = facts
        .statement_bindings
        .iter()
        .chain(
            facts
                .expression_bindings
                .iter()
                .filter(|(id, _)| unpacked.contains(id)),
        )
        .map(|(id, owner)| (id, owner))
        .chain(
            facts
                .comprehension_bindings
                .iter()
                .flat_map(|(id, binders)| binders.iter().map(move |binder| (id, &binder.owner))),
        )
        // A nested `def` declares its parameters at its statement, after
        // the name the statement binds.
        .chain(
            facts
                .nested_defs
                .iter()
                .flat_map(|(id, recipe)| recipe.params.iter().map(move |param| (id, param))),
        )
        .filter_map(|(id, owner)| match owner {
            TemplateOwner::Local(index) => Some((position(*id)?, *index)),
            _ => None,
        })
        .collect();
    // A comprehension's binders are declared while the statement holding it
    // is checked, before the statement's own binding, so pre-order is not
    // the checking order there. Only a body with no unrolled copies holds
    // one, and it keeps every declaration, in the template's own order.
    if facts.comprehension_bindings.is_empty() {
        declared.sort_by_key(|(at, _)| *at);
    } else if occurrences.iter().all(|id| id.copy == 0) {
        declared.sort_by_key(|(_, index)| *index);
    } else {
        return Err("a comprehension sits in an unrolled copy");
    }
    let mut ambiguous = false;
    for_each_owner(facts, &mut |at, owner| {
        let TemplateOwner::Local(index) = owner else {
            return;
        };
        let mut copies = declared
            .iter()
            .enumerate()
            .filter(|(_, (_, local))| local == index);
        let declaration = if let Some(at) = at {
            let at = position(at);
            copies.rfind(|(_, (declared_at, _))| at.is_some_and(|at| *declared_at <= at))
        } else {
            let only = copies.next();
            ambiguous |= copies.next().is_some();
            only
        };
        if let Some((dense, _)) = declaration {
            *index = u32::try_from(dense).unwrap_or(u32::MAX);
        }
    });
    if ambiguous {
        return Err("a local declared in several unrolled copies is named outside any occurrence");
    }
    facts.locals = u32::try_from(declared.len()).unwrap_or(u32::MAX);
    Ok(())
}

/// Visit every binding a bundle names by template owner, with the occurrence
/// the fact naming it sits at: the binding tables, each invalidation's root
/// and exception, and the root of every place a reference's origin holds. A
/// transferred origin is the body's, and sits at none.
fn for_each_owner(
    facts: &mut CheckedBodyFacts,
    visit: &mut dyn FnMut(Option<OccurrenceId>, &mut TemplateOwner),
) {
    fn origin_owners(
        origin: &mut TemplateOrigin,
        at: Option<OccurrenceId>,
        visit: &mut dyn FnMut(Option<OccurrenceId>, &mut TemplateOwner),
    ) {
        match origin {
            TemplateOrigin::Place(place) => visit(at, &mut place.root),
            TemplateOrigin::Union(members) => members
                .iter_mut()
                .for_each(|member| origin_owners(member, at, visit)),
            TemplateOrigin::Unrooted(_) => {}
        }
    }
    /// Every call contract a bundle keeps, at its call: the selected calls,
    /// those an element store embeds, and each place's in-place update.
    fn calls<'f>(
        selected: &'f mut [(OccurrenceId, TemplateCallContract)],
        stores: &'f mut [(OccurrenceId, TemplateAugmentedSubscript)],
        updates: &'f mut [(OccurrenceId, TemplateCallContract)],
    ) -> impl Iterator<Item = (OccurrenceId, &'f mut TemplateCallContract)> {
        selected
            .iter_mut()
            .chain(updates)
            .map(|(id, call)| (*id, call))
            .chain(stores.iter_mut().flat_map(|(id, store)| {
                let id = *id;
                store.contracts_mut().map(move |call| (id, call))
            }))
    }
    for (id, owner) in facts
        .statement_bindings
        .iter_mut()
        .chain(&mut facts.expression_bindings)
    {
        visit(Some(*id), owner);
    }
    for (id, place) in &mut facts.interior_references {
        visit(Some(*id), &mut place.root);
    }
    for (id, iteration) in &mut facts.iterations {
        if let Some(source) = &mut iteration.source {
            visit(Some(*id), &mut source.root);
        }
    }
    for (id, unpack) in &mut facts.tuple_unpacks {
        if let Some(source) = &mut unpack.source {
            origin_owners(&mut source.origin, Some(*id), visit);
        }
    }
    for (id, binders) in &mut facts.comprehension_bindings {
        for binder in binders {
            visit(Some(*id), &mut binder.owner);
        }
    }
    for (id, recipe) in &mut facts.nested_defs {
        for param in &mut recipe.params {
            visit(Some(*id), param);
        }
        for origin in &mut recipe.function_origins {
            origin_owners(origin, Some(*id), visit);
        }
        for capture in &mut recipe.captures {
            visit(Some(*id), &mut capture.owner);
            for captured in &mut capture.origins {
                origin_owners(&mut captured.origin, Some(*id), visit);
            }
        }
    }
    for (id, accesses) in &mut facts.capture_accesses {
        for access in accesses {
            origin_owners(&mut access.origin, Some(*id), visit);
        }
    }
    for (id, reference) in facts
        .reference_results
        .iter_mut()
        .chain(&mut facts.reference_binding_types)
        .chain(&mut facts.reference_place_types)
    {
        origin_owners(&mut reference.origin, Some(*id), visit);
    }
    for (id, call) in calls(
        &mut facts.selected_calls,
        &mut facts.augmented_subscripts,
        &mut facts.inplace_updates,
    ) {
        if let Some(reference) = &mut call.reference_result {
            origin_owners(&mut reference.origin, Some(id), visit);
        }
        for origin in &mut call.result_origins {
            origin_owners(origin, Some(id), visit);
        }
    }
    for typed in &mut facts.typed_origins {
        for origin in &mut typed.origins {
            origin_owners(origin, Some(typed.occurrence), visit);
        }
    }
    for (id, slots) in &mut facts.call_result_origins {
        for resolved in slots {
            origin_owners(&mut resolved.origin, Some(*id), visit);
        }
    }
    for (id, transfers) in &mut facts.call_transfers {
        for source in transfers
            .iter_mut()
            .flat_map(|transfer| &mut transfer.sources)
        {
            origin_owners(&mut source.origin, Some(*id), visit);
        }
    }
    for (dest, sources) in &mut facts.transferred_origins {
        visit(None, dest);
        for source in sources {
            origin_owners(&mut source.origin, None, visit);
        }
    }
    let call_invalidations = calls(
        &mut facts.selected_calls,
        &mut facts.augmented_subscripts,
        &mut facts.inplace_updates,
    )
    .flat_map(|(id, call)| {
        call.arguments
            .iter_mut()
            .flat_map(|argument| &mut argument.invalidations)
            .chain(&mut call.invalidations)
            .map(move |invalidation| (id, invalidation))
    });
    let invalidations = facts
        .interior_invalidations
        .iter_mut()
        .flat_map(|(id, invalidations)| {
            let id = *id;
            invalidations
                .iter_mut()
                .map(move |invalidation| (id, invalidation))
        })
        .chain(call_invalidations);
    for (id, invalidation) in invalidations {
        visit(Some(id), &mut invalidation.root);
        if let Some(except) = &mut invalidation.except {
            visit(Some(id), except);
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

/// A type with every struct origin argument, and every origin a capturing
/// callable's environment retains, mapped by `origin`, in pre-order: the one
/// traversal behind keeping those origins by template owner
/// (`unbound_struct_origins`) and giving them an instance's bindings back
/// (`bind_struct_origins`). A pointer's or a reference's own origin is not a
/// struct argument and passes through.
fn map_struct_origins<E>(
    ty: &Ty,
    origin: &mut dyn FnMut(
        &mojito_types::origin::Origin,
    ) -> Result<mojito_types::origin::Origin, E>,
) -> Result<Ty, E> {
    use mojito_types::origin::{CallableEnvironment, CaptureOriginSet, Origin};
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
        Ty::Func {
            environment: CallableEnvironment::Capturing(CaptureOriginSet::Concrete(members)),
            ..
        } => {
            let members = members
                .iter()
                .map(|member| {
                    Ok(mojito_types::origin::CaptureOrigin {
                        origin: origin(&member.origin)?,
                        access: member.access,
                    })
                })
                .collect::<Result<Vec<_>, E>>()?;
            let mut callable = ty.clone();
            if let Ty::Func { environment, .. } = &mut callable {
                *environment = CallableEnvironment::Capturing(CaptureOriginSet::Concrete(members));
            }
            callable
        }
        _ => ty.clone(),
    })
}

/// A struct-typed value's type with its origin slots that name a binding
/// unbound, and those origins by template owner, for a type that names a
/// binding only there. A slot naming no binding (a binder, `static`) is kept
/// in the type, where an instance's substitution reaches it. `None` when the
/// type names no binding at all, so it is kept as written.
fn unbound_struct_origins(
    ty: &Ty,
    place: &dyn Fn(&mojito_types::origin::OriginPlace) -> Result<TemplatePlace, IncompleteReason>,
) -> Result<Option<(Ty, Vec<TemplateOrigin>)>, IncompleteReason> {
    if !names_place(ty) {
        return Ok(None);
    }
    let mut origins = Vec::new();
    let unbound = map_struct_origins(ty, &mut |origin| {
        if !origin_names_place(origin) && *origin != mojito_types::origin::Origin::Unbound {
            return Ok(origin.clone());
        }
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
        .map(|(id, ty)| match self::typed_origins(&ty, place)? {
            Some(KeptType {
                ty,
                origins,
                pointer,
            }) => {
                typed_origins.push(TypedOrigins {
                    table,
                    occurrence: id,
                    origins,
                    pointer,
                });
                Ok((id, ty))
            }
            None => Ok((id, ty)),
        })
        .collect()
}

/// A retained type with the origins it names kept by template owner: the
/// parts of one [`TypedOrigins`] entry.
struct KeptType {
    ty: Ty,
    origins: Vec<TemplateOrigin>,
    pointer: Option<bool>,
}

/// [`unbound_struct_origins`] for a retained type, which may also be a
/// pointer to a place (`Pointer(to=self.items)`): its provenance is kept
/// first by template owner and the type with it untracked, with the
/// pointer's capability beside.
fn typed_origins(
    ty: &Ty,
    place: &dyn Fn(&mojito_types::origin::OriginPlace) -> Result<TemplatePlace, IncompleteReason>,
) -> Result<Option<KeptType>, IncompleteReason> {
    use mojito_types::origin::PointerOrigin;
    let Ty::Pointer {
        element,
        origin: PointerOrigin::Place {
            place: pointee,
            mutable,
        },
    } = ty
    else {
        return Ok(
            unbound_struct_origins(ty, place)?.map(|(ty, origins)| KeptType {
                ty,
                origins,
                pointer: None,
            }),
        );
    };
    let (element, rest) = unbound_struct_origins(element, place)?
        .unwrap_or_else(|| ((**element).clone(), Vec::new()));
    let origins = std::iter::once(Ok(TemplateOrigin::Place(place(pointee)?)))
        .chain(rest.into_iter().map(Ok))
        .collect::<Result<Vec<_>, IncompleteReason>>()?;
    Ok(Some(KeptType {
        ty: Ty::Pointer {
            element: Box::new(element),
            origin: PointerOrigin::Untracked { mutable: *mutable },
        },
        origins,
        pointer: Some(*mutable),
    }))
}

/// The inverse of [`typed_origins`]: a kept pointer's provenance rooted at
/// the instance's own place, then its struct origins written back.
fn bind_typed_origins(
    ty: &Ty,
    typed: &TypedOrigins,
    rooted: &dyn Fn(&TemplatePlace) -> Result<mojito_types::origin::OriginPlace, TypeError>,
) -> Result<Ty, TypeError> {
    use mojito_types::origin::PointerOrigin;
    let (Some(mutable), Ty::Pointer { element, .. }, [TemplateOrigin::Place(pointee), rest @ ..]) =
        (typed.pointer, ty, typed.origins.as_slice())
    else {
        return bind_struct_origins(ty, &typed.origins, rooted);
    };
    Ok(Ty::Pointer {
        element: Box::new(bind_struct_origins(element, rest, rooted)?),
        origin: PointerOrigin::Place {
            place: rooted(pointee)?,
            mutable,
        },
    })
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
/// instance's own bindings, written back into the type's unbound struct
/// origin slots in the order they were taken out. A slot the instance's
/// substitution brought in (a loan-carrying argument's clone binder) is
/// kept as it stands; an unbound slot left over, or an origin never written
/// back, is a lost origin.
fn bind_struct_origins(
    ty: &Ty,
    origins: &[TemplateOrigin],
    rooted: &dyn Fn(&TemplatePlace) -> Result<mojito_types::origin::OriginPlace, TypeError>,
) -> Result<Ty, TypeError> {
    let lost =
        || TypeError::InvariantViolation("template derivation lost a struct origin".to_string());
    let mut slots = origins.iter();
    let bound = map_struct_origins(ty, &mut |slot| match slot {
        mojito_types::origin::Origin::Unbound => {
            checked_origin(slots.next().ok_or_else(lost)?, rooted)
        }
        kept => Ok(kept.clone()),
    })?;
    if slots.next().is_some() {
        return Err(lost());
    }
    Ok(canonical_environment(&bound))
}

/// A capturing callable's environment in the canonical order
/// `CaptureOriginSet::concrete` gives it, which rebinding its origins to
/// other bindings may have broken.
fn canonical_environment(ty: &Ty) -> Ty {
    use mojito_types::origin::{CallableEnvironment, CaptureOriginSet};
    let mut ty = ty.clone();
    if let Ty::Func {
        environment: environment @ CallableEnvironment::Capturing(CaptureOriginSet::Concrete(_)),
        ..
    } = &mut ty
        && let CallableEnvironment::Capturing(CaptureOriginSet::Concrete(members)) =
            std::mem::take(environment)
    {
        *environment = CallableEnvironment::Capturing(CaptureOriginSet::concrete(members));
    }
    ty
}

/// Whether a type names a checker-local place: an origin rooted at a binding
/// identity, in a pointer, a reference, or a struct's origin argument.
fn names_place(ty: &Ty) -> bool {
    use mojito_types::origin::PointerOrigin;
    let rooted = origin_names_place;
    mojito_types::types::mentions(ty, &|ty| {
        match ty {
        Ty::Pointer { origin, .. } => matches!(origin, PointerOrigin::Place { .. }),
        Ty::Ref(reference) => rooted(&reference.origin) || names_place(&reference.referent),
        Ty::Struct(_, arguments) => arguments.iter().any(|argument| {
            matches!(argument, mojito_types::types::TyArg::Origin(origin) if rooted(origin))
        }),
        Ty::Func {
            environment:
                mojito_types::origin::CallableEnvironment::Capturing(
                    mojito_types::origin::CaptureOriginSet::Concrete(members),
                ),
            ..
        } => members.iter().any(|member| rooted(&member.origin)),
        _ => false,
    }
    })
}

/// Whether an origin is rooted at a binding identity.
fn origin_names_place(origin: &mojito_types::origin::Origin) -> bool {
    use mojito_types::origin::Origin;
    match origin {
        Origin::Place(_) => true,
        Origin::Union(members) => members.iter().any(origin_names_place),
        Origin::Param(_)
        | Origin::SelfParam
        | Origin::Static
        | Origin::Untracked { .. }
        | Origin::Unbound => false,
    }
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
/// Whether a method body selected a callee, or read an effect summary, that
/// its grammar did not admit.
///
/// The grammar admitted every method call it judged closed, every call an
/// element store embeds, every in-place update of a place, every
/// construction, every call through a callable parameter, and every direct
/// call `method_direct_calls` names.
fn stray_method_call(facts: &CheckedBodyFacts, shape: &BodyShape<'_>) -> bool {
    let targets: Vec<&str> = facts
        .selected_calls
        .iter()
        .chain(&facts.inplace_updates)
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
    let direct_calls = method_direct_calls(facts);
    let nested_calls = nested_def_calls(facts);
    let static_calls = shape.static_calls.borrow();
    // An admitted operator's target is its reflected dunder's
    // ([`BodyShape::operator`]).
    let operators = shape.operators.borrow();
    if !direct_calls.is_empty() {
        shape.holds(MethodFeatures::DIRECT_CALLS);
    }
    let admitted_call = |id: &OccurrenceId| {
        facts.selected_calls.iter().any(|(call, _)| call == id)
            || facts.inplace_updates.iter().any(|(update, _)| update == id)
            || constructions.contains(id)
            || callable_calls.contains(id)
            || direct_calls.iter().any(|(call, _)| call == id)
            || nested_calls.iter().any(|(call, _)| call == id)
            || static_calls.iter().any(|(call, _)| call == id)
    };
    // A call through a bound, at an occurrence or embedded in a store,
    // reads the summaries of every conformer's method of that name.
    let conformer_copy = |callee: &str| dispatched_conformer(&targets, callee);
    facts
        .call_parameters
        .iter()
        .any(|(id, _)| !admitted_call(id))
        || !facts
            .generic_instantiations
            .iter()
            .all(|(id, instantiation)| {
                direct_calls.iter().any(|(call, _)| call == id)
                    || (callable_calls.contains(id)
                        && shape
                            .callable_binders
                            .contains(&instantiation.callee.as_str()))
            })
        || !facts
            .overload_targets
            .iter()
            .all(|(id, _)| admitted_call(id) || operators.contains(id))
        || !summary_callees(facts).all(|callee| {
            targets.contains(&callee.as_str())
                || direct_calls.iter().any(|(_, direct)| direct == callee)
                || nested_calls.iter().any(|(_, nested)| nested == callee)
                || static_calls.iter().any(|(_, member)| member == callee)
                || conformer_copy(callee)
                || shape.callable_params.contains(&callee.as_str())
                || shape.callable_binders.contains(&callee.as_str())
        })
}

/// Whether `callee` is a conformer's method a call through a bound among
/// `targets` read the summaries of: such a call reads every conformer's
/// method of its name, one key per conformer (`Struct.method`, or the
/// overload symbol `Struct.method$ov$…`), none of them a target. An
/// instance reads its own witness's summaries again
/// ([`Checker::realize_bound_dispatch`]).
fn dispatched_conformer(targets: &[&str], callee: &str) -> bool {
    let member = |name: &'_ str| name.split('$').next().unwrap_or(name).to_string();
    mojito_symbol::symbol::split_method_symbol(callee).is_some_and(|(_, called)| {
        targets
            .iter()
            .filter(|target| mojito_symbol::symbol::is_trait_dispatch_symbol(target))
            .any(|target| {
                member(
                    mojito_symbol::symbol::split_method_symbol(target)
                        .map_or(*target, |(_, method)| method),
                ) == member(called)
            })
    })
}

/// The direct calls a method body makes of a module-scope function that
/// takes only closed scalars, or reads a value at one of its own binders,
/// each with its callee.
///
/// The call selects the same declaration under every instance, or the same
/// member of an overload set, which ranks only the closed argument types,
/// and binds its arguments by value at types no substitution changes. A
/// generic callee's application (`unsafe_alloc[Self.T](n)`) is recorded with
/// the template's arguments, which the instance substitutes, so an instance
/// realizes it as a function template's direct call
/// ([`Checker::realize_direct_call`]): the application's existing clone, or
/// the one the elaborator retargeted the call to. A read parameter typed by
/// the callee's own binder (`hash(e)`) is declared in the callee's binder
/// scope, which no substitution reaches: the argument binds it exactly, and
/// only the application's argument changes per instance
/// ([`BodyShape::generic_call_place`]).
fn method_direct_calls(facts: &CheckedBodyFacts) -> Vec<(OccurrenceId, &str)> {
    facts
        .call_parameters
        .iter()
        .filter(|(id, parameters)| {
            let application = fact_at(&facts.generic_instantiations, *id);
            !facts.selected_calls.iter().any(|(call, _)| call == id)
                && application.is_none_or(|application| application.variadic.is_none())
                && parameters.iter().all(|parameter| {
                    parameter.convention.is_none()
                        && (!mojito_types::types::is_symbolic(&parameter.ty)
                            || (application.is_some() && matches!(parameter.ty, Ty::Param { .. })))
                })
        })
        .filter_map(|(id, _)| template_callee(facts, *id).map(|callee| (*id, callee)))
        .collect()
}

/// The calls a method body makes of a nested `def` it declares, each with
/// the callee's name: the call's binding is the one the declaration's
/// statement introduced.
fn nested_def_calls(facts: &CheckedBodyFacts) -> Vec<(OccurrenceId, &str)> {
    facts
        .call_parameters
        .iter()
        .filter_map(|(call, _)| {
            let callee = fact_at(&facts.expression_bindings, *call)?;
            facts
                .nested_defs
                .iter()
                .find(|(declaration, _)| {
                    fact_at(&facts.statement_bindings, *declaration) == Some(callee)
                })
                .map(|(_, recipe)| (*call, recipe.name.as_str()))
        })
        .collect()
}

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

/// The type pack a method's struct declares, and the method's variadic
/// parameters collecting it (`var *args: *Self.Ts`).
fn struct_pack_collectors<'m, 'd>(
    method: &'m mojito_ast::ast::Method,
    decls: &'d [ParamDecl],
) -> (Option<&'d str>, Vec<&'m str>) {
    let pack = decls.iter().find_map(|decl| match decl {
        ParamDecl::Type {
            name,
            variadic: true,
            ..
        } => Some(name.trim_start_matches('*')),
        _ => None,
    });
    let collectors = method
        .params
        .iter()
        .filter(|parameter| {
            let collected = match &parameter.ty {
                mojito_ast::ast::Type::SelfParam(name) | mojito_ast::ast::Type::Named(name, _) => {
                    name.strip_prefix('*')
                }
                _ => None,
            };
            parameter.kind == mojito_ast::ast::ParamKind::Variadic
                && collected.is_some()
                && collected == pack
        })
        .map(|parameter| parameter.name.as_str())
        .collect();
    (pack, collectors)
}

/// The names of a struct's `DType` and `Int` binders, which key a struct
/// specialized whole and name its members' symbolic lane
/// (`Scalar[Self.dtype]`, `SIMD[dt, Self.n]`).
fn struct_lane_binders(decls: &[ParamDecl]) -> Vec<&str> {
    decls
        .iter()
        .filter_map(|decl| match decl {
            ParamDecl::Value {
                name,
                ty,
                variadic: false,
                ..
            } if matches!(**ty, Ty::Dtype | Ty::Int) => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

/// A struct's scalar value binders of a closed type (`rows: Int`).
fn struct_scalar_binders(decls: &[ParamDecl]) -> Vec<&str> {
    decls
        .iter()
        .filter_map(|decl| match decl {
            ParamDecl::Value {
                name,
                ty,
                variadic: false,
                ..
            } if closed_scalar(ty) => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

/// A struct's vector value binders of a closed type (`key: U256`).
fn struct_vector_binders(decls: &[ParamDecl]) -> Vec<&str> {
    decls
        .iter()
        .filter_map(|decl| match decl {
            ParamDecl::Value {
                name,
                ty,
                variadic: false,
                ..
            } if grammar_scalar(ty) && matches!(**ty, Ty::Simd { .. }) => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

/// The names of a method's own value binders (`[n: Int]`, `[dt: DType]`),
/// given its own declarations, or `None` when one is not a plain `Int`,
/// `Bool`, or `DType`. The
/// parser spells such a binder's type as a bound (`n: Int`), so only its
/// declaration tells it from a trait-bounded type binder ([`bound_binder`]).
/// A compile-time callable binder is apart ([`method_callable_binders`]).
fn method_value_binders<'m>(
    method: &'m mojito_ast::ast::Method,
    own_decls: &[ParamDecl],
) -> Option<Vec<&'m str>> {
    let callables = method_callable_binders(method, own_decls);
    method
        .type_params
        .iter()
        .filter(|binder| !callables.contains(&binder.name.as_str()))
        .filter_map(|binder| {
            own_decls
                .iter()
                .find(|decl| decl.name().trim_start_matches('*') == binder.name)
                .filter(|decl| matches!(decl, ParamDecl::Value { .. }))
                .map(|decl| (binder, decl))
        })
        .map(|(binder, decl)| {
            matches!(decl, ParamDecl::Value {
                ty,
                default: None,
                infer_only: false,
                variadic: false,
                ..
            } if matches!(**ty, Ty::Int | Ty::Bool | Ty::Dtype))
            .then_some(binder.name.as_str())
        })
        .collect()
}

/// The names of a method's own compile-time callable binders
/// (`elt_handler: def[index: Int](var element: Self.Ts[index])`), given its
/// own declarations: a callable value with no default, which every clone
/// keeps ([`MethodFeatures::CALLABLE_BINDERS`]).
fn method_callable_binders<'m>(
    method: &'m mojito_ast::ast::Method,
    own_decls: &[ParamDecl],
) -> Vec<&'m str> {
    method
        .type_params
        .iter()
        .filter(|binder| callable_binder(binder))
        .filter(|binder| {
            own_decls.iter().any(|decl| {
                matches!(decl, ParamDecl::Value {
                    name,
                    ty,
                    default: None,
                    callable_default: None,
                    infer_only: false,
                    variadic: false,
                    ..
                } if *name == binder.name
                    && matches!(**ty, Ty::Func { .. } | Ty::GenericFunc { .. }))
            })
        })
        .map(|binder| binder.name.as_str())
        .collect()
}

/// A binder spelled with a callable right-hand side and nothing else
/// (`f: def(Int) -> Int`, whose bound the parser spells `<function type>`),
/// whose declaration decides whether it is a compile-time callable value
/// ([`method_callable_binders`]).
fn callable_binder(binder: &mojito_ast::ast::TypeParam) -> bool {
    binder.callable_bound.is_some()
        && binder.bounds.iter().all(|bound| bound == "<function type>")
        && binder.origin_mutability.is_none()
        && binder.value_type.is_none()
        && binder.default.is_none()
        && !binder.infer_only
}

/// Whether every origin a type argument names sits in a struct origin tail
/// bound to a binder (`Span[Int, __clone_origin0]`), with no pointer or
/// reference beside it: the loans such a value carries are exactly those
/// binders'. [`TemplateObligation::PlainDataArguments`] admits it for a
/// clone whose only binders are the elaborator's origin binders.
fn binder_tail_loans(ty: &Ty) -> bool {
    use mojito_types::origin::Origin;
    use mojito_types::types::TyArg;
    let tail_binder = mojito_types::types::mentions(ty, &|candidate| {
        matches!(candidate, Ty::Struct(_, arguments)
            if arguments.iter().any(|argument| matches!(argument, TyArg::Origin(Origin::Param(_)))))
    });
    let other_origin = mojito_types::types::mentions(ty, &|candidate| {
        match candidate {
        Ty::Pointer { .. } | Ty::Ref(_) => true,
        Ty::Struct(_, arguments) => arguments
            .iter()
            .any(|argument| matches!(argument, TyArg::Origin(origin) if !matches!(origin, Origin::Param(_)))),
        _ => false,
    }
    });
    tail_binder && !other_origin
}

/// Whether a clone's binder is one the elaborator declared for an origin
/// slot of a loan-carrying type argument: it stands for no template
/// parameter, so a clone keeping only such binders has baked every one.
fn clone_origin_binder(binder: &mojito_ast::ast::TypeParam) -> bool {
    binder
        .name
        .starts_with(mojito_symbol::symbol::CLONE_ORIGIN_BINDER_PREFIX)
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

/// Whether an expression checks to one type under any expected type other
/// than a reference: everything `infer_with_expected` types without the
/// expected type. A collection or tuple display, a leading-dot member chain,
/// or an explicit application may take its type from the parameter it is
/// handed to.
fn context_free(expr: &Expr) -> bool {
    if matches!(
        expr.kind,
        ExprKind::ListLit(_)
            | ExprKind::BraceLit(_)
            | ExprKind::TupleLit(_)
            | ExprKind::TypeApply { .. }
    ) {
        return false;
    }
    let mut current = expr;
    loop {
        match &current.kind {
            ExprKind::Identifier(name) => return name != mojito_ast::ast::CONTEXTUAL_SENTINEL,
            ExprKind::Member { object, .. }
            | ExprKind::MethodCall { object, .. }
            | ExprKind::Index { object, .. } => current = object,
            ExprKind::Invoke { callee, .. } => current = callee,
            _ => return true,
        }
    }
}

/// Set the entry a table holds at `id`, or add one.
fn upsert<T>(table: &mut Vec<(OccurrenceId, T)>, id: OccurrenceId, value: T) {
    match table.iter_mut().find(|(site, _)| *site == id) {
        Some(entry) => entry.1 = value,
        None => table.push((id, value)),
    }
}

/// The generated Tuple member a tuple element's read selected, from its
/// lowered callee `Tuple$….__getitem_param__$k`.
fn tuple_element_accessor(target: &str) -> Option<&str> {
    let start = target.rfind(&format!(".{TUPLE_ELEMENT_ACCESSOR}$"))?;
    Some(&target[start + 1..])
}

/// Whether an argument of the recorded type binds a parameter typed by the
/// callee's existential `Some[…]` binder under every instance: the argument
/// is a caller binder each of whose existential's bounds one of its own
/// bounds names or refines, so every type an instance binds conforms.
fn existential_argument(
    traits: &HashMap<String, super::TraitInfo>,
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
                    held.iter()
                        .any(|own| own == bound || super::traits::refines_trait(traits, own, bound))
                })
        }
        _ => false,
    }
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
    // A callee whose summary was replayed is read again per instance
    // (`CheckedBodyFacts::transfer_reads`), not observed empty.
    let replayed = |callee: &str| {
        reads
            .effect_queries
            .iter()
            .any(|(read, effects)| read == callee && matches!(effects, EffectRead::Transfers(_)))
    };
    let names = |value: bool| {
        let mut names: Vec<String> = reads
            .effect_queries
            .iter()
            .filter(|(callee, read)| {
                (!value || matches!(read, EffectRead::Value))
                    && !call_through_reads.iter().any(|(read, _)| read == callee)
                    && !replayed(callee)
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

/// Every callee whose transfer summary a body read at a call: those observed
/// empty and those replayed.
fn summary_callees(facts: &CheckedBodyFacts) -> impl Iterator<Item = &String> {
    facts
        .effect_free_callees
        .iter()
        .chain(facts.transfer_reads.iter().map(|(callee, _)| callee))
}

/// Record that realization resolved the template's callee `selected` to the
/// instance's `target`: the target's summaries are read as the template read
/// them, empty or with the residue the template kept.
fn note_realized_callee(facts: &mut CheckedBodyFacts, selected: &str, target: &str) {
    // A second call on the same callee finds the read already rekeyed.
    let residue = facts
        .call_through_reads
        .iter_mut()
        .map(|(callee, _)| callee)
        .chain(facts.transfer_reads.iter_mut().map(|(callee, _)| callee))
        .find(|callee| *callee == selected || *callee == target);
    match residue {
        Some(read) => target.clone_into(read),
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
    for (_, call) in facts
        .selected_calls
        .iter_mut()
        .chain(&mut facts.inplace_updates)
    {
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

/// Whether a call's recorded effect derives: raising is the one it may
/// carry.
///
/// The raised type substitutes (`substituted_facts`), and the judgment an
/// instance repeats at the call — whether that type is the declared error
/// type, or the handler's in a `with` desugar — compares two functions of the
/// same parameters, so it holds under every substitution the template's
/// holds under.
const fn effect_derives(effects: &mojito_checked::checked::EffectFacts) -> bool {
    !effects.may_suspend && !effects.diverges
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

/// A construction of a binder the instance keeps symbolic (`H()` in a
/// method's `[H: Hasher]`), which the instance records as the template did.
///
/// The adjustment names the binder only by spelling; the recorded type names
/// its declaration, which the substitution leaves alone exactly when the
/// binder is the method's own rather than the struct's.
fn kept_binder_construction(
    template: &CheckedBodyFacts,
    id: OccurrenceId,
    adjustment: &mojito_checked::checked::SemanticAdjustment,
    substitute: &dyn Fn(&Ty) -> Ty,
) -> Option<mojito_checked::checked::SemanticAdjustment> {
    let mojito_checked::checked::SemanticAdjustment::ConstructTypeParam { param } = adjustment
    else {
        return None;
    };
    fact_at(&template.expression_types, id)
        .filter(|ty| matches!(ty, Ty::Param { binder, .. } if binder == param))
        .filter(|ty| substitute(ty) == **ty)
        .map(|_| adjustment.clone())
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

/// The dimensions of each construction whose dtype or width named a folded
/// value binder (`Scalar[dt](x)`), which the template left unrecorded: the
/// instance's are its substituted construction type's, as a closed
/// construction's are its recorded type's.
fn realize_value_shaped_constructions(
    template: &CheckedBodyFacts,
    facts: &mut CheckedBodyFacts,
    occurrences: &[Occurrence],
) -> Result<(), &'static str> {
    for occurrence in occurrences {
        let id = occurrence.id;
        let open = fact_at(&template.expression_types, id).is_some_and(|ty| {
            matches!(ty, Ty::Simd { .. }) && mojito_types::types::is_symbolic(ty)
        });
        if !open
            || occurrence.callee.is_none()
            || fact_at(&template.simd_constructions, id).is_some()
        {
            continue;
        }
        let dimensions = fact_at(&facts.expression_types, id)
            .and_then(mojito_types::types::simd_shape)
            .ok_or("a construction's lane dtype or width stays open in the instance")?;
        facts.simd_constructions.push((id, dimensions));
    }
    Ok(())
}

/// The shape of each `to_bits` reinterpretation, `cast`, and `.length` read
/// the template made over a lane-shaped receiver, which it left unrecorded:
/// the instance's are its substituted result type's and receiver type's, as
/// a closed read's are its recorded adjustment's. A reinterpretation's
/// target must be at least as wide as the instance's lane, and a cast's
/// lanes must not be `bool`, the constraints `infer_method_call` checks only
/// on a closed source; an instance that breaks one refuses, and the clone
/// check reports it. The adjustment
/// table keeps the body's occurrence order, as a capture writes it.
fn realize_simd_intrinsics(
    template: &CheckedBodyFacts,
    facts: &mut CheckedBodyFacts,
    occurrences: &[Occurrence],
) -> Result<(), &'static str> {
    use mojito_checked::checked::SemanticAdjustment;
    let shape = |id: OccurrenceId| {
        fact_at(&facts.expression_types, id).and_then(mojito_types::types::simd_shape)
    };
    let mut realized = Vec::new();
    for (id, receiver) in &template.simd_to_bits {
        let (dtype, width) = shape(*id)
            .ok_or("a reinterpretation's lane dtype or width stays open in the instance")?;
        let (source, _) = shape(*receiver)
            .ok_or("a reinterpretation's source lane stays open in the instance")?;
        if super::builtins::dtype_bit_width(dtype) < super::builtins::dtype_bit_width(source) {
            return Err("a reinterpretation's target is narrower than the instance's lane");
        }
        realized.push((*id, SemanticAdjustment::SimdToBits { dtype, width }));
    }
    for (id, receiver) in &template.simd_casts {
        let (dtype, width) =
            shape(*id).ok_or("a cast's lane dtype or width stays open in the instance")?;
        let (source, _) =
            shape(*receiver).ok_or("a cast's source lane stays open in the instance")?;
        if dtype == mojito_ast::ast::Dtype::Bool || source == mojito_ast::ast::Dtype::Bool {
            return Err("a cast's instance lane is `bool`");
        }
        realized.push((*id, SemanticAdjustment::SimdCast { dtype, width }));
    }
    for (id, receiver) in &template.simd_lengths {
        let (_, width) =
            shape(*receiver).ok_or("a lane count's receiver width stays open in the instance")?;
        realized.push((*id, SemanticAdjustment::SimdLength { width }));
    }
    if realized.is_empty() {
        return Ok(());
    }
    facts
        .operation_adjustments
        .retain(|(id, _)| realized.iter().all(|(read, _)| read != id));
    facts.operation_adjustments.extend(realized);
    let order = |id: OccurrenceId| occurrences.iter().position(|found| found.id == id);
    facts
        .operation_adjustments
        .sort_by_key(|(id, _)| order(*id));
    facts.simd_to_bits.clear();
    facts.simd_casts.clear();
    facts.simd_lengths.clear();
    Ok(())
}

/// The hidden dtype and width values a clone folds where its template
/// viewed the wildcard vector binder `decl` as a lane-shaped vector
/// (`simd_binder_view`): the slots of the closed vector type `ty` the clone
/// bakes the binder to, with the whole view paired to `ty` in `views`.
/// Empty for any other binder; an open slot does not resolve.
fn simd_binder_values(
    decl: &ParamDecl,
    ty: &Ty,
    views: &mut Vec<(Ty, Ty)>,
) -> Result<Vec<(mojito_types::param_expr::ParamId, mojito_types::ct::CtValue)>, TypeError> {
    use mojito_types::ct::CtValue;
    let ParamDecl::Type {
        id, name, bounds, ..
    } = decl
    else {
        return Ok(Vec::new());
    };
    if !matches!(bounds.as_slice(), [bound] if bound == SIMD_WILDCARD_BOUND) {
        return Ok(Vec::new());
    }
    let (dtype, width) = mojito_types::types::simd_shape(ty).ok_or_else(|| {
        TypeError::InvariantViolation(format!(
            "a per-call clone binds the wildcard vector binder '{name}' to '{ty}', which is not \
             a closed vector"
        ))
    })?;
    let binder = mojito_types::param_expr::ParamRef {
        id: id.clone(),
        name: name.as_str().into(),
    };
    let view = simd_binder_view(&Ty::Param {
        binder: binder.clone(),
        bounds: bounds.clone(),
        callable_bound: None,
    })
    .ok_or_else(|| {
        TypeError::InvariantViolation(format!(
            "the wildcard vector binder '{name}' has no lane-shaped view"
        ))
    })?;
    views.push((view, ty.clone()));
    let (dtype_slot, size_slot) = simd_binder_slots(&binder);
    Ok(vec![
        (dtype_slot.id, CtValue::Dtype(dtype)),
        (size_slot.id, CtValue::Int(width)),
    ])
}

/// `ty` with every type equal to a wildcard vector binder's whole view
/// replaced by the type the clone bakes the binder to, before its lane
/// slots are folded ([`InstanceSubstitution::views`]).
fn fold_binder_views(ty: &Ty, views: &[(Ty, Ty)]) -> Ty {
    struct Folder<'a>(&'a [(Ty, Ty)]);

    impl mojito_types::types::TyRewrite for Folder<'_> {
        fn whole(&mut self, ty: &Ty) -> Option<Ty> {
            self.0
                .iter()
                .find(|(view, _)| view == ty)
                .map(|(_, baked)| baked.clone())
        }

        fn expr(
            &mut self,
            expr: &mojito_types::param_expr::ParamExpr,
        ) -> Result<mojito_types::param_expr::ParamExpr, mojito_types::param_expr::ParamError>
        {
            Ok(expr.clone())
        }
    }

    if views.is_empty() {
        return ty.clone();
    }
    mojito_types::types::rewrite_ty(ty, &mut Folder(views)).unwrap_or_else(|_| ty.clone())
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
        views,
        values,
        ..
    }: &InstanceSubstitution,
    indices: &ElementIndices,
    canonical: &dyn Fn(Ty) -> Ty,
) -> Result<CheckedBodyFacts, &'static str> {
    let substitute = |ty: &Ty| {
        canonical(mojito_types::types::substitute_packs(
            &fold_binder_views(ty, views),
            substitution,
            packs,
            values,
        ))
    };
    // A pack element's index binder is the copy's own at its occurrence.
    let substitute_at = |id: &OccurrenceId, ty: &Ty| {
        let values: Vec<_> = indices
            .get(id)
            .cloned()
            .into_iter()
            .chain(values.iter().cloned())
            .collect();
        canonical(mojito_types::types::substitute_packs(
            &fold_binder_views(ty, views),
            substitution,
            packs,
            &values,
        ))
    };
    let typed = |entries: &[(OccurrenceId, Ty)]| -> Vec<(OccurrenceId, Ty)> {
        entries
            .iter()
            .map(|(id, ty)| (*id, substitute_at(id, ty)))
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
                kept_binder_construction(template, *id, adjustment, &substitute)
                    .or_else(|| {
                        mojito_checked::templates::derive_adjustment(adjustment, &substitute)
                    })
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
                        operand: substitute_at(id, &assertion.operand),
                        dest: substitute_at(id, &assertion.dest),
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
/// substituted. The value getter a store embeds is realized with the call
/// at the site (`realize_element_getters`), and an in-place dunder
/// dispatched through a bound on the instance's element
/// (`realize_element_dunders`); any other dunder is kept as it stands, so
/// it must name only closed types.
fn substituted_element_stores(
    stores: &[(OccurrenceId, TemplateAugmentedSubscript)],
    substitute: &dyn Fn(&Ty) -> Ty,
) -> Result<Vec<(OccurrenceId, TemplateAugmentedSubscript)>, &'static str> {
    stores
        .iter()
        .map(|(id, store)| {
            let mut store = store.clone();
            let closed = store.inplace.as_ref().is_none_or(|call| {
                mojito_symbol::symbol::is_trait_dispatch_symbol(&call.contract.target)
                    || !mojito_types::types::is_symbolic(&call.contract.result_ty)
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

/// Whether an adjustment names a binding or a span, so a bundle keeps it
/// apart by template owner or occurrence: a call's reference result, the
/// places a capturing call's environment reaches, or an in-place update's
/// contract, whose boundary names spans (`inplace_updates`).
const fn kept_apart(adjustment: &mojito_checked::checked::SemanticAdjustment) -> bool {
    matches!(
        adjustment,
        mojito_checked::checked::SemanticAdjustment::ReferenceResult { .. }
            | mojito_checked::checked::SemanticAdjustment::CallableCaptureAccesses(_)
            | mojito_checked::checked::SemanticAdjustment::AugmentedInPlace(_)
    )
}

/// Append `entry` unless the list already holds it: a grammar arm may judge
/// one occurrence more than once.
fn push_unique<V: PartialEq>(list: &mut Vec<V>, entry: V) {
    if !list.contains(&entry) {
        list.push(entry);
    }
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
    /// The declared traits, whose refinements an existential argument's
    /// bounds may reach ([`existential_argument`]).
    traits: &'a HashMap<String, super::TraitInfo>,
    /// Each checked `with` statement's desugar, which the grammar judges in
    /// the statement's place once the facts are captured.
    desugars: &'a HashMap<SourceSpan, super::with_stmt::WithDesugar>,
    /// How many `with` desugars the statement being judged lies in: only a
    /// desugar's own `try` and liveness anchor are admitted.
    desugar_depth: std::cell::Cell<u32>,
    /// The error binders of the desugar handlers in scope, which a `raise`
    /// may raise again.
    error_binders: RefCell<Vec<String>>,
    params: Vec<&'a str>,
    /// The module's exact integer constants (`comptime MULTIPLE = 6364…`),
    /// which a body reads by name as it would the literal they fold to.
    constants: &'a HashMap<String, mojito_common::literal::IntLiteral>,
    /// The module's vector aliases (`comptime U256 = SIMD[DType.uint64, 4]`),
    /// which a body constructs as the `SIMD` they spell.
    vector_aliases: Vec<&'a str>,
    /// The `mut` and `ref` parameters among them: a place the body borrows,
    /// so never the source of a `^` transfer.
    borrowed_params: Vec<&'a str>,
    /// The `mut` parameters, which the body may store to.
    mut_params: Vec<&'a str>,
    /// Whether source validation produced the facts. A body it checks may
    /// hold compile-time control flow over scalar locals, assignments, and
    /// runtime `if`s, under that check's own rules; any other body may hold
    /// runtime statements instead: scalar locals and assignments, `if`,
    /// `while`, and a bare `return`, each checked once.
    keyed: bool,
    /// Whether the body is a method's, which may read `self`'s fields.
    receiver: bool,
    /// The receiver's declared convention: a `mut`, `var`, `out`, or `deinit`
    /// receiver's fields the body may write, and a `var` receiver it owns
    /// whole and may transfer out (`return self^`).
    self_convention: Option<mojito_ast::ast::ArgConvention>,
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
    /// The method's own trait-bounded type binders, which the body may
    /// construct (`H()`).
    binders: Vec<&'a str>,
    /// The enclosing struct's binders: a construction naming one of them
    /// would construct another type per instance.
    struct_binders: Vec<&'a mojito_types::param_expr::ParamId>,
    /// The constructions of a method's own binder admitted, whose
    /// adjustment every clone records again as the template does.
    binder_constructions: RefCell<Vec<OccurrenceId>>,
    /// The parameters declared with a `def(...)` type, which the body may
    /// call or forward.
    callable_params: Vec<&'a str>,
    /// The calls through such a parameter admitted, whose contract an
    /// instance takes from its own parameter binding.
    callable_calls: RefCell<Vec<OccurrenceId>>,
    /// The method's own compile-time callable binders, which the body may
    /// apply at a folded loop index and call (`CALLABLE_BINDERS`).
    callable_binders: Vec<&'a str>,
    /// The `repr(value)` calls admitted, whose argument an instance proves
    /// `Writable` at its own type.
    repr_calls: RefCell<Vec<OccurrenceId>>,
    /// The static calls admitted, each with the `Struct.method` whose
    /// summaries it reads.
    static_calls: RefCell<Vec<(OccurrenceId, String)>>,
    /// The variadic parameters collecting a type pack of the declaration's
    /// own, whose elements the body may read by loop index.
    packs: Vec<&'a str>,
    /// The type pack the method's struct declares, whose storage field
    /// (`self.storage`, `other.storage`) the body may read by loop index,
    /// and whose length (`Self.Ts.length`) every instance folds.
    pack_struct: Option<&'a str>,
    /// The `comptime for` variables in scope, innermost last.
    loop_vars: RefCell<Vec<String>>,
    /// The declaration's scalar value parameters, which the elaborator
    /// folds to a literal in every instance, as it folds a loop variable: a
    /// `def`'s, or a method's own (`VALUE_BINDERS`).
    values: Vec<&'a str>,
    /// The struct's scalar value binders a method body reads as
    /// `Self.<value>` ([`Self::struct_value`]).
    struct_values: Vec<&'a str>,
    /// The `DType` binders of a struct specialized whole
    /// (`_SequentialRange[dtype]`), which a validated member's lane types
    /// name and every specialization folds ([`Self::value_shaped_scalar`]).
    struct_lanes: Vec<&'a str>,
    /// The closed vector binders of a struct specialized whole
    /// (`AHasher[key: U256]`), which a validated member reads as
    /// `Self.<value>` ([`Self::struct_vector`]).
    struct_vectors: Vec<&'a str>,
    /// The `print(...)` calls admitted, whose arguments an instance proves
    /// `Writable` at its own types.
    print_calls: RefCell<Vec<OccurrenceId>>,
    /// How many nested `def` bodies the statement being judged lies in.
    nested_depth: std::cell::Cell<u32>,
    /// The hidden dtype and width binders of the method's wildcard vector
    /// binders (`$simd.dtype`, `$simd.size`), which a lane-shaped type may
    /// name and every clone folds ([`Self::lane_shaped_simd`]).
    lane_binders: Vec<String>,
    /// The `to_bits` reinterpretations admitted over a lane-shaped receiver,
    /// each with its receiver occurrence, whose adjustment an instance
    /// records from its substituted result.
    simd_to_bits: RefCell<Vec<(OccurrenceId, OccurrenceId)>>,
    /// The `cast[DType.<name>]()` conversions admitted over a lane-shaped
    /// receiver, each with its receiver occurrence, whose adjustment an
    /// instance records from its substituted result.
    simd_casts: RefCell<Vec<(OccurrenceId, OccurrenceId)>>,
    /// The `.length` reads admitted over a lane-shaped receiver, each with
    /// its receiver occurrence, whose adjustment an instance records from
    /// the receiver's substituted type.
    simd_lengths: RefCell<Vec<(OccurrenceId, OccurrenceId)>>,
    /// The pack storages admitted ([`Self::pack_storage`]), which an
    /// instance reads as its own relocation.
    pack_relocations: RefCell<Vec<mojito_checked::templates::PackRelocation>>,
    /// The stringify calls admitted ([`Self::stringify`]), each routed to
    /// the builtin by an overload target no instance changes.
    stringified: RefCell<Vec<OccurrenceId>>,
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
    /// A nested `def`'s name, which the body may only call.
    Callable,
}

impl BodyShape<'_> {
    fn statement(&self, statement: &Stmt) -> bool {
        match &statement.kind {
            // A nested body returns to its own caller: a closed scalar, a
            // whole value of its declared result, or nothing.
            StmtKind::Return(value) if self.nested_depth.get() > 0 => {
                value.as_ref().is_none_or(|value| {
                    (self.expression(value) && self.scalar(value)) || self.whole_value(value)
                }) && self.holds(MethodFeatures::STATEMENTS)
            }
            StmtKind::Def { .. }
                if !self.keyed
                    && self.moved_result.is_some()
                    && self.loop_vars.borrow().is_empty() =>
            {
                self.nested_def(statement)
            }
            StmtKind::Return(value) if self.reference_result.is_some() => value
                .as_ref()
                .is_some_and(|value| self.returned_place(value)),
            StmtKind::Return(Some(value)) => {
                (self.expression(value) && self.scalar(value))
                    || self.moved_result.is_some_and(|result| {
                        (self.whole_value(value)
                            || self.reference_read(value)
                            || self.tuple_element_value(value))
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
                .all(|arm| self.block(arm)),
            // An unrolled body is copied once per iteration, every copy
            // sharing the bindings around the loop; a local declared inside
            // is one binding per copy (`renumber_locals`). The loop variable
            // folds to a literal wherever it survives, which `folded_value`
            // admits where the literal's facts are the name's.
            StmtKind::ComptimeFor { var, body, .. } if self.keyed => {
                self.loop_vars.borrow_mut().push(var.clone());
                let admitted = self.block(body);
                self.loop_vars.borrow_mut().pop();
                admitted
            }
            StmtKind::VarDecl { name, value, .. } if self.keyed => {
                let scalar =
                    (self.expression(value) && self.scalar(value)) || self.simd_value(value);
                self.locals
                    .borrow_mut()
                    .push((name.clone(), LocalKind::Scalar));
                scalar
            }
            // A validated method keeps its compile-time control flow: every
            // arm is checked once, and an instance keeps the arms the
            // elaborator selected, once per unrolled copy, and drops the
            // rest with their facts (`COMPTIME_CONTROL`).
            StmtKind::ComptimeIf { branches, orelse } => {
                branches
                    .iter()
                    .map(|(_, arm)| arm)
                    .chain(orelse)
                    .all(|arm| self.block(arm))
                    && self.holds(MethodFeatures::STATEMENTS)
                    && self.holds(MethodFeatures::COMPTIME_CONTROL)
            }
            StmtKind::ComptimeFor { var, body, .. } => {
                self.loop_vars.borrow_mut().push(var.clone());
                let admitted = self.block(body);
                self.loop_vars.borrow_mut().pop();
                admitted
                    && self.holds(MethodFeatures::STATEMENTS)
                    && self.holds(MethodFeatures::COMPTIME_CONTROL)
            }
            // A runtime statement is checked once, whatever runs it, so it
            // neither drops nor copies an occurrence. A scalar local is one
            // binding wherever it is declared. An annotated local holds its
            // declared type, which the value may convert to.
            StmtKind::VarDecl { name, ty, value } if !self.keyed => {
                let closed =
                    (self.expression(value) && self.scalar(value)) || self.lane_local_value(value);
                let scalar = closed && (ty.is_none() || self.scalar_binding(value));
                let moved = !scalar
                    && self.moved_result.is_some()
                    && (closed
                        || self.whole_value(value)
                        || self.simd_value(value)
                        || self.reference_read(value)
                        || self.tuple_element_value(value)
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
            // A runtime `if` is checked once, or once per unrolled copy in
            // a keyed body, each copy's condition folding its loop variable
            // as the copy's other statements do.
            StmtKind::If { branches, orelse } => {
                branches
                    .iter()
                    .all(|(condition, arm)| self.condition(condition) && self.block(arm))
                    && orelse.as_ref().is_none_or(|arm| self.block(arm))
                    && (self.keyed || self.holds(MethodFeatures::STATEMENTS))
            }
            // A runtime loop in a method selects its iterator protocol from
            // the iterable's type, which an instance selects again. The loop
            // variable is one local wherever the loop runs, of the kind its
            // recorded binding type makes it.
            StmtKind::For {
                var,
                iter,
                body,
                orelse: None,
                ..
            } if !self.keyed
                && (self.moved_result.is_some() || self.reference_result.is_some()) =>
            {
                let iterable = self.iterable(iter);
                let kind = self.loop_local(statement);
                self.locals.borrow_mut().push((var.clone(), kind));
                iterable
                    && self.block(body)
                    && self.holds(MethodFeatures::STATEMENTS)
                    && self.holds(MethodFeatures::ITERATION)
            }
            StmtKind::With { items, body } if !self.keyed && self.moved_result.is_some() => {
                self.with_statement(statement, items, body)
                    && self.holds(MethodFeatures::STATEMENTS)
                    && self.holds(MethodFeatures::WITH_STATEMENTS)
            }
            // A `with` desugar's guard: the body, then its cleanup, and for
            // an error exit a handler binding the body's error.
            StmtKind::Try {
                body,
                except,
                orelse: None,
                finalbody: Some(finalbody),
            } if self.desugar_depth.get() > 0 => {
                self.block(body)
                    && except.as_ref().is_none_or(|(binder, handler)| {
                        let Some(binder) = binder else {
                            return false;
                        };
                        let scope = self.locals.borrow().len();
                        self.locals
                            .borrow_mut()
                            .push((binder.clone(), LocalKind::Value));
                        self.error_binders.borrow_mut().push(binder.clone());
                        let admitted = self.block(handler);
                        self.error_binders.borrow_mut().pop();
                        self.locals.borrow_mut().truncate(scope);
                        admitted
                    })
                    && self.block(finalbody)
            }
            // A runtime guard: the body, then a handler, bare or binding
            // the body's error, which it may raise again.
            StmtKind::Try {
                body,
                except: Some((binder, handler)),
                orelse: None,
                finalbody: None,
            } if !self.keyed && self.moved_result.is_some() => {
                let scope = self.locals.borrow().len();
                let admitted = self.block(body) && {
                    if let Some(binder) = binder {
                        self.locals
                            .borrow_mut()
                            .push((binder.clone(), LocalKind::Value));
                        self.error_binders.borrow_mut().push(binder.clone());
                    }
                    let handled = self.block(handler);
                    if binder.is_some() {
                        self.error_binders.borrow_mut().pop();
                    }
                    handled
                };
                self.locals.borrow_mut().truncate(scope);
                admitted
                    && self.holds(MethodFeatures::STATEMENTS)
                    && self.holds(MethodFeatures::TRY_STATEMENTS)
            }
            StmtKind::Unpack {
                targets,
                value,
                declares,
            } if !self.keyed => {
                self.tuple_unpack(targets, value, *declares)
                    && self.holds(MethodFeatures::STATEMENTS)
            }
            StmtKind::While {
                cond,
                body,
                orelse: None,
            } if !self.keyed => {
                self.condition(cond) && self.block(body) && self.holds(MethodFeatures::STATEMENTS)
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
                    ExprKind::Call { .. } | ExprKind::MethodCall { .. } | ExprKind::Invoke { .. }
                ) && self.expression(value)
                    && self.closed(value);
                call || (self.moved_result.is_some() && self.pointer_statement(value))
                    || (!self.keyed && self.abort(value))
                    || (self.desugar_depth.get() > 0 && self.keep_alive(value))
                    || (self.print_call(value)
                        && (self.keyed || self.holds(MethodFeatures::STATEMENTS)))
            }
            // A discarded value: a closed scalar, or a whole value copied or
            // moved out as any other ([`Self::whole_value`]) and destroyed
            // at the instance's type as any temporary is.
            StmtKind::Assign { name, value } if name == "_" => {
                (self.expression(value) && self.scalar(value))
                    || (!self.keyed
                        && self.whole_value(value)
                        && self.holds(MethodFeatures::STATEMENTS))
            }
            // A scalar local takes a closed scalar. Any `var` local may be
            // rebound whole to a value of its own type: the old value's
            // destruction is the local's, and the new one is moved or a
            // temporary, converted by nothing an instance selects.
            StmtKind::Assign { name, value } if self.declared(name) => {
                let scalar = self.local(name) && self.expression(value) && self.scalar(value);
                let rebound = !self.keyed
                    && self.moved_result.is_some()
                    && self.whole_value(value)
                    && self.facts.is_none_or(|facts| {
                        fact_at(&facts.conversions, self.occurrence(value)).is_none()
                    });
                (scalar || rebound) && (self.keyed || self.holds(MethodFeatures::STATEMENTS))
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
                        && (self.inplace_element(place, value) || self.inplace_place(place, value))
                        && self.holds(MethodFeatures::STATEMENTS))
            }
            _ => false,
        }
    }

    /// The value of a `return` in a method that returns a reference: a field
    /// of `self`, a pointer slot, a `ref` local, a `mut` or `ref` parameter, a
    /// reference a call on a field yields, or a field read through such a
    /// reference, of exactly the declared referent type, so neither check
    /// converts it.
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
            || self.reference_call(value)
            || self.reference_member(value)
            || self.pack_element(value))
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

    /// A reference-returning call on a field of `self` or of a parameter
    /// holding a struct (`other.items[i]`), passing scalars:
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
            && (self.receiver_field(object)
                || on_self
                || self.value_local(object)
                || self.parameter_field(object))
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

    /// `slice.indices(n)` on a slice parameter or local: the built-in
    /// normalization, which reads a closed slice and a closed scalar length
    /// and yields a tuple of `Int`s. It selects nothing, so the call records
    /// only closed types, alike under every instance.
    fn slice_indices(
        &self,
        expr: &Expr,
        object: &Expr,
        method: &str,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> bool {
        let receiver = matches!(&object.kind, ExprKind::Identifier(name)
            if self.params.contains(&name.as_str()) || self.local_kind(name).is_some());
        let shape = !self.keyed
            && method == "indices"
            && receiver
            && kwargs.is_empty()
            && matches!(args, [length] if self.expression(length) && self.scalar(length));
        shape
            && self.facts.is_none_or(|facts| {
                let id = self.occurrence(expr);
                let slice = fact_at(&facts.expression_types, self.occurrence(object)).is_some_and(
                    |ty| matches!(ty, Ty::Struct(name, arguments)
                        if arguments.is_empty()
                            && matches!(name.as_str(), "Slice" | "StridedSlice" | "ContiguousSlice")),
                );
                slice
                    && fact_at(&facts.selected_calls, id).is_none()
                    && fact_at(&facts.overload_targets, id).is_none()
                    && fact_at(&facts.operation_adjustments, id).is_none()
                    && fact_at(&facts.call_parameters, id).is_none()
            })
    }

    /// `local[k]`: an element of a tuple-typed `var` local at a literal
    /// index, read as a scalar operand or by value into a `var` or the
    /// result ([`Self::tuple_element_value`]).
    ///
    /// The index is the element's position, which no instance changes, and
    /// the element's type substitutes. The template either typed the element
    /// from the tuple's arguments and recorded nothing else there, or
    /// selected the generated Tuple's accessor for that position
    /// (`__getitem_param__$k`) as a closed reference call read by copy, which
    /// an instance records again on its own Tuple
    /// ([`Checker::realize_tuple_elements`]).
    fn tuple_element(&self, expr: &Expr) -> bool {
        let ExprKind::Index { object, index } = &expr.kind else {
            return false;
        };
        // Without facts a whole-value declaration reads as a scalar one;
        // the tuple's recorded type rules a scalar local out.
        let local = matches!(&object.kind, ExprKind::Identifier(name)
            if matches!(self.local_kind(name), Some(LocalKind::Value | LocalKind::Scalar)));
        if self.keyed || !local || !matches!(index.kind, ExprKind::Int(_)) {
            return false;
        }
        let id = self.occurrence(expr);
        let Some(facts) = self.facts else {
            return true;
        };
        let Some(tuple @ Ty::Struct(owner, _)) =
            fact_at(&facts.expression_types, self.occurrence(object))
        else {
            return false;
        };
        if mojito_types::types::tuple_elements(tuple).is_none() {
            return false;
        }
        let Some(call) = fact_at(&facts.selected_calls, id) else {
            return fact_at(&facts.overload_targets, id).is_none()
                && fact_at(&facts.operation_adjustments, id).is_none();
        };
        let accessor = names_method(&call.contract.target, owner, TUPLE_ELEMENT_ACCESSOR)
            && mojito_checked::templates::closed_reference_contract(call)
            && facts.copyable_reference_result_reads.contains(&id);
        if accessor {
            self.references.borrow_mut().push(id);
        }
        accessor && self.holds(MethodFeatures::REFERENCE_CALLS)
    }

    /// A tuple element read by value into a `var` or the result, whatever
    /// its type. The template either typed it from the tuple's arguments or
    /// selected the accessor as a copyable reference read, and an instance
    /// records the accessor read again and owes the copy at its own element
    /// type ([`Checker::realize_tuple_elements`]).
    fn tuple_element_value(&self, expr: &Expr) -> bool {
        self.tuple_element(expr) && self.holds(MethodFeatures::OPAQUE_MOVES)
    }

    /// What the certificate hands the instance from the grammar's walk: the
    /// occurrences it must dispatch or prove itself.
    fn grammar_notes(&self) -> GrammarNotes {
        GrammarNotes {
            operators: self.operators.borrow().clone(),
            bound_builtins: self.bound_builtins.borrow().clone(),
            constructions: self.constructions.borrow().clone(),
            callable_calls: self.callable_calls.borrow().clone(),
            repr_calls: self.repr_calls.borrow().clone(),
            print_calls: self.print_calls.borrow().clone(),
            simd_to_bits: self.simd_to_bits.borrow().clone(),
            simd_casts: self.simd_casts.borrow().clone(),
            simd_lengths: self.simd_lengths.borrow().clone(),
            pack_relocations: self.pack_relocations.borrow().clone(),
        }
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
    /// string literal or from a whole value of a closed type
    /// (`Error(String("…") + fspath)`).
    ///
    /// `require_error` asks whether the operand is a string, which a
    /// constructed struct's name and the builtin `Error` settle, and whether
    /// its type is the declared error type. Both types are functions of the
    /// same parameters, so the instance's answer is the template's.
    fn raised(&self, value: &Expr) -> bool {
        let id = self.occurrence(value);
        let message = |message: &Expr| {
            matches!(message.kind, ExprKind::Str(_))
                || (!self.keyed && self.whole_value(message) && self.closed(message))
        };
        let error = matches!(&value.kind, ExprKind::Call { name, param_args, args, kwargs }
            if name == "Error"
                && !self.structs.contains_key(name)
                && param_args.is_empty()
                && kwargs.is_empty()
                && matches!(args.as_slice(), [argument] if message(argument)))
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.expression_types, id) == Some(&Ty::Error)
                    && fact_at(&facts.call_parameters, id).is_none()
                    && fact_at(&facts.expression_bindings, id).is_none()
            });
        let literal = matches!(value.kind, ExprKind::Str(_))
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.expression_types, id) == Some(&Ty::StringLiteral)
            });
        let reraised = matches!(&value.kind, ExprKind::Identifier(name)
            if self.error_binders.borrow().contains(name))
            && self
                .facts
                .is_none_or(|facts| fact_at(&facts.expression_types, id) == Some(&Ty::Error));
        error || literal || reraised || self.construction(value)
    }

    /// A nested `def` (`NESTED_DEFS`), at any depth.
    ///
    /// It declares no compile-time parameters, decorators, `where` clause,
    /// or typed `raises`, and takes regular parameters, each read, `mut`,
    /// `var`, or a bare `ref`, with at most a closed-scalar default. A
    /// raising one keeps its effect in the recipe. Its recorded parameter
    /// and result types substitute in the recipe, where they may mention the
    /// struct's parameters, and each parameter's deletability is judged at
    /// the instance's type. Each capture, listed or reached through a
    /// capture-all default, names a local, a parameter, or `self`, by any
    /// convention, which an instance maps to its own binding; an owned one
    /// owes its capability again at the instance's type. Its body is judged
    /// in place with its parameters as locals: a `ref` one of a whole value
    /// a handle, a `mut` or `var` one of a whole value a `var` local, and
    /// any other read where it lies. It returns a closed scalar or a whole
    /// value, and its name is a local the body may only call.
    fn nested_def(&self, statement: &Stmt) -> bool {
        use mojito_ast::ast::ArgConvention;
        let StmtKind::Def {
            name,
            decorators,
            type_params,
            params,
            positional_only,
            keyword_only,
            captures,
            raises_type,
            where_clauses,
            body,
            ..
        } = &statement.kind
        else {
            return false;
        };
        let declaration = decorators.is_empty()
            && type_params.is_empty()
            && positional_only.is_none()
            && keyword_only.is_none()
            && raises_type.is_none()
            && where_clauses.is_empty()
            && params.iter().all(|parameter| {
                parameter.kind == mojito_ast::ast::ParamKind::Regular
                    && matches!(
                        parameter.convention,
                        None | Some(
                            ArgConvention::Imm
                                | ArgConvention::Mut
                                | ArgConvention::Var
                                | ArgConvention::Ref
                        )
                    )
                    && parameter.origin.is_none()
                    && parameter
                        .default
                        .as_ref()
                        .is_none_or(|default| self.expression(default) && self.scalar(default))
            });
        let captured = captures.as_ref().is_none_or(|list| {
            list.entries.iter().all(|capture| {
                self.declared(&capture.name)
                    || self.params.contains(&capture.name.as_str())
                    || (self.receiver && capture.name == "self")
            })
        });
        let recipe = self
            .facts
            .map(|facts| fact_at(&facts.nested_defs, self.occurrence_of(statement)));
        if !declaration || !captured || recipe.is_some_and(|recipe| recipe.is_none()) {
            return false;
        }
        let scope = self.locals.borrow().len();
        self.locals
            .borrow_mut()
            .extend(params.iter().enumerate().map(|(index, parameter)| {
                let whole = recipe.flatten().is_some_and(|recipe| {
                    recipe
                        .param_types
                        .get(index)
                        .is_some_and(|ty| !grammar_scalar(ty))
                });
                let kind = match parameter.convention {
                    Some(ArgConvention::Ref) if whole => LocalKind::Reference,
                    Some(ArgConvention::Mut | ArgConvention::Var) if whole => LocalKind::Value,
                    _ => LocalKind::Scalar,
                };
                (parameter.name.clone(), kind)
            }));
        self.nested_depth.set(self.nested_depth.get() + 1);
        let admitted = self.block(body);
        self.nested_depth.set(self.nested_depth.get() - 1);
        self.locals.borrow_mut().truncate(scope);
        self.locals
            .borrow_mut()
            .push((name.clone(), LocalKind::Callable));
        admitted
            && self.holds(MethodFeatures::STATEMENTS)
            && self.holds(MethodFeatures::NESTED_DEFS)
    }

    /// A `with` statement. Before capture it is judged from its syntax: each
    /// context a whole value, each `as` name a local scoped to the block,
    /// and the block. After capture the desugar the check recorded stands
    /// in its place, judged as the body's own statements are: the manager a
    /// `var` local, its `__enter__` and `__exit__` sibling calls, the
    /// guarding `try`, and the liveness anchor. Its form is the manager
    /// struct's declaration, which an instance builds its own desugar from
    /// (`Checker::instance_with_desugars`).
    fn with_statement(
        &self,
        statement: &Stmt,
        items: &[mojito_ast::ast::WithItem],
        body: &[Stmt],
    ) -> bool {
        let Some(facts) = self.facts else {
            let scope = self.locals.borrow().len();
            let admitted = items.iter().all(|item| {
                let context = self.whole_value(&item.context);
                if let Some(name) = &item.var {
                    let kind = self.entered_kind(&item.context);
                    self.locals.borrow_mut().push((name.clone(), kind));
                }
                context
            }) && self.block(body);
            self.locals.borrow_mut().truncate(scope);
            return admitted;
        };
        let Some(desugar) = self.desugars.get(&statement.source_span()) else {
            return false;
        };
        if fact_at(&facts.with_forms, self.occurrence_of(statement)) != Some(&desugar.form) {
            return false;
        }
        self.desugar_depth.set(self.desugar_depth.get() + 1);
        let admitted = self.block(&desugar.statements);
        self.desugar_depth.set(self.desugar_depth.get() - 1);
        admitted
    }

    /// The kind of local a `with` binds its context's `__enter__` result
    /// to, read from the manager struct's declaration when the context
    /// constructs one: the facts that decide it are not captured yet.
    fn entered_kind(&self, context: &Expr) -> LocalKind {
        let scalar = matches!(&context.kind, ExprKind::Call { name, .. }
            if self.structs.get(name).and_then(|info| info.methods.get("__enter__"))
                .is_some_and(|sigs| sigs.iter().any(|sig| sig.has_self && closed_scalar(&sig.ret))));
        if scalar {
            LocalKind::Scalar
        } else {
            LocalKind::Value
        }
    }

    /// The `with` desugar's liveness anchor, `_mojito_keep_alive(name)` of
    /// a local: it selects no callee and records only the local's read.
    fn keep_alive(&self, expr: &Expr) -> bool {
        let id = self.occurrence(expr);
        matches!(&expr.kind, ExprKind::Call { name, param_args, args, kwargs }
            if name == super::with_stmt::KEEP_ALIVE_BUILTIN
                && param_args.is_empty()
                && kwargs.is_empty()
                && matches!(args.as_slice(), [local]
                    if matches!(&local.kind, ExprKind::Identifier(name) if self.declared(name))))
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.call_parameters, id).is_none()
                    && fact_at(&facts.expression_bindings, id).is_none()
            })
    }

    /// Note that the body holds `feature`.
    fn holds(&self, feature: MethodFeatures) -> bool {
        self.features.set(self.features.get().union(feature));
        true
    }

    /// A runtime condition: a scalar expression whose recorded type is
    /// exactly `Bool`, which `expect_bool` accepts without a truthiness fact,
    /// a closed bool lane tested through `Bool(x)` (`n != 0` over a
    /// `UInt32`), whose mark no instance changes, or a place tested through
    /// `Bool(x)` ([`Self::truthiness_condition`]).
    fn condition(&self, expr: &Expr) -> bool {
        let id = self.occurrence(expr);
        let expression = self.expression(expr);
        let boolean = expression
            && self.facts.is_none_or(|facts| {
                facts
                    .expression_types
                    .iter()
                    .any(|(site, ty)| *site == id && *ty == Ty::Bool)
            });
        let lane = || {
            expression
                && self.scalar(expr)
                && self.facts.is_none_or(|facts| {
                    facts.truthiness_conditions.contains(&id)
                        && fact_at(&facts.conversions, id).is_none()
                        && fact_at(&facts.operation_adjustments, id).is_none()
                })
                && self.holds(MethodFeatures::TRUTHINESS)
        };
        boolean || lane() || self.truthiness_condition(expr)
    }

    /// A condition that reads a parameter, a local, or a field of `self`
    /// whole and tests it through `__bool__`.
    ///
    /// The read records only the place's type and binding, and the mark is
    /// decided by that type alone, which an instance judges again
    /// (`realize_instance_facts`). Nothing else may be recorded at the
    /// condition: it is neither copied nor converted.
    fn truthiness_condition(&self, expr: &Expr) -> bool {
        let place = match &expr.kind {
            ExprKind::Identifier(name) => {
                self.params.contains(&name.as_str())
                    || self.declared(name)
                    || self.reference_local(name)
            }
            ExprKind::Member { .. } => self.receiver_field(expr) || self.reference_member(expr),
            _ => false,
        };
        let id = self.occurrence(expr);
        let admitted = place
            && self.facts.is_none_or(|facts| {
                facts.truthiness_conditions.contains(&id)
                    && !facts.copy_place_value_uses.contains(&id)
                    && fact_at(&facts.conversions, id).is_none()
                    && fact_at(&facts.operation_adjustments, id).is_none()
            });
        admitted && self.holds(MethodFeatures::TRUTHINESS)
    }

    /// A whole value of any type, moved or copied out of a parameter, a
    /// local, or a field of `self`, or a `var` receiver moved out whole. It is never an operand, a receiver, or a
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
                    || self.receiver_itself(place)
            }
            ExprKind::Member { .. } => {
                self.receiver_field(place)
                    || self.local_field(place)
                    || self.parameter_field(place)
                    || self.reference_member(place)
            }
            _ => false,
        };
        // A place the body only borrows is copied, never moved out of.
        let borrowed = |place: &Expr| match &place.kind {
            ExprKind::Identifier(name) => {
                self.borrowed_params.contains(&name.as_str())
                    || self.reference_local(name)
                    || (self.receiver_itself(place) && !self.self_owned())
            }
            ExprKind::Member { .. } => !self.receiver_field(place),
            _ => false,
        };
        let admitted = match &expr.kind {
            ExprKind::Transfer(inner) => {
                (source(inner) && !borrowed(inner)) || self.call_result(inner)
            }
            _ if self.call_result(expr)
                || self.pack_storage(expr)
                || self.fieldwise_copy(expr)
                || self.construction(expr)
                || self.binder_construction(expr)
                || self.operator_value(expr)
                || self.comprehension(expr)
                || self.tuple_display(expr)
                || self.stringify(expr)
                || self.closed_operator(expr) =>
            {
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

    /// `__mojito_fieldwise_copy(self)`, the synthesized `copy`'s result: a
    /// copy of the receiver whole, which the declaration's `Copyable`
    /// clause guarantees every instance
    /// ([`TemplateObligation::DeclarationConstraints`]).
    fn fieldwise_copy(&self, expr: &Expr) -> bool {
        matches!(&expr.kind, ExprKind::Call { name, param_args, args, kwargs }
            if name == "__mojito_fieldwise_copy"
                && param_args.is_empty()
                && kwargs.is_empty()
                && matches!(args.as_slice(), [receiver] if self.receiver_itself(receiver)))
    }

    /// `__RuntimeTuple(*args^)`: a pack struct's storage built from the
    /// initializer's own pack collector, moved whole. The template typed
    /// the storage and the collector over the symbolic pack, which an
    /// instance substitutes element by element.
    fn pack_storage(&self, expr: &Expr) -> bool {
        let ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } = &expr.kind
        else {
            return false;
        };
        let [argument] = args.as_slice() else {
            return false;
        };
        let ExprKind::Spread(transfer) = &argument.kind else {
            return false;
        };
        let ExprKind::Transfer(pack) = &transfer.kind else {
            return false;
        };
        let param = match &pack.kind {
            ExprKind::Identifier(pack) if self.packs.contains(&pack.as_str()) => {
                self.params.iter().position(|param| param == pack)
            }
            _ => None,
        };
        let Some(param) = param.filter(|_| {
            name == "__RuntimeTuple"
                && param_args.is_empty()
                && kwargs.is_empty()
                && self.pack_struct.is_some()
        }) else {
            return false;
        };
        push_unique(
            &mut self.pack_relocations.borrow_mut(),
            mojito_checked::templates::PackRelocation {
                call: self.occurrence(expr),
                spread: self.occurrence(argument),
                transfer: self.occurrence(transfer),
                pack: self.occurrence(pack),
                param,
            },
        );
        true
    }

    /// The iterable of a runtime `for`: a place the loop borrows (`self`, a
    /// field of it, a parameter, a local), the `^` transfer of a place the
    /// body owns, a sibling call's result, or a direct call's (`range(n)`). The loop records its protocol
    /// at the iterable, which a recipe must then hold
    /// ([`Checker::realize_iterations`]).
    fn iterable(&self, iter: &Expr) -> bool {
        let admitted = match &iter.kind {
            ExprKind::Transfer(_) => self.whole_value(iter),
            ExprKind::Identifier(name) => {
                self.receiver_itself(iter)
                    || self.params.contains(&name.as_str())
                    || self.local_kind(name).is_some()
            }
            ExprKind::Member { .. } => self.receiver_field(iter) || self.reference_member(iter),
            ExprKind::Call { .. } => self.expression(iter),
            _ => self.call_result(iter),
        };
        admitted
            && self
                .facts
                .is_none_or(|facts| fact_at(&facts.iterations, self.occurrence(iter)).is_some())
    }

    /// A tuple unpacked from a parameter, a local, a field of `self`, or a
    /// sibling call's result, into `_` and `var` locals: each declared by the
    /// statement, or declared before it.
    ///
    /// The element reads are synthesized from the value's type and place,
    /// which an instance derives them from again
    /// ([`Checker::realize_tuple_unpacks`]); the value records no conversion
    /// or adjustment an instance could not repeat. A declared target is a
    /// scalar local where its recorded binding type is a closed scalar, and a
    /// whole value otherwise, which only a method that may move whole values
    /// holds.
    fn tuple_unpack(&self, targets: &[Expr], value: &Expr, declares: bool) -> bool {
        let id = self.occurrence(value);
        let source = match &value.kind {
            ExprKind::Identifier(name) => {
                self.params.contains(&name.as_str()) || self.declared(name)
            }
            ExprKind::Member { .. } => self.receiver_field(value),
            ExprKind::Call { param_args, .. } if param_args.is_empty() => self.expression(value),
            _ => self.call_result(value),
        };
        let recorded = self.facts.is_none_or(|facts| {
            fact_at(&facts.tuple_unpacks, id).is_some()
                && fact_at(&facts.conversions, id).is_none()
                && fact_at(&facts.operation_adjustments, id).is_none()
        });
        let bound = targets.iter().all(|target| match &target.kind {
            ExprKind::Identifier(name) if name == "_" => true,
            ExprKind::Identifier(name) if declares => {
                let kind = self.target_local(target);
                self.locals.borrow_mut().push((name.clone(), kind));
                kind == LocalKind::Scalar || self.moved_result.is_some()
            }
            ExprKind::Identifier(name) => self.declared(name),
            _ => false,
        });
        source && recorded && bound && self.holds(MethodFeatures::TUPLE_UNPACKS)
    }

    /// What a local an unpacking declares is, from the binding type recorded
    /// at its target: a scalar local where a closed scalar, and a whole value
    /// otherwise. With no facts yet, the syntax alone never rules a use out.
    fn target_local(&self, target: &Expr) -> LocalKind {
        let id = self.occurrence(target);
        self.facts.map_or(LocalKind::Scalar, |facts| {
            if fact_at(&facts.binding_types, id).is_some_and(grammar_scalar) {
                LocalKind::Scalar
            } else {
                LocalKind::Value
            }
        })
    }

    /// What a loop variable is, from the binding type its loop recorded: a
    /// handle where the loop binds a reference, a scalar local where a
    /// closed scalar, and a whole value otherwise. With no facts yet, the
    /// syntax alone never rules a use out.
    fn loop_local(&self, statement: &Stmt) -> LocalKind {
        let declaration = OccurrenceId {
            syntax: self.origins.origin(statement.syntax_id),
            copy: 0,
        };
        self.facts.map_or(LocalKind::Scalar, |facts| {
            // A temporary source leaves the reference's declared origin
            // unrooted, so its type stays among the plain binding types.
            let binding = fact_at(&facts.binding_types, declaration);
            if fact_at(&facts.reference_binding_types, declaration).is_some()
                || matches!(binding, Some(Ty::Ref(_)))
            {
                LocalKind::Reference
            } else if binding.is_some_and(grammar_scalar) {
                LocalKind::Scalar
            } else {
                LocalKind::Value
            }
        })
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

    /// The result of a sibling call, of a generic callee's explicit
    /// application, or of a function body's direct call, of any type: a
    /// temporary, whose type is the contract's or the application's
    /// substituted result.
    fn call_result(&self, expr: &Expr) -> bool {
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
            ExprKind::Call {
                name, param_args, ..
            } => !param_args.is_empty() || self.local_kind(name) == Some(LocalKind::Callable),
            _ => false,
        };
        call && self.expression(expr)
    }

    /// An operator over two operands of one closed struct type, or such an
    /// operand and a string literal converted to it
    /// (`String("bad: ") + name + " n: "`): a temporary of that type.
    ///
    /// The operand type's own dunder answers, which no instance changes, so
    /// the template recorded nothing at the operator, and a literal operand's
    /// conversion is selected again at the same closed types. An operand is
    /// a named place, read where it lies, or a whole value.
    fn closed_operator(&self, expr: &Expr) -> bool {
        let ExprKind::Infix(op, left, right) = &expr.kind else {
            return false;
        };
        let operand = |operand: &Expr| {
            matches!(operand.kind, ExprKind::Str(_))
                || matches!(&operand.kind, ExprKind::Identifier(name)
                    if self.declared(name) || self.params.contains(&name.as_str()))
                || self.receiver_field(operand)
                || self.whole_value(operand)
        };
        let id = self.occurrence(expr);
        let admitted = !self.keyed
            && operator_dispatch(*op)
            && operand(left)
            && operand(right)
            && !matches!(left.kind, ExprKind::Str(_))
            && self.facts.is_none_or(|facts| {
                let at = |operand: &Expr| self.occurrence(operand);
                // A nominal-string wrap is its producer's own conversion
                // ([`Self::stringify`]), not one of the operand.
                let unconverted = |operand: &Expr| {
                    fact_at(&facts.conversions, at(operand))
                        .is_none_or(|conversion| conversion.result.is_none())
                };
                let Some(ty @ Ty::Struct(..)) = fact_at(&facts.expression_types, at(left)) else {
                    return false;
                };
                let right_typed = match &right.kind {
                    ExprKind::Str(_) => fact_at(&facts.conversions, at(right))
                        .is_some_and(|conversion| conversion.result.as_ref() == Some(ty)),
                    _ => {
                        fact_at(&facts.expression_types, at(right)) == Some(ty)
                            && unconverted(right)
                    }
                };
                !mojito_types::types::is_symbolic(ty)
                    && right_typed
                    && fact_at(&facts.expression_types, id) == Some(ty)
                    && unconverted(left)
                    && fact_at(&facts.overload_targets, id).is_none()
                    && fact_at(&facts.operation_adjustments, id).is_none()
                    && fact_at(&facts.selected_calls, id).is_none()
                    && fact_at(&facts.call_parameters, id).is_none()
            });
        admitted && self.holds(MethodFeatures::CLOSED_OPERATORS)
    }

    /// A string literal a construction takes as the `StringLiteral` it is
    /// (`String("x")`): a temporary of a closed type no instance changes,
    /// converted to nothing.
    fn unconverted_string_literal(&self, expr: &Expr) -> bool {
        let id = self.occurrence(expr);
        matches!(expr.kind, ExprKind::Str(_))
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.expression_types, id) == Some(&Ty::StringLiteral)
                    && fact_at(&facts.conversions, id).is_none()
            })
    }

    /// `String(value)` of one value of a closed type other than a string
    /// literal (`String(fspath[byte=:i])`, `String(err)`): the stringify
    /// builtin, which reads the value where it lies to write it through its
    /// `Writable` conformance and wraps the text as the nominal `String`.
    ///
    /// The call routes to the builtin (an overload target of `"String"`) and
    /// records the wrap's conversion at itself, both chosen by the closed
    /// type alone. A value other than a scalar is written through its
    /// `Writable` conformance, which keeps its place, admitted here
    /// ([`Self::references_recorded`]).
    fn stringify(&self, expr: &Expr) -> bool {
        let ExprKind::Call {
            name,
            param_args,
            args,
            kwargs,
        } = &expr.kind
        else {
            return false;
        };
        let [argument] = args.as_slice() else {
            return false;
        };
        let named = match &argument.kind {
            ExprKind::Identifier(name) => {
                self.declared(name) || self.params.contains(&name.as_str())
            }
            _ => self.receiver_field(argument),
        };
        let id = self.occurrence(expr);
        let value = self.occurrence(argument);
        let scalar = self.expression(argument) && self.scalar(argument);
        let kept = self
            .facts
            .is_some_and(|facts| facts.call_place_uses.contains(&value));
        let admitted = !self.keyed
            && mojito_types::types::is_stdlib_string_struct(name)
            && param_args.is_empty()
            && kwargs.is_empty()
            && (scalar || named || self.whole_value(argument))
            && self.closed(argument)
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.overload_targets, id).is_some_and(|target| target == "String")
                    && fact_at(&facts.conversions, id).is_some()
                    && fact_at(&facts.call_parameters, id).is_none()
                    && fact_at(&facts.selected_calls, id).is_none()
                    && fact_at(&facts.expression_types, value) != Some(&Ty::StringLiteral)
                    && (scalar || kept)
            });
        if admitted {
            if kept {
                push_unique(&mut self.places.borrow_mut(), value);
            }
            push_unique(&mut self.stringified.borrow_mut(), id);
        }
        admitted && self.holds(MethodFeatures::STRINGIFY)
    }

    /// A tuple display (`return head, tail`): a temporary `Tuple` built
    /// from its elements, each a closed scalar or a whole value the display
    /// takes as it stands, converted by nothing an instance selects.
    ///
    /// The display records its `Tuple` type as a collection construction,
    /// which an instance substitutes element by element.
    fn tuple_display(&self, expr: &Expr) -> bool {
        let ExprKind::TupleLit(elements) = &expr.kind else {
            return false;
        };
        !self.keyed
            && !elements.is_empty()
            && elements.iter().all(|element| {
                ((self.expression(element) && self.scalar(element)) || self.whole_value(element))
                    && self.facts.is_none_or(|facts| {
                        fact_at(&facts.conversions, self.occurrence(element)).is_none()
                    })
            })
            && self.facts.is_none_or(|facts| {
                matches!(
                    fact_at(&facts.operation_adjustments, self.occurrence(expr)),
                    Some(
                        mojito_checked::checked::SemanticAdjustment::ConstructCollection {
                            insert: None,
                            ..
                        }
                    )
                )
            })
    }

    /// A keyword slice of a local or a parameter of a closed type
    /// (`fspath[byte=:i]`), with closed scalar bounds: a view temporary its
    /// getter returns over the place, whose origin an instance roots at its
    /// own binding of the place.
    ///
    /// The getter is selected on the place's recorded struct, which every
    /// instance shares, and binds the slice descriptor by value; the
    /// descriptor the check keys at the subscript depends only on the
    /// bounds' syntax.
    fn keyword_slice(&self, expr: &Expr) -> bool {
        use mojito_ast::ast::SubscriptArg;
        let ExprKind::MultiIndex { object, args } = &expr.kind else {
            return false;
        };
        let named = matches!(&object.kind, ExprKind::Identifier(name)
            if self.declared(name) || self.params.contains(&name.as_str()));
        let bounds = !args.is_empty()
            && args.iter().all(|argument| match argument {
                SubscriptArg::KeywordSlice {
                    lower, upper, step, ..
                } => [lower, upper, step]
                    .into_iter()
                    .flatten()
                    .all(|bound| self.expression(bound) && self.scalar(bound)),
                _ => false,
            });
        let id = self.occurrence(expr);
        let admitted = !self.keyed
            && named
            && self.closed(object)
            && bounds
            && self.facts.is_none_or(|facts| {
                self.named_contract(facts, expr, object, "__getitem__")
                    .is_some_and(mojito_checked::templates::value_method_contract)
                    && fact_at(&facts.subscript_descriptors, id).is_some()
            });
        if admitted {
            self.subscript(id);
        }
        admitted && self.holds(MethodFeatures::SLICE_VIEWS)
    }

    /// A list, set, or dict comprehension: a temporary of the collection
    /// type, built through its insert method (`ConstructCollection`).
    ///
    /// Each generator clause iterates what a runtime `for` may, and records
    /// its protocol at the iterable ([`Self::iterable`]). Its binder is a
    /// local scoped to the clauses after it and to the produced elements, of
    /// the kind its recorded binding type makes it, and is declared from the
    /// protocol's binding plan, which an instance selects again
    /// ([`Checker::install_comprehension_bindings`]). A filter is a runtime
    /// condition, and each produced key or value a closed scalar or a whole
    /// value the collection consumes.
    fn comprehension(&self, expr: &Expr) -> bool {
        let ExprKind::Comprehension {
            key,
            value,
            clauses,
            ..
        } = &expr.kind
        else {
            return false;
        };
        let binders = self
            .facts
            .map(|facts| fact_at(&facts.comprehension_bindings, self.occurrence(expr)));
        if self.keyed || binders.is_some_and(|binders| binders.is_none()) {
            return false;
        }
        let kind = |index: usize| {
            binders
                .flatten()
                .map_or(Some(LocalKind::Scalar), |binders| {
                    binders.get(index).map(|binder| {
                        if binder.reference {
                            LocalKind::Reference
                        } else if grammar_scalar(&binder.ty) {
                            LocalKind::Scalar
                        } else {
                            LocalKind::Value
                        }
                    })
                })
        };
        let element = |element: &Expr| {
            (self.expression(element) && self.scalar(element)) || self.whole_value(element)
        };
        let scope = self.locals.borrow().len();
        let mut declared = 0;
        let admitted = clauses.iter().all(|clause| match clause {
            mojito_ast::ast::ComprehensionClause::For { var, iter, .. } => {
                let iterable = self.iterable(iter);
                let binder = kind(declared);
                declared += 1;
                binder.is_some_and(|binder| {
                    self.locals.borrow_mut().push((var.clone(), binder));
                    iterable
                })
            }
            mojito_ast::ast::ComprehensionClause::If(condition) => self.condition(condition),
        }) && key.as_deref().is_none_or(element)
            && element(value);
        self.locals.borrow_mut().truncate(scope);
        admitted && self.holds(MethodFeatures::COMPREHENSIONS)
    }

    /// A construction of one of the method's own trait-bounded binders
    /// (`H()`): a temporary of the binder's type, which every clone keeps
    /// symbolic, so each records the same `ConstructTypeParam`.
    ///
    /// The adjustment names the binder it constructs: a binder of the
    /// enclosing struct is another type under each instance, and stays
    /// outside.
    fn binder_construction(&self, expr: &Expr) -> bool {
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
        let admitted = !self.keyed
            && param_args.is_empty()
            && args.is_empty()
            && kwargs.is_empty()
            && self.binders.contains(&name.as_str())
            && self.facts.is_none_or(|facts| {
                matches!(fact_at(&facts.operation_adjustments, id),
                    Some(mojito_checked::checked::SemanticAdjustment::ConstructTypeParam { param })
                        if *param.name == **name
                            && matches!(fact_at(&facts.expression_types, id),
                            Some(Ty::Param { binder, .. })
                                if binder == param && !self.struct_binders.contains(&&binder.id)))
            });
        if admitted {
            self.binder_constructions.borrow_mut().push(id);
        }
        admitted && self.holds(MethodFeatures::BOUND_BINDERS)
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
        // The stringify builtin types as the struct it wraps its text in
        // ([`Self::stringify`]), and is routed to the builtin.
        let constructed = self.facts.is_none_or(|facts| {
            matches!(fact_at(&facts.expression_types, id),
                Some(Ty::Struct(constructed, _)) if constructed == name)
                && fact_at(&facts.overload_targets, id).is_none_or(|target| target != "String")
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
        // An untracked pointer is plain data under every instance: a read
        // parameter reads it where it lies and a `var` one copies it, as the
        // template did.
        let value = |field: Option<&(String, Ty)>, argument: &Expr| {
            (self.expression(argument) && self.scalar(argument))
                || self.whole_value(argument)
                || self.unconverted_string_literal(argument)
                || self.pointer(argument)
                || self.receiver_pointer(argument)
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

    /// A pointer into storage `self` owns: a field of `self`, a `var` local
    /// (`var new_data = unsafe_alloc[Self.T](n)`), or a field of one, whose
    /// recorded type is a pointer with no tracked provenance, or an element
    /// offset from one. Such a pointer holds no loan and names no place, and
    /// it is a pointer under every instance, so its methods are the built-in
    /// ones.
    fn pointer(&self, expr: &Expr) -> bool {
        let admitted = match &expr.kind {
            // An untracked pointer field or `var` local, or a field whose
            // provenance is the struct's own origin parameter (`Span._data`):
            // neither names a checker-local place, so the retained type is
            // the template's under every instance.
            ExprKind::Member { .. } | ExprKind::Identifier(_) => {
                let local = matches!(&expr.kind, ExprKind::Identifier(name) if self.declared(name));
                (self.receiver_field(expr) || self.local_field(expr) || local)
                    && self.facts.is_none_or(|facts| {
                        fact_at(&facts.expression_types, self.occurrence(expr)).is_some_and(|ty| {
                            matches!(ty, Ty::Pointer { origin, .. }
                            if matches!(
                                origin.as_origin(),
                                None | Some(mojito_types::origin::Origin::Param(_))
                            ))
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

    /// A tracked pointer to `self` or a field of it, rebound to the whole
    /// receiver: `Pointer(to=self.items).unsafe_origin_cast[origin_of(self)]()`.
    ///
    /// The pointee is the place's declared type under the instance's
    /// arguments, and both provenances are the receiver's own place, the
    /// inner one rooted at `self` and the cast's the symbolic
    /// `origin_of(self)`, so an instance roots them at its own `self`.
    fn receiver_pointer(&self, expr: &Expr) -> bool {
        let ExprKind::Invoke {
            callee,
            param_args,
            args,
            kwargs,
        } = &expr.kind
        else {
            return false;
        };
        let ExprKind::Member { object, field } = &callee.kind else {
            return false;
        };
        let receiver_origin = matches!(param_args.as_slice(),
            [mojito_ast::ast::ParamArg::Value(Expr { kind: ExprKind::Call { name, param_args, args, kwargs }, .. })]
                if name == "origin_of"
                    && param_args.is_empty()
                    && kwargs.is_empty()
                    && matches!(args.as_slice(), [origin] if self.receiver_itself(origin)));
        let pointer_to_receiver = matches!(&object.kind, ExprKind::Call { name, param_args, args, kwargs }
            if name == "Pointer"
                && param_args.is_empty()
                && args.is_empty()
                && matches!(kwargs.as_slice(), [to]
                    if to.name == "to"
                        && (self.receiver_itself(&to.value) || self.receiver_field(&to.value))));
        field == "unsafe_origin_cast"
            && receiver_origin
            && args.is_empty()
            && kwargs.is_empty()
            && pointer_to_receiver
            && self.holds(MethodFeatures::RECEIVER_POINTERS)
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

    /// A whole value stored to a field of a writable `self` or of a `var`
    /// local whose declared type is the value's own, so neither check
    /// converts it. An element offset from a pointer is such a value, a
    /// temporary of the pointer's own type.
    fn whole_store(&self, place: &Expr, value: &Expr) -> bool {
        let scalar = || self.expression(value) && self.scalar(value);
        let offset = || matches!(value.kind, ExprKind::MethodCall { .. }) && self.pointer(value);
        self.moved_result.is_some()
            && ((self.self_writable() && self.receiver_field(place))
                || self.local_field(place)
                || self.slot(place))
            && (self.whole_value(value) || scalar() || offset())
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
            fact_at(&facts.binding_types, self.occurrence(value)).is_some_and(grammar_scalar)
        })
    }

    /// Whether an annotated `var`'s value reaches the declared type either
    /// as it is or through a recorded conversion, which an instance selects
    /// again at its own types. A view conversion to an annotation whose
    /// origin is left to inference (`Span[Self.T, _]`) is one: it borrows
    /// the source place as the template's did.
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
        let admitted = ((self.self_writable() && self.receiver_field(place))
            || self.local_field(place)
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

    /// A value stored to an element of a writable base
    /// ([`Self::element_base`]), where the subscripted struct declares a
    /// setter (`self.counts[i] = n`, `self.index[b] = entries^`,
    /// `self[k] = v^`, `other.items[i] = x^` on a `mut other`).
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
        let admitted = self.element_base(object)
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
    /// The getter is kept beside it in `augmented_subscripts`, and an
    /// instance realizes it on its own subscripted value as it realizes the
    /// setter (`realize_element_getters`).
    fn setter_element(&self, place: &Expr) -> bool {
        self.through_setter(place)
            && self.scalar(place)
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.augmented_subscripts, self.occurrence(place))
                    .is_some_and(|store| store.inplace.is_none())
            })
    }

    /// A struct element stored augmented through its in-place dunder
    /// (`self.counters[i] += 3` → `__iadd__`), read through a mutable
    /// reference getter or through a value getter and a setter.
    ///
    /// On an element of a closed type the dunder is selected on that type,
    /// so the contract is kept in `augmented_subscripts` as it stands, and
    /// binds the operand, a closed scalar, by value. On an element of a
    /// bare parameter type the dunder is dispatched through the parameter's
    /// bound, and an instance selects its witness on its own element type
    /// (`realize_element_dunders`); the operand is then a value of the
    /// element's own type, bound by value to the witness's `Self`.
    fn inplace_element(&self, place: &Expr, value: &Expr) -> bool {
        (self.through_reference(place) || self.through_setter(place))
            && self.expression(value)
            && self.facts.is_none_or(|facts| {
                let store = fact_at(&facts.augmented_subscripts, self.occurrence(place));
                store.is_some_and(|store| {
                    store.inplace.as_ref().is_some_and(|inplace| {
                        let closed = !mojito_types::types::is_symbolic(&store.operand_ty)
                            && self.scalar(value)
                            && mojito_checked::templates::closed_method_contract(inplace);
                        let dispatched = mojito_symbol::symbol::is_trait_dispatch_symbol(
                            &inplace.contract.target,
                        ) && mojito_checked::templates::value_method_contract(
                            inplace,
                        ) && self.typed(value, &store.operand_ty);
                        closed || dispatched
                    })
                })
            })
    }

    /// A place of a struct or bare parameter type updated through its
    /// in-place dunder (`self.total += x`, `self.meter += Meter(1)`,
    /// `into += x`, `m += Meter(3)`): a field of a writable `self`, a `var`
    /// local holding a whole value, or a `mut` parameter.
    ///
    /// The contract is kept at the place in `inplace_updates`, and the
    /// operand is judged against it as a method call's argument. The dunder
    /// may raise in a method declared `raises`. A dunder dispatched through a
    /// bare parameter's bound is re-selected on the instance's type of the
    /// place ([`Checker::realize_embedded_dispatch`]), whose witness of a
    /// raising requirement may not raise.
    /// A struct's own dunder is realized as any method call on that struct
    /// is, which names the instance's clone of a struct built over the
    /// parameter (`realize_inplace_updates`).
    fn inplace_place(&self, place: &Expr, value: &Expr) -> bool {
        let writable = (self.self_writable() && self.receiver_field(place))
            || self.value_local(place)
            || matches!(&place.kind, ExprKind::Identifier(name)
                if self.mut_params.contains(&name.as_str()));
        writable
            && self.argument(place, value)
            && self.facts.is_none_or(|facts| {
                let id = self.occurrence(place);
                fact_at(&facts.inplace_updates, id).is_some_and(|call| {
                    let dispatched =
                        mojito_symbol::symbol::is_trait_dispatch_symbol(&call.contract.target);
                    let raising = mojito_checked::templates::raising_method_contract(call)
                        && self.holds(MethodFeatures::RAISES);
                    (mojito_checked::templates::value_method_contract(call) || raising)
                        && match fact_at(&facts.expression_types, id) {
                            Some(Ty::Param { .. }) => dispatched,
                            Some(Ty::Struct(..)) => !dispatched,
                            _ => false,
                        }
                })
            })
    }

    /// The subscript `place` of a writable base ([`Self::element_base`]),
    /// stored through the mutable reference its getter yields.
    fn through_reference(&self, place: &Expr) -> bool {
        let ExprKind::Index { object, .. } = &place.kind else {
            return false;
        };
        let id = self.occurrence(place);
        let admitted = self.element_base(object)
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

    /// The subscript `place` of a writable base ([`Self::element_base`]),
    /// read through a value getter and written back through a setter that
    /// takes the element by value. Both change per instance only in their
    /// targets and, by substitution, their types.
    fn through_setter(&self, place: &Expr) -> bool {
        let ExprKind::Index { object, index } = &place.kind else {
            return false;
        };
        let id = self.occurrence(place);
        let admitted = self.element_base(object)
            && self.argument(place, index)
            && self.facts.is_none_or(|facts| {
                self.named_contract(facts, place, object, "__setitem__")
                    .is_some_and(mojito_checked::templates::value_method_contract)
                    && fact_at(&facts.augmented_subscripts, id)
                        .and_then(|store| store.getter.as_ref())
                        .is_some_and(mojito_checked::templates::value_method_contract)
            });
        if admitted {
            self.subscript(id);
        }
        admitted
            && self.holds(MethodFeatures::SIBLING_CALLS)
            && self.holds(MethodFeatures::SUBSCRIPT_STORES)
    }

    /// Whether `object` is a base whose elements a store may write: `self`
    /// or one of its fields in a body that may write `self`, or a `mut`
    /// parameter holding a struct or one of its fields. Either is bound to
    /// the instance's argument and written where it lies, and a field has its
    /// declared type under its base's recorded arguments in a template and a
    /// clone alike.
    fn element_base(&self, object: &Expr) -> bool {
        let mut_parameter = |expr: &Expr| {
            matches!(&expr.kind, ExprKind::Identifier(name)
                if self.mut_params.contains(&name.as_str()))
                && self.parameter_receiver(expr)
        };
        (self.self_writable() && (self.receiver_field(object) || self.receiver_itself(object)))
            || mut_parameter(object)
            || matches!(&object.kind, ExprKind::Member { object, .. } if mut_parameter(object))
    }

    /// Note that the subscript `id` is the base of a store.
    fn subscript(&self, id: OccurrenceId) {
        let mut subscripts = self.subscripts.borrow_mut();
        if !subscripts.contains(&id) {
            subscripts.push(id);
        }
    }

    /// A nested block: its locals go out of scope with it.
    fn block(&self, statements: &[Stmt]) -> bool {
        let outer = self.locals.borrow().len();
        let admitted = statements.iter().all(|statement| self.statement(statement));
        self.locals.borrow_mut().truncate(outer);
        admitted
    }

    /// A call of a method on `self`, on one of its fields, on a `var` local,
    /// through a reference, or on a call's temporary result
    /// (`self.entries().size()`), passing admitted arguments, whose recorded
    /// contract changes per instance only in its target and its substituted
    /// result ([`Self::sibling_call`]). A temporary receiver's read and its
    /// destruction are recorded at the call's occurrence, by syntax.
    fn method_call(
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
    /// `receiver.method[3](x)`, on a receiver [`Self::method_call`] admits.
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
    fn parameterized_call(
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
            ParamArg::Value(value) => matches!(
                value.kind,
                ExprKind::Int(_) | ExprKind::Bool(_) | ExprKind::Float(_)
            ),
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
            && self.method_call(expr, object, field, args, kwargs);
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
    /// Each member of a generic struct's static has no binders of its own,
    /// no availability condition, and no reference or variadic parameter,
    /// and the members differ only in closed parameter types, so the call
    /// ranks the same member whatever solves the struct's parameters. Those
    /// are solved from the receiver's `[...]` type arguments, or from the
    /// arguments' types, and an instance solves them at the substituted
    /// types, as its struct application is substituted. Where the instance's
    /// struct has a clone of the static the call retargets to it by the
    /// receiver's arguments: a lone declaration records nothing that names
    /// it, and an overloaded member's recorded target is re-keyed to the
    /// clone of that member (`realize_static_overloads`). An argument is a
    /// closed scalar or a whole value bound to a parameter of its own type
    /// ([`Self::static_argument`]).
    fn static_call(
        &self,
        expr: &Expr,
        object: &Expr,
        method: &str,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
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
        let plain_static = |sig: &super::MethodSig| {
            !sig.has_self
                && sig.decls.is_empty()
                && sig.availability.is_empty()
                && sig.variadic.is_none()
                && sig.kw_variadic.is_none()
                && sig.ref_return.is_none()
                && sig.ref_params.iter().all(Option::is_none)
                && sig.view_return.is_empty()
                && sig
                    .conventions
                    .iter()
                    .all(|convention| matches!(convention, None | Some(ArgConvention::Var)))
        };
        // Members that differ only in closed parameter types rank alike
        // under every instance: a parameter of the struct's parameter type
        // is the same one in each of them.
        let closed_differences = |signatures: &[super::MethodSig]| {
            let arity = signatures.iter().map(|sig| sig.params.len()).max();
            (0..arity.unwrap_or(0)).all(|position| {
                let declared: Vec<Option<&Ty>> = signatures
                    .iter()
                    .map(|sig| sig.params.get(position))
                    .collect();
                declared
                    .iter()
                    .flatten()
                    .all(|ty| !mojito_types::types::is_symbolic(ty))
                    || declared
                        .windows(2)
                        .all(|pair| pair[0].is_some() && pair[0] == pair[1])
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
                if generic {
                    signatures.iter().all(plain_static) && closed_differences(signatures)
                } else {
                    signatures.iter().all(|sig| !sig.has_self)
                }
            });
        let shadowed =
            self.local_kind(spelled).is_some() || self.params.contains(&spelled.as_str());
        let id = self.occurrence(expr);
        let admitted = !self.keyed
            && !shadowed
            && (static_member || (contextual && self.facts.is_none()))
            && kwargs.is_empty()
            && args.iter().all(|argument| {
                (self.expression(argument) && self.scalar(argument))
                    || ((generic || self.facts.is_none()) && self.static_argument(argument))
            })
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.expression_types, id)
                    .is_some_and(|ty| generic || !mojito_types::types::is_symbolic(ty))
                    && fact_at(&facts.call_parameters, id).is_none()
                    && fact_at(&facts.selected_calls, id).is_none()
                    && fact_at(&facts.generic_instantiations, id).is_none()
                    && fact_at(&facts.method_instantiations, id).is_none()
                    && fact_at(&facts.parameterized_method_calls, id).is_none()
                    && args.iter().all(|argument| {
                        fact_at(&facts.conversions, self.occurrence(argument)).is_none()
                    })
            });
        if admitted {
            self.static_calls
                .borrow_mut()
                .push((id, format!("{base}.{method}")));
        }
        admitted && self.holds(MethodFeatures::STATIC_CALLS)
    }

    /// A whole value passed to a static call ([`Self::static_call`]) that
    /// records no contract: moved, a temporary, copied where the template
    /// recorded the copy, or a named place read where it lies.
    ///
    /// The call records no conversion at it, so it binds a parameter of its
    /// own type under every instance, and what the call records for it is
    /// decided by its syntax and the callee's convention alone, as for a
    /// method's argument ([`Self::argument`]). A copy into a `var` parameter
    /// is owed again at the instance's type.
    fn static_argument(&self, argument: &Expr) -> bool {
        let named = match &argument.kind {
            ExprKind::Identifier(name) => {
                self.declared(name) || self.params.contains(&name.as_str())
            }
            _ => self.receiver_field(argument),
        };
        let admitted = !self.keyed
            && self.facts.is_none_or(|facts| {
                let id = self.occurrence(argument);
                let read_in_place = facts.borrowed_read_call_places.contains(&id) && named;
                !facts.call_place_uses.contains(&id)
                    && (read_in_place || self.whole_value(argument))
            });
        admitted && self.holds(MethodFeatures::VALUE_ARGUMENTS)
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
    fn consuming_call(
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
    fn argument(&self, call: &Expr, argument: &Expr) -> bool {
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
    /// An argument of a call of a nested `def`. A `mut` or `ref` parameter
    /// keeps a named place of exactly its recorded type. Any other takes a
    /// closed scalar, or a whole value of exactly its recorded type, read
    /// where it lies when it is a named place and a temporary otherwise, and
    /// a `var` one a transfer or a temporary as it stands. The call records
    /// its parameters and no contract, and its parameters take no
    /// conversion, so an instance substitutes both sides alike.
    fn nested_argument(&self, call: OccurrenceId, index: usize, argument: &Expr) -> bool {
        use mojito_ast::ast::ArgConvention;
        let named = match &argument.kind {
            ExprKind::Identifier(name) => {
                self.declared(name) || self.params.contains(&name.as_str())
            }
            _ => self.receiver_field(argument),
        };
        let scalar = self.expression(argument) && self.scalar(argument);
        let place = named || self.reference_argument(argument);
        let Some(facts) = self.facts else {
            return scalar || place || self.whole_value(argument);
        };
        let id = self.occurrence(argument);
        let Some(parameter) =
            fact_at(&facts.call_parameters, call).and_then(|params| params.get(index))
        else {
            return false;
        };
        let typed = fact_at(&facts.expression_types, id) == Some(&parameter.ty);
        if matches!(
            parameter.convention,
            Some(ArgConvention::Mut | ArgConvention::Ref)
        ) {
            let kept = named && typed && facts.call_place_uses.contains(&id);
            if kept {
                let mut places = self.places.borrow_mut();
                if !places.contains(&id) {
                    places.push(id);
                }
            }
            return kept && self.holds(MethodFeatures::PLACE_ARGUMENTS);
        }
        if facts.call_place_uses.contains(&id) {
            return false;
        }
        if scalar {
            return true;
        }
        let read_in_place = facts.borrowed_read_call_places.contains(&id) && place;
        matches!(
            parameter.convention,
            None | Some(ArgConvention::Imm | ArgConvention::Var)
        ) && typed
            && (read_in_place || self.whole_value(argument))
            && self.holds(MethodFeatures::VALUE_ARGUMENTS)
    }

    fn reference_argument(&self, argument: &Expr) -> bool {
        let admitted = match &argument.kind {
            ExprKind::Identifier(name) => self.reference_local(name),
            ExprKind::Member { .. } => self.reference_member(argument),
            _ => self.reference_call(argument),
        };
        admitted && self.holds(MethodFeatures::REFERENCE_ARGUMENTS)
    }

    /// Whether `expr` is a field of `self` in a method body: `self.<field>`,
    /// or a field of such a field holding a struct (`self.scaler.base`). A
    /// field has its declared type under its base's recorded arguments, so
    /// every instance reads the same path and only `self`'s binding changes.
    fn receiver_field(&self, expr: &Expr) -> bool {
        self.receiver
            && matches!(&expr.kind, ExprKind::Member { object, .. }
                if matches!(&object.kind, ExprKind::Identifier(name) if name == "self")
                    || (self.receiver_field(object) && self.nominal(object)))
    }

    /// Whether `expr` is `Self.<value>` in a method body, reading the
    /// struct's own scalar value binder: a runtime read of the reified
    /// parameter on the erased path, and in a struct specialized whole a
    /// literal every specialization folds under the name's identity, with no
    /// binding of its own either way.
    fn struct_value(&self, expr: &Expr) -> bool {
        self.receiver
            && matches!(&expr.kind, ExprKind::Member { object, field }
                if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self")
                    && self.struct_values.contains(&field.as_str()))
    }

    /// `Self.<v>` of a closed vector value binder (`Self.key`), which every
    /// specialization folds to its vector's construction under the name's
    /// identity (`construct_folded_vectors`): the template typed it as that
    /// closed vector and recorded nothing else there.
    fn struct_vector(&self, expr: &Expr) -> bool {
        matches!(&expr.kind, ExprKind::Member { object, field }
            if matches!(&object.kind, ExprKind::Identifier(name) if name == "Self")
                && self.struct_vectors.contains(&field.as_str()))
            && self.facts.is_none_or(|facts| {
                let id = self.occurrence(expr);
                fact_at(&facts.expression_bindings, id).is_none()
                    && fact_at(&facts.operation_adjustments, id).is_none()
                    && fact_at(&facts.expression_place_types, id).is_none()
            })
    }

    /// Whether `expr` is `self` itself in a method body.
    fn receiver_itself(&self, expr: &Expr) -> bool {
        self.receiver && matches!(&expr.kind, ExprKind::Identifier(name) if name == "self")
    }

    /// Whether the body may write `self`'s fields.
    const fn self_writable(&self) -> bool {
        use mojito_ast::ast::ArgConvention;
        matches!(
            self.self_convention,
            Some(
                ArgConvention::Mut
                    | ArgConvention::Var
                    | ArgConvention::Out
                    | ArgConvention::Deinit
            )
        )
    }

    /// Whether the body owns `self` whole and may transfer it out.
    const fn self_owned(&self) -> bool {
        matches!(
            self.self_convention,
            Some(mojito_ast::ast::ArgConvention::Var)
        )
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
    /// method call or `len` reads or writes in place. With no facts, a local
    /// taken as a scalar may hold a whole value too (`var m = self.meter`):
    /// its declaration's syntax alone cannot tell, and the check with facts
    /// judges its kind from the recorded types.
    fn value_local(&self, expr: &Expr) -> bool {
        let ExprKind::Identifier(name) = &expr.kind else {
            return false;
        };
        match self.local_kind(name) {
            Some(LocalKind::Value) => true,
            Some(LocalKind::Scalar) => self.facts.is_none(),
            _ => false,
        }
    }

    /// Whether `expr` is a field of a `var` local holding a whole value. The
    /// local is the body's own, so the body reads and writes the field in
    /// place as it does a field of a writable `self`, and the field has its
    /// declared type under the local's recorded arguments in a template and a
    /// clone alike.
    fn local_field(&self, expr: &Expr) -> bool {
        !self.keyed
            && matches!(&expr.kind, ExprKind::Member { object, .. } if self.value_local(object))
    }

    /// Whether `expr` names a parameter holding a struct, such as `value` of a
    /// `value: Box[Self.T]` parameter: the parameter is bound to the
    /// instance's argument and read where it lies, as `self` is, and its
    /// recorded type is the declared one under the instance's arguments.
    fn parameter_receiver(&self, expr: &Expr) -> bool {
        !self.keyed
            && matches!(&expr.kind, ExprKind::Identifier(name)
                if self.params.contains(&name.as_str())
                    && !self.callable_params.contains(&name.as_str())
                    && self.local_kind(name).is_none())
            && self.nominal(expr)
    }

    /// Whether `expr` is a field of a parameter holding a struct, such as
    /// `entry._hash` of a `var entry` parameter: the parameter is bound to
    /// the instance's argument, and the field has its declared type under
    /// the parameter's recorded arguments in a template and a clone alike.
    fn parameter_field(&self, expr: &Expr) -> bool {
        !self.keyed
            && matches!(&expr.kind, ExprKind::Member { object, .. }
                if self.parameter_receiver(object))
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
                Some(kind) => !matches!(kind, LocalKind::Value | LocalKind::Callable),
                None => {
                    self.params.contains(&name.as_str())
                        || self.folded_value(expr)
                        || self.module_constant(name)
                }
            },
            // A field of `self`, admitted where its recorded type is a closed
            // scalar: every use site of `expression` also demands `scalar`.
            ExprKind::Member { .. } => {
                self.pack_length(expr)
                    || (self.receiver_field(expr)
                        || self.local_field(expr)
                        || self.parameter_field(expr)
                        || self.reference_member(expr)
                        || self.struct_value(expr)
                        || self.struct_vector(expr)
                        || self.simd_intrinsic(expr))
                        && self.scalar(expr)
            }
            ExprKind::Index { .. } => {
                (self.tuple_element(expr) || self.simd_intrinsic(expr)) && self.scalar(expr)
            }
            ExprKind::MultiIndex { .. } => self.keyword_slice(expr),
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
                self.method_call(expr, object, method, args, kwargs)
                    || self.slice_indices(expr, object, method, args, kwargs)
                    || self.static_call(expr, object, method, args, kwargs)
                    || self.consuming_call(expr, object, method, args, kwargs)
                    || self.bound_dispatch(expr, object, args, kwargs)
                    || self.bound_builtin(expr, object, method, args, kwargs)
                    || self.simd_intrinsic(expr)
            }
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => {
                self.parameterized_call(expr, callee, param_args, args, kwargs)
                    || self.simd_intrinsic(expr)
            }
            ExprKind::Prefix(_, value) => {
                (self.folding(expr) || !self.folding(value))
                    && self.expression(value)
                    && self.scalar(value)
            }
            // A folded operand is a literal in every instance. Over literals
            // alone an operator folds too, to a literal the template's `Int`
            // or `Float64` materializes or to a `Bool` ([`folded_literals`]);
            // anything else keeps a runtime value on the operand's other
            // side.
            ExprKind::Infix(op, left, right) => {
                let runtime = |operand: &Expr| {
                    !self.folding(operand)
                        && self.facts.is_none_or(|facts| {
                            fact_at(&facts.expression_types, self.occurrence(operand)).is_some_and(
                                |ty| grammar_scalar(ty) || self.value_shaped_scalar(ty),
                            )
                        })
                };
                let folds = self.folding(expr)
                    || ((!self.folding(left) || runtime(right))
                        && (!self.folding(right) || runtime(left)));
                // A comparison over a value-shaped operand is a
                // `SIMD[DType.bool, 1]` mask while the lane is a vector, but
                // a `Bool` where it folds to a native scalar
                // (`Scalar[DType.int]` is `Int`).
                let lane_comparison = matches!(
                    op,
                    mojito_ast::ast::InfixOp::Lt
                        | mojito_ast::ast::InfixOp::Gt
                        | mojito_ast::ast::InfixOp::Le
                        | mojito_ast::ast::InfixOp::Ge
                        | mojito_ast::ast::InfixOp::Eq
                        | mojito_ast::ast::InfixOp::Ne
                ) && [left, right].iter().any(|operand| {
                    self.facts.is_some_and(|facts| {
                        fact_at(&facts.expression_types, self.occurrence(operand))
                            .is_some_and(|ty| self.value_shaped_scalar(ty))
                    })
                });
                (folds
                    && !lane_comparison
                    && self.expression(left)
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
                if self.callable_binders.contains(&name.as_str()) && self.local_kind(name).is_none()
                {
                    return self.callable_binder_call(id, param_args, args, kwargs, known);
                }
                if self.local_kind(name) == Some(LocalKind::Callable) {
                    // A keyword argument binds the recorded parameter of its
                    // name; with no facts yet its position decides nothing.
                    let keyword = |kwarg: &mojito_ast::ast::KwArg| {
                        self.facts
                            .map_or(Some(0), |facts| {
                                fact_at(&facts.call_parameters, id).and_then(|params| {
                                    params
                                        .iter()
                                        .position(|parameter| parameter.name == kwarg.name)
                                })
                            })
                            .is_some_and(|index| self.nested_argument(id, index, &kwarg.value))
                    };
                    return known
                        && param_args.is_empty()
                        && args
                            .iter()
                            .enumerate()
                            .all(|(index, argument)| self.nested_argument(id, index, argument))
                        && kwargs.iter().all(keyword)
                        && self.holds(MethodFeatures::NESTED_DEFS);
                }
                if name == "_unqualified_type_name" || name == "repr" {
                    return self.string_builtin(id, name, param_args, args, kwargs);
                }
                if self.scalar_conversion(id, name, param_args, args, kwargs)
                    || self.simd_construction(id, name, args, kwargs)
                    || self.foreign_call(id, name, param_args, args, kwargs)
                {
                    return true;
                }
                // The built-in `len` reads its operand in place and realizes
                // its witness per instance, so a method may hand it a field of
                // `self`, a `var` local, or a parameter holding a struct or its
                // field, of any type, not only a scalar one.
                let builtin_len = self
                    .facts
                    .is_none_or(|facts| facts.builtin_len_calls.contains(&id));
                // A generic callee applied to types only
                // (`unsafe_alloc[Self.T](n)`) records its application,
                // which the instance substitutes.
                let applied = param_args.is_empty()
                    || (param_args.iter().all(type_argument)
                        && !self.structs.contains_key(name)
                        && self.facts.is_none_or(|facts| {
                            fact_at(&facts.generic_instantiations, id).is_some()
                        }));
                known && applied && kwargs.is_empty() && args.iter().all(|argument| {
                    let held = matches!(&argument.kind, ExprKind::Identifier(name)
                            if self.reference_local(name));
                    let on_self = self.receiver
                        && matches!(&argument.kind, ExprKind::Identifier(name) if name == "self");
                    self.expression(argument)
                        || (builtin_len
                            && (held
                                || on_self
                                || self.receiver_field(argument)
                                || self.value_local(argument)
                                || self.parameter_receiver(argument)
                                || self.parameter_field(argument)))
                        || self.direct_call_value(argument)
                        || self.generic_call_place(id, argument)
                })
            }
            _ => false,
        }
    }

    /// A named place a method body hands to a generic module function's
    /// read parameter (`hash(e)`, `hash(self.value)`), which the call reads
    /// where it lies.
    ///
    /// The template bound the callee's binder to the place's symbolic type
    /// and selected the callee once; whether the call borrows the place is
    /// decided by its syntax and the read convention, so an instance keeps
    /// both and substitutes only the application (`method_direct_calls`).
    fn generic_call_place(&self, call: OccurrenceId, argument: &Expr) -> bool {
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
    fn direct_call_value(&self, argument: &Expr) -> bool {
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
    fn foreign_call(
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

    /// A built-in scalar conversion of one value of a closed type
    /// (`Int(key_hash)`, or `Bool(result)` of a closed struct place, which
    /// its conversion dunder reads in place), or of a value-shaped vector a
    /// keyed body holds (`Int(Scalar[dt](v))`). It selects no callee and
    /// records only its closed result type, which no instance changes; a
    /// declaration of that name would record a selection at the call, and
    /// such a call is not this.
    fn scalar_conversion(
        &self,
        id: OccurrenceId,
        name: &str,
        param_args: &[mojito_ast::ast::ParamArg],
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> bool {
        let [argument] = args else {
            return false;
        };
        let place = match &argument.kind {
            ExprKind::Identifier(name) => {
                self.params.contains(&name.as_str())
                    || self.declared(name)
                    || self.reference_local(name)
            }
            ExprKind::Member { .. } => self.receiver_field(argument),
            _ => false,
        };
        matches!(name, "Int" | "UInt" | "Bool" | "Float64")
            && param_args.is_empty()
            && kwargs.is_empty()
            && (self.expression(argument) || place)
            && (self.closed(argument) || self.value_shaped(argument))
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.expression_types, id).is_some_and(closed_scalar)
                    && fact_at(&facts.call_parameters, id).is_none()
                    && fact_at(&facts.overload_targets, id).is_none()
                    && fact_at(&facts.generic_instantiations, id).is_none()
                    && fact_at(&facts.selected_calls, id).is_none()
                    && fact_at(&facts.conversions, id).is_none()
            })
    }

    /// A construction of a closed `SIMD`, `Scalar`, or scalar-alias value
    /// from closed scalars (`UInt8(1)`, `SIMD[DType.uint8, 2](1, 2)`).
    ///
    /// Inference records a construction's dtype and width only when both
    /// are closed, so a recorded one is the same under every instance. It
    /// selects no callee and converts nothing, and its value is admitted
    /// only where a closed value may go ([`Self::simd_value`]).
    ///
    /// A construction whose dtype or width names one of the declaration's
    /// own value binders (`Scalar[dt](x)`, `SIMD[DType.int32, w](x)`)
    /// records no dimensions: the elaborator folds each binder in the
    /// instance, whose record is the substituted construction type's shape
    /// (`realize_value_shaped_constructions`).
    fn simd_construction(
        &self,
        id: OccurrenceId,
        name: &str,
        args: &[Expr],
        kwargs: &[mojito_ast::ast::KwArg],
    ) -> bool {
        let admitted = (name == "SIMD"
            || name == "Scalar"
            || mojito_ast::ast::Dtype::from_scalar_alias(name).is_some()
            || self.vector_aliases.contains(&name))
            && kwargs.is_empty()
            && args
                .iter()
                .all(|argument| self.expression(argument) && self.scalar(argument))
            && self.facts.is_none_or(|facts| {
                let recorded = fact_at(&facts.simd_constructions, id).is_some();
                fact_at(&facts.expression_types, id).is_some_and(|ty| {
                    (recorded && !mojito_types::types::is_symbolic(ty))
                        || (!recorded && (self.value_shaped_simd(ty) || self.struct_lane_simd(ty)))
                }) && fact_at(&facts.call_parameters, id).is_none()
                    && fact_at(&facts.overload_targets, id).is_none()
                    && fact_at(&facts.generic_instantiations, id).is_none()
                    && fact_at(&facts.selected_calls, id).is_none()
                    && fact_at(&facts.conversions, id).is_none()
                    && fact_at(&facts.operation_adjustments, id).is_none()
            });
        admitted && self.holds(MethodFeatures::SIMD_CONSTRUCTIONS)
    }

    /// A lane read on a vector the body holds: `v.to_bits[DType.<name>]()`
    /// or `v.cast[DType.<name>]()` (an `Invoke` with a `Member` callee and
    /// one `DType` argument), `v.to_bits()` or `v.reduce_*()` (a
    /// `MethodCall` without arguments),
    /// `v.length` (a `Member`), or `v[i]` (an `Index` over a scalar), on a
    /// receiver [`Self::lane_receiver`] admits: a parameter, a `var` local,
    /// a field of `self`, or another lane read, of a closed vector type or a
    /// lane-shaped one ([`Self::lane_shaped_simd`]).
    ///
    /// Each is a compiler-known operation on `Ty::Simd`: it selects no
    /// callee, converts nothing, and records at most its shape, the
    /// `SimdToBits`, `SimdCast`, or `SimdLength` adjustment, which inference
    /// writes only over a closed receiver and which is then the same under
    /// every instance. Over a lane-shaped receiver the template records no
    /// adjustment, and the instance records its own from the substituted
    /// types (`realize_simd_intrinsics`). A lane read and a reduction stand
    /// on a receiver whose dtype is closed, so their results are closed; a
    /// cast's or a reinterpretation's explicit target closes the result's
    /// dtype itself, a defaulted reinterpretation's is the unsigned dtype of
    /// the receiver's lane width, and a lane count is an `Int`.
    fn simd_intrinsic(&self, expr: &Expr) -> bool {
        use mojito_ast::ast::ParamArg;
        use mojito_checked::checked::SemanticAdjustment;
        let dtype_argument = |argument: &ParamArg| match argument {
            ParamArg::Type(_) => true,
            ParamArg::Value(value) => matches!(&value.kind, ExprKind::Member { object, .. }
                if matches!(&object.kind, ExprKind::Identifier(name) if name == "DType")),
            ParamArg::Named { .. } => false,
        };
        let reinterpretation = |method: &str| method == "to_bits";
        let cast = |method: &str| method == "cast";
        let reduction = |method: &str| {
            matches!(
                method,
                "reduce_add"
                    | "reduce_mul"
                    | "reduce_min"
                    | "reduce_max"
                    | "reduce_and"
                    | "reduce_or"
            )
        };
        let (receiver, closed_dtype) = match &expr.kind {
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => {
                let ExprKind::Member { object, field } = &callee.kind else {
                    return false;
                };
                if !(reinterpretation(field) || cast(field))
                    || !args.is_empty()
                    || !kwargs.is_empty()
                    || !matches!(param_args.as_slice(), [argument] if dtype_argument(argument))
                {
                    return false;
                }
                (object, false)
            }
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => {
                if !(reinterpretation(method) || reduction(method))
                    || !args.is_empty()
                    || !kwargs.is_empty()
                {
                    return false;
                }
                (object, reduction(method))
            }
            ExprKind::Member { object, field } if field == "length" => (object, false),
            ExprKind::Index { object, index } => {
                if !(self.expression(index) && self.scalar(index)) {
                    return false;
                }
                (object, true)
            }
            _ => return false,
        };
        if !self.lane_receiver(receiver, closed_dtype) {
            return false;
        }
        let id = self.occurrence(expr);
        let Some(facts) = self.facts else {
            return self.holds(MethodFeatures::SIMD_INTRINSICS);
        };
        let Some(ty) = fact_at(&facts.expression_types, id) else {
            return false;
        };
        let shaped = match &expr.kind {
            ExprKind::Member { .. } => *ty == Ty::Int,
            _ => {
                mojito_types::types::simd_slots(ty).is_some()
                    && (!mojito_types::types::is_symbolic(ty)
                        || self.lane_shaped_simd(ty)
                        || self.value_shaped_scalar(ty))
            }
        };
        let adjustment = fact_at(&facts.operation_adjustments, id);
        // A closed lane read into a place is copied out of the vector, as
        // it is under every instance.
        let closed_lane =
            matches!(expr.kind, ExprKind::Index { .. }) && !mojito_types::types::is_symbolic(ty);
        let source = fact_at(&facts.expression_types, self.occurrence(receiver));
        let open_source = source.is_some_and(mojito_types::types::is_symbolic);
        let adjusted = match (adjustment, &expr.kind) {
            (None, _) => true,
            (
                Some(SemanticAdjustment::SimdToBits { .. }),
                ExprKind::Invoke { .. } | ExprKind::MethodCall { .. },
            ) => !mojito_types::types::is_symbolic(ty),
            // A recorded cast stands under every instance only when its
            // source lane is closed too, since a `bool` source refuses; over
            // a value-shaped source the instance checks its own lane.
            (Some(SemanticAdjustment::SimdCast { .. }), ExprKind::Invoke { .. }) => {
                !mojito_types::types::is_symbolic(ty)
                    && source.is_some_and(|source| {
                        !mojito_types::types::is_symbolic(source)
                            || self.value_shaped_scalar(source)
                    })
            }
            (Some(SemanticAdjustment::SimdLength { .. }), ExprKind::Member { .. }) => true,
            _ => false,
        };
        let admitted = shaped
            && adjusted
            && fact_at(&facts.call_parameters, id).is_none()
            && fact_at(&facts.selected_calls, id).is_none()
            && fact_at(&facts.overload_targets, id).is_none()
            && fact_at(&facts.generic_instantiations, id).is_none()
            && fact_at(&facts.conversions, id).is_none()
            && fact_at(&facts.parameterized_method_calls, id).is_none()
            && fact_at(&facts.method_instantiations, id).is_none()
            && fact_at(&facts.subscript_descriptors, id).is_none()
            && (!facts.copy_place_value_uses.contains(&id) || closed_lane);
        // A reinterpretation or cast the template recorded over an open
        // source lane is noted too: its closed shape stands, but the
        // instance checks its own source lane against it.
        if admitted && (adjustment.is_none() || open_source) {
            let read = (id, self.occurrence(receiver));
            match &expr.kind {
                ExprKind::Invoke { callee, .. } => match &callee.kind {
                    ExprKind::Member { field, .. } if cast(field) => {
                        push_unique(&mut self.simd_casts.borrow_mut(), read);
                    }
                    _ => push_unique(&mut self.simd_to_bits.borrow_mut(), read),
                },
                ExprKind::MethodCall { method, .. } if reinterpretation(method) => {
                    push_unique(&mut self.simd_to_bits.borrow_mut(), read);
                }
                ExprKind::Member { .. } => push_unique(&mut self.simd_lengths.borrow_mut(), read),
                _ => {}
            }
        }
        admitted && self.holds(MethodFeatures::SIMD_INTRINSICS)
    }

    /// The receiver of a lane read: a parameter, a `var` local, a field of
    /// `self`, or another lane read, whose recorded type is a closed vector
    /// or a lane-shaped one. With `closed_dtype`, the dtype slot must be
    /// closed, so a lane or a reduction of it is a closed scalar.
    fn lane_receiver(&self, expr: &Expr, closed_dtype: bool) -> bool {
        let named = match &expr.kind {
            ExprKind::Identifier(name) => {
                (self.params.contains(&name.as_str())
                    && !self.callable_params.contains(&name.as_str())
                    && self.local_kind(name).is_none())
                    || self.declared(name)
            }
            ExprKind::Member { .. } => self.receiver_field(expr),
            _ => false,
        };
        let held = named || self.simd_intrinsic(expr);
        held && self.facts.is_none_or(|facts| {
            fact_at(&facts.expression_types, self.occurrence(expr)).is_some_and(|ty| {
                let Some((dtype, _)) = mojito_types::types::simd_slots(ty) else {
                    return false;
                };
                let symbolic = mojito_types::types::is_symbolic(ty);
                // A native scalar (`Int`, `UInt`, `Float64`) has no lane
                // reads: a lane-shaped receiver an instance may close to
                // one keeps its dtype open, and a closed one with such a
                // dtype closes to a vector only at a width above one.
                let vector = match dtype {
                    mojito_types::types::SimdDtype::Known(dtype) => {
                        !symbolic
                            || matches!(
                                mojito_types::types::canonical_simd_ty(dtype, 1),
                                Ty::Simd { .. }
                            )
                    }
                    mojito_types::types::SimdDtype::Expr(_) => false,
                };
                (!symbolic || self.lane_shaped_simd(ty) || self.value_shaped_scalar(ty))
                    && (!closed_dtype || vector)
            })
        })
    }

    /// Whether `expr` is a lane read whose value a `var` local may hold as
    /// a scalar: its recorded type is a closed vector or a lane-shaped one
    /// whose dtype is closed (`var bits = value.to_bits[DType.uint64]()`),
    /// so the local's own lane reads are closed scalars.
    fn lane_local_value(&self, expr: &Expr) -> bool {
        self.simd_intrinsic(expr)
            && self.facts.is_none_or(|facts| {
                fact_at(&facts.expression_types, self.occurrence(expr)).is_some_and(|ty| {
                    grammar_scalar(ty)
                        || (self.lane_shaped_simd(ty)
                            && matches!(
                                ty,
                                Ty::Simd {
                                    dtype: mojito_types::types::SimdDtype::Known(_),
                                    ..
                                }
                            ))
                })
            })
    }

    /// A `SIMD` type whose open slots name only the hidden dtype and width
    /// binders of the method's wildcard vector binders, which every clone
    /// folds from its baked argument.
    fn lane_shaped_simd(&self, ty: &Ty) -> bool {
        let mut named = HashSet::new();
        mojito_types::types::referenced_parameters(ty, &mut named);
        matches!(ty, Ty::Simd { .. })
            && !named.is_empty()
            && named.iter().all(|name| self.lane_binders.contains(name))
    }

    /// A `SIMD` type whose open slots name only the declaration's own
    /// value binders, which every instance folds to literals.
    fn value_shaped_simd(&self, ty: &Ty) -> bool {
        let mut named = HashSet::new();
        mojito_types::types::referenced_parameters(ty, &mut named);
        matches!(ty, Ty::Simd { .. })
            && !named.is_empty()
            && named
                .iter()
                .all(|name| self.values.contains(&name.as_str()))
    }

    /// Whether `expr` is an admitted closed `SIMD` construction, a value of
    /// a closed type that is not one of the grammar's scalars.
    fn simd_value(&self, expr: &Expr) -> bool {
        matches!(&expr.kind, ExprKind::Call { name, args, kwargs, .. }
            if self.simd_construction(self.occurrence(expr), name, args, kwargs))
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
    fn callable_binder_call(
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

    /// `print(...)` as a statement: a checker builtin that selects no
    /// callee, over closed scalars, pack elements, and string literals.
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
                (self.expression(argument) && self.scalar(argument))
                    || self.pack_element(argument)
                    || (matches!(argument.kind, ExprKind::Str(_))
                        && self.facts.is_none_or(|facts| {
                            fact_at(&facts.expression_types, self.occurrence(argument))
                                == Some(&Ty::StringLiteral)
                        }))
            });
        if admitted {
            let mut calls = self.print_calls.borrow_mut();
            if !calls.contains(&id) {
                calls.push(id);
            }
        }
        admitted
    }

    /// Whether `expr` names a compile-time value the elaborator folds to a
    /// literal in every instance: a `comptime for` variable, or a value
    /// parameter, that no local shadows.
    fn folds(&self, expr: &Expr) -> bool {
        matches!(&expr.kind, ExprKind::Identifier(name)
            if self.local_kind(name).is_none()
                && !self.params.contains(&name.as_str())
                && (self.values.contains(&name.as_str())
                    || self.loop_vars.borrow().iter().any(|var| var == name)))
    }

    /// Whether every instance folds `expr` to a literal: a folded value, or
    /// arithmetic, a comparison, a negation, or an inversion over folded
    /// values and literals that names at least one folded value.
    fn folding(&self, expr: &Expr) -> bool {
        fn literal_tree(shape: &BodyShape<'_>, expr: &Expr) -> Option<bool> {
            use mojito_ast::ast::InfixOp::{
                Add, BitAnd, BitOr, BitXor, Div, Eq, FloorDiv, Ge, Gt, Le, Lt, Mod, Mul, Ne, Pow,
                Shl, Shr, Sub,
            };
            match &expr.kind {
                ExprKind::Int(_) => Some(false),
                ExprKind::Identifier(name) if shape.module_constant(name) => Some(false),
                ExprKind::Identifier(_) => shape.folds(expr).then_some(true),
                ExprKind::Prefix(
                    mojito_ast::ast::PrefixOp::Neg | mojito_ast::ast::PrefixOp::Invert,
                    value,
                ) => literal_tree(shape, value),
                ExprKind::Infix(
                    Add | Sub | Mul | Div | FloorDiv | Mod | Pow | Shl | Shr | BitAnd | BitOr
                    | BitXor | Eq | Ne | Lt | Le | Gt | Ge,
                    left,
                    right,
                ) => Some(literal_tree(shape, left)? | literal_tree(shape, right)?),
                _ => None,
            }
        }
        literal_tree(self, expr) == Some(true)
    }

    /// A folded compile-time value read where it stands, as an `Int` or a
    /// `Bool`: the instance's literal materializes to exactly that type
    /// ([`folded_literals`]).
    /// `Self.Ts.length` over the struct's own type pack, which the
    /// elaborator folds to the instance's element count
    /// (`folded_literals`): the template typed it `Int` and recorded nothing
    /// an instance keeps.
    fn pack_length(&self, expr: &Expr) -> bool {
        let ExprKind::Member { object, field } = &expr.kind else {
            return false;
        };
        let pack = matches!(&object.kind, ExprKind::Member { object, field: pack }
            if matches!(&object.kind, ExprKind::Identifier(base) if base == "Self")
                && self.pack_struct == Some(pack.as_str()));
        field == "length"
            && pack
            && self.facts.is_none_or(|facts| {
                let id = self.occurrence(expr);
                fact_at(&facts.expression_types, id) == Some(&Ty::Int)
                    && fact_at(&facts.operation_adjustments, id).is_none()
            })
            && self.holds(MethodFeatures::STATEMENTS)
            && self.holds(MethodFeatures::COMPTIME_CONTROL)
    }

    /// A module's exact integer constant, read where no local or parameter
    /// shadows it: its binding and its `IntLiteral` type are the same under
    /// every instance, as a literal's are.
    fn module_constant(&self, name: &str) -> bool {
        self.local_kind(name).is_none()
            && !self.params.contains(&name)
            && self.constants.contains_key(name)
    }

    fn folded_value(&self, expr: &Expr) -> bool {
        self.folds(expr)
            && self.facts.is_none_or(|facts| {
                let id = self.occurrence(expr);
                matches!(
                    fact_at(&facts.expression_types, id),
                    Some(Ty::Int | Ty::Bool)
                ) && fact_at(&facts.expression_bindings, id).is_some()
                    && fact_at(&facts.operation_adjustments, id).is_none()
            })
    }

    /// `pack[i]`: an element of a pack-typed parameter at the innermost
    /// `comptime for` variable, or of a pack struct's storage field read on
    /// `self` or on a parameter of the struct (`other.storage[i]`).
    ///
    /// The template typed the element once, as the dependent `Ts[i]` over
    /// the loop's own binder, and recorded nothing else there: no place, no
    /// adjustment, no borrow. The elaborator folds `i` to the iteration's
    /// literal in each unrolled copy, which the instance reads back to fix
    /// the element ([`Checker::realize_instance_facts`]).
    fn pack_element(&self, expr: &Expr) -> bool {
        self.pack_element_read(expr, false)
    }

    /// [`Self::pack_element`] handed to a checker builtin that reads it
    /// where it lies (`writer.write(self.storage[i])`): the builtin's borrow
    /// of the element is decided by its syntax, as a named place's is.
    fn lent_pack_element(&self, expr: &Expr) -> bool {
        self.pack_element_read(expr, true)
    }

    fn pack_element_read(&self, expr: &Expr, lent: bool) -> bool {
        let ExprKind::Index { object, index } = &expr.kind else {
            return false;
        };
        let collection = match &object.kind {
            ExprKind::Identifier(name) => self.packs.contains(&name.as_str()),
            ExprKind::Member { .. } => {
                self.pack_struct.is_some()
                    && (self.receiver_field(object) || self.parameter_field(object))
            }
            _ => false,
        };
        // The innermost loop variable, or a method's own index binder
        // (`__getitem_param__[index: Int]`), which every instance folds.
        let named = collection
            && matches!(&index.kind, ExprKind::Identifier(name)
                if self.loop_vars.borrow().last() == Some(name)
                    || (self.loop_vars.borrow().is_empty()
                        && self.local_kind(name).is_none()
                        && self.values.contains(&name.as_str())));
        named
            && self.facts.is_none_or(|facts| {
                let id = self.occurrence(expr);
                let ty = fact_at(&facts.expression_types, id);
                let dependent_element = |ty: &Ty| {
                    matches!(ty, Ty::Dependent(dependent)
                    if dependent.pack_element().is_some_and(|(_, index)| {
                        matches!(index.kind(), mojito_types::param_expr::ParamKind::DeclRef(_))
                    }))
                };
                // An erased `rebind[T](self.storage[i])` retypes the element
                // to its target, and asserts the two equal
                // (`TemplateObligation::RebindEqualities`).
                let element = ty.is_some_and(dependent_element)
                    || fact_at(&facts.rebind_assertions, id)
                        .is_some_and(|assertion| dependent_element(&assertion.operand));
                // The element is a place of the collector, read where it
                // lies. A lent one is kept by the builtin that reads it
                // (`repr`), which admits the place it keeps
                // ([`Self::references_recorded`]).
                element
                    && fact_at(&facts.expression_place_types, id)
                        .is_none_or(|place| Some(place) == ty)
                    && fact_at(&facts.operation_adjustments, id).is_none()
                    && (lent
                        || (!facts.call_place_uses.contains(&id)
                            && !facts.borrowed_read_call_places.contains(&id)))
            })
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
                        Some(
                            mojito_ast::ast::ArgConvention::Var
                            | mojito_ast::ast::ArgConvention::Deinit,
                        ) => consumed && call.invalidations.is_empty(),
                        Some(_) => false,
                    }
                    && (call.contract.result_ty == *receiver
                        || !mojito_types::types::is_symbolic(&call.contract.result_ty))
                    && call.contract.arguments.len() == args.len()
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
    fn bound_builtin(
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
                self.receiver_field(argument) || self.reference_member(argument)
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

    /// An operator over two operands of one type that mentions a struct
    /// parameter, which dispatches on the type alone, or over such an operand
    /// and a literal the dunder of a struct built over the parameter accepts.
    ///
    /// An operand is a place, a call result, or another admitted operator.
    /// The template, whose type is symbolic, recorded nothing at the
    /// operator: a bound (or a `where` assumption) proves it, or the dunder
    /// the template dispatched on a struct built over the parameter answers,
    /// with a literal operand converted into its parameter type where it
    /// must be. An instance decides the same operator on its substituted
    /// types ([`Checker::realize_operator`]). Neither check records anything
    /// at a temporary operand: a place is read where it lies, and a call
    /// result or an operator's value is moved into the dunder, or dropped
    /// after it.
    ///
    /// A closed left operand beside such a struct — a literal, or a closed
    /// scalar with no dunder for the pair — dispatches the struct's
    /// reflected dunder (`1 + self.bag` → `self.bag.__radd__(1)`), whose
    /// target and `ReflectedOperator` adjustment the template recorded at
    /// the operator; an instance names the target again at its own types.
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
        let operand = |operand: &Expr| match &operand.kind {
            ExprKind::Identifier(name) => {
                self.params.contains(&name.as_str()) || self.local_kind(name).is_some()
            }
            ExprKind::Member { .. } => {
                self.receiver_field(operand) || self.reference_member(operand)
            }
            ExprKind::Index { .. } => self.slot(operand) || self.pack_element(operand),
            ExprKind::MethodCall { .. } | ExprKind::Invoke { .. } => self.call_result(operand),
            ExprKind::Infix(..) => self.operator_value(operand),
            _ => false,
        };
        let literal = |operand: &Expr| {
            matches!(
                operand.kind,
                ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::Str(_) | ExprKind::Bool(_)
            )
        };
        let right_literal = literal(right);
        let id = self.occurrence(expr);
        let admitted = !self.keyed
            && operator_dispatch(op)
            && (literal(left) || operand(left))
            && (right_literal || operand(right))
            && !(literal(left) && right_literal)
            && self.facts.is_none_or(|facts| {
                let targeted = facts.overload_targets.iter().any(|(site, _)| *site == id);
                let adjustments: Vec<_> = facts
                    .operation_adjustments
                    .iter()
                    .filter(|(site, _)| *site == id)
                    .map(|(_, adjustment)| adjustment)
                    .collect();
                let reflected = matches!(
                    adjustments.as_slice(),
                    [mojito_checked::checked::SemanticAdjustment::ReflectedOperator]
                );
                let typed = fact_at(&facts.expression_types, self.occurrence(left))
                    .zip(fact_at(&facts.expression_types, self.occurrence(right)))
                    .is_some_and(|(left, right)| {
                        if reflected {
                            !mojito_types::types::is_symbolic(left)
                                && mojito_types::types::is_symbolic(right)
                                && matches!(right, Ty::Struct(..))
                        } else {
                            mojito_types::types::is_symbolic(left)
                                && (left == right
                                    || (right_literal && matches!(left, Ty::Struct(..))))
                        }
                    });
                typed && (targeted == reflected) && (reflected || adjustments.is_empty())
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

    fn occurrence_of(&self, statement: &Stmt) -> OccurrenceId {
        OccurrenceId {
            syntax: self.origins.origin(statement.syntax_id),
            copy: 0,
        }
    }

    /// Whether the recorded type of `expr` mentions no parameter.
    fn closed(&self, expr: &Expr) -> bool {
        self.closed_value(self.occurrence(expr))
    }

    /// Whether the type recorded at `id` mentions no parameter.
    fn closed_value(&self, id: OccurrenceId) -> bool {
        self.facts.is_none_or(|facts| {
            facts
                .expression_types
                .iter()
                .any(|(site, ty)| *site == id && !mojito_types::types::is_symbolic(ty))
        })
    }

    /// Whether the recorded type of `expr` is a struct, whose fields a body
    /// may read. With no facts yet, the syntax alone never rules it out.
    fn nominal(&self, expr: &Expr) -> bool {
        let id = self.occurrence(expr);
        self.facts.is_none_or(|facts| {
            facts
                .expression_types
                .iter()
                .any(|(site, ty)| *site == id && matches!(ty, Ty::Struct(..)))
        })
    }

    /// Whether the recorded type of `expr` is a closed scalar or a
    /// value-shaped vector ([`Self::value_shaped_scalar`]).
    /// With no facts yet, the syntax alone never rules a type out.
    fn scalar(&self, expr: &Expr) -> bool {
        let id = self.occurrence(expr);
        self.facts.is_none_or(|facts| {
            facts.expression_types.iter().any(|(site, ty)| {
                *site == id && (grammar_scalar_or_literal(ty) || self.value_shaped_scalar(ty))
            })
        })
    }

    /// A value-shaped vector a body holds as a scalar (`SIMD[DType.int64,
    /// w]`, over a keyed `def`'s or a method's own binders, or a struct
    /// lane): every instance folds the binders its slots
    /// name, and an operator, a reduction, or a lane read over it records
    /// nothing its dimensions decide, so the instance's facts are the
    /// template's under the folded dimensions.
    fn value_shaped_scalar(&self, ty: &Ty) -> bool {
        self.value_shaped_simd(ty) || self.struct_lane_simd(ty)
    }

    /// A `SIMD` type whose open slots name only the struct's lane binders,
    /// which every specialization of a struct specialized whole folds, and
    /// the declaration's own value binders, folded as a keyed body's are
    /// (`SIMD[dt, Self.n]` in `rep[dt: DType]` of `Width[n: Int]`).
    fn struct_lane_simd(&self, ty: &Ty) -> bool {
        let mut named = HashSet::new();
        mojito_types::types::referenced_parameters(ty, &mut named);
        matches!(ty, Ty::Simd { .. })
            && named
                .iter()
                .any(|name| self.struct_lanes.contains(&name.as_str()))
            && named.iter().all(|name| {
                self.struct_lanes.contains(&name.as_str()) || self.values.contains(&name.as_str())
            })
    }

    /// Whether the recorded type of `expr` is a value-shaped vector the
    /// body holds ([`Self::value_shaped_scalar`]).
    fn value_shaped(&self, expr: &Expr) -> bool {
        let id = self.occurrence(expr);
        self.facts.is_none_or(|facts| {
            fact_at(&facts.expression_types, id).is_some_and(|ty| self.value_shaped_scalar(ty))
        })
    }
}

/// The literals an instance holds where its template read a compile-time
/// value by name, each with the facts its own check records there; `None`
/// when a value does not fit the type the template recorded for the name.
///
/// A literal whose syntax the template recorded a binding at is the fold of
/// that name: nothing else keeps a name's identity on a literal. An `Int` of
/// the instance fits the template's `Int` exactly and a `Bool` its `Bool`, so
/// the literal materializes to the type the template's facts around it were
/// judged at. A pack element's index is consumed by the fold instead. Where
/// the template lent the name to a read parameter, or printed it, the
/// literal is a temporary the call reads.
fn folded_literals(
    template: &CheckedBodyFacts,
    occurrences: &[Occurrence],
) -> Option<Vec<FoldedLiteral>> {
    use mojito_types::ct::CtValue;
    let indices: HashSet<SyntaxId> = occurrences
        .iter()
        .filter_map(|occurrence| occurrence.folded_index.map(|(index, _)| index))
        .collect();
    let printed: HashSet<SyntaxId> = occurrences
        .iter()
        .filter(|occurrence| {
            occurrence.callee.as_deref() == Some("print")
                && template
                    .print_calls
                    .iter()
                    .any(|id| id.syntax == occurrence.id.syntax)
        })
        .flat_map(|occurrence| occurrence.arguments.iter().copied())
        .collect();
    // A vector construction takes a literal lane as it stands.
    let lanes: HashSet<SyntaxId> = occurrences
        .iter()
        .filter(|occurrence| matches!(occurrence.callee.as_deref(), Some("SIMD" | "Scalar")))
        .flat_map(|occurrence| occurrence.arguments.iter().copied())
        .collect();
    // A fold keeps the identity of the name, or of the use (`Self.Ts.length`)
    // the template typed as the runtime `Int` it stands for.
    let named = |occurrence: &Occurrence| {
        let at = |id: &OccurrenceId| id.syntax == occurrence.id.syntax;
        template.expression_bindings.iter().any(|(id, _)| at(id))
            || (template
                .expression_types
                .iter()
                .any(|(id, ty)| at(id) && *ty == Ty::Int)
                && !template.operation_adjustments.iter().any(|(id, _)| at(id)))
    };
    let arithmetic = folded_arithmetic(template, occurrences, &named)?;
    let mut literals: Vec<FoldedLiteral> = occurrences
        .iter()
        .filter(|occurrence| {
            occurrence.literal.is_some()
                && named(occurrence)
                && !arithmetic
                    .iter()
                    .any(|literal| literal.occurrence == occurrence.id)
        })
        .map(|occurrence| {
            let recorded = template
                .expression_types
                .iter()
                .find(|(id, _)| id.syntax == occurrence.id.syntax)
                .map(|(_, ty)| ty);
            let (ty, materialized) = match (&occurrence.literal, recorded) {
                (Some(CtValue::Int(_)), _) if indices.contains(&occurrence.id.syntax) => {
                    (Ty::IntLiteral, None)
                }
                (Some(CtValue::Int(_)), Some(Ty::Int)) if lanes.contains(&occurrence.id.syntax) => {
                    (Ty::IntLiteral, None)
                }
                (Some(CtValue::Int(_)), Some(Ty::Int)) => (Ty::IntLiteral, Some(Ty::Int)),
                // A module's integer constant is an `IntLiteral` already.
                (Some(CtValue::Int(_)), Some(Ty::IntLiteral)) => (Ty::IntLiteral, None),
                (Some(CtValue::Bool(_)), Some(Ty::Bool)) => (Ty::Bool, None),
                _ => return None,
            };
            let read_temporary = template
                .borrowed_read_call_places
                .iter()
                .any(|id| id.syntax == occurrence.id.syntax);
            Some(FoldedLiteral {
                occurrence: occurrence.id,
                ty,
                materialized,
                read_temporary,
                unconsumed_temporary: read_temporary || printed.contains(&occurrence.id.syntax),
            })
        })
        .collect::<Option<_>>()?;
    literals.extend(arithmetic);
    // A folded vector value's lanes are literals of its own.
    literals.extend(occurrences.iter().filter_map(|occurrence| {
        Some(FoldedLiteral {
            occurrence: occurrence.id,
            ty: match &occurrence.vector_fold {
                Some(VectorFold::Lane(kind)) => kind.ty(),
                _ => return None,
            },
            materialized: None,
            read_temporary: false,
            unconsumed_temporary: false,
        })
    }));
    Some(literals)
}

/// `indices` with each `^` transfer of a pack element fixed at its
/// element's index: the transfer is typed as the element it moves, and in
/// pre-order the element is the occurrence right after it.
fn transferred_element_indices(
    mut indices: ElementIndices,
    occurrences: &[Occurrence],
) -> ElementIndices {
    let transfers: Vec<_> = occurrences
        .windows(2)
        .filter_map(|pair| {
            let [transfer, element] = pair else {
                return None;
            };
            let index = indices.get(&element.id)?;
            transfer.transfer.then(|| (transfer.id, index.clone()))
        })
        .collect();
    indices.extend(transfers);
    indices
}

/// Tell the syntax the elaborator wrote for a vector apart from the
/// template's: the dimensions it spelled for a vector alias's construction
/// (`U256(…)` as `SIMD[DType.uint64, 4](…)`) are part of the type, which no
/// check records anything at; and a struct's vector value binder it folded
/// (`Self.key`) is a construction under the name's identity whose lanes are
/// all its own.
fn fold_vector_values(occurrences: &mut [Occurrence], template: &[OccurrenceId]) {
    let checked = |syntax: &SyntaxId| template.iter().any(|id| id.syntax == *syntax);
    let written: HashSet<SyntaxId> = occurrences
        .iter()
        .flat_map(|occurrence| occurrence.dimensions.iter().copied())
        .filter(|dimension| !checked(dimension))
        .collect();
    let mut lanes: HashSet<SyntaxId> = HashSet::new();
    for occurrence in occurrences.iter_mut() {
        if occurrence.callee.as_deref() == Some("SIMD")
            && checked(&occurrence.id.syntax)
            && !occurrence.arguments.is_empty()
            && !occurrence.arguments.iter().any(checked)
        {
            occurrence.vector_fold = Some(VectorFold::Construction);
            lanes.extend(occurrence.arguments.iter().copied());
        }
    }
    for occurrence in occurrences.iter_mut() {
        if written.contains(&occurrence.id.syntax) {
            occurrence.vector_fold = Some(VectorFold::Dimension);
        } else if lanes.contains(&occurrence.id.syntax) {
            occurrence.vector_fold = occurrence.literal_kind.map(VectorFold::Lane);
        }
    }
}

/// Record the construction each folded vector value is in the instance: the
/// template typed the name as the closed vector the fold spells, and the
/// instance's check records its dimensions there. `false` when the template
/// typed it otherwise.
fn construct_folded_vectors(facts: &mut CheckedBodyFacts, occurrences: &[Occurrence]) -> bool {
    use mojito_types::types::{SimdDtype, SimdWidth};
    for occurrence in occurrences
        .iter()
        .filter(|occurrence| matches!(occurrence.vector_fold, Some(VectorFold::Construction)))
    {
        let Some(Ty::Simd {
            dtype: SimdDtype::Known(dtype),
            width: SimdWidth::Known(width),
        }) = fact_at(&facts.expression_types, occurrence.id)
        else {
            return false;
        };
        let dimensions = (*dtype, *width);
        upsert(&mut facts.simd_constructions, occurrence.id, dimensions);
    }
    let order = |id: &OccurrenceId| occurrences.iter().position(|found| found.id == *id);
    facts.simd_constructions.sort_by_key(|(id, _)| order(id));
    true
}

/// The occurrence identities of the values a body's `return`s hand out.
fn returned_values(body: &[Stmt]) -> Vec<SyntaxId> {
    struct Returns(Vec<SyntaxId>);

    impl mojito_ast::visit::Visitor for Returns {
        fn visit_stmt(&mut self, statement: &Stmt) {
            if let StmtKind::Return(Some(value)) = &statement.kind {
                self.0.push(value.syntax_id);
            }
        }
    }
    let mut returns = Returns(Vec::new());
    mojito_ast::visit::walk_block(&mut returns, body);
    returns.0
}

/// Read each returned place the template handed out as a reference by
/// value instead: the place is copied where it lies, which the instance
/// owes at its own type (`realize_instance_facts`' copy check).
fn read_returns_by_value(facts: &mut CheckedBodyFacts, returned: &[SyntaxId]) {
    let (copied, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut facts.reference_value_uses)
        .into_iter()
        .partition(|(id, through)| !through && returned.contains(&id.syntax));
    facts.reference_value_uses = kept;
    for (id, _) in copied {
        push_unique(&mut facts.copy_place_value_uses, id);
    }
}

/// The mangling values of a variadic struct applied to a closed pack,
/// spelled as the elaborator spells them. The template itself is not
/// registered in the clone check, so only a specialization minted under that
/// name says it was one.
fn closed_pack_values(
    arguments: &[mojito_types::types::TyArg],
) -> Option<Vec<mojito_types::ct::CtValue>> {
    let elements = arguments
        .iter()
        .map(|argument| match argument {
            mojito_types::types::TyArg::Ty(ty) if !mojito_types::types::is_symbolic(ty) => {
                Some(ty.clone())
            }
            _ => None,
        })
        .collect::<Option<Vec<_>>>()
        .filter(|elements| !elements.is_empty())?;
    Some(mojito_symbol::symbol::tuple_specialization_values(
        &elements,
    ))
}

/// Lay each [`PackRelocation`](mojito_checked::templates::PackRelocation)
/// over the instance's occurrences: the collector is read where it lies, as
/// the moved place the call's value is, bound to its parameter. `false` when
/// the instance holds no such read.
fn relocate_packs(facts: &mut CheckedBodyFacts, order: &[OccurrenceId]) -> bool {
    fn insert<V>(
        table: &mut Vec<(OccurrenceId, V)>,
        order: &[OccurrenceId],
        at: OccurrenceId,
        value: V,
    ) {
        let rank = |id: &OccurrenceId| order.iter().position(|entry| entry == id);
        let position = table
            .iter()
            .position(|(id, _)| rank(id) > rank(&at))
            .unwrap_or(table.len());
        table.insert(position, (at, value));
    }
    for relocation in std::mem::take(&mut facts.pack_relocations) {
        let pack = relocation.pack;
        let Some(ty) = fact_at(&facts.expression_types, relocation.call).cloned() else {
            return false;
        };
        if !order.contains(&pack) || fact_at(&facts.expression_types, pack).is_some() {
            return false;
        }
        let rank = |id: &OccurrenceId| order.iter().position(|entry| entry == id);
        let position = facts
            .transfers
            .iter()
            .position(|id| rank(id) > rank(&relocation.call))
            .unwrap_or(facts.transfers.len());
        facts.transfers.insert(position, relocation.call);
        insert(&mut facts.expression_types, order, pack, ty.clone());
        insert(&mut facts.expression_place_types, order, pack, ty);
        insert(
            &mut facts.expression_bindings,
            order,
            pack,
            mojito_checked::templates::TemplateOwner::Param(relocation.param),
        );
    }
    true
}

/// The operators an instance folds where its template computed a value from
/// folded values: `i * 10 + j` over a `comptime for` variable is an
/// `IntLiteral` in every instance, `i / 2` a `FloatLiteral`, and `i < 3` a
/// `Bool`. `None` when the fold leaves the literals, fails, or its value
/// does not take the template's type.
///
/// The outermost folded arithmetic takes the literal type and materializes
/// to the template's `Int` (`Float64` for a division), where the template
/// recorded nothing but that type and the temporary a read or `print`
/// argument makes of it; an `Int` past the machine range wraps there, as
/// the clone check's materialization does. The outermost folded comparison
/// is a `Bool` in both checks and keeps its own facts. Every operand below
/// either is a literal typed as one and nothing else: a named value loses
/// its binding and a literal its own materialization.
fn folded_arithmetic(
    template: &CheckedBodyFacts,
    occurrences: &[Occurrence],
    named: &impl Fn(&Occurrence) -> bool,
) -> Option<Vec<FoldedLiteral>> {
    use mojito_ast::ast::PrefixOp;
    use mojito_common::literal::IntLiteral;
    use mojito_types::ct::CtValue;
    use mojito_types::param_expr::fold::{fold_infix, fold_invert, fold_neg};
    fn names<'a>(sites: impl IntoIterator<Item = &'a OccurrenceId>, id: OccurrenceId) -> bool {
        sites.into_iter().any(|site| site.syntax == id.syntax)
    }
    let recorded = |id: OccurrenceId| {
        template
            .expression_types
            .iter()
            .find(|(site, _)| site.syntax == id.syntax)
            .map(|(_, ty)| ty)
    };
    let operand = |id: OccurrenceId, syntax| OccurrenceId {
        syntax,
        copy: id.copy,
    };
    // Each occurrence's literal value, and whether it names a folded value;
    // an operand follows its operator in pre-order.
    let mut values: HashMap<OccurrenceId, (CtValue, bool)> = HashMap::new();
    for occurrence in occurrences.iter().rev() {
        let id = occurrence.id;
        let value = match (&occurrence.literal, occurrence.operator, occurrence.prefix) {
            (Some(CtValue::Int(value)), _, _) => Some((
                CtValue::IntLiteral(IntLiteral::from(*value)),
                named(occurrence),
            )),
            (Some(literal @ CtValue::IntLiteral(_)), _, _) => {
                Some((literal.clone(), named(occurrence)))
            }
            (None, Some((op, left, right, _)), _) => {
                match (
                    values.get(&operand(id, left)),
                    values.get(&operand(id, right)),
                ) {
                    (Some((left, left_named)), Some((right, right_named))) => {
                        let folded = *left_named || *right_named;
                        match fold_infix(op, left, right) {
                            Ok(value) => Some((value, folded)),
                            // The instance's own check refuses the fold.
                            Err(_) if folded => return None,
                            Err(_) => None,
                        }
                    }
                    _ => None,
                }
            }
            (None, None, Some((op, value))) => match values.get(&operand(id, value)) {
                Some((value, folded)) => {
                    let fold = match op {
                        PrefixOp::Invert => fold_invert(value),
                        _ => fold_neg(value),
                    };
                    match fold {
                        Ok(value) => Some((value, *folded)),
                        Err(_) if *folded => return None,
                        Err(_) => None,
                    }
                }
                None => None,
            },
            _ => None,
        };
        if let Some(
            value @ (CtValue::IntLiteral(_) | CtValue::FloatLiteral(_) | CtValue::Bool(_), _),
        ) = value
        {
            values.insert(id, value);
        }
    }
    let operands = |occurrence: &Occurrence| {
        let id = occurrence.id;
        occurrence
            .operator
            .map(|(_, left, right, _)| vec![operand(id, left), operand(id, right)])
            .or_else(|| occurrence.prefix.map(|(_, value)| vec![operand(id, value)]))
            .unwrap_or_default()
    };
    let folding = |occurrence: &Occurrence| {
        !operands(occurrence).is_empty()
            && values.get(&occurrence.id).is_some_and(|(_, named)| *named)
    };
    let mut inner: HashSet<OccurrenceId> = HashSet::new();
    let mut literals = Vec::new();
    for occurrence in occurrences {
        let id = occurrence.id;
        if inner.contains(&id) {
            let ty = match values.get(&id)? {
                (CtValue::IntLiteral(_), _) => Ty::IntLiteral,
                (CtValue::FloatLiteral(_), _) => Ty::FloatLiteral,
                _ => Ty::Bool,
            };
            literals.push(FoldedLiteral {
                occurrence: id,
                ty,
                materialized: None,
                read_temporary: false,
                unconsumed_temporary: false,
            });
        } else if !folding(occurrence)
            || matches!(recorded(id), Some(Ty::IntLiteral | Ty::FloatLiteral))
        {
            // Arithmetic the template typed as a literal already reads a
            // module constant, whose literal keeps the template's facts
            // around it (`folded_literals`).
            continue;
        } else {
            let (ty, materialized) = match values.get(&id)? {
                (CtValue::IntLiteral(_), _) => (Ty::IntLiteral, Ty::Int),
                (CtValue::FloatLiteral(_), _) => (Ty::FloatLiteral, Ty::Float64),
                _ => {
                    inner.extend(operands(occurrence));
                    continue;
                }
            };
            let bare = !names(
                template.operation_adjustments.iter().map(|(site, _)| site),
                id,
            ) && !names(template.overload_targets.iter().map(|(site, _)| site), id)
                && !names(template.expression_effects.iter().map(|(site, _)| site), id);
            if recorded(id) != Some(&materialized) || !bare {
                return None;
            }
            literals.push(FoldedLiteral {
                occurrence: id,
                ty,
                materialized: Some(materialized),
                read_temporary: names(&template.read_temporary_arguments, id),
                unconsumed_temporary: names(&template.unconsumed_temporaries, id),
            });
        }
        if folding(occurrence) || inner.contains(&id) {
            inner.extend(operands(occurrence));
        }
    }
    Some(literals)
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

/// A value the method grammar reads as a scalar operand or local: a closed
/// scalar, or a `SIMD` value whose dtype and width are both closed
/// (`UInt64`, `SIMD[DType.uint8, 4]`), which no instance changes. The
/// enabled classes' return types and value binders stay [`closed_scalar`].
const fn grammar_scalar(ty: &Ty) -> bool {
    closed_scalar(ty)
        || matches!(
            ty,
            Ty::Simd {
                dtype: mojito_types::types::SimdDtype::Known(_),
                width: mojito_types::types::SimdWidth::Known(_),
            }
        )
}

const fn grammar_scalar_or_literal(ty: &Ty) -> bool {
    grammar_scalar(ty) || matches!(ty, Ty::IntLiteral | Ty::FloatLiteral)
}

const fn holds_comptime_if(statement: &Stmt) -> bool {
    matches!(statement.kind, StmtKind::ComptimeIf { .. })
}

/// The members of a transfer effect's source: a union's, in its order, or
/// the source alone.
fn sig_origin_members(
    origin: &mojito_types::origin::SigOrigin,
) -> Vec<mojito_types::origin::SigOrigin> {
    match origin {
        mojito_types::origin::SigOrigin::Union(members) => members.clone(),
        single => vec![single.clone()],
    }
}

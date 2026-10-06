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

use super::{Checker, EffectRead};
use mojito_ast::ast::{ExprKind, Stmt, StmtKind};
use mojito_checked::templates::{
    BoundBuiltin, CheckedBodyFacts, FactTable, IncompleteReason, InstanceName, InstanceTrace,
    MethodFeatures, OccurrenceId, TemplateCoverage, TemplateId, TemplateOrigin, TemplateOwner,
    TemplatePlace, TypedOrigins, TypedTable,
};
use mojito_common::error::TypeError;
use mojito_common::timing;
use mojito_common::token::{SourceSpan, SyntaxId};
use mojito_types::origin::OwnerId;
use mojito_types::types::{ParamDecl, Ty, TySubst};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

mod bound_dispatch;
mod capture;
mod certificate;
mod comprehensions;
mod constructions;
mod grammar;
mod grammar_builtins;
mod grammar_calls;
mod grammar_constructions;
mod grammar_control;
mod grammar_operators;
mod grammar_packs;
mod grammar_references;
mod grammar_simd;
mod grammar_stores;
mod install;
mod iterations;
mod nested_defs;
mod realization;
mod realization_calls;
mod realization_folds;
mod tuple_unpacks;
mod verify;

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
    /// A field read's base occurrence (`self` of `self.value`).
    member_base: Option<SyntaxId>,
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
    /// A `var` declaration's value occurrence.
    declared: Option<SyntaxId>,
    /// Whether this is a `^` transfer.
    transfer: bool,
    /// Whether, as an argument, it hands over a value the callee may own
    /// rather than a place the callee borrows or copies.
    owned: bool,
    /// The value of an integer or `Bool` literal, which may be a folded
    /// compile-time value.
    literal: Option<mojito_types::ct::CtValue>,
    /// The value of a float literal, which only a lane literal's fit reads
    /// (`realize_lane_literals`).
    float_literal: Option<mojito_common::literal::FloatLiteral>,
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

/// What an instance's arguments stand for in its template's facts.
struct InstanceSubstitution {
    /// Each baked type binder's checked type.
    types: TySubst,
    /// Each baked type pack's element types.
    packs: HashMap<mojito_types::param_expr::ParamId, Vec<Ty>>,
    /// Each folded value binder's value, which a lane dtype or width the
    /// template left open takes (`SIMD[DType.int32, w]`).
    values: Vec<(mojito_types::param_expr::ParamId, mojito_types::ct::CtValue)>,
    /// The method's own value binders a per-instantiation clone keeps
    /// symbolic (`VALUE_BINDERS`): an occurrence reading one is bound to
    /// the clone's own compile-time parameter of that name.
    kept_values: Vec<String>,
    /// The template's `Self` at the instance's pack, with the specialization
    /// the instance names it by where that is not the pack's own mangling:
    /// a `TString` is named by its public segments, while its storage pack
    /// holds each textual segment as an owning `String`.
    named_self: Option<(Ty, Ty)>,
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
    pack_spreads: Vec<mojito_checked::templates::PackSpread>,
    element_constructions: Vec<OccurrenceId>,
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
                Ty::Struct(_, arguments) => Some(arguments.clone().into()),
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

    /// Record that the body being inferred read `callee`'s transfer or
    /// call-through summary, and what the read found.
    pub(super) fn note_effect_query(&self, callee: &str, read: EffectRead) {
        if let Some(Some(frame)) = self.effect_query_frames.borrow_mut().last_mut() {
            frame.push((callee.to_string(), read));
        }
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
                stats.derived_instances.insert(site.instance.clone());
            }
            return Ok(());
        }
        // Capturing a body walks every fact table for every occurrence, so
        // the syntax is judged first: a generic declaration outside every
        // class is retained as such without being captured.
        let shape = template.then(|| self.certificate(site, None).0);
        let admitted = matches!(shape, Some(TemplateCoverage::Certified(_)));
        // The first copy checked of a body the elaborator shaped itself is
        // that body's template.
        let stub_template = (generated && derived.is_none())
            .then(|| self.instance_trace(site))
            .flatten()
            .filter(|trace| {
                trace.first_copy_template
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
        if generated {
            self.template_catalog
                .borrow_mut()
                .stats_mut()
                .inferred_instances
                .insert(site.instance.clone());
        }
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
            let rebinding = || {
                let arguments: Vec<OccurrenceId> = self
                    .body_occurrences(body)
                    .iter()
                    .filter(|occurrence| {
                        facts
                            .overload_targets
                            .iter()
                            .any(|(id, _)| *id == occurrence.id)
                    })
                    .flat_map(|occurrence| {
                        occurrence.arguments.iter().map(|syntax| OccurrenceId {
                            syntax: *syntax,
                            copy: occurrence.id.copy,
                        })
                    })
                    .collect();
                overload_rebinding_only(facts, &inferred, &arguments)
            };
            if inferred != *facts && rebinding() {
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

    fn body_fact_baseline(&self, body: &[Stmt]) -> BodyFactBaseline {
        BodyFactBaseline {
            tables: FactTable::ALL.map(|table| self.span_table(table).entries()),
            unkeyed: self.unkeyed_fact_entries(),
            nested_defs: self.nested_def_entries(body),
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
}

/// The generated Tuple accessor a subscript at a literal index selects, one
/// member per position (`__getitem_param__$k`).
const TUPLE_ELEMENT_ACCESSOR: &str = "__getitem_param__";

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
/// overload target — a method call's contract among them, and the callee
/// each bundle names there as effect-free — and what those calls'
/// `arguments` owe the selected member: the adjustments that convert them
/// to its parameters, and the temporaries its parameters read. Every other
/// table, and every other occurrence, must still agree.
fn overload_rebinding_only(
    derived: &CheckedBodyFacts,
    inferred: &CheckedBodyFacts,
    arguments: &[OccurrenceId],
) -> bool {
    let selected: Vec<OccurrenceId> = derived.overload_targets.iter().map(|(id, _)| *id).collect();
    if selected.is_empty() {
        return false;
    }
    let without_selection = |facts: &CheckedBodyFacts| {
        let mut facts = facts.clone();
        let callees: Vec<String> = facts
            .overload_targets
            .iter()
            .filter(|(id, _)| selected.contains(id))
            .map(|(_, target)| target.clone())
            .collect();
        facts
            .effect_free_callees
            .retain(|callee| !callees.contains(callee));
        facts
            .selected_calls
            .retain(|(id, _)| !selected.contains(id));
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
            .operation_adjustments
            .retain(|(id, _)| !arguments.contains(id));
        facts
            .read_temporary_arguments
            .retain(|id| !arguments.contains(id));
        facts
            .unconsumed_temporaries
            .retain(|id| !arguments.contains(id));
        facts
    };
    without_selection(derived) == without_selection(inferred)
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
            arguments.reusing(
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

/// An infer-only scalar value binder of a method's own (`[w: SIMDLength,
/// //]`, `SIMD[_, _]`'s desugared slots), which a call's arguments solve
/// and a clone keeps as [`bound_binder`]'s value binders are kept.
fn inferred_value_binder(binder: &mojito_ast::ast::TypeParam) -> bool {
    binder.infer_only
        && matches!(binder.bounds.as_slice(), [bound]
            if matches!(bound.as_str(), "Int" | "Bool" | "DType" | "SIMDLength"))
        && binder.origin_mutability.is_none()
        && binder.value_type.is_none()
        && binder.callable_bound.is_none()
        && binder.default.is_none()
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

fn closed_differences(signatures: &[super::MethodSig]) -> bool {
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
}

/// Set the entry a table holds at `id`, or add one.
fn upsert<T>(table: &mut Vec<(OccurrenceId, T)>, id: OccurrenceId, value: T) {
    match table.iter_mut().find(|(site, _)| *site == id) {
        Some(entry) => entry.1 = value,
        None => table.push((id, value)),
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

/// Whether one adjustment has a derivation recipe, as a template's own
/// symbolic facts can be judged.
///
/// `derive_adjustment` answers for one instance, under that instance's
/// substitution. A template applies it under the identity, where an
/// adjustment naming a type still names a parameter: a type name is the one
/// recipe that re-renders such a type, so only an instance's substitution
/// decides it, and the derivation refuses there if the type stays symbolic.
/// A compile-time branch or loop header has no instance counterpart: the
/// instance decided or unrolled it, and realization drops the fact.
fn adjustment_derives(adjustment: &mojito_checked::checked::SemanticAdjustment) -> bool {
    matches!(
        adjustment,
        mojito_checked::checked::SemanticAdjustment::TypeName { .. }
            | mojito_checked::checked::SemanticAdjustment::ComptimeCondition(..)
            | mojito_checked::checked::SemanticAdjustment::ComptimeIteration(..)
            | mojito_checked::checked::SemanticAdjustment::ComptimeDisplay { .. }
            | mojito_checked::checked::SemanticAdjustment::ComptimeApplication { .. }
    ) || mojito_checked::templates::derive_adjustment(adjustment, &Ty::clone).is_some()
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
            | mojito_checked::checked::SemanticAdjustment::AugmentedInPlace { .. }
    )
}

/// Append `entry` unless the list already holds it: a grammar arm may judge
/// one occurrence more than once.
fn push_unique<V: PartialEq>(list: &mut Vec<V>, entry: V) {
    if !list.contains(&entry) {
        list.push(entry);
    }
}

/// The target type of a conversion built-in spelled `name`.
fn conversion_target(name: &str) -> Option<Ty> {
    match name {
        "Int" => Some(Ty::Int),
        "Float64" => Some(Ty::Float64),
        "Bool" => Some(Ty::Bool),
        _ => None,
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
    /// The `deinit` parameters (a move initializer's `deinit move: Self`),
    /// which the body tears down: a field of one is moved out with `^`.
    deinit_params: Vec<&'a str>,
    /// Whether source validation produced the facts. A body it checks may
    /// hold compile-time control flow over scalar locals, assignments, and
    /// runtime `if`s and `while`s, under that check's own rules; any other
    /// body may hold runtime statements instead: scalar locals and
    /// assignments, `if`, `while`, and a bare `return`, each checked once.
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
    /// The positions in `locals` of the nested `def` read parameters holding
    /// a whole value, each read where it lies as a method's parameter is
    /// ([`Self::parameter_receiver`]).
    nested_params: RefCell<Vec<usize>>,
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
    /// The closed struct types an explicit application spells as bare
    /// identifiers (`pick[String](…)`), whose erasure every clone records
    /// again as the template does ([`Self::closed_type_argument`]).
    type_arguments: RefCell<Vec<OccurrenceId>>,
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
    /// The `DType` and `Int` binders of a struct whose members source
    /// validation checks, which a validated member's lane types name
    /// (`SIMD[dt, Self.n]`) and every clone folds
    /// ([`Self::value_shaped_scalar`]).
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
    /// How many runtime `while` loops of a keyed body the statement being
    /// judged lies in: a `break` or `continue` there leaves the runtime
    /// loop, never an unrolled `comptime for`.
    runtime_loops: std::cell::Cell<u32>,
    /// The `to_bits` reinterpretations admitted over a value-shaped receiver,
    /// each with its receiver occurrence, whose adjustment an instance
    /// records from its substituted result.
    simd_to_bits: RefCell<Vec<(OccurrenceId, OccurrenceId)>>,
    /// The `cast[DType.<name>]()` conversions admitted over a value-shaped
    /// receiver, each with its receiver occurrence, whose adjustment an
    /// instance records from its substituted result.
    simd_casts: RefCell<Vec<(OccurrenceId, OccurrenceId)>>,
    /// The `.length` reads admitted over a value-shaped receiver, each with
    /// its receiver occurrence, whose adjustment an instance records from
    /// the receiver's substituted type.
    simd_lengths: RefCell<Vec<(OccurrenceId, OccurrenceId)>>,
    /// The pack storages admitted ([`Self::pack_storage`]), which an
    /// instance reads as its own relocation.
    pack_relocations: RefCell<Vec<mojito_checked::templates::PackRelocation>>,
    /// The pack storages built through the public tuple
    /// ([`Self::pack_storage`]), whose elements an instance moves one by one.
    pack_spreads: RefCell<Vec<mojito_checked::templates::PackSpread>>,
    /// The pack element default constructions admitted
    /// ([`Self::element_initialization`]), each the element's own concrete
    /// construction in an instance.
    element_constructions: RefCell<Vec<OccurrenceId>>,
    /// The stringify calls admitted ([`Self::stringify`]), each routed to
    /// the builtin by an overload target no instance changes.
    stringified: RefCell<Vec<OccurrenceId>>,
    /// The values of the copying pointer writes admitted
    /// ([`Self::pointer_statement`]), each a copyable read with no
    /// reference result.
    copied_writes: RefCell<Vec<OccurrenceId>>,
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
    /// A `var` bound to a lane comparison's mask
    /// ([`BodyShape::lane_comparison`]), read only as a condition or through
    /// `Bool(...)`: an instance re-types the binding and each read alike.
    Mask,
}

/// Whether `occurrence` is a node the elaborator wrote under a pack
/// element's default construction, its identity derived from the
/// construction's.
fn element_construction_part(constructed: &[OccurrenceId], occurrence: &Occurrence) -> bool {
    occurrence
        .id
        .syntax
        .derivation()
        .is_some_and(|(parent, _)| constructed.iter().any(|element| element.syntax == parent))
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

/// Equality and ordering, which [`BodyShape::lane_comparison`] admits over
/// values of a symbolic lane.
const fn comparison(op: mojito_ast::ast::InfixOp) -> bool {
    use mojito_ast::ast::InfixOp::{Eq, Ge, Gt, Le, Lt, Ne};
    matches!(op, Lt | Gt | Le | Ge | Eq | Ne)
}

/// The width-1 `Bool` mask a comparison over a value-shaped scalar yields.
const fn lane_mask() -> Ty {
    Ty::Simd {
        dtype: mojito_types::types::SimdDtype::Known(mojito_ast::ast::Dtype::Bool),
        width: mojito_types::types::SimdWidth::Known(1),
    }
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

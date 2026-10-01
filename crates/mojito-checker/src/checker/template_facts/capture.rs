//! Template capture: what one body inference recorded, rewritten in
//! template-local terms and retained in the catalog.

use super::{
    BodyFactBaseline, BodyParams, BodyReads, BodySite, LiteralKind, Occurrence, SpanKeyed,
    VectorFold, call_result_immutable_binders, callee_reads, kept_apart, names_place,
    operator_dispatch, rooted_reference, sig_origin_members, sorted_applications, template_origin,
    unbound_struct_origins, unbound_typed, values, without_struct_origins,
};
use crate::checker::{Checker, EffectRead};
use mojito_ast::ast::{Expr, ExprKind, Stmt, StmtKind};
use mojito_checked::templates::{
    BoundBuiltin, CallParameterFact, CheckedBodyFacts, CheckedTemplate, FactTable,
    IncompleteReason, MethodFeatures, OccurrenceId, TemplateArgumentBoundary,
    TemplateAugmentedSubscript, TemplateCallContract, TemplateCallResultOrigin,
    TemplateCallTransfer, TemplateClass, TemplateCoverage, TemplateEffectSource,
    TemplateInvalidation, TemplateObligation, TemplateOwner, TemplatePlace, TemplateProducer,
    TemplateReference, TemplateTransferDest, TemplateTransferEffect, TemplateTransferSource,
    TypedTable, WithForm,
};
use mojito_common::timing;
use mojito_common::token::{SourceSpan, SyntaxId};
use mojito_types::origin::OwnerId;
use mojito_types::types::Ty;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

impl Checker {
    /// Retain a module-level generic declaration's freshly inferred body
    /// facts, with the certificate its class earns.
    pub(super) fn record_template(
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
                        // A bound hasher update's leaf is the instance's to
                        // record: at the builtin it realizes again, at a
                        // struct's own method it records none.
                        let updates: Vec<OccurrenceId> = facts
                            .bound_builtins
                            .iter()
                            .filter(|(_, builtin)| {
                                matches!(builtin, BoundBuiltin::Update | BoundBuiltin::UpdateSimd)
                            })
                            .map(|(id, _)| *id)
                            .collect();
                        if !updates.is_empty() {
                            let sites = self
                                .body_occurrences(site.body)
                                .into_iter()
                                .filter(|occurrence| updates.contains(&occurrence.id))
                                .map(|occurrence| occurrence.span)
                                .collect();
                            facts.hash_leaves =
                                self.hash_leaves_outside(baseline.hash_leaf_demands, &sites);
                        }
                        facts.constructions = notes.constructions;
                        facts.callable_calls = notes.callable_calls;
                        facts.repr_calls = notes.repr_calls;
                        facts.print_calls = notes.print_calls;
                        facts.simd_to_bits = notes.simd_to_bits;
                        facts.simd_casts = notes.simd_casts;
                        facts.simd_lengths = notes.simd_lengths;
                        facts.pack_relocations = notes.pack_relocations;
                        facts.pack_spreads = notes.pack_spreads;
                        facts.element_constructions = notes.element_constructions;
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

    /// Put a template in the catalog, counted by its coverage.
    pub(super) fn retain_template(
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

    /// Whether the adjustment at `site` constructs a binder that is not the
    /// enclosing struct's, which every clone keeps symbolic
    /// ([`BodyShape::binder_construction`]).
    pub(super) fn own_binder_construction(
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
    pub(super) fn kept_element_store(
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

    /// Every statement and expression occurrence of a body in pre-order, by
    /// the identity it had before the final re-key and its copy number.
    pub(super) fn body_occurrences(&self, body: &[Stmt]) -> Vec<Occurrence> {
        self.occurrences_over(body, &self.with_desugars.borrow(), None)
    }

    /// The occurrences of `body` with each `with` statement's children read
    /// from its desugar in `desugars`: the statement stays an occurrence,
    /// and the nodes the desugar synthesized are occurrences beside the ones
    /// it kept from the source. `template` holds the occurrences of the
    /// checked template an instance body is matched against.
    pub(super) fn occurrences_over(
        &self,
        body: &[Stmt],
        desugars: &HashMap<SourceSpan, crate::checker::with_stmt::WithDesugar>,
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
                let Some(occurrence) = self.found.last_mut() else {
                    return;
                };
                match &statement.kind {
                    StmtKind::AugAssign { place, value, .. } => {
                        occurrence.augmented = Some((
                            self.origins.origin(place.syntax_id),
                            self.origins.origin(value.syntax_id),
                        ));
                    }
                    StmtKind::VarDecl { value, .. } => {
                        occurrence.declared = Some(self.origins.origin(value.syntax_id));
                    }
                    _ => {}
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
                    member_base: None,
                    method_call: None,
                    type_receiver: None,
                    operator: None,
                    prefix: None,
                    augmented: None,
                    declared: None,
                    transfer: false,
                    owned: false,
                    literal: None,
                    float_literal: None,
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
                    // binder (`Scalar[Self.dtype]`) takes a fresh identity,
                    // or one derived from the spelling call: the template
                    // spelled a type there, not an occurrence. A
                    // value-position binder (`Scalar[dt]`) folds under the
                    // name's identity, which the template checked: matched
                    // against that template, the constant stands for the
                    // name's occurrence ([`VectorFold::Constant`]).
                    let origin = self.origins.origin(expr.syntax_id);
                    if origin.is_fresh() || origin.derivation().is_some() {
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
                    member_base: match &expr.kind {
                        ExprKind::Member { object, .. } => {
                            Some(self.origins.origin(object.syntax_id))
                        }
                        _ => None,
                    },
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
                        ExprKind::MethodCall { object, .. } => Some(object.as_ref()),
                        ExprKind::Invoke { callee, .. } => match &callee.kind {
                            ExprKind::Member { object, .. } => Some(object.as_ref()),
                            _ => None,
                        },
                        _ => None,
                    }
                    .and_then(|object| match &object.kind {
                        ExprKind::TypeApply { name, args } => Some((name.clone(), args.clone())),
                        _ => None,
                    }),
                    operator: match &expr.kind {
                        ExprKind::Infix(op, left, right) if operator_dispatch(*op) => Some((
                            *op,
                            self.origins.origin(left.syntax_id),
                            self.origins.origin(right.syntax_id),
                            crate::checker::places::is_place_expr(right),
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
                    declared: None,
                    transfer: matches!(expr.kind, ExprKind::Transfer(_)),
                    owned: crate::checker::overload_support::argument_is_owned(expr),
                    literal: match &expr.kind {
                        ExprKind::Int(value) => Some(value.to_i64().map_or_else(
                            || mojito_types::ct::CtValue::IntLiteral(value.clone()),
                            mojito_types::ct::CtValue::Int,
                        )),
                        ExprKind::Bool(value) => Some(mojito_types::ct::CtValue::Bool(*value)),
                        _ => None,
                    },
                    float_literal: match &expr.kind {
                        ExprKind::Float(value) => Some(value.clone()),
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
    pub(super) fn instance_with_desugars(
        &self,
        body: &[Stmt],
        forms: &[(OccurrenceId, WithForm)],
    ) -> Option<HashMap<SourceSpan, crate::checker::with_stmt::WithDesugar>> {
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
            let statements = crate::checker::with_stmt::with_desugar(&statement, form)?;
            mojito_ast::visit::walk_block(&mut pending, &statements);
            desugars.insert(
                span,
                crate::checker::with_stmt::WithDesugar { form, statements },
            );
        }
        Some(desugars)
    }

    /// What one body inference recorded, in template-local terms. Every
    /// table is accounted for: an entry in a table without a recipe, an
    /// entry keyed outside the body, growth in a store not keyed by
    /// occurrence, or a callee effect summary that was not empty refuses the
    /// body.
    pub(super) fn capture_body_facts(
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

    /// The immutable-binder records a body keeps. A view-returning call's is
    /// the immutable slots of its result origins, which installation derives
    /// again, and an empty one is what installation writes at every
    /// construction; any other names slots and field paths alone.
    pub(super) fn captured_immutable_binders(
        &self,
        occurrences: &[Occurrence],
    ) -> Vec<(OccurrenceId, Vec<crate::checker::ImmutableOriginBinder>)> {
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

    /// The expression spans of the annotations a body's check resolves
    /// outside its occurrences: the return annotation the body being checked
    /// re-resolves at each `return`, and every `rebind` target in `body`.
    pub(super) fn annotation_spans_of(&self, body: &[Stmt]) -> HashSet<SourceSpan> {
        let mut spans = self
            .return_annotations
            .last()
            .and_then(Option::as_ref)
            .map(|(annotation, _)| annotation_spans(annotation))
            .unwrap_or_default();
        spans.extend(self.rebind_targets.target_spans(body));
        spans
    }

    /// The checker's storage for one occurrence-keyed fact table.
    pub(super) fn span_table(&self, table: FactTable) -> std::cell::Ref<'_, dyn SpanKeyed> {
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

/// `body` with each `with` statement that has a desugar in `desugars` made a
/// scope of that desugar under the statement's own identity, or `None` when
/// the body holds no such statement.
fn expand_with_statements(
    body: &[Stmt],
    desugars: &HashMap<SourceSpan, crate::checker::with_stmt::WithDesugar>,
) -> Option<Vec<Stmt>> {
    struct Finds<'a> {
        desugars: &'a HashMap<SourceSpan, crate::checker::with_stmt::WithDesugar>,
        found: bool,
    }

    impl mojito_ast::visit::Visitor for Finds<'_> {
        fn visit_stmt(&mut self, statement: &Stmt) {
            self.found |= matches!(statement.kind, StmtKind::With { .. })
                && self.desugars.contains_key(&statement.source_span());
        }
    }

    struct Expand<'a>(&'a HashMap<SourceSpan, crate::checker::with_stmt::WithDesugar>);

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

/// The parameters each call at a body's occurrences binds, in pre-order.
fn captured_call_parameters(
    occurrences: &[Occurrence],
    table: &HashMap<SourceSpan, Vec<crate::checker::CallParameter>>,
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

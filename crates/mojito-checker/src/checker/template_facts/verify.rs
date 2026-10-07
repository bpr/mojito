//! Verification that a body inference recorded only what capture can keep
//! and derivation can reproduce, and the census of what keeps a body out.

use super::{
    BodyDeclaration, BodyFactBaseline, BodyParams, BodyReads, BodyRole, BodySite, Occurrence,
    UNKEYED_STORES, adjustment_derives, closed_scalar, rooted_reference, typed_origins,
};
use crate::checker::{Checker, EffectRead};
use mojito_ast::ast::{Expr, ExprKind, Stmt};
use mojito_checked::templates::{FactTable, IncompleteReason, TemplateOwner, TemplatePlace};
use mojito_common::timing;
use mojito_common::token::SourceSpan;
use mojito_types::types::Ty;
use std::collections::HashSet;

impl Checker {
    /// Whether one body inference replayed or published a transfer residue
    /// no derivation accounts for: a named callable's effects behind a
    /// call-through residue, a function value's baked effects, or an effect
    /// or a replayed summary naming a captured binding. A callee's summary
    /// replayed on the call's own receiver and arguments is retained instead
    /// ([`TemplateObligation::ReplayedTransfers`]), and a call-through residue
    /// the body reads or publishes is kept too
    /// ([`TemplateObligation::CallThroughResidue`]).
    pub(super) fn transfer_residue(&self, reads: &BodyReads) -> bool {
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

    /// Report, for one generic body, everything that keeps it from being
    /// captured: the census that orders which recipes to write next.
    ///
    /// Counters are `template_census.<class>.bodies`, `.capturable`,
    /// `.blocked_by.<reason>` (the body has that reason),
    /// `.sole_blocker.<reason>` (it has no other), and, for a method,
    /// `.grammar.<construct>` for each construct its declaration and body
    /// hold, whatever class admits it (`grammar_features`).
    pub(super) fn census(
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
            .unkeyed_growth(baseline)
            .into_iter()
            .map(|store| format!("store:{store}"))
            .collect();
        if self.transfer_residue(reads) {
            reasons.push("effects".to_string());
        }
        let annotation = self.annotation_spans_of(site.body);
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
                        | mojito_checked::checked::SemanticAdjustment::AugmentedInPlace { .. }
                        | mojito_checked::checked::SemanticAdjustment::ConstructPackElement { .. }
                        | mojito_checked::checked::SemanticAdjustment::ConstructType { .. }
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

    /// Whether one body inference recorded only what capture can keep: no
    /// growth in a store not keyed by occurrence, no callee effect summary
    /// that was not empty, no entry keyed outside the body or in a table
    /// without a recipe, and no retained type naming a place.
    pub(super) fn capturable(
        &self,
        body: &[Stmt],
        occurrences: &[Occurrence],
        baseline: &BodyFactBaseline,
        reads: &BodyReads,
    ) -> Result<(), IncompleteReason> {
        if let Some(store) = self.unkeyed_growth(baseline).first() {
            return Err(IncompleteReason::UnkeyedFact(store));
        }
        // The body's locals are told from every other binding by their
        // identity range; a check whose range split (`reserve_owners`)
        // has no such range.
        if self.owner_range_split.get() {
            return Err(IncompleteReason::UnkeyedFact("binding identities"));
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
        let annotation = self.annotation_spans_of(body);
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

    /// Entries in the fact stores a body inference can grow that are not
    /// keyed by one of its occurrences. A derivation has no recipe for them,
    /// so any growth refuses the body and names the store
    /// (`unkeyed_growth`).
    pub(super) fn unkeyed_fact_entries(&self) -> [(&'static str, usize); UNKEYED_STORES] {
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

    /// The unkeyed stores the body's check grew. The transferred-origin
    /// store is told entry by entry elsewhere.
    fn unkeyed_growth(&self, baseline: &BodyFactBaseline) -> Vec<&'static str> {
        self.unkeyed_fact_entries()
            .into_iter()
            .zip(baseline.unkeyed)
            .filter(|((store, now), (_, before))| *store != TRANSFERRED_ORIGINS && now != before)
            .map(|((store, _), _)| store)
            .collect()
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
}

/// The unkeyed store a replayed transfer merges origins into.
const TRANSFERRED_ORIGINS: &str = "transferred origins";

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
        (
            mojito_ast::ast::has_body_decorator(&method.decorators),
            "decorated",
        ),
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

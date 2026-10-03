//! Realization of what the elaborator folds per instance: pack elements
//! and their constructions, folded literals and arithmetic, and `SIMD`
//! lanes.

use super::{
    BodyReads, BodySite, ElementIndices, InstanceSubstitution, Occurrence, VectorFold,
    callee_reads, closed_scalar, comparison, element_construction_part, fact_at, lane_mask,
    push_unique, set_fact, sorted_applications, unbound_typed, upsert, values,
    without_struct_origins,
};
use crate::checker::Checker;
use crate::checker::annotations::{simd_binder_slots, simd_binder_view};
use crate::checker::builtins::SIMD_WILDCARD_BOUND;
use mojito_ast::ast::{Expr, ExprKind, Stmt, StmtKind};
use mojito_checked::templates::{
    CheckedBodyFacts, FactTable, FoldedLiteral, IncompleteReason, InstanceTrace, OccurrenceId,
    PackElementNode, TypedTable,
};
use mojito_common::error::TypeError;
use mojito_common::timing;
use mojito_common::token::SyntaxId;
use mojito_types::types::{ParamDecl, Ty};
use std::collections::{HashMap, HashSet};

impl Checker {
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
    pub(super) fn realize_lane_literals(
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
                .and_then(|occurrence| {
                    occurrence.literal.clone().or_else(|| {
                        occurrence
                            .float_literal
                            .clone()
                            .map(mojito_types::ct::CtValue::FloatLiteral)
                    })
                })
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

    /// What an instance's own check records at each pack element's default
    /// construction ([`BodyShape::element_initialization`]), `None` for a
    /// template that admitted none.
    ///
    /// The elaborator wrote the element type's concrete construction where
    /// the template spelled `Self.Ts[i]()`: closed syntax over literals and
    /// type names, which names nothing the body binds. It is checked here as
    /// the store checks its value, alone, and what that check recorded at
    /// the construction's own occurrences is the instance's. A construction
    /// recording a fact outside the tables carried here refuses.
    pub(super) fn element_construction_facts(
        &self,
        site: &BodySite<'_>,
        trace: &InstanceTrace,
    ) -> Result<Option<CheckedBodyFacts>, &'static str> {
        struct Stores<'a> {
            origins: &'a mojito_ast::ast::SyntaxOrigins,
            constructed: &'a [OccurrenceId],
            found: Vec<(Option<&'a Expr>, &'a Expr)>,
        }

        impl<'a> Stores<'a> {
            fn built(&self, value: &Expr) -> bool {
                self.constructed
                    .iter()
                    .any(|element| element.syntax == self.origins.origin(value.syntax_id))
            }

            fn collect(&mut self, body: &'a [Stmt]) {
                for statement in body {
                    match &statement.kind {
                        StmtKind::SetPlace { place, value } if self.built(value) => {
                            self.found.push((Some(place), value));
                        }
                        StmtKind::VarDecl { value, .. } if self.built(value) => {
                            self.found.push((None, value));
                        }
                        StmtKind::Expr(Expr {
                            kind: ExprKind::Call { name, args, .. },
                            ..
                        }) if name == "print" => {
                            let built: Vec<_> = args
                                .iter()
                                .filter(|argument| self.built(argument))
                                .map(|argument| (None, argument))
                                .collect();
                            self.found.extend(built);
                        }
                        StmtKind::Scope(body) => self.collect(body),
                        _ => {}
                    }
                }
            }
        }

        const UNCHECKED: &str = "an element's default construction does not check for the instance";
        let (constructed, template_occurrences) = {
            let catalog = self.template_catalog.borrow();
            match catalog.template(&trace.template) {
                Some(checked) if !checked.facts.element_constructions.is_empty() => (
                    checked.facts.element_constructions.clone(),
                    checked.facts.occurrences.clone(),
                ),
                _ => return Ok(None),
            }
        };
        let mut stores = Stores {
            origins: &self.syntax_origins,
            constructed: &constructed,
            found: Vec::new(),
        };
        stores.collect(site.body);
        self.effect_query_frames.borrow_mut().push(Some(Vec::new()));
        self.struct_application_frames
            .borrow_mut()
            .push(Some(Vec::new()));
        let checked = stores.found.iter().try_for_each(|(place, value)| {
            // A construction handed to `print` or bound to a local is
            // checked as that statement checks its value, with nothing
            // expected of it.
            let Some(place) = place else {
                self.infer(value).map_err(|_| UNCHECKED)?;
                return Ok(());
            };
            let target = self.place_storage_ty(place).ok_or(UNCHECKED)?;
            let found = self
                .infer_with_expected(value, &target, true)
                .map_err(|_| UNCHECKED)?;
            let converted = self
                .record_implicit_conversion(value, &found, &target)
                .map_err(|_| UNCHECKED)?;
            if !converted || found != target {
                return Err("an element's default construction is not of its element's type");
            }
            if self.is_copyable(&found) {
                self.check_consuming(value, &found, "assignment target")
                    .map_err(|_| UNCHECKED)?;
            }
            Ok(())
        });
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
        if let Some(Some(outer)) = self.effect_query_frames.borrow_mut().last_mut() {
            outer.extend(reads.effect_queries.iter().cloned());
        }
        if let Some(Some(outer)) = self.struct_application_frames.borrow_mut().last_mut() {
            outer.extend(reads.struct_applications.iter().cloned());
        }
        let (occurrences, others): (Vec<_>, Vec<_>) = self
            .occurrences_over(site.body, &HashMap::new(), Some(&template_occurrences))
            .into_iter()
            .partition(|occurrence| {
                constructed
                    .iter()
                    .any(|element| element.syntax == occurrence.id.syntax)
                    || element_construction_part(&constructed, occurrence)
            });
        let facts =
            checked.and_then(|()| self.captured_element_constructions(site, &occurrences, &reads));
        // The body's own check, or the installation of its derived facts,
        // starts from tables that hold nothing of it.
        for occurrence in occurrences.iter().chain(&others) {
            self.remove_occurrence_facts(&occurrence.span);
        }
        facts.map(Some)
    }

    /// What the check of an instance's element constructions recorded at
    /// their `occurrences` ([`Self::element_construction_facts`]).
    fn captured_element_constructions(
        &self,
        site: &BodySite<'_>,
        occurrences: &[Occurrence],
        reads: &BodyReads,
    ) -> Result<CheckedBodyFacts, &'static str> {
        for table in FactTable::ALL {
            let entries = self.span_table(table);
            let recorded = occurrences
                .iter()
                .any(|occurrence| entries.has(&occurrence.span));
            if recorded && !element_construction_table(table) {
                timing::note("template_derivations.element_table", || {
                    format!("{}: {table:?}", site.display)
                });
                return Err("an element's default construction records a fact no recipe carries");
            }
        }
        let mut typed_origins = Vec::new();
        let expression_types = unbound_typed(
            TypedTable::Expression,
            values(occurrences, &self.expression_types.borrow()),
            &|_| Err(IncompleteReason::ExternalBinding),
            &mut typed_origins,
        )
        .map_err(|_| "an element's default construction names a place")?;
        if !typed_origins.is_empty() || self.transfer_residue(reads) {
            return Err("an element's default construction carries an origin or a transfer");
        }
        let (effect_free_callees, value_callees, call_through_reads) = callee_reads(reads);
        if !call_through_reads.is_empty() {
            return Err("an element's default construction reads a call-through residue");
        }
        Ok(CheckedBodyFacts {
            expression_types,
            operation_adjustments: values(occurrences, &self.operation_adjustments.borrow()),
            overload_targets: values(occurrences, &self.overload_targets.borrow()),
            construction_immutable_binders: self.captured_immutable_binders(occurrences),
            simd_constructions: values(occurrences, &self.simd_constructions.borrow()),
            unconsumed_temporaries: occurrences
                .iter()
                .filter(|occurrence| {
                    self.unconsumed_temporaries
                        .borrow()
                        .contains(&occurrence.span)
                })
                .map(|occurrence| occurrence.id)
                .collect(),
            struct_applications: reads
                .struct_applications
                .iter()
                .map(|(name, arguments)| {
                    match without_struct_origins(&Ty::Struct(
                        name.clone(),
                        arguments.clone().into(),
                    )) {
                        Ty::Struct(name, arguments) => (name, arguments.into_vec()),
                        _ => (name.clone(), arguments.clone()),
                    }
                })
                .collect(),
            effect_free_callees,
            value_callees,
            ..CheckedBodyFacts::default()
        })
    }
}

/// The dimensions of each construction whose dtype or width named a folded
/// value binder (`Scalar[dt](x)`), which the template left unrecorded: the
/// instance's are its substituted construction type's, as a closed
/// construction's are its recorded type's.
pub(super) fn realize_value_shaped_constructions(
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
pub(super) fn realize_simd_intrinsics(
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
        if crate::checker::builtins::dtype_bit_width(dtype)
            < crate::checker::builtins::dtype_bit_width(source)
        {
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

/// [`BodyShape::lane_float_method`]'s calls, realized at the instance's
/// lane: a sized float lane records what the template recorded, and the
/// native `Float64` resolves the call as its own method, which borrows each
/// place argument and reads each temporary one, which the caller destroys.
/// Any other lane refuses, for the clone check to report.
pub(super) fn realize_lane_float_methods(
    template: &CheckedBodyFacts,
    facts: &mut CheckedBodyFacts,
    occurrences: &[Occurrence],
) -> Result<(), &'static str> {
    let mut borrowed = Vec::new();
    let mut temporaries = Vec::new();
    for occurrence in occurrences {
        let lane_method = occurrence.method_call.as_ref().is_some_and(|(_, method)| {
            matches!(
                method.as_str(),
                "__floor__" | "__ceil__" | "__trunc__" | "__fma__"
            )
        });
        let open = fact_at(&template.expression_types, occurrence.id).is_some_and(|ty| {
            matches!(ty, Ty::Simd { .. }) && mojito_types::types::is_symbolic(ty)
        });
        if !lane_method || !open {
            continue;
        }
        match fact_at(&facts.expression_types, occurrence.id) {
            Some(Ty::Float64) => {
                for syntax in &occurrence.arguments {
                    let id = OccurrenceId {
                        syntax: *syntax,
                        copy: occurrence.id.copy,
                    };
                    let place = occurrences
                        .iter()
                        .find(|found| found.id == id)
                        .is_some_and(|found| found.identifier || found.member_base.is_some());
                    if place {
                        borrowed.push(id);
                    } else {
                        temporaries.push(id);
                    }
                }
            }
            Some(ty)
                if mojito_types::types::simd_shape(ty)
                    .is_some_and(|(dtype, width)| dtype.is_float() && width == 1) => {}
            _ => return Err("a float lane method's instance lane is not a float scalar"),
        }
    }
    let order = |id: &OccurrenceId| occurrences.iter().position(|found| found.id == *id);
    for (table, realized) in [
        (&mut facts.borrowed_read_call_places, &borrowed),
        (&mut facts.read_temporary_arguments, &temporaries),
        (&mut facts.unconsumed_temporaries, &temporaries),
    ] {
        let fresh: Vec<OccurrenceId> = realized
            .iter()
            .filter(|id| !table.contains(id))
            .copied()
            .collect();
        if !fresh.is_empty() {
            table.extend(fresh);
            table.sort_by_key(order);
        }
    }
    Ok(())
}

/// [`BodyShape::lane_comparison`]'s comparisons, re-typed at the instance's
/// lane: over a sized scalar vector the comparison stays the template's
/// mask, and over a native scalar it compares natively to a `Bool`, as does
/// a local it initializes ([`LocalKind::Mask`]) and every read of that
/// local. Any other lane refuses, for the clone check to report. A
/// condition's truthiness mark is judged again from the re-typed value
/// afterwards.
pub(super) fn realize_lane_comparisons(
    template: &CheckedBodyFacts,
    facts: &mut CheckedBodyFacts,
    occurrences: &[Occurrence],
) -> Result<(), &'static str> {
    for occurrence in occurrences {
        let Some((op, left, right, _)) = occurrence.operator else {
            continue;
        };
        let at = |syntax| OccurrenceId {
            syntax,
            copy: occurrence.id.copy,
        };
        let open = |syntax| {
            fact_at(&template.expression_types, at(syntax)).is_some_and(|ty| {
                matches!(ty, Ty::Simd { .. }) && mojito_types::types::is_symbolic(ty)
            })
        };
        let Some(operand) = [left, right].into_iter().find(|syntax| open(*syntax)) else {
            continue;
        };
        if !comparison(op)
            || fact_at(&template.expression_types, occurrence.id) != Some(&lane_mask())
        {
            continue;
        }
        let lane = fact_at(&facts.expression_types, at(operand))
            .ok_or("a lane comparison's operand has no retained type")?;
        if closed_scalar(lane) {
            if crate::checker::operators::scalar_operator_result(op, lane) != Some(Ty::Bool) {
                return Err("a lane comparison is not the instance's scalar operation");
            }
            set_fact(&mut facts.expression_types, occurrence.id, Ty::Bool);
            let declaration = occurrences.iter().find(|found| {
                found.id.copy == occurrence.id.copy && found.declared == Some(occurrence.id.syntax)
            });
            if let Some(declaration) = declaration {
                retype_mask_local(facts, declaration.id, occurrence.id)?;
            }
        } else if mojito_types::types::simd_shape(lane).is_none_or(|(_, width)| width != 1) {
            return Err("a lane comparison's instance lane is not a scalar");
        }
    }
    Ok(())
}

/// The hidden dtype and width values a clone folds where its template
/// viewed the wildcard vector binder `decl` as a lane-shaped vector
/// (`simd_binder_view`): the slots of the closed vector type `ty` the clone
/// bakes the binder to, with the whole view paired to `ty` in `views`.
/// Empty for any other binder; an open slot does not resolve.
pub(super) fn simd_binder_values(
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
pub(super) fn fold_binder_views(ty: &Ty, views: &[(Ty, Ty)]) -> Ty {
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
pub(super) fn folded_literals(
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

/// `indices` with every other occurrence whose template type names a pack
/// element at a `comptime for` index (`element` of `ref element =
/// self.storage[i]`) fixed at the index of its own unrolled copy: the one an
/// indexed occurrence of that copy was folded to under the same binder.
/// Copies are numbered per template occurrence in pre-order, so only an
/// occurrence copied as often as the indexed one shares its numbering; one
/// in a loop nested deeper is left unfixed, and its derivation refuses.
pub(super) fn loop_element_indices(
    mut indices: ElementIndices,
    template: &CheckedBodyFacts,
    occurrences: &[Occurrence],
) -> ElementIndices {
    let copies = |syntax: SyntaxId| {
        occurrences
            .iter()
            .filter(|occurrence| occurrence.id.syntax == syntax)
            .count()
    };
    let references = template
        .reference_binding_types
        .iter()
        .chain(&template.reference_place_types)
        .map(|(id, reference)| (id, &reference.referent));
    let binders: HashMap<SyntaxId, mojito_types::param_expr::ParamId> = template
        .expression_types
        .iter()
        .chain(&template.expression_place_types)
        .chain(&template.binding_types)
        .map(|(id, ty)| (id, ty))
        .chain(references)
        .filter_map(|(id, ty)| Some((id.syntax, element_index_binder(ty)?)))
        .collect();
    let fixed: Vec<_> = occurrences
        .iter()
        .filter(|occurrence| !indices.contains_key(&occurrence.id))
        .filter_map(|occurrence| {
            let binder = binders.get(&occurrence.id.syntax)?;
            let count = copies(occurrence.id.syntax);
            let (_, value) = indices.iter().find_map(|(id, index)| {
                (id.copy == occurrence.id.copy && index.0 == *binder && copies(id.syntax) == count)
                    .then_some(index)
            })?;
            Some((occurrence.id, (binder.clone(), value.clone())))
        })
        .collect();
    indices.extend(fixed);
    indices
}

/// `indices` with each pack element default construction no folded index
/// fixed (`print(Self.Ts[i]())`, `var value = Ts[i]()`) fixed at the one
/// loop index whose element is the type the instance's own check of the
/// construction found ([`Checker::element_construction_facts`]), and every
/// other occurrence of that copy with it ([`loop_element_indices`]). Where
/// several elements share that type, the first is taken when the copy reads
/// the same types whichever it is ([`index_indifferent`]). `None` when the
/// element's type names no index, or indices the copy tells apart.
pub(super) fn constructed_element_indices(
    mut indices: ElementIndices,
    template: &CheckedBodyFacts,
    elements: Option<&CheckedBodyFacts>,
    substitution: &InstanceSubstitution,
    occurrences: &[Occurrence],
) -> Option<ElementIndices> {
    let Some(elements) = elements else {
        return Some(indices);
    };
    let mut seeded = false;
    for occurrence in occurrences {
        let constructed = template
            .element_constructions
            .iter()
            .any(|element| element.syntax == occurrence.id.syntax);
        if !constructed || indices.contains_key(&occurrence.id) {
            continue;
        }
        let built = fact_at(&elements.expression_types, occurrence.id)?;
        let Some(Ty::Dependent(dependent)) = template
            .expression_types
            .iter()
            .find_map(|(id, ty)| (id.syntax == occurrence.id.syntax).then_some(ty))
        else {
            return None;
        };
        let (pack, index) = dependent.pack_element()?;
        let (
            mojito_types::param_expr::ParamKind::DeclRef(pack),
            mojito_types::param_expr::ParamKind::DeclRef(index),
        ) = (pack.kind(), index.kind())
        else {
            return None;
        };
        let packed = substitution.packs.get(&pack.id)?;
        let matching: Vec<mojito_types::ct::CtValue> = packed
            .iter()
            .enumerate()
            .filter(|(_, element)| {
                mojito_types::types::substitute_packs(
                    element,
                    &substitution.types,
                    &substitution.packs,
                    &substitution.values,
                ) == *built
            })
            .map(|(at, _)| i64::try_from(at).map(mojito_types::ct::CtValue::Int))
            .collect::<Result<_, _>>()
            .ok()?;
        let (at, rivals) = matching.split_first()?;
        if !rivals.is_empty()
            && !index_indifferent(
                template,
                substitution,
                occurrences,
                occurrence.id,
                &index.id,
                (at, rivals),
            )
        {
            return None;
        }
        indices.insert(occurrence.id, (index.id.clone(), at.clone()));
        seeded = true;
    }
    Some(if seeded {
        loop_element_indices(indices, template, occurrences)
    } else {
        indices
    })
}

/// `indices` with each `^` transfer of a pack element fixed at its
/// element's index: the transfer is typed as the element it moves, and in
/// pre-order the element is the occurrence right after it.
pub(super) fn transferred_element_indices(
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

/// Lay the facts an instance's element constructions recorded over its
/// realized facts, each table kept in the occurrences' pre-order `order`.
/// The template typed each construction as its element, which the instance's
/// check must have typed it as too.
pub(super) fn merge_element_constructions(
    facts: &mut CheckedBodyFacts,
    elements: &CheckedBodyFacts,
    constructed: &[OccurrenceId],
    order: &[OccurrenceId],
) -> Result<(), &'static str> {
    fn merge<V: Clone>(
        table: &mut Vec<(OccurrenceId, V)>,
        entries: &[(OccurrenceId, V)],
        order: &[OccurrenceId],
    ) {
        for (id, entry) in entries {
            upsert(table, *id, entry.clone());
        }
        table.sort_by_key(|(id, _)| order.iter().position(|entry| entry == id));
    }
    let typed = constructed.iter().all(|id| {
        fact_at(&facts.expression_types, *id) == fact_at(&elements.expression_types, *id)
    });
    if !typed {
        return Err("an element's default construction is not of its element's type");
    }
    merge(
        &mut facts.expression_types,
        &elements.expression_types,
        order,
    );
    merge(
        &mut facts.operation_adjustments,
        &elements.operation_adjustments,
        order,
    );
    merge(
        &mut facts.overload_targets,
        &elements.overload_targets,
        order,
    );
    merge(
        &mut facts.construction_immutable_binders,
        &elements.construction_immutable_binders,
        order,
    );
    merge(
        &mut facts.simd_constructions,
        &elements.simd_constructions,
        order,
    );
    for id in &elements.unconsumed_temporaries {
        push_unique(&mut facts.unconsumed_temporaries, *id);
    }
    facts
        .unconsumed_temporaries
        .sort_by_key(|id| order.iter().position(|entry| entry == id));
    facts.struct_applications = sorted_applications(
        facts
            .struct_applications
            .iter()
            .chain(&elements.struct_applications)
            .cloned()
            .collect(),
    );
    for callee in &elements.effect_free_callees {
        push_unique(&mut facts.effect_free_callees, callee.clone());
    }
    facts.effect_free_callees.sort();
    for callee in &elements.value_callees {
        push_unique(&mut facts.value_callees, callee.clone());
    }
    Ok(())
}

/// Tell the syntax the elaborator wrote for a vector apart from the
/// template's: the dimensions it spelled for a vector alias's construction
/// (`U256(…)` as `SIMD[DType.uint64, 4](…)`) are part of the type, which no
/// check records anything at; and a struct's vector value binder it folded
/// (`Self.key`) is a construction under the name's identity whose lanes are
/// all its own.
pub(super) fn fold_vector_values(
    occurrences: &mut [Occurrence],
    template: &[OccurrenceId],
    element_constructions: &[OccurrenceId],
) {
    let constructed = |syntax: &SyntaxId| {
        element_constructions
            .iter()
            .any(|element| element.syntax == *syntax)
    };
    let checked =
        |syntax: &SyntaxId| !constructed(syntax) && template.iter().any(|id| id.syntax == *syntax);
    let written: HashSet<SyntaxId> = occurrences
        .iter()
        .filter(|occurrence| !element_construction_part(element_constructions, occurrence))
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
pub(super) fn construct_folded_vectors(
    facts: &mut CheckedBodyFacts,
    occurrences: &[Occurrence],
) -> bool {
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

/// The mangling values of a variadic struct applied to a closed pack,
/// spelled as the elaborator spells them. The template itself is not
/// registered in the clone check, so only a specialization minted under that
/// name says it was one.
pub(super) fn closed_pack_values(
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
pub(super) fn relocate_packs(facts: &mut CheckedBodyFacts, order: &[OccurrenceId]) -> bool {
    for relocation in std::mem::take(&mut facts.pack_relocations) {
        let pack = relocation.pack;
        let Some(ty) = fact_at(&facts.expression_types, relocation.call).cloned() else {
            return false;
        };
        if !order.contains(&pack) || fact_at(&facts.expression_types, pack).is_some() {
            return false;
        }
        insert_ranked(&mut facts.transfers, order, relocation.call);
        insert_ranked(&mut facts.expression_types, order, (pack, ty.clone()));
        insert_ranked(&mut facts.expression_place_types, order, (pack, ty));
        insert_ranked(
            &mut facts.expression_bindings,
            order,
            (
                pack,
                mojito_checked::templates::TemplateOwner::Param(relocation.param),
            ),
        );
    }
    true
}

/// Lay each [`PackSpread`](mojito_checked::templates::PackSpread) over the
/// instance's substituted facts: the public tuple the call constructs names its
/// element types, and each element `args[k]^` the elaborator spelled moves
/// the collector's `k`-th element out of its place, the collector bound to
/// its parameter. An `Err` when the instance's elements are not exactly one
/// such move per element type.
pub(super) fn spread_packs(
    facts: &mut CheckedBodyFacts,
    occurrences: &[Occurrence],
) -> Result<(), &'static str> {
    use mojito_types::types::TyArg;
    const REFUSAL: &str = "a spread pack's elements are not the instance's moves";
    let order: Vec<OccurrenceId> = occurrences.iter().map(|occurrence| occurrence.id).collect();
    let order = order.as_slice();
    for spread in std::mem::take(&mut facts.pack_spreads) {
        let Some(Ty::Struct(tuple, arguments)) =
            fact_at(&facts.expression_types, spread.call).cloned()
        else {
            return Err(REFUSAL);
        };
        let Some(elements) = arguments
            .into_iter()
            .map(|argument| match argument {
                TyArg::Ty(ty) => Some(ty),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()
        else {
            return Err(REFUSAL);
        };
        let collector = Ty::Tuple(elements.clone());
        let nodes: Vec<(OccurrenceId, u32, PackElementNode)> = order
            .iter()
            .filter(|id| id.copy == spread.call.copy)
            .filter_map(|id| {
                PackElementNode::of(id.syntax)
                    .filter(|(parent, _, _)| *parent == spread.spread.syntax)
                    .map(|(_, index, node)| (*id, index, node))
            })
            .collect();
        if nodes.len() != elements.len() * 4 {
            return Err(REFUSAL);
        }
        for (id, index, node) in nodes {
            let Some(element) = usize::try_from(index)
                .ok()
                .and_then(|index| elements.get(index))
            else {
                return Err(REFUSAL);
            };
            if fact_at(&facts.expression_types, id).is_some() {
                return Err(REFUSAL);
            }
            let (ty, place) = match node {
                PackElementNode::Collector => {
                    insert_ranked(
                        &mut facts.expression_bindings,
                        order,
                        (
                            id,
                            mojito_checked::templates::TemplateOwner::Param(spread.param),
                        ),
                    );
                    (collector.clone(), true)
                }
                PackElementNode::Index => (Ty::IntLiteral, false),
                PackElementNode::Element => (element.clone(), true),
                PackElementNode::Transfer => {
                    insert_ranked(&mut facts.transfers, order, id);
                    (element.clone(), false)
                }
            };
            if place {
                insert_ranked(&mut facts.expression_place_types, order, (id, ty.clone()));
            }
            insert_ranked(&mut facts.expression_types, order, (id, ty));
        }
        insert_ranked(&mut facts.overload_targets, order, (spread.call, tuple));
    }
    Ok(())
}

/// A lane comparison's mask local declared at `declaration` from the
/// comparison `value`, at an instance whose lane compares to a `Bool`: its
/// binding, recorded at the value, and every read of it.
fn retype_mask_local(
    facts: &mut CheckedBodyFacts,
    declaration: OccurrenceId,
    value: OccurrenceId,
) -> Result<(), &'static str> {
    if fact_at(&facts.binding_types, value) != Some(&lane_mask()) {
        return Err("a lane comparison's local is not bound to its mask");
    }
    set_fact(&mut facts.binding_types, value, Ty::Bool);
    let owner = fact_at(&facts.statement_bindings, declaration)
        .cloned()
        .ok_or("a lane comparison's local has no binding")?;
    let reads: Vec<OccurrenceId> = facts
        .expression_bindings
        .iter()
        .filter(|(_, bound)| *bound == owner)
        .map(|(id, _)| *id)
        .collect();
    for read in reads {
        for table in [
            &mut facts.expression_types,
            &mut facts.expression_place_types,
        ] {
            if fact_at(table, read) == Some(&lane_mask()) {
                set_fact(table, read, Ty::Bool);
            }
        }
    }
    Ok(())
}

/// Insert `entry` into a table kept in the occurrences' pre-order `order`.
fn insert_ranked<E: Ranked>(table: &mut Vec<E>, order: &[OccurrenceId], entry: E) {
    let rank = |id: OccurrenceId| order.iter().position(|entry| *entry == id);
    let at = rank(entry.occurrence());
    let position = table
        .iter()
        .position(|existing| rank(existing.occurrence()) > at)
        .unwrap_or(table.len());
    table.insert(position, entry);
}

/// A fact table's entry, keyed by the occurrence it was recorded at.
trait Ranked {
    fn occurrence(&self) -> OccurrenceId;
}

impl Ranked for OccurrenceId {
    fn occurrence(&self) -> OccurrenceId {
        *self
    }
}

impl<V> Ranked for (OccurrenceId, V) {
    fn occurrence(&self) -> OccurrenceId {
        self.0
    }
}

/// The index binder of the pack element a type names, directly or as a
/// reference's referent (`Ts[i]`, `ref Ts[i]`).
fn element_index_binder(ty: &Ty) -> Option<mojito_types::param_expr::ParamId> {
    let dependent = match ty {
        Ty::Dependent(dependent) => dependent,
        Ty::Ref(reference) => match &*reference.referent {
            Ty::Dependent(dependent) => dependent,
            _ => return None,
        },
        _ => return None,
    };
    let (_, index) = dependent.pack_element()?;
    match index.kind() {
        mojito_types::param_expr::ParamKind::DeclRef(reference) => Some(reference.id.clone()),
        _ => None,
    }
}

/// Whether every occurrence of `construction`'s loop copy that
/// [`loop_element_indices`] fixes at `binder` takes the same types at
/// `first` as at each of `rivals`: the copy then cannot tell those indices
/// apart, where it reads only the constructed pack at that index, and does
/// where it also reads another pack there.
fn index_indifferent(
    template: &CheckedBodyFacts,
    substitution: &InstanceSubstitution,
    occurrences: &[Occurrence],
    construction: OccurrenceId,
    binder: &mojito_types::param_expr::ParamId,
    (first, rivals): (&mojito_types::ct::CtValue, &[mojito_types::ct::CtValue]),
) -> bool {
    let copies = |syntax: SyntaxId| {
        occurrences
            .iter()
            .filter(|occurrence| occurrence.id.syntax == syntax)
            .count()
    };
    let count = copies(construction.syntax);
    let references = template
        .reference_binding_types
        .iter()
        .chain(&template.reference_place_types)
        .map(|(id, reference)| (id.syntax, &reference.referent));
    let typed: Vec<(SyntaxId, &Ty)> = template
        .expression_types
        .iter()
        .chain(&template.expression_place_types)
        .chain(&template.binding_types)
        .map(|(id, ty)| (id.syntax, ty))
        .chain(references)
        .filter(|(syntax, _)| {
            copies(*syntax) == count
                && occurrences.iter().any(|occurrence| {
                    occurrence.id.syntax == *syntax && occurrence.id.copy == construction.copy
                })
        })
        .collect();
    let fixed: HashSet<SyntaxId> = typed
        .iter()
        .filter(|(_, ty)| element_index_binder(ty).as_ref() == Some(binder))
        .map(|(syntax, _)| *syntax)
        .collect();
    let at = |candidate: &mojito_types::ct::CtValue| -> Vec<Ty> {
        let values: Vec<_> = std::iter::once((binder.clone(), candidate.clone()))
            .chain(substitution.values.iter().cloned())
            .collect();
        typed
            .iter()
            .filter(|(syntax, _)| fixed.contains(syntax))
            .map(|(_, ty)| {
                mojito_types::types::substitute_packs(
                    &fold_binder_views(ty, &substitution.views),
                    &substitution.types,
                    &substitution.packs,
                    &values,
                )
            })
            .collect()
    };
    let expected = at(first);
    rivals.iter().all(|candidate| at(candidate) == expected)
}

/// The tables an element's default construction may record in, each carried
/// into the instance as its own check wrote it
/// ([`Checker::element_construction_facts`]).
const fn element_construction_table(table: FactTable) -> bool {
    matches!(
        table,
        FactTable::ExpressionTypes
            | FactTable::OperationAdjustments
            | FactTable::OverloadTargets
            | FactTable::ConstructionImmutableBinders
            | FactTable::SimdConstructions
            | FactTable::UnconsumedTemporaries
    )
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

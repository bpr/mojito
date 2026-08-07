//! Program, block, and statement checking: the top-level `check_program`
//! entry, block scoping, the `check_stmt` statement dispatcher, and parameter
//! type lowering. Extracted from `checker.rs`; see `docs/symbol-map.md`.

use super::*;

impl Checker {
    pub fn check_program(&mut self, stmts: &[Stmt]) -> Result<(), TypeError> {
        self.declared_structs
            .extend(stmts.iter().filter_map(|statement| match &statement.kind {
                StmtKind::Struct { name, .. } => Some(name.clone()),
                _ => None,
            }));
        // Phase one for generated public Tuples: recover every concrete pack
        // identity from its materialized `element_types` member before any
        // declaration body is checked. Reverse transforms can be requested in
        // both directions, so no sequential declaration order can make both
        // result types complete. The forward-type gate remains compiler-owned;
        // user declarations are still checked in source order below.
        let saved_forward_types =
            std::mem::replace(&mut self.allow_generated_tuple_forward_types, true);
        for statement in stmts {
            let StmtKind::Struct {
                name,
                type_params,
                associated,
                ..
            } = &statement.kind
            else {
                continue;
            };
            if !(name.starts_with("Tuple$") || name.contains("$Tuple$")) {
                continue;
            }
            let saved_type_params =
                std::mem::replace(&mut self.enclosing_type_params, type_params.clone());
            let arguments = self.generated_tuple_arguments(name, associated);
            self.enclosing_type_params = saved_type_params;
            if let Some(arguments) = arguments? {
                self.predeclared_generated_tuple_arguments
                    .insert(name.clone(), arguments);
            }
        }
        self.allow_generated_tuple_forward_types = saved_forward_types;
        // Same-module declarations resolve order-independently: every
        // top-level struct registers its shell (name and parameters), then
        // every trait, then every struct's field/associated types, then every
        // struct's method signatures — all before the source-order walk below
        // checks conformance and method bodies. A struct field or method
        // signature may therefore reference a struct declared later in its
        // module (an iterator holding `ref[o] List[T]` above `List` itself).
        for statement in stmts {
            let Some(declaration) = struct_declaration(statement) else {
                continue;
            };
            self.check_struct_shell(&declaration)?;
            self.predeclared_structs
                .insert(declaration.name.to_string());
        }
        for statement in stmts {
            let StmtKind::Trait {
                name,
                refines,
                methods,
                comptime_members,
            } = &statement.kind
            else {
                continue;
            };
            self.check_trait(name, refines, methods, comptime_members)?;
            self.predeclared_traits.insert(name.clone());
        }
        for statement in stmts {
            let Some(declaration) = struct_declaration(statement) else {
                continue;
            };
            self.check_struct_types(&declaration)?;
        }
        for statement in stmts {
            let Some(declaration) = struct_declaration(statement) else {
                continue;
            };
            self.check_struct_method_signatures(&declaration)?;
        }
        // `ret = None` marks "not inside a function", so a top-level `return`
        // is rejected; `in_loop = false` likewise rejects a top-level `break`.
        self.check_block(stmts, None, false)
    }

    /// Check the statements of a block in the current scope. `ret` is the
    /// enclosing function's declared return type (or `None` at module level);
    /// `in_loop` is true inside a `while`/`for` body (gating `break`/`continue`).
    pub(super) fn check_block(
        &mut self,
        stmts: &[Stmt],
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<(), TypeError> {
        for stmt in stmts {
            self.check_stmt(stmt, ret, in_loop)?;
        }
        Ok(())
    }

    /// Check a block in a fresh nested scope (the body of an `if`/`elif`/`else`
    /// or loop). The new scope is popped before returning.
    pub(super) fn check_scoped_block(
        &mut self,
        stmts: &[Stmt],
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<(), TypeError> {
        self.push_scope();
        let result = self.check_block(stmts, ret, in_loop);
        self.pop_scope();
        result
    }

    /// Select the in-place dunder (`__iadd__`, …) for `receiver OP= value` on a
    /// user-defined `operand_ty`, returning its complete checked method-call
    /// contract. The contract (and the call's effects/conversions) is keyed at
    /// `span`. A missing in-place dunder is a hard error: Mojo dispatches
    /// augmented assignment to the dedicated method and does not fall back to the
    /// ordinary binary operator. `__iadd__` returns `None`; a value-returning
    /// method is rejected.
    pub(super) fn select_inplace_operator(
        &mut self,
        span: SourceSpan,
        receiver: &Expr,
        op: InfixOp,
        value: &Expr,
        operand_ty: &Ty,
    ) -> Result<crate::checked::CheckedCallContract, TypeError> {
        let missing = || TypeError::MissingInPlaceOperator {
            op: infix_symbol(op).to_string(),
            ty: operand_ty.to_string(),
        };
        let Ty::Struct(name, _) = operand_ty else {
            return Err(missing());
        };
        let dunder = op.inplace_dunder().ok_or_else(missing)?;
        let has_method = self
            .structs
            .get(name)
            .and_then(|info| info.methods.get(dunder))
            .is_some();
        if !has_method {
            return Err(missing());
        }
        let args = std::slice::from_ref(value);
        let ret = self.infer_method_call(
            span.clone(),
            receiver,
            dunder,
            MethodCallArguments::ordinary(args, &[]),
        )?;
        if ret != Ty::None {
            return Err(TypeError::TypeMismatch {
                expected: "None".to_string(),
                found: ret.to_string(),
                context: format!("in-place operator '{dunder}'"),
            });
        }
        let contract = self
            .selected_calls
            .borrow()
            .get(&span)
            .cloned()
            .ok_or_else(|| {
                TypeError::InvariantViolation(
                    "in-place operator lost its selected call contract".to_string(),
                )
            })?;
        // The receiver is a place, not a computed call. Keep the selected
        // contract solely in the `AugmentedInPlace` record so lowering treats the
        // receiver as a place (write-back through `recv_place`) rather than as a
        // reference-returning call result keyed at this span.
        self.selected_calls.borrow_mut().remove(&span);
        self.overload_targets.borrow_mut().remove(&span);
        self.generic_instantiations.borrow_mut().remove(&span);
        Ok(contract)
    }

    /// Select the in-place dunder for a nominal-subscript element
    /// (`c[i] += v` → `element.__iadd__(v)`). The receiver is a fresh mutable
    /// temporary typed as the element, declared in a throwaway scope, so
    /// selection reuses `select_inplace_operator` without re-inferring the
    /// subscript `place` — which would disturb the getter/setter adjustment state
    /// this branch carefully snapshots. The RHS conversion is recorded on the real
    /// `value` span and survives the scope pop; lowering materializes the element
    /// into its own temporary and applies the returned contract.
    pub(super) fn select_subscript_inplace_operator(
        &mut self,
        place: &Expr,
        op: InfixOp,
        value: &Expr,
        operand_ty: &Ty,
    ) -> Result<crate::checked::CheckedCallContract, TypeError> {
        self.push_scope();
        let selection = self
            .declare("$inplace_elem", operand_ty.clone())
            .and_then(|()| {
                let mut receiver = Expr::new(
                    ExprKind::Identifier("$inplace_elem".to_string()),
                    place.span,
                );
                receiver.source = place.source.clone();
                self.select_inplace_operator(
                    receiver.source_span(),
                    &receiver,
                    op,
                    value,
                    operand_ty,
                )
            });
        self.pop_scope();
        selection
    }

    /// The store-outward escape rule shared by ordinary place assignment and
    /// unpack-into-place: a store whose destination roots at outliving
    /// storage (a parameter or `self`) must not smuggle a frame-local loan
    /// outward — the symmetric twin of the Return escape check. Rebinding a
    /// `ref` destination counts even when the value type carries no loan:
    /// the destination handle itself becomes the loan. An accepted store
    /// records its transfer effect for call-site replay.
    pub(super) fn check_outward_store(
        &self,
        place: &Expr,
        value: &Expr,
        found: &Ty,
        storage: &Option<Ty>,
    ) -> Result<(), TypeError> {
        if let Some((_, allowed)) = self.aggregate_escape_contexts.last()
            && place_root_name(place)
                .and_then(|root| self.lookup_owner(root))
                .is_some_and(|owner| allowed.contains(&owner))
            && (self.type_carries_loans(found) || matches!(storage, Some(Ty::Ref(_))))
        {
            let mut origins = self.aggregate_origins(value);
            // A rebinding store's loan roots at the right-hand place
            // itself, which a plain value expression does not surface
            // as an aggregate origin.
            if matches!(storage, Some(Ty::Ref(_)))
                && let Ok(rhs_place) = self.origin_place(value)
            {
                origins.push(crate::origin::Origin::Place(rhs_place));
            }
            if origins
                .iter()
                .any(|origin| self.aggregate_origin_escapes(origin))
            {
                return Err(TypeError::StoredReferenceEscapesOrigin);
            }
            // Accepted: every origin is caller-visible. Record the
            // transfer so later call sites install the caller-side
            // loan this store implies.
            self.record_transfer_effect(place, &origins, storage);
        }
        Ok(())
    }

    pub(super) fn check_stmt(
        &mut self,
        stmt: &Stmt,
        ret: Option<&Ty>,
        in_loop: bool,
    ) -> Result<(), TypeError> {
        match &stmt.kind {
            StmtKind::RefDecl { name, value } => {
                let reference = match self.infer(value)? {
                    Ty::Ref(reference) => reference,
                    _ => {
                        // Ordinary expression inference reads through a
                        // reference-returning method to its referent.  The
                        // method call nevertheless left an exact checked
                        // `ReferenceResult` adjustment, which a `ref` binding
                        // must retain instead of trying to reinterpret the call
                        // expression as a syntactic place. `reference_actual`
                        // consumes that checked result and remains the shared
                        // fallback for an ordinary place binding.
                        self.reference_actual(value)?
                    }
                };
                let mutable = reference.mutability == crate::origin::Mutability::Mutable;
                // A reference to an ordinary projection below a named owned
                // interior (for example `dict[key].field`) carries the full
                // projected generation, even though only the nested index
                // expression originally introduced the interior fact. Retain
                // that canonical origin on the binding expression so MIR can
                // establish the projected generation rather than degrading it
                // to an untracked ordinary place loan.
                if let crate::origin::Origin::Place(origin) = &reference.origin
                    && origin
                        .path
                        .iter()
                        .any(|segment| matches!(segment, crate::origin::OriginSeg::Interior(_)))
                {
                    self.interior_references
                        .borrow_mut()
                        .insert(value.source_span(), origin.clone());
                }
                self.reference_value_uses
                    .borrow_mut()
                    .insert(value.source_span(), mutable);
                let binding_ty = Ty::Ref(reference);
                self.binding_types
                    .borrow_mut()
                    .insert(stmt.source_span(), binding_ty.clone());
                self.declare_with_mutability(name, binding_ty, mutable)?;
                self.record_statement_binding(stmt, name);
                Ok(())
            }
            StmtKind::VarDecl { name, ty, value } => {
                if matches!(value.kind, ExprKind::Uninitialized) {
                    let Some(annotation) = ty else {
                        return Err(TypeError::Unsupported(
                            "an uninitialized variable requires a type annotation".to_string(),
                        ));
                    };
                    let declared = self.ty_from_anno(annotation)?;
                    self.declare(name, declared)?;
                    self.record_statement_binding(stmt, name);
                    if let Some(owner) = self.lookup_owner(name) {
                        self.uninitialized.borrow_mut().insert(owner);
                    }
                    return Ok(());
                }
                self.register_named_bindings(value)?;
                let contextual = ty
                    .as_ref()
                    .map(|annotation| self.ty_from_anno(annotation))
                    .transpose()?;
                let found = match contextual.as_ref() {
                    Some(expected) => self.infer_with_expected(value, expected, true)?,
                    None => self.infer(value)?,
                };
                self.check_consuming(value, &found, &format!("variable '{name}'"))?;
                let declared = match ty {
                    // Annotated: the value must coerce to the annotation.
                    Some(anno) => {
                        let expected = contextual.clone().unwrap_or(self.ty_from_anno(anno)?);
                        if contains_infer(&expected) {
                            if contains_infer(&found) {
                                return Err(TypeError::CannotInferTypeParam {
                                    name: expected.to_string(),
                                    param: "_".to_string(),
                                });
                            }
                            found.clone()
                        } else {
                            if !self.record_implicit_conversion(value, &found, &expected)? {
                                return Err(TypeError::TypeMismatch {
                                    expected: expected.to_string(),
                                    found: found.to_string(),
                                    context: format!("variable '{}'", name),
                                });
                            }
                            expected
                        }
                    }
                    // Inferred `var x = e`: declare the value's materialized type.
                    None if matches!(value.kind, ExprKind::TypeApply { .. })
                        && self
                            .overload_targets
                            .borrow()
                            .contains_key(&value.source_span()) =>
                    {
                        // Explicitly specializing an Origin parameter produces
                        // a first-class, non-capturing function value whose
                        // checked `Ty::Func` contains the bound origin. Plain
                        // inferred function/closure values remain non-escaping.
                        found.clone()
                    }
                    None => self.inferred_binding_ty(&found, name)?,
                };
                if ty.is_none() {
                    self.record_literal_materializations(value, &found, &declared)?;
                }
                self.binding_types
                    .borrow_mut()
                    .insert(value.source_span(), declared.clone());
                if self.is_implicitly_deletable(&declared) {
                    self.explicit_destroy_deletability
                        .borrow_mut()
                        .bindings
                        .insert(value.source_span());
                }
                let (aggregate_origins, aggregate_field_origins) =
                    if !matches!(declared, Ty::Ref(_)) && self.type_may_carry_loans(&declared) {
                        (
                            self.aggregate_origins(value),
                            self.aggregate_field_origins(value),
                        )
                    } else {
                        (Vec::new(), HashMap::new())
                    };
                self.declare(name, declared)?;
                self.record_statement_binding(stmt, name);
                self.set_aggregate_origins(name, aggregate_origins);
                self.set_aggregate_field_origins(name, aggregate_field_origins);
                Ok(())
            }

            StmtKind::Assign { name, value } => {
                self.register_named_bindings(value)?;
                // `_ = e` discards the value: evaluate it so any move/effect still
                // registers, but introduce no binding and require none.
                if name == "_" {
                    let found = self.infer(value)?;
                    self.check_consuming(value, &found, "discard assignment")?;
                    return Ok(());
                }
                self.check_capture_access(name, true)?;
                let found = self.infer(value)?;
                self.check_consuming(value, &found, &format!("assignment to '{name}'"))?;
                // Mojo treats a bare assignment in a function as a local
                // introduction unless that name is already local to this
                // function. Its initializer may still read an outer binding.
                let target = if let Some(&base) = self.function_bases.last() {
                    self.scopes[base..]
                        .iter()
                        .rev()
                        .find_map(|s| s.get(name))
                        .cloned()
                        .or_else(|| {
                            // A mutable captured variable is updated by reference;
                            // an immutable capture (notably `comptime`) is instead
                            // shadowed by a new function-local binding.
                            let mutable = self.mutable_scopes[..base]
                                .iter()
                                .rev()
                                .find_map(|s| s.get(name))
                                .copied()
                                .unwrap_or(false);
                            if mutable {
                                self.scopes[..base]
                                    .iter()
                                    .rev()
                                    .find_map(|s| s.get(name))
                                    .cloned()
                            } else {
                                None
                            }
                        })
                } else {
                    self.lookup(name).cloned()
                };
                match target {
                    // Re-assignment: the value must keep the variable's type.
                    Some(target) => {
                        if !self.is_binding_mutable(name) {
                            return Err(TypeError::ImmutableBinding(name.clone()));
                        }
                        match &target {
                            // Assignment to a reference binding writes through
                            // the handle; it does not rebind the handle.  Use
                            // the reference's checked origin so replacing a
                            // whole container also expires references into its
                            // owned interior regions.  The handle itself stays
                            // valid across that write.
                            Ty::Ref(reference) => {
                                if let crate::origin::Origin::Place(base) = &reference.origin {
                                    self.record_origin_invalidation(
                                        value.source_span(),
                                        base.clone(),
                                        self.lookup_owner(name),
                                    );
                                }
                            }
                            _ => {
                                if let Some(owner) = self.lookup_owner(name) {
                                    self.record_owner_invalidation(
                                        value.source_span(),
                                        owner,
                                        Vec::new(),
                                    );
                                }
                            }
                        }
                        let (aggregate_origins, aggregate_field_origins) = if !matches!(
                            target,
                            Ty::Ref(_)
                        ) && self
                            .type_may_carry_loans(&target)
                        {
                            (
                                self.aggregate_origins(value),
                                self.aggregate_field_origins(value),
                            )
                        } else {
                            (Vec::new(), HashMap::new())
                        };
                        let target = match target {
                            Ty::Ref(reference) => *reference.referent,
                            other => other,
                        };
                        // Assigning a closure could move it to an outer binding.
                        if matches!(
                            found,
                            Ty::Func { .. } | Ty::GenericFunc { .. } | Ty::Overload(_)
                        ) {
                            return Err(TypeError::ClosureEscape);
                        }
                        if !self.record_implicit_conversion(value, &found, &target)? {
                            return Err(TypeError::TypeMismatch {
                                expected: target.to_string(),
                                found: found.to_string(),
                                context: format!("assignment to '{}'", name),
                            });
                        }
                        if let Some(owner) = self.lookup_owner(name) {
                            self.uninitialized.borrow_mut().remove(&owner);
                        }
                        self.set_aggregate_origins(name, aggregate_origins);
                        self.set_aggregate_field_origins(name, aggregate_field_origins);
                        Ok(())
                    }
                    // Mojito requires `var` to introduce a new binding; a bare
                    // assignment to a name not in scope is an error.
                    None => Err(TypeError::AssignToUndeclared(name.clone())),
                }
            }

            StmtKind::AugAssign { place, op, value } => {
                if let Some(root) = place_root_name(place) {
                    self.check_capture_access(root, true)?;
                }
                let nominal_subscript = match &place.kind {
                    ExprKind::Index { object, .. }
                    | ExprKind::Slice { object, .. }
                    | ExprKind::MultiIndex { object, .. } => {
                        matches!(self.infer(object)?, Ty::Struct(name, _) if self.structs.contains_key(&name))
                    }
                    _ => false,
                };
                // `target OP= value` means `target = target OP value`: the place
                // must be writable, and the result of the operator must keep the
                // place's type. A nominal subscript is two selected calls, not a
                // raw writable projection: infer its getter first, then select
                // the setter against the computed operator result.
                let shared_argument_sources = if nominal_subscript {
                    match &place.kind {
                        ExprKind::Index { index, .. } => vec![index.source_span()],
                        ExprKind::MultiIndex { args, .. } => args
                            .iter()
                            .filter_map(|argument| match argument {
                                SubscriptArg::Index(index) => Some(index.source_span()),
                                SubscriptArg::Slice { .. } => None,
                            })
                            .collect(),
                        ExprKind::Slice { .. } => Vec::new(),
                        _ => unreachable!("nominal subscript classification"),
                    }
                } else {
                    Vec::new()
                };
                let shared_adjustments = self.snapshot_value_adjustments(&shared_argument_sources);
                let target = if nominal_subscript {
                    self.infer(place)?
                } else {
                    self.check_place(place)?
                };
                let getter = if nominal_subscript {
                    let contract = self
                        .selected_calls
                        .borrow()
                        .get(&place.source_span())
                        .cloned()
                        .ok_or_else(|| {
                            TypeError::InvariantViolation(
                                "augmented nominal subscript lost its getter contract".to_string(),
                            )
                        })?;
                    if let Some(reference) = &contract.reference_result
                        && reference.mutability != crate::origin::Mutability::Mutable
                    {
                        return Err(TypeError::ImmutableBinding(
                            "immutable reference returned by '__getitem__'".to_string(),
                        ));
                    }
                    Some(contract)
                } else {
                    None
                };
                if !nominal_subscript {
                    self.record_place_write_invalidation(place.source_span(), place);
                }
                if let Some(Ty::Ref(reference)) = self.place_storage_ty(place)
                    && reference.mutability != crate::origin::Mutability::Mutable
                {
                    return Err(TypeError::ImmutableBinding(
                        "immutable reference field".to_string(),
                    ));
                }
                // A user-defined value dispatches augmented assignment to its
                // dedicated in-place dunder (`x += y` → `x.__iadd__(y)`), which
                // mutates `mut self`; Mojo does not fall back to the binary
                // operator. Native scalars keep the builtin operator path below.
                if !nominal_subscript
                    && matches!(&target, Ty::Struct(name, _) if self.structs.contains_key(name))
                {
                    let contract = self.select_inplace_operator(
                        place.source_span(),
                        place,
                        *op,
                        value,
                        &target,
                    )?;
                    self.operation_adjustments.borrow_mut().insert(
                        place.source_span(),
                        crate::checked::SemanticAdjustment::AugmentedInPlace(Box::new(contract)),
                    );
                    return Ok(());
                }
                // A user-struct subscript element dispatches augmented assignment
                // to its in-place dunder, exactly like a variable or field target;
                // a native element keeps the binary operator + setter path. The
                // mutated element keeps its own type, so `result` is the element
                // type and the value-getter's `__setitem__` selection binds it.
                let (result, inplace_contract) = if nominal_subscript
                    && matches!(&target, Ty::Struct(name, _) if self.structs.contains_key(name))
                {
                    let contract =
                        self.select_subscript_inplace_operator(place, *op, value, &target)?;
                    (target.clone(), Some(contract))
                } else {
                    let result = self.infer_infix(None, *op, place, value)?;
                    if !coerces(&result, &target) {
                        return Err(TypeError::TypeMismatch {
                            expected: target.to_string(),
                            found: result.to_string(),
                            context: "augmented assignment".to_string(),
                        });
                    }
                    (result, None)
                };
                if nominal_subscript {
                    let site = place.source_span();
                    let getter = getter.expect("nominal getter was captured above");
                    if getter.reference_result.is_some() {
                        // A value-returning augmented subscript reaches the
                        // setter checker below, which also records descriptor
                        // shape. A mutable-reference getter has no setter, so a
                        // plain index must retain that metadata here for MIR.
                        if matches!(place.kind, ExprKind::Index { .. }) {
                            self.subscript_descriptors
                                .borrow_mut()
                                .entry(site.clone())
                                .or_insert((vec![None], false));
                        }
                        self.expression_types
                            .borrow_mut()
                            .insert(site.clone(), target.clone());
                        self.operation_adjustments.borrow_mut().insert(
                            site,
                            crate::checked::SemanticAdjustment::AugmentedSubscript(Box::new(
                                crate::checked::CheckedAugmentedSubscript {
                                    getter,
                                    setter: None,
                                    inplace: inplace_contract.clone(),
                                    operand_ty: target,
                                    result_ty: result,
                                    value_source: None,
                                },
                            )),
                        );
                        return Ok(());
                    }
                    // The getter and setter share syntax, not parameter
                    // adaptation or call effects. Freeze the getter above, then
                    // return those source-keyed compatibility tables to their
                    // pre-getter state before selecting the setter.
                    self.restore_value_adjustments(&shared_adjustments);
                    self.remove_call_boundary_invalidations(&site, &getter.boundary);

                    let mut computed = Expr::new(ExprKind::None, crate::token::DUMMY_SPAN);
                    computed.source = place.source.clone();
                    let value_source = computed.source_span();
                    self.expression_types
                        .borrow_mut()
                        .insert(value_source.clone(), result.clone());
                    self.check_nominal_subscript_assignment(place, &computed)?
                        .ok_or_else(|| {
                            TypeError::InvariantViolation(
                                "augmented nominal subscript lost its setter selection".to_string(),
                            )
                        })?;
                    let mut setter = self
                        .selected_calls
                        .borrow()
                        .get(&site)
                        .cloned()
                        .ok_or_else(|| {
                            TypeError::InvariantViolation(
                                "augmented nominal subscript lost its setter contract".to_string(),
                            )
                        })?;
                    self.implicit_conversions.borrow_mut().remove(&value_source);
                    self.expression_types.borrow_mut().remove(&value_source);
                    if matches!(
                        self.operation_adjustments.borrow().get(&value_source),
                        Some(crate::checked::SemanticAdjustment::MaterializeLiteral(_))
                    ) {
                        self.operation_adjustments
                            .borrow_mut()
                            .remove(&value_source);
                    }
                    let before_write = self
                        .interior_invalidations
                        .borrow()
                        .get(&site)
                        .cloned()
                        .unwrap_or_default();
                    self.record_place_write_invalidation(site.clone(), place);
                    if let Some(after_write) = self.interior_invalidations.borrow().get(&site) {
                        let existing = setter.boundary.invalidations.clone();
                        let additional = after_write
                            .iter()
                            .filter(|fact| !before_write.contains(fact))
                            .filter(|fact| !existing.contains(fact))
                            .cloned()
                            .collect::<Vec<_>>();
                        setter.boundary.invalidations.extend(additional);
                    }
                    self.restore_value_adjustments(&shared_adjustments);
                    self.remove_call_boundary_invalidations(&site, &setter.boundary);
                    self.selected_calls
                        .borrow_mut()
                        .insert(site.clone(), setter.clone());
                    self.expression_types
                        .borrow_mut()
                        .insert(site.clone(), target.clone());
                    self.operation_adjustments.borrow_mut().insert(
                        site,
                        crate::checked::SemanticAdjustment::AugmentedSubscript(Box::new(
                            crate::checked::CheckedAugmentedSubscript {
                                getter,
                                setter: Some(setter),
                                inplace: inplace_contract,
                                operand_ty: target,
                                result_ty: result,
                                value_source: Some(value_source),
                            },
                        )),
                    );
                }
                Ok(())
            }

            // Tuple unpacking `a, b = t`: `t` must be a tuple of matching arity; each
            // target (a NAME or a place) receives the corresponding element type. A
            // NAME follows the assignment rules (re-assign if in scope, else a
            // var-less introduction).
            StmtKind::Unpack {
                targets,
                value,
                declares,
            } => {
                let vt = self.infer(value)?;
                let Some(elems) = tuple_elements(&vt) else {
                    return Err(TypeError::TypeMismatch {
                        expected: "a tuple".to_string(),
                        found: vt.to_string(),
                        context: "tuple unpacking".to_string(),
                    });
                };
                if elems.len() != targets.len() {
                    return Err(TypeError::TypeMismatch {
                        expected: format!("a {}-element tuple", targets.len()),
                        found: vt.to_string(),
                        context: "tuple unpacking".to_string(),
                    });
                }
                let elems = elems.into_iter().cloned().collect::<Vec<_>>();
                let mut unpack_plan = elems
                    .iter()
                    .cloned()
                    .map(|ty| crate::checked::CheckedTupleUnpackElement {
                        ty,
                        accessor: None,
                        reference: None,
                    })
                    .collect::<Vec<_>>();
                if let Ty::Struct(name, _) = &vt
                    && let Some(info) = self.structs.get(name)
                    && let Some(family) = dependent_index_accessor_family(info)
                {
                    let value_receiver = !is_place_expr(value);
                    let self_reference = if value_receiver {
                        None
                    } else {
                        Some(self.reference_actual(value)?)
                    };
                    for (index, element) in unpack_plan.iter_mut().enumerate() {
                        let method = if value_receiver {
                            format!("{}${index}", family.value)
                        } else {
                            format!("{}${index}", family.place)
                        };
                        let signature = info
                            .methods
                            .get(&method)
                            .and_then(|overloads| overloads.first())
                            .ok_or_else(|| {
                                if value_receiver {
                                    TypeError::NonCopyable {
                                        ty: vt.to_string(),
                                        context: "unpacking an rvalue Tuple requires implicitly copyable elements"
                                            .to_string(),
                                    }
                                } else {
                                    TypeError::InvariantViolation(format!(
                                        "generated Tuple '{name}' is missing accessor '{method}'"
                                    ))
                                }
                            })?;
                        element.ty = signature.ret.clone();
                        element.accessor = Some(format!("{name}.{method}"));
                        if let Some(reference_return) = &signature.ref_return {
                            let self_reference = self_reference.as_ref().ok_or_else(|| {
                                TypeError::InvariantViolation(format!(
                                    "generated Tuple value accessor '{method}' returns a reference"
                                ))
                            })?;
                            let origin = substitute_sig_origin_with_self(
                                &reference_return.origin,
                                &[],
                                Some(self_reference.origin.clone()),
                            );
                            let mutability = match reference_return.mutability {
                                crate::origin::SigMutability::Immutable => {
                                    crate::origin::Mutability::Immutable
                                }
                                crate::origin::SigMutability::Mutable => {
                                    crate::origin::Mutability::Mutable
                                }
                                _ if self_reference.mutability
                                    == crate::origin::Mutability::Mutable =>
                                {
                                    crate::origin::Mutability::Mutable
                                }
                                _ => crate::origin::Mutability::Immutable,
                            };
                            element.reference = Some(crate::origin::RefTy {
                                referent: Box::new(signature.ret.clone()),
                                origin,
                                mutability,
                            });
                        }
                    }
                }
                self.tuple_unpack_plans
                    .borrow_mut()
                    .insert(value.source_span(), unpack_plan);
                for (index, (target, elem)) in targets.iter().zip(&elems).enumerate() {
                    match &target.kind {
                        // `_` discards its element (no binding, no declaration).
                        ExprKind::Identifier(name) if name == "_" => {}
                        // `var a, b = e` declares each target; a bare `a, b = e`
                        // requires every target already in scope.
                        ExprKind::Identifier(name) if *declares => {
                            let declared = self.inferred_binding_ty(elem, name)?;
                            self.declare(name, declared)?;
                        }
                        ExprKind::Identifier(name) => match self.lookup(name).cloned() {
                            Some(existing) => {
                                self.check_capture_access(name, true)?;
                                if !self.is_binding_mutable(name) {
                                    return Err(TypeError::ImmutableBinding(name.clone()));
                                }
                                if !coerces(elem, &existing) {
                                    return Err(TypeError::TypeMismatch {
                                        expected: existing.to_string(),
                                        found: elem.to_string(),
                                        context: format!("unpacking into '{name}'"),
                                    });
                                }
                                if matches!(existing, Ty::Ref(_)) {
                                    self.record_place_write_invalidation(
                                        target.source_span(),
                                        target,
                                    );
                                } else if let Some(owner) = self.lookup_owner(name) {
                                    self.record_owner_invalidation(
                                        target.source_span(),
                                        owner,
                                        Vec::new(),
                                    );
                                }
                            }
                            None => return Err(TypeError::AssignToUndeclared(name.clone())),
                        },
                        _ => {
                            if let Some(root) = place_root_name(target) {
                                self.check_capture_access(root, true)?;
                            }
                            let target_ty = self.check_place(target)?;
                            self.record_place_write_invalidation(target.source_span(), target);
                            if !coerces(elem, &target_ty) {
                                return Err(TypeError::TypeMismatch {
                                    expected: target_ty.to_string(),
                                    found: elem.to_string(),
                                    context: "unpacking into a place".to_string(),
                                });
                            }
                            // Store-outward twin for unpack-into-place. A
                            // loan-carrying element cannot currently reach
                            // here (rvalue unpack demands implicitly copyable
                            // elements and place targets type-match exactly),
                            // so this is defensive coverage that keeps the
                            // boundary closed if those restrictions loosen.
                            let element_value = match &value.kind {
                                ExprKind::TupleLit(items) if items.len() == targets.len() => {
                                    &items[index]
                                }
                                _ => value,
                            };
                            let storage = Some(target_ty.clone());
                            self.check_outward_store(target, element_value, elem, &storage)?;
                        }
                    }
                    if let ExprKind::Identifier(name) = &target.kind
                        && let Some(owner) = self.lookup_owner(name)
                    {
                        self.expression_bindings
                            .borrow_mut()
                            .insert(target.source_span(), owner);
                        // Unpack targets are binder/place syntax, not ordinary
                        // inferred expressions, so retain their complete slot
                        // metadata explicitly for checked MIR verification and
                        // for a nested closure that captures the introduction.
                        if let Some(storage_ty) = self.lookup(name).cloned() {
                            let value_ty = match &storage_ty {
                                Ty::Ref(reference) => (*reference.referent).clone(),
                                value => value.clone(),
                            };
                            self.expression_types
                                .borrow_mut()
                                .insert(target.source_span(), value_ty);
                            self.expression_place_types
                                .borrow_mut()
                                .insert(target.source_span(), storage_ty.clone());
                            self.binding_types
                                .borrow_mut()
                                .insert(target.source_span(), storage_ty);
                        }
                    }
                }
                Ok(())
            }

            // `with` blocks parse, but the context-manager (`__enter__`/`__exit__`)
            // protocol is deferred — flagged, like the other parse-only constructs.
            StmtKind::With { .. } => Err(TypeError::Unsupported("with statement".to_string())),

            StmtKind::SetPlace { place, value } => {
                if let Some(root) = place_root_name(place) {
                    self.check_capture_access(root, true)?;
                }
                // Nominal subscript assignment is a method call, not a raw
                // storage projection. Resolve it with the RHS present so
                // overloads, conversions, ownership conventions, origins,
                // aliases, captures, and raising behavior all use the same
                // machinery as explicit `.__setitem__(...)` syntax.
                if self
                    .check_nominal_subscript_assignment(place, value)?
                    .is_some()
                {
                    self.record_place_write_invalidation(place.source_span(), place);
                    return Ok(());
                }
                // The place must be a writable location (a field/index chain
                // rooted at a mutable variable or `mut self`); the value must
                // keep the place's type. A width-1 SIMD target (a lane write, or
                // a scalar-alias field) additionally accepts a splatting literal.
                let target = self.check_place(place)?;
                self.record_place_write_invalidation(place.source_span(), place);
                let storage = self.place_storage_ty(place);
                let found = self.infer(value)?;
                // A store into storage that outlives this frame (a parameter
                // or `self` root) must not smuggle a frame-local loan outward
                // — the symmetric twin of the Return escape check. Rebinding a
                // `ref` field counts even when the value type carries no loan:
                // the destination handle itself becomes the loan. Nominal
                // subscript assignment (early-returned above) needs no twin:
                // a `List[ref T]` subscript reaches the referent only
                // (augmented writes), and handle replacement or ref-append is
                // not offered by any overload, so no collection store can
                // install a frame-local handle.
                self.check_outward_store(place, value, &found, &storage)?;
                if let Some(Ty::Ref(expected_reference)) = &storage {
                    let initializes_reference =
                        self.self_initializing && place_root_name(place) == Some("self");
                    if initializes_reference {
                        let actual = self.infer_reference_value(value).ok_or_else(|| {
                            TypeError::TypeMismatch {
                                expected: format!("ref {}", expected_reference.referent),
                                found: found.to_string(),
                                context: "reference field initialization".to_string(),
                            }
                        })?;
                        if !coerces(&actual.referent, &expected_reference.referent)
                            || (expected_reference.mutability == crate::origin::Mutability::Mutable
                                && actual.mutability != crate::origin::Mutability::Mutable)
                        {
                            return Err(TypeError::TypeMismatch {
                                expected: format!("ref {}", expected_reference.referent),
                                found: format!("ref {}", actual.referent),
                                context: "reference field initialization".to_string(),
                            });
                        }
                        self.reference_value_uses.borrow_mut().insert(
                            value.source_span(),
                            expected_reference.mutability == crate::origin::Mutability::Mutable,
                        );
                    } else if expected_reference.mutability != crate::origin::Mutability::Mutable {
                        return Err(TypeError::ImmutableBinding(
                            "immutable reference field".to_string(),
                        ));
                    } else {
                        // A write-through replaces the referent's value: run
                        // the referent's consuming/copy analysis, which
                        // out-self initialization deliberately skips — a `^`
                        // transfer stays a move, and a Copyable place read
                        // records its copy so the lowering runs the lifecycle
                        // instead of sharing the source's storage.
                        self.check_consuming(value, &found, "assignment target")?;
                    }
                }
                let ok = self.record_implicit_conversion(value, &found, &target)?;
                if !ok {
                    return Err(TypeError::TypeMismatch {
                        expected: target.to_string(),
                        found: found.to_string(),
                        context: "assignment target".to_string(),
                    });
                }
                // Existing out-self initialization permits a non-Copyable
                // generic parameter to be installed without spelling a second
                // source-level transfer. Preserve that constructor convention;
                // this marker is needed only for an actual Copyable place read.
                if !matches!(storage, Some(Ty::Ref(_))) && self.is_copyable(&found) {
                    self.check_consuming(value, &found, "assignment target")?;
                }
                Ok(())
            }

            // `raises` is parsed but its effect is not analyzed (deferred).
            StmtKind::Def {
                name,
                type_params,
                params,
                positional_only,
                keyword_only,
                captures,
                ret: ret_anno,
                body,
                raises,
                raises_type,
                decorators,
                where_clause,
            } => {
                if self.structs.contains_key(name) {
                    return Err(TypeError::Redeclaration(name.clone()));
                }
                // Free functions, including generic functions, share one binder
                // for regular, `*args`, and homogeneous `**kwargs` parameters.
                if let Some(feature) = Self::advanced_param_feature(
                    params,
                    *positional_only,
                    *keyword_only,
                    false,
                    false,
                    false,
                ) {
                    return Err(TypeError::Unsupported(feature.to_string()));
                }
                // A `*args` variadic is supported on non-generic functions; any
                // regular parameters after it are keyword-only.
                let variadic_idx = params
                    .iter()
                    .position(|p| p.kind == crate::ast::ParamKind::Variadic);
                let kw_variadic_idx = params
                    .iter()
                    .position(|p| p.kind == crate::ast::ParamKind::KwVariadic);
                // Regular (non-variadic) parameters, over which arity is computed.
                let regular: Vec<&crate::ast::FnParam> = params
                    .iter()
                    .filter(|p| p.kind == crate::ast::ParamKind::Regular)
                    .collect();
                let out_params: Vec<_> = regular
                    .iter()
                    .copied()
                    .filter(|p| matches!(p.convention, Some(crate::ast::ArgConvention::Out)))
                    .collect();
                if out_params.len() > 1 {
                    return Err(TypeError::Unsupported(
                        "multiple named 'out' results".to_string(),
                    ));
                }
                let named_result = out_params.first().copied();
                if named_result.is_some() && ret_anno.is_some() {
                    return Err(TypeError::Unsupported(
                        "a function cannot declare both a named result and '->' return type"
                            .to_string(),
                    ));
                }
                let caller_regular: Vec<_> = regular
                    .iter()
                    .copied()
                    .filter(|p| !matches!(p.convention, Some(crate::ast::ArgConvention::Out)))
                    .collect();
                let pos_only = regular_marker_index(params, *positional_only);
                let kw_only = effective_keyword_only_index(params, *keyword_only, variadic_idx);
                let required = required_mask(&caller_regular, kw_only)?;
                self.validate_origin_signature(type_params, params, None)?;
                let mut decls = self.classify_params(type_params)?;
                let mut function_assumptions = HashSet::new();
                if let Some(condition) = where_clause {
                    let constraint = self.compile_generic_constraint(condition)?;
                    let mut facts = Vec::new();
                    guaranteed_conformance_atoms(&constraint, &mut facts);
                    function_assumptions.extend(facts.into_iter().map(
                        |(parameter, trait_name)| {
                            (parameter.trim_start_matches('*').to_string(), trait_name)
                        },
                    ));
                    let Some(last) = decls.last_mut() else {
                        return Err(TypeError::Unsupported(
                            "a where clause requires compile-time parameters".to_string(),
                        ));
                    };
                    match last {
                        ParamDecl::Type { constraints, .. }
                        | ParamDecl::Value { constraints, .. } => constraints.push(constraint),
                    }
                }
                self.generic_parameters.borrow_mut().insert(
                    crate::checked::GenericSite::Function {
                        module: stmt.module.clone(),
                        declaration: stmt.span,
                        syntax: stmt.syntax_id,
                    },
                    decls.clone(),
                );
                // Type parameters are in scope while resolving the signature and
                // checking the body (as bare `T`).
                self.tparams.push(type_scope(&decls));

                let signature = (|| {
                    let param_tys = self.param_tys(params)?;
                    let ret_ty = match (ret_anno, named_result) {
                        (Some(SourceType::Ref { referent, .. }), _) => {
                            self.ty_from_anno(referent)?
                        }
                        (Some(t), _) => self.ty_from_anno(t)?,
                        (None, Some(result)) => self.ty_from_anno(&result.ty)?,
                        (None, None) => Ty::None,
                    };
                    Ok::<_, TypeError>((param_tys, ret_ty))
                })();
                let (param_tys, ret_ty) = match signature {
                    Ok(sig) => sig,
                    Err(e) => {
                        self.tparams.pop();
                        return Err(e);
                    }
                };
                let ref_params = lower_ref_param_sigs(type_params, &caller_regular)?;
                let ref_return = match ret_anno {
                    Some(SourceType::Ref { origin, .. }) => Some(lower_ref_sig(
                        origin.as_ref().ok_or_else(|| {
                            TypeError::Unsupported(
                                "reference return requires an origin".to_string(),
                            )
                        })?,
                        type_params,
                        &regular,
                    )?),
                    _ => None,
                };
                for (param, ty) in param_tys.iter().enumerate() {
                    self.declaration_types.borrow_mut().insert(
                        crate::checked::AnnotationSite::FunctionParam {
                            module: stmt.module.clone(),
                            declaration: stmt.span,
                            syntax: stmt.syntax_id,
                            param,
                        },
                        ty.clone(),
                    );
                }
                self.declaration_types.borrow_mut().insert(
                    crate::checked::AnnotationSite::FunctionReturn {
                        module: stmt.module.clone(),
                        declaration: stmt.span,
                        syntax: stmt.syntax_id,
                    },
                    ret_ty.clone(),
                );
                // A default value must fit its parameter's type.
                for (p, pty) in params.iter().zip(&param_tys) {
                    if let Some(d) = &p.default {
                        let dty = match self.infer(d) {
                            Ok(t) => t,
                            Err(e) => {
                                self.tparams.pop();
                                return Err(e);
                            }
                        };
                        if !coerces(&dty, pty) {
                            self.tparams.pop();
                            return Err(TypeError::TypeMismatch {
                                expected: pty.to_string(),
                                found: dty.to_string(),
                                context: format!("default value of '{}'", p.name),
                            });
                        }
                    }
                }

                // Bind the function in the enclosing scope before checking its
                // body, so it can call itself (recursion). A generic `def`
                // becomes a `GenericFunc` (its call sites infer/supply parameters).
                let declared_error = self.declared_error(*raises, raises_type.as_ref())?;
                let effect_raises = declared_error.as_ref().is_some_and(|ty| *ty != Ty::Never);
                let parameter_closure = decorators
                    .iter()
                    .any(|decorator| decorator.path.len() == 1 && decorator.path[0] == "parameter");
                let initial_environment = if parameter_closure {
                    crate::origin::CallableEnvironment::Capturing(
                        crate::origin::CaptureOriginSet::Infer,
                    )
                } else if self.function_bases.is_empty() {
                    crate::origin::CallableEnvironment::Thin
                } else {
                    crate::origin::CallableEnvironment::Default
                };
                self.declaration_effects.borrow_mut().insert(
                    crate::checked::AnnotationSite::FunctionReturn {
                        module: stmt.module.clone(),
                        declaration: stmt.span,
                        syntax: stmt.syntax_id,
                    },
                    crate::checked::DeclarationEffect {
                        raises: effect_raises,
                        error: effect_raises.then(|| declared_error.clone()).flatten(),
                        returns_reference: ref_return.is_some(),
                    },
                );
                let fn_ty = if decls.is_empty() {
                    let regular_tys: Vec<Ty> = params
                        .iter()
                        .zip(&param_tys)
                        .filter(|(p, _)| {
                            p.kind == crate::ast::ParamKind::Regular
                                && !matches!(p.convention, Some(crate::ast::ArgConvention::Out))
                        })
                        .map(|(_, ty)| ty.clone())
                        .collect();
                    Ty::Func {
                        environment: initial_environment.clone(),
                        params: regular_tys,
                        names: caller_regular.iter().map(|p| p.name.clone()).collect(),
                        ret: Box::new(ret_ty.clone()),
                        required,
                        variadic: variadic_idx.map(|vi| Box::new(param_tys[vi].clone())),
                        kw_variadic: kw_variadic_idx
                            .map(|index| Box::new(param_tys[index].clone())),
                        positional_only: pos_only,
                        keyword_only: kw_only,
                        raises: effect_raises,
                        error: declared_error.clone().map(Box::new),
                        conventions: caller_regular.iter().map(|p| p.convention).collect(),
                        ref_params: Box::new(ref_params.clone()),
                        ref_return: ref_return.clone().map(Box::new),
                    }
                } else {
                    let regular_tys: Vec<Ty> = params
                        .iter()
                        .zip(&param_tys)
                        .filter(|(p, _)| {
                            p.kind == crate::ast::ParamKind::Regular
                                && !matches!(p.convention, Some(crate::ast::ArgConvention::Out))
                        })
                        .map(|(_, ty)| ty.clone())
                        .collect();
                    Ty::GenericFunc {
                        environment: initial_environment,
                        decls: decls.clone(),
                        params: regular_tys,
                        names: caller_regular.iter().map(|p| p.name.clone()).collect(),
                        ret: Box::new(ret_ty.clone()),
                        required,
                        variadic: variadic_idx.map(|vi| Box::new(param_tys[vi].clone())),
                        kw_variadic: kw_variadic_idx
                            .map(|index| Box::new(param_tys[index].clone())),
                        positional_only: pos_only,
                        keyword_only: kw_only,
                        raises: effect_raises,
                        error: declared_error.clone().map(Box::new),
                        conventions: caller_regular.iter().map(|p| p.convention).collect(),
                        ref_params: Box::new(ref_params.clone()),
                        ref_return: ref_return.clone().map(Box::new),
                    }
                };
                self.declaration_types.borrow_mut().insert(
                    crate::checked::AnnotationSite::FunctionType {
                        module: stmt.module.clone(),
                        declaration: stmt.span,
                        syntax: stmt.syntax_id,
                    },
                    fn_ty.clone(),
                );
                if let Err(e) = self.declare(name, fn_ty.clone()) {
                    self.tparams.pop();
                    return Err(e);
                }
                self.record_statement_binding(stmt, name);
                self.register_callable_origins(
                    name,
                    callable_origin_signature(type_params, &caller_regular),
                );
                let capture_policy = if self.function_bases.is_empty() {
                    if captures.is_some() {
                        self.tparams.pop();
                        return Err(TypeError::Unsupported(
                            "unified capture lists are valid only on nested functions".to_string(),
                        ));
                    }
                    None
                } else {
                    let mut entries = HashMap::new();
                    let mut checked_captures = Vec::new();
                    if let Some(captures) = captures {
                        for capture in &captures.entries {
                            if let Err(error) = self.check_capture_access(&capture.name, false) {
                                self.tparams.pop();
                                return Err(error);
                            }
                            let Some(scope) = self.binding_scope(&capture.name) else {
                                self.tparams.pop();
                                return Err(TypeError::UndefinedVariable(capture.name.clone()));
                            };
                            if scope == 0 {
                                self.tparams.pop();
                                return Err(TypeError::Unsupported(format!(
                                    "module binding '{}' is not a closure capture",
                                    capture.name
                                )));
                            }
                            if capture.kind == crate::ast::CaptureKind::Mut
                                && !self.is_binding_mutable(&capture.name)
                            {
                                self.tparams.pop();
                                return Err(TypeError::ImmutableBinding(capture.name.clone()));
                            }
                            if let Err(error) =
                                self.check_capture_capability(&capture.name, capture.kind)
                            {
                                self.tparams.pop();
                                return Err(error);
                            }
                            let binding = self.lookup_owner(&capture.name).ok_or_else(|| {
                                TypeError::InvariantViolation(format!(
                                    "capture '{}' lost its checked binding",
                                    capture.name
                                ))
                            })?;
                            let ty = self.lookup(&capture.name).cloned().ok_or_else(|| {
                                TypeError::InvariantViolation(format!(
                                    "capture '{}' lost its checked storage type",
                                    capture.name
                                ))
                            })?;
                            checked_captures.push(self.checked_capture(
                                &capture.name,
                                binding,
                                ty,
                                capture.kind,
                            ));
                            entries.insert(capture.name.clone(), capture.kind);
                        }
                    }
                    self.declaration_captures
                        .borrow_mut()
                        .insert(stmt.source_span(), checked_captures);
                    Some(CapturePolicy {
                        base: self.scopes.len(),
                        function_name: name.clone(),
                        declaration: stmt.source_span(),
                        entries,
                        default: captures
                            .as_ref()
                            .and_then(|list| list.default)
                            .or_else(|| parameter_closure.then_some(crate::ast::CaptureKind::Read)),
                    })
                };
                self.assumed_conformances.push(function_assumptions);
                for (param, ty) in param_tys.iter().enumerate() {
                    if self.is_implicitly_deletable(ty) {
                        self.explicit_destroy_deletability
                            .borrow_mut()
                            .declarations
                            .insert(crate::checked::AnnotationSite::FunctionParam {
                                module: stmt.module.clone(),
                                declaration: stmt.span,
                                syntax: stmt.syntax_id,
                                param,
                            });
                    }
                }
                self.push_scope();
                self.function_bases.push(self.scopes.len() - 1);
                if let Some(policy) = capture_policy {
                    self.capture_contexts.borrow_mut().push(policy);
                }
                self.raising_context.push(declared_error);
                let mut result = Ok(());
                // Value parameters are ordinary `Int` locals in the body.
                for d in &decls {
                    if let ParamDecl::Value { name, ty, .. } = d {
                        result = self.declare_immutable(
                            name.trim_start_matches('*'),
                            if matches!(d, ParamDecl::Value { variadic: true, .. }) {
                                Ty::VariadicPack(ty.clone())
                            } else {
                                (**ty).clone()
                            },
                        );
                        if result.is_err() {
                            break;
                        }
                    }
                }
                if result.is_ok() {
                    for (param, ty) in params.iter().zip(&param_tys) {
                        // A `*args` parameter is compiler pack storage inside the
                        // body; it must not impersonate the nominal stdlib List.
                        let bind_ty = match param.kind {
                            crate::ast::ParamKind::Variadic => match ty {
                                Ty::RuntimePack(elements) => Ty::Tuple(elements.clone()),
                                _ => Ty::VariadicPack(Box::new(ty.clone())),
                            },
                            crate::ast::ParamKind::KwVariadic => self.kwargs_collector_ty(
                                ty.clone(),
                                &format!("keyword collector '{}'", param.name),
                            )?,
                            crate::ast::ParamKind::Regular => ty.clone(),
                        };
                        // Duplicate parameter names are a redeclaration.
                        result = self.declare_with_mutability(
                            &param.name,
                            bind_ty.clone(),
                            param.kind == crate::ast::ParamKind::KwVariadic
                                || matches!(param.convention, Some(crate::ast::ArgConvention::Out))
                                || ref_parameter_is_writable(param, type_params),
                        );
                        if result.is_ok()
                            && matches!(param.convention, Some(crate::ast::ArgConvention::Ref))
                        {
                            self.register_reference_parameter(
                                &param.name,
                                bind_ty.clone(),
                                ref_parameter_is_writable(param, type_params),
                            );
                        }
                        if result.is_ok()
                            && !matches!(bind_ty, Ty::Ref(_))
                            && self.type_carries_loans(&bind_ty)
                            && let Some(owner) = self.lookup_owner(&param.name)
                        {
                            self.set_aggregate_origins(
                                &param.name,
                                vec![crate::origin::Origin::Place(crate::origin::OriginPlace {
                                    root: owner,
                                    path: Vec::new(),
                                })],
                            );
                        }
                        if result.is_err() {
                            break;
                        }
                    }
                }
                // A function body is a fresh loop context: `break`/`continue`
                // do not cross into a nested `def`.
                if result.is_ok() {
                    let owners: Vec<_> = caller_regular
                        .iter()
                        .map(|param| {
                            self.lookup_owner(&param.name)
                                .expect("bound function parameter")
                        })
                        .collect();
                    let base = *self
                        .function_bases
                        .last()
                        .expect("function scope is active");
                    let mut allowed: std::collections::HashSet<_> =
                        owners.iter().copied().collect();
                    // Variadic collectors are parameters too (see the method
                    // twin in declarations.rs).
                    allowed.extend(
                        params
                            .iter()
                            .filter(|param| param.kind != crate::ast::ParamKind::Regular)
                            .filter_map(|param| self.lookup_owner(&param.name)),
                    );
                    // A nested def can reach the enclosing callable's outliving
                    // storage (`self`, parameters) through captures; stores
                    // into those owners face the same store-outward rule, with
                    // this def's own frame deciding source locality.
                    if let Some((_, enclosing)) = self.aggregate_escape_contexts.last() {
                        allowed.extend(enclosing.iter().copied());
                    }
                    self.aggregate_escape_contexts.push((base, allowed));
                    self.transfer_frames.borrow_mut().push(TransferFrame {
                        callable: name.clone(),
                        param_owners: owners.clone(),
                        param_borrowed: caller_regular
                            .iter()
                            .map(|param| {
                                matches!(
                                    param.convention,
                                    Some(
                                        crate::ast::ArgConvention::Mut
                                            | crate::ast::ArgConvention::Ref
                                    )
                                )
                            })
                            .collect(),
                        self_owner: None,
                        effects: Vec::new(),
                    });
                    self.raise_observation_frames
                        .borrow_mut()
                        .push((self.handled_raise_depth, false));
                    self.return_ref_contracts.push(
                        ref_return
                            .clone()
                            .map(|signature| (signature, owners, None)),
                    );
                    self.named_result_context.push(named_result.is_some());
                    result = self.check_block(body, Some(&ret_ty), false);
                    self.named_result_context.pop();
                    self.return_ref_contracts.pop();
                    self.raise_observation_frames.borrow_mut().pop();
                    if let Some(frame) = self.transfer_frames.borrow_mut().pop()
                        && !frame.effects.is_empty()
                    {
                        self.transfer_effects
                            .borrow_mut()
                            .insert(frame.callable, frame.effects);
                    }
                    self.aggregate_escape_contexts.pop();
                }
                // A function with a non-`None` return type must return on every
                // path (falling off the end would yield `None`).
                if result.is_ok()
                    && named_result.is_none()
                    && ret_ty != Ty::None
                    && !definitely_returns(body)
                {
                    result = Err(TypeError::MissingReturn(name.clone()));
                }
                if result.is_ok()
                    && let Some(named_result) = named_result
                    && !definitely_initializes_named_result(body, &named_result.name)
                {
                    result = Err(TypeError::MissingReturn(name.clone()));
                }
                if result.is_ok() {
                    let captures = self
                        .declaration_captures
                        .borrow()
                        .get(&stmt.source_span())
                        .cloned()
                        .unwrap_or_default();
                    let concrete = crate::origin::CaptureOriginSet::concrete(
                        captures
                            .iter()
                            .flat_map(|capture| capture.origins.iter().cloned()),
                    );
                    let environment = if parameter_closure || !captures.is_empty() {
                        crate::origin::CallableEnvironment::Capturing(concrete)
                    } else {
                        crate::origin::CallableEnvironment::Thin
                    };
                    let finalized = with_callable_environment(fn_ty.clone(), environment);
                    let function_scope = *self
                        .function_bases
                        .last()
                        .expect("function scope remains active through finalization");
                    if let Some(existing) = function_scope
                        .checked_sub(1)
                        .and_then(|scope| self.scopes.get_mut(scope))
                        .and_then(|scope| scope.get_mut(name))
                        && !matches!(existing, Ty::Overload(_))
                    {
                        *existing = finalized.clone();
                    }
                    self.declaration_types.borrow_mut().insert(
                        crate::checked::AnnotationSite::FunctionType {
                            module: stmt.module.clone(),
                            declaration: stmt.span,
                            syntax: stmt.syntax_id,
                        },
                        finalized,
                    );
                }
                self.pop_scope();
                self.function_bases.pop();
                if !self.function_bases.is_empty() {
                    self.capture_contexts.borrow_mut().pop();
                }
                self.raising_context.pop();
                self.assumed_conformances.pop();
                self.tparams.pop();
                result
            }

            StmtKind::Struct {
                name,
                type_params,
                conforms,
                callable_conformance,
                conformance_conditions,
                fields,
                associated,
                methods,
                fieldwise_init,
                decorators,
            } => {
                if self.lookup(name).is_some() {
                    return Err(TypeError::Redeclaration(name.clone()));
                }
                let declaration = StructDeclaration {
                    module: &stmt.module,
                    name,
                    type_params,
                    conforms,
                    callable_conformance,
                    conformance_conditions,
                    fields,
                    associated,
                    methods,
                    fieldwise_init: *fieldwise_init,
                    decorators,
                };
                if self.predeclared_structs.remove(name) {
                    self.check_struct_completion(&declaration)
                } else {
                    self.check_struct(&declaration)
                }
            }

            StmtKind::Trait {
                name,
                refines,
                methods,
                comptime_members,
            } => {
                if self.predeclared_traits.remove(name) {
                    Ok(())
                } else {
                    self.check_trait(name, refines, methods, comptime_members)
                }
            }

            StmtKind::Comptime { name, value } => {
                // A comptime `Int` is recorded (for value-parameter use) and bound as
                // `Int`. A richer comptime value (tuple/list/string) the `Int` folder
                // can't evaluate is still an ordinary binding — the elaborator has
                // already consumed it for any `comptime for`/`comptime if`.
                match self.eval_ct(value) {
                    Ok(v) => {
                        self.comptimes.insert(name.clone(), v);
                        self.declare_immutable(name, Ty::IntLiteral)?;
                    }
                    Err(_) => {
                        let ty = self.infer(value)?;
                        let declared = self.inferred_binding_ty(&ty, name)?;
                        self.declare_immutable(name, declared)?;
                    }
                }
                self.record_statement_binding(stmt, name);
                Ok(())
            }

            // `comptime if` / `comptime for` parse and are grammar-documented, but
            // compile-time branch selection / loop unrolling is deferred — flagged
            // here, like the other syntax-first parse-only constructs.
            StmtKind::ComptimeIf { .. } => Err(TypeError::Unsupported("comptime if".to_string())),
            StmtKind::ComptimeFor { .. } => Err(TypeError::Unsupported("comptime for".to_string())),

            StmtKind::If { branches, orelse } => {
                let before = self.uninitialized.borrow().clone();
                let mut exits = Vec::new();
                // Definite initialization follows only reachable exits when a
                // condition is a compile-time Bool literal. We still check every
                // source branch for type errors, but `if True: x = ...` establishes
                // a function-scoped implicit binding just as an unconditional
                // assignment does. Unknown conditions retain both the taken and
                // fallthrough possibilities.
                let mut fallthrough_reachable = true;
                for (cond, body) in branches {
                    *self.uninitialized.borrow_mut() = before.clone();
                    self.register_named_bindings(cond)?;
                    self.expect_bool(cond, "if condition")?;
                    self.check_scoped_block(body, ret, in_loop)?;
                    let condition = match &cond.kind {
                        ExprKind::Bool(value) => Some(*value),
                        _ => None,
                    };
                    if fallthrough_reachable && condition != Some(false) {
                        exits.push(self.uninitialized.borrow().clone());
                    }
                    if condition == Some(true) {
                        fallthrough_reachable = false;
                    }
                }
                if let Some(body) = orelse {
                    *self.uninitialized.borrow_mut() = before.clone();
                    self.check_scoped_block(body, ret, in_loop)?;
                    if fallthrough_reachable {
                        exits.push(self.uninitialized.borrow().clone());
                    }
                } else if fallthrough_reachable {
                    exits.push(before);
                }
                *self.uninitialized.borrow_mut() =
                    exits.into_iter().flatten().collect::<HashSet<_>>();
                Ok(())
            }

            StmtKind::While { cond, body, orelse } => {
                let before = self.uninitialized.borrow().clone();
                self.register_named_bindings(cond)?;
                self.expect_bool(cond, "while condition")?;
                self.check_scoped_block(body, ret, true)?;
                *self.uninitialized.borrow_mut() = before;
                if let Some(body) = orelse {
                    self.check_scoped_block(body, ret, in_loop)?;
                }
                Ok(())
            }

            // `raise` — its operand must be an `Error` (or a `String`, the
            // shorthand). The raises *effect* (that this must be in a `raises`
            // function or a `try`) is deliberately not analyzed.
            StmtKind::Raise(expr) => {
                self.register_named_bindings(expr)?;
                let ty = self.infer(expr)?;
                let error = if ty == Ty::String { Ty::Error } else { ty };
                self.require_error("'raise'", error)?;
                Ok(())
            }

            // Imports are parsed but not resolved (no module system yet), so
            // they are a checker no-op — imported names are not made available.
            StmtKind::Import { .. } | StmtKind::FromImport { .. } => Ok(()),

            StmtKind::Try {
                body,
                except,
                orelse,
                finalbody,
            } => {
                if except.is_some() {
                    self.handled_raise_depth += 1;
                    self.handled_raise_types.borrow_mut().push(Vec::new());
                }
                let body_result = self.check_scoped_block(body, ret, in_loop);
                let handled_types = except.as_ref().map(|_| {
                    self.handled_raise_types
                        .borrow_mut()
                        .pop()
                        .unwrap_or_default()
                });
                if except.is_some() {
                    self.handled_raise_depth -= 1;
                }
                body_result?;
                if let Some((name, ex_body)) = except {
                    self.push_scope();
                    let error = handled_types
                        .as_ref()
                        .and_then(|types| types.first().cloned())
                        .unwrap_or(Ty::Error);
                    if handled_types
                        .as_ref()
                        .is_some_and(|types| types.iter().any(|candidate| *candidate != error))
                    {
                        self.pop_scope();
                        return Err(TypeError::RaiseTypeMismatch {
                            expected: error.to_string(),
                            found: "multiple error types in one try block".to_string(),
                        });
                    }
                    let result = match name {
                        Some(n) => self.declare(n, error.clone()).and_then(|()| {
                            // The exception target is a real lexical binding. Its
                            // owner is attached to the containing `try` syntax
                            // because the AST stores the target as a name rather
                            // than as an expression node.
                            self.record_statement_binding(stmt, n);
                            self.binding_types
                                .borrow_mut()
                                .insert(stmt.source_span(), error.clone());
                            self.check_block(ex_body, ret, in_loop)
                        }),
                        None => self.check_block(ex_body, ret, in_loop),
                    };
                    self.pop_scope();
                    result?;
                }
                if let Some(body) = orelse {
                    self.check_scoped_block(body, ret, in_loop)?;
                }
                if let Some(body) = finalbody {
                    self.check_scoped_block(body, ret, in_loop)?;
                }
                Ok(())
            }

            StmtKind::For {
                var,
                binding,
                iter,
                body,
                orelse,
            } => {
                self.register_named_bindings(iter)?;
                // The loop variable's type comes from the iterable: `Int` for a
                // `range`, the element type for a `List`, or — for a user struct —
                // the element type of its `__iter__()` iterator (`__next__`'s return).
                let iter_ty = self.infer(iter)?;
                let source_mode = Self::iteration_mode(iter);
                let (mut yielded_ty, mut protocol) =
                    self.iteration_protocol(&iter_ty, source_mode)?;
                self.attach_borrowed_iteration_origin(iter, &iter_ty, source_mode, &mut protocol);
                if let Some(resolved) =
                    self.resolve_borrowed_iteration_reference(iter, &mut protocol)
                {
                    yielded_ty = resolved;
                }
                if *binding == crate::ast::LoopBindingMode::Ref
                    && Self::is_abstract_iteration_dispatch(&protocol)
                {
                    return Err(TypeError::Unsupported(
                        "`for ref` over a generic Iterable bound requires a reference-yielding iterator; the abstract Iterator.__next__ yields Element values — bind by value, or iterate the concrete collection"
                            .to_string(),
                    ));
                }
                let binding_plan = self.iteration_binding_plan(*binding, &yielded_ty)?;
                if source_mode == crate::checked::IterationMode::Owned
                    && binding_plan.action == crate::checked::IterationBindingAction::MoveValue
                    && !self.is_implicitly_deletable(&binding_plan.binding_ty)
                    && block_can_escape_owned_iteration(body, 0)
                {
                    // Name the element's declared obligation so the rejection
                    // says what each residual element still requires.
                    let obligation = self.residual_obligation_suffix(&binding_plan.binding_ty);
                    return Err(TypeError::Unsupported(format!(
                        "owned iteration over non-ImplicitlyDeletable '{}' cannot exit early; its residual elements would require explicit destruction{obligation}",
                        binding_plan.binding_ty
                    )));
                }
                // A linear-element body must also be free of unhandled raising
                // calls: the syntactic walk above sees only `raise` statements,
                // while a propagating call error abandons the residuals just
                // the same. Observed while the body checks (a `try`-handled
                // call is contained and does not mark).
                let linear_element = (source_mode == crate::checked::IterationMode::Owned
                    && binding_plan.action == crate::checked::IterationBindingAction::MoveValue
                    && !self.is_implicitly_deletable(&binding_plan.binding_ty))
                .then(|| binding_plan.binding_ty.clone());
                let binding_ty = binding_plan.binding_ty.clone();
                protocol.binding = Some(Box::new(binding_plan.clone()));
                self.iteration_protocols
                    .borrow_mut()
                    .insert(iter.source_span(), protocol);
                self.push_scope();
                self.binding_types
                    .borrow_mut()
                    .insert(stmt.source_span(), binding_ty.clone());
                if self.is_implicitly_deletable(&binding_ty) {
                    self.explicit_destroy_deletability
                        .borrow_mut()
                        .bindings
                        .insert(stmt.source_span());
                }
                if linear_element.is_some() {
                    let baseline = self.handled_raise_depth;
                    self.raise_observation_frames
                        .borrow_mut()
                        .push((baseline, false));
                }
                let result = (|| {
                    self.declare_with_mutability(var, binding_ty, binding_plan.mutable)?;
                    self.record_statement_binding(stmt, var);
                    self.check_block(body, ret, true)
                })();
                self.pop_scope();
                let raised = linear_element.is_some()
                    && self
                        .raise_observation_frames
                        .borrow_mut()
                        .pop()
                        .is_some_and(|(_, flag)| flag);
                result?;
                if raised && let Some(element) = linear_element {
                    let obligation = self.residual_obligation_suffix(&element);
                    return Err(TypeError::Unsupported(format!(
                        "owned iteration over non-ImplicitlyDeletable '{element}' cannot contain an unhandled raising call; a propagating error would abandon its residual elements{obligation}",
                    )));
                }
                if let Some(body) = orelse {
                    self.check_scoped_block(body, ret, in_loop)?;
                }
                Ok(())
            }

            StmtKind::Break => {
                if in_loop {
                    Ok(())
                } else {
                    Err(TypeError::BreakOutsideLoop)
                }
            }

            StmtKind::Continue => {
                if in_loop {
                    Ok(())
                } else {
                    Err(TypeError::ContinueOutsideLoop)
                }
            }

            StmtKind::Return(expr) => {
                if let Some(expression) = expr {
                    self.register_named_bindings(expression)?;
                }
                let expected = match ret {
                    Some(ty) => ty,
                    None => return Err(TypeError::ReturnOutsideFunction),
                };
                let found = match expr {
                    Some(e) => self.infer_with_expected(e, expected, true)?,
                    None if self.named_result_context.last() == Some(&true) => expected.clone(),
                    None => Ty::None,
                };
                if let Some(expression) = expr
                    && !matches!(expected, Ty::Ref(_))
                    && (self.type_may_carry_loans(expected) || self.type_may_carry_loans(&found))
                    && self
                        .aggregate_origins(expression)
                        .iter()
                        .any(|origin| self.aggregate_origin_escapes(origin))
                {
                    // A bare pointer return gets the pointer-specific
                    // diagnostic; an aggregate may mix reference and pointer
                    // loans and keeps the reference wording.
                    if matches!(found, Ty::Pointer { .. }) {
                        return Err(TypeError::PointerEscapesOrigin);
                    }
                    return Err(TypeError::ReturnsReferenceToLocal);
                }
                if let (Some(e), Some(Some((signature, parameter_owners, self_owner)))) =
                    (expr, self.return_ref_contracts.last())
                {
                    let actual = match self.returned_reference_parameter_origin(e) {
                        Some(origin) => origin,
                        None => self.reference_actual(e)?.origin,
                    };
                    let parameter_origins: Vec<_> = parameter_owners
                        .iter()
                        .map(|owner| {
                            Some(crate::origin::Origin::Place(crate::origin::OriginPlace {
                                root: *owner,
                                path: Vec::new(),
                            }))
                        })
                        .collect();
                    let allowed = substitute_sig_origin_with_self(
                        &signature.origin,
                        &parameter_origins,
                        self_owner.clone().map(crate::origin::Origin::Place),
                    );
                    // Bundled collections intentionally abstract their private
                    // backing storage behind a public owned-interior region:
                    // List bridges its raw pointer slot to `element`, while Dict
                    // bridges its entries List to the replace-on-lookup `value`
                    // generation. Callers inherit the public region, never the
                    // implementation storage origin. Keep this privilege at the
                    // return boundary alongside the private List take/destroy
                    // gate, and restrict it to compiler-shipped source paths.
                    let bundled_collection_interior_bridge =
                        self.self_ty.as_ref().is_some_and(|ty| {
                            list_element(ty).is_some() || dict_elements(ty).is_some()
                        }) && is_bundled_collection_source(e.source.as_deref());
                    if !origin_is_within(&actual, &allowed) && !bundled_collection_interior_bridge {
                        return Err(TypeError::ReturnsReferenceToLocal);
                    }
                }
                // `expected` is the referent type for a reference-returning
                // declaration; the reference contract is retained separately.
                // Returning a place through that checked contract borrows it —
                // it does not copy/consume the referent.
                let returning_reference = self
                    .return_ref_contracts
                    .last()
                    .is_some_and(Option::is_some);
                if let Some(e) = expr
                    && !returning_reference
                {
                    self.check_consuming(e, &found, "return value")?;
                }
                // Returning a callable *value* is an escape; returning a
                // checked reference to callable storage is not. The latter is
                // how a nominal Tuple's indexed accessor exposes a function
                // element while the Tuple owner remains live.
                if !returning_reference
                    && matches!(
                        found,
                        Ty::Func { .. } | Ty::GenericFunc { .. } | Ty::Overload(_)
                    )
                {
                    return Err(TypeError::ClosureEscape);
                }
                let compatible = match expr {
                    Some(expression) => {
                        self.record_implicit_conversion(expression, &found, expected)?
                    }
                    None => self.value_coerces(&found, expected),
                };
                if !compatible {
                    return Err(TypeError::TypeMismatch {
                        expected: expected.to_string(),
                        found: found.to_string(),
                        context: "return".to_string(),
                    });
                }
                Ok(())
            }

            StmtKind::Pass => Ok(()),

            StmtKind::Expr(expr) => {
                self.register_named_bindings(expr)?;
                self.infer(expr)?;
                Ok(())
            }
        }
    }

    /// Resolve a parameter/field list to its types.
    pub(super) fn param_tys(&self, params: &[crate::ast::FnParam]) -> Result<Vec<Ty>, TypeError> {
        params
            .iter()
            .map(|parameter| {
                if matches!(
                    &parameter.ty,
                    SourceType::Func { type_params, .. } if !type_params.is_empty()
                ) {
                    return Err(TypeError::Unsupported(
                        "a runtime parameter cannot use a parametric function type; declare it as a compile-time callable parameter"
                            .to_string(),
                    ));
                }
                self.ty_from_anno(&parameter.ty)
            })
            .collect()
    }
}

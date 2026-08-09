//! Expression type inference core: the `infer`/`infer_impl` dispatch, reference-
//! and storage-value inference, comprehension and named-binding checking, and
//! list/tuple/variant construction inference. Extracted from `checker.rs`;
//! see `docs/symbol-map.md`.

use super::*;

impl Checker {
    /// Check a comprehension in its own lexical scope and cache its result type
    /// for ordinary read-only expression inference. Clauses are visited in
    /// source order, so a later iterable/filter sees earlier bindings while the
    /// produced key/value sees all generator bindings.
    pub(super) fn check_comprehension(&mut self, expression: &Expr) -> Result<(), TypeError> {
        let ExprKind::Comprehension {
            kind,
            key,
            value,
            clauses,
        } = &expression.kind
        else {
            return Ok(());
        };

        let scope_base = self.scopes.len();
        let mut bindings = Vec::new();
        self.raise_observation_frames
            .borrow_mut()
            .push((self.handled_raise_depth, false));
        let result = (|| {
            for clause in clauses {
                match clause {
                    crate::ast::ComprehensionClause::For { var, binding, iter } => {
                        self.register_named_bindings(iter)?;
                        let iter_ty = self.infer(iter)?;
                        let source_mode = Self::iteration_mode(iter);
                        let (mut yielded_ty, mut protocol) =
                            self.iteration_protocol(&iter_ty, source_mode)?;
                        self.attach_borrowed_iteration_origin(
                            iter,
                            &iter_ty,
                            source_mode,
                            &mut protocol,
                        );
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
                        protocol.binding = Some(Box::new(binding_plan.clone()));
                        self.iteration_protocols
                            .borrow_mut()
                            .insert(iter.source_span(), protocol);
                        let binding_ty = binding_plan.binding_ty.clone();
                        let implicitly_deletable = self.is_implicitly_deletable(&binding_ty);
                        // A generator binder scopes everything to its right, but
                        // not its own iterable. Giving every generator a lexical
                        // scope also permits a later generator to shadow the same
                        // spelling without changing an outer local.
                        self.push_scope();
                        self.declare_with_mutability(var, binding_ty, binding_plan.mutable)?;
                        let owner = self.lookup_owner(var).ok_or_else(|| {
                            TypeError::InvariantViolation(format!(
                                "comprehension binder '{var}' has no stable owner"
                            ))
                        })?;
                        bindings.push(crate::checked::CheckedComprehensionBinding {
                            name: var.clone(),
                            owner,
                            ty: self.lookup(var).cloned().ok_or_else(|| {
                                TypeError::InvariantViolation(format!(
                                    "comprehension binder '{var}' has no checked type"
                                ))
                            })?,
                            plan: binding_plan,
                            implicitly_deletable,
                        });
                    }
                    crate::ast::ComprehensionClause::If(condition) => {
                        self.register_named_bindings(condition)?;
                        self.expect_bool(condition, "comprehension filter")?;
                        // A filter creates a non-consuming path: an element the
                        // condition skips is dropped without its explicit
                        // destructor, which a linear binder cannot permit.
                        if let Some(linear) = bindings.iter().find(|binding| {
                            !binding.implicitly_deletable
                                && matches!(
                                    binding.plan.action,
                                    crate::checked::IterationBindingAction::MoveValue
                                )
                        }) {
                            let obligation = self.residual_obligation_suffix(&linear.ty);
                            return Err(TypeError::Unsupported(format!(
                                "a comprehension filter would abandon skipped non-ImplicitlyDeletable '{}' elements without explicit destruction{obligation}",
                                linear.ty
                            )));
                        }
                    }
                }
            }

            if let Some(key) = key {
                self.register_named_bindings(key)?;
            }
            self.register_named_bindings(value)?;
            let value_ty = default_literal(&self.infer(value)?);
            self.check_consuming(value, &value_ty, "collection comprehension element")?;
            let result_ty = match kind {
                crate::ast::CollectionKind::List => list_type(value_ty),
                crate::ast::CollectionKind::Set => {
                    if !self.is_hashable(&value_ty) {
                        return Err(TypeError::TraitNotSatisfied {
                            param: "T".to_string(),
                            ty: value_ty.to_string(),
                            trait_name: "Hashable".to_string(),
                            reason: self.trait_failure_reason(&value_ty, "Hashable"),
                        });
                    }
                    set_type(value_ty)
                }
                crate::ast::CollectionKind::Dict => {
                    let key = key.as_ref().expect("dictionary comprehension has a key");
                    let key_ty = default_literal(&self.infer(key)?);
                    self.check_consuming(key, &key_ty, "dictionary comprehension key")?;
                    if !self.is_hashable(&key_ty) {
                        return Err(TypeError::TraitNotSatisfied {
                            param: "K".to_string(),
                            ty: key_ty.to_string(),
                            trait_name: "Hashable".to_string(),
                            reason: self.trait_failure_reason(&key_ty, "Hashable"),
                        });
                    }
                    dict_type(key_ty, value_ty)
                }
            };
            self.record_collection_construction(expression.source_span(), &result_ty);
            self.expression_types
                .borrow_mut()
                .insert(expression.source_span(), result_ty);
            self.comprehension_bindings
                .borrow_mut()
                .insert(expression.source_span(), bindings.clone());
            Ok(())
        })();
        while self.scopes.len() > scope_base {
            self.pop_scope();
        }
        let raised = self
            .raise_observation_frames
            .borrow_mut()
            .pop()
            .is_some_and(|(_, flag)| flag);
        if result.is_ok()
            && raised
            && let Some(linear) = bindings.iter().find(|binding| {
                !binding.implicitly_deletable
                    && matches!(
                        binding.plan.action,
                        crate::checked::IterationBindingAction::MoveValue
                    )
            })
        {
            // A propagating error aborts the comprehension mid-iteration,
            // abandoning residual elements exactly like a loop's raise.
            let obligation = self.residual_obligation_suffix(&linear.ty);
            return Err(TypeError::Unsupported(format!(
                "a raising call in a comprehension would abandon non-ImplicitlyDeletable '{}' elements without explicit destruction{obligation}",
                linear.ty
            )));
        }
        result
    }

    pub(super) fn register_named_bindings(&mut self, expression: &Expr) -> Result<(), TypeError> {
        if matches!(expression.kind, ExprKind::Comprehension { .. }) {
            return self.check_comprehension(expression);
        }
        if let ExprKind::Named { name, value } = &expression.kind {
            self.register_named_bindings(value)?;
            let found = self.infer(value)?;
            let base = self
                .function_bases
                .last()
                .copied()
                .unwrap_or(self.scopes.len().saturating_sub(1));
            let existing = self.scopes[base..]
                .iter()
                .rev()
                .find_map(|scope| scope.get(name))
                .cloned();
            if let Some(existing) = existing {
                if !self.value_coerces(&found, &existing) {
                    return Err(TypeError::TypeMismatch {
                        expected: existing.to_string(),
                        found: found.to_string(),
                        context: format!("walrus assignment to '{name}'"),
                    });
                }
            } else {
                let declared = self.inferred_binding_ty(&found, name)?;
                self.declare_function_implicit(name, declared)?;
            }
            return Ok(());
        }
        match &expression.kind {
            ExprKind::Prefix(_, value) | ExprKind::Transfer(value) => {
                self.register_named_bindings(value)?
            }
            ExprKind::Infix(_, left, right)
            | ExprKind::Index {
                object: left,
                index: right,
            } => {
                self.register_named_bindings(left)?;
                self.register_named_bindings(right)?;
            }
            ExprKind::Call { args, kwargs, .. } => {
                for argument in args {
                    self.register_named_bindings(argument)?;
                }
                for argument in kwargs {
                    self.register_named_bindings(&argument.value)?;
                }
            }
            ExprKind::Invoke {
                callee,
                args,
                kwargs,
                ..
            } => {
                self.register_named_bindings(callee)?;
                for argument in args {
                    self.register_named_bindings(argument)?;
                }
                for argument in kwargs {
                    self.register_named_bindings(&argument.value)?;
                }
            }
            ExprKind::Member { object, .. } => self.register_named_bindings(object)?,
            ExprKind::MethodCall {
                object,
                args,
                kwargs,
                ..
            } => {
                self.register_named_bindings(object)?;
                for argument in args {
                    self.register_named_bindings(argument)?;
                }
                for argument in kwargs {
                    self.register_named_bindings(&argument.value)?;
                }
            }
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                ..
            } => {
                self.register_named_bindings(object)?;
                for bound in [lower, upper, step].into_iter().flatten() {
                    self.register_named_bindings(bound)?;
                }
            }
            ExprKind::MultiIndex { object, args } => {
                self.register_named_bindings(object)?;
                for argument in args {
                    match argument {
                        crate::ast::SubscriptArg::Index(value)
                        | crate::ast::SubscriptArg::Keyword { value, .. } => {
                            self.register_named_bindings(value)?
                        }
                        crate::ast::SubscriptArg::Slice {
                            lower, upper, step, ..
                        } => {
                            for bound in [lower, upper, step].into_iter().flatten() {
                                self.register_named_bindings(bound)?;
                            }
                        }
                    }
                }
            }
            ExprKind::ListLit(values) | ExprKind::TupleLit(values) => {
                for value in values {
                    self.register_named_bindings(value)?;
                }
            }
            ExprKind::BraceLit(entries) => {
                for (key, value) in entries {
                    self.register_named_bindings(key)?;
                    if let Some(value) = value {
                        self.register_named_bindings(value)?;
                    }
                }
            }
            ExprKind::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => {
                self.register_named_bindings(cond)?;
                self.register_named_bindings(then_branch)?;
                self.register_named_bindings(else_branch)?;
            }
            ExprKind::Compare { first, rest } => {
                self.register_named_bindings(first)?;
                for (_, value) in rest {
                    self.register_named_bindings(value)?;
                }
            }
            ExprKind::TString { parts, .. } => {
                for part in parts {
                    if let TStringPart::Expr(value) = part {
                        self.register_named_bindings(value)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn infer(&self, expr: &Expr) -> Result<Ty, TypeError> {
        let result = self.infer_impl(expr);
        if let Ok(ty) = &result {
            if matches!(
                ty,
                Ty::Int | Ty::UInt | Ty::Float64 | Ty::Simd { width: 1, .. }
            ) && let Some(value) = self.exact_literal_value(expr)
            {
                let literal_ty = match value {
                    CtValue::IntLiteral(_) => Ty::IntLiteral,
                    CtValue::FloatLiteral(_) => Ty::FloatLiteral,
                    _ => unreachable!("exact_literal_value only returns literal values"),
                };
                self.record_literal_materializations(expr, &literal_ty, ty)?;
            }
            self.expression_types
                .borrow_mut()
                .insert(expr.source_span(), ty.clone());
            if let Some(crate::checked::SemanticAdjustment::ReferenceResult { reference }) =
                self.operation_adjustments.borrow().get(&expr.source_span())
                && self.is_copyable(&reference.referent)
            {
                self.copyable_reference_result_reads
                    .borrow_mut()
                    .insert(expr.source_span());
            }
            if let ExprKind::Call {
                name,
                param_args,
                args,
                ..
            } = &expr.kind
            {
                let dimensions = if name == "SIMD" {
                    self.simd_dims(param_args).ok().map(|(dtype, width)| {
                        let width = if width == -1 {
                            i64::try_from(args.len()).unwrap_or(0)
                        } else {
                            width
                        };
                        (dtype, width)
                    })
                } else if name == "Scalar" && param_args.len() == 1 {
                    // `Scalar[DType.x](arg)` is width-1 SIMD construction; the
                    // VM canonicalizes width-1 int/float64 back to the native
                    // scalars, matching `simd_ty`.
                    dtype_from_arg(&param_args[0]).ok().map(|dtype| (dtype, 1))
                } else {
                    Dtype::from_scalar_alias(name).map(|dtype| (dtype, 1))
                };
                if let Some(dimensions) = dimensions {
                    self.simd_constructions
                        .borrow_mut()
                        .insert(expr.source_span(), dimensions);
                }
            }
            let place_ty = match &expr.kind {
                ExprKind::Identifier(name) => self.lookup(name).cloned(),
                ExprKind::Member { object, field } => self.infer(object).ok().and_then(|base| {
                    let Ty::Struct(name, arguments) = base else {
                        return None;
                    };
                    let info = self.structs.get(&name)?;
                    let (_, field_ty) = info
                        .fields
                        .iter()
                        .find(|(candidate, _)| candidate == field)?;
                    Some(substitute(field_ty, &struct_subst(&info.decls, &arguments)))
                }),
                ExprKind::Index { object, index } => self
                    .index_storage_ty(object, index)
                    .or_else(|| Some(ty.clone())),
                ExprKind::TypeApply { .. }
                    if self
                        .operation_adjustments
                        .borrow()
                        .get(&expr.source_span())
                        .is_some_and(|operation| {
                            matches!(
                                operation,
                                crate::checked::SemanticAdjustment::VariantProject { .. }
                            )
                        }) =>
                {
                    Some(ty.clone())
                }
                _ => None,
            };
            if let Some(place_ty) = place_ty {
                self.expression_place_types
                    .borrow_mut()
                    .insert(expr.source_span(), place_ty);
            }
        }
        result
    }

    /// Type of a reference *handle* in a context that stores or forwards one.
    /// Ordinary expression inference intentionally reads through references.
    pub(super) fn infer_reference_value(&self, expr: &Expr) -> Option<crate::origin::RefTy> {
        if let Some(crate::checked::SemanticAdjustment::ReferenceResult { reference }) =
            self.operation_adjustments.borrow().get(&expr.source_span())
        {
            return Some(reference.clone());
        }
        match &expr.kind {
            // `^` changes ownership of the reference *value*, not what that
            // value refers to.  Named arguments are likewise transparent to
            // contextual reference-value inference.  Ordinary inference still
            // reads through both forms unless the surrounding storage/call
            // context explicitly asks to retain the handle.
            ExprKind::Transfer(inner) | ExprKind::Named { value: inner, .. } => {
                self.infer_reference_value(inner)
            }
            ExprKind::Identifier(name) => match self.lookup(name) {
                Some(Ty::Ref(reference)) => Some(reference.clone()),
                _ => self.lookup_reference_parameter(name),
            },
            ExprKind::Member { object, field } => {
                let object_ty = self.infer(object).ok()?;
                let Ty::Struct(name, _) = object_ty else {
                    return None;
                };
                self.structs
                    .get(&name)?
                    .fields
                    .iter()
                    .find_map(|(candidate, ty)| {
                        (candidate == field).then_some(ty).and_then(|ty| match ty {
                            Ty::Ref(reference) => Some(reference.clone()),
                            _ => None,
                        })
                    })
            }
            ExprKind::Index { object, index } => match self.index_storage_ty(object, index)? {
                Ty::Ref(reference) => Some(reference),
                referent => self
                    .interior_references
                    .borrow()
                    .get(&expr.source_span())
                    .cloned()
                    .map(|origin| crate::origin::RefTy {
                        referent: Box::new(referent),
                        mutability: if self.owner_is_mutable(origin.root) {
                            crate::origin::Mutability::Mutable
                        } else {
                            crate::origin::Mutability::Immutable
                        },
                        origin: crate::origin::Origin::Place(origin),
                    }),
            },
            ExprKind::TypeApply { .. } => self
                .interior_references
                .borrow()
                .get(&expr.source_span())
                .cloned()
                .and_then(|origin| {
                    self.infer(expr).ok().map(|referent| crate::origin::RefTy {
                        referent: Box::new(referent),
                        mutability: if self.owner_is_mutable(origin.root) {
                            crate::origin::Mutability::Mutable
                        } else {
                            crate::origin::Mutability::Immutable
                        },
                        origin: crate::origin::Origin::Place(origin),
                    })
                }),
            _ => None,
        }
    }

    /// Infer the type stored by `expr` when the surrounding context expects a
    /// reference-bearing value.  Normal expression inference intentionally reads
    /// through a `ref`; aggregate construction instead needs to preserve the
    /// handle.  Keeping this contextual and recursive avoids changing ordinary
    /// expression semantics for tuples/lists which merely contain references.
    pub(super) fn infer_storage_value(&self, expr: &Expr, expected: &Ty) -> Result<Ty, TypeError> {
        match expected {
            Ty::Ref(_) => self
                .infer_reference_value(expr)
                .map(Ty::Ref)
                .ok_or_else(|| TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: self
                        .infer(expr)
                        .map_or_else(|_| "<error>".to_string(), |ty| ty.to_string()),
                    context: "reference-valued aggregate element".to_string(),
                }),
            expected if tuple_elements(expected).is_some() => {
                let expected_elements = tuple_elements(expected).expect("tuple elements");
                let values = match &expr.kind {
                    ExprKind::TupleLit(values) => Some(values.as_slice()),
                    ExprKind::Call { name, args, .. } if name == "Tuple" => Some(args.as_slice()),
                    _ => None,
                };
                if let Some(values) = values {
                    if values.len() != expected_elements.len() {
                        return Err(TypeError::ArityMismatch {
                            name: "Tuple".to_string(),
                            expected: expected_elements.len(),
                            got: values.len(),
                        });
                    }
                    let actual = values
                        .iter()
                        .zip(expected_elements)
                        .map(|(value, expected)| self.infer_storage_value(value, expected))
                        .collect::<Result<Vec<_>, _>>()
                        .map(nominal_tuple_type)?;
                    self.record_collection_construction(expr.source_span(), expected);
                    self.expression_types
                        .borrow_mut()
                        .insert(expr.source_span(), expected.clone());
                    return Ok(actual);
                }
                self.infer(expr)
            }
            expected if list_element(expected).is_some() => {
                let expected_element = list_element(expected).expect("list element");
                let values = match &expr.kind {
                    ExprKind::ListLit(values) => Some(values.as_slice()),
                    ExprKind::Call { name, args, .. } if name == "List" => Some(args.as_slice()),
                    _ => None,
                };
                if let Some(values) = values {
                    for value in values {
                        let actual = self.infer_storage_value(value, expected_element)?;
                        if !Self::storage_value_coerces(&actual, expected_element) {
                            return Err(TypeError::TypeMismatch {
                                expected: expected_element.to_string(),
                                found: actual.to_string(),
                                context: "reference-valued list element".to_string(),
                            });
                        }
                    }
                    self.record_collection_construction(expr.source_span(), expected);
                    self.expression_types
                        .borrow_mut()
                        .insert(expr.source_span(), expected.clone());
                    return Ok(list_type(expected_element.clone()));
                }
                self.infer(expr)
            }
            _ => {
                let actual = self.infer(expr)?;
                // A storage element that does not coerce may still convert
                // implicitly (a string literal stored where the nominal
                // String is expected); the recorded constructor wrap makes
                // the element the stored type.
                if !Self::storage_value_coerces(&actual, expected)
                    && self.record_constructor_conversion(expr, &actual, expected)?
                {
                    return Ok(expected.clone());
                }
                Ok(actual)
            }
        }
    }

    /// Storage compatibility is ordinary coercion plus recursive reference
    /// compatibility.  A tracked origin parameter accepts a tracked place; an
    /// explicitly untracked field must only receive the same untracked kind.
    pub(super) fn storage_value_coerces(from: &Ty, to: &Ty) -> bool {
        match (from, to) {
            (Ty::Ref(actual), Ty::Ref(expected)) => {
                coerces(&actual.referent, &expected.referent)
                    && (expected.mutability != crate::origin::Mutability::Mutable
                        || actual.mutability == crate::origin::Mutability::Mutable)
                    && match &expected.origin {
                        crate::origin::Origin::Untracked { mutable } => matches!(
                            &actual.origin,
                            crate::origin::Origin::Untracked {
                                mutable: actual_mutability
                            } if actual_mutability == mutable
                        ),
                        _ => !matches!(actual.origin, crate::origin::Origin::Untracked { .. }),
                    }
            }
            (
                Ty::Pointer {
                    element: actual_element,
                    origin: actual_origin,
                },
                Ty::Pointer {
                    element: expected_element,
                    origin: expected_origin,
                },
            ) => {
                coerces(actual_element, expected_element)
                    && match (actual_origin, expected_origin) {
                        // A concrete place provenance may bind a declared origin
                        // parameter, but storage must not invent mutable
                        // capability an immutable place never had.
                        (
                            crate::origin::PointerOrigin::Place { mutable, .. },
                            crate::origin::PointerOrigin::Param { mutability, .. },
                        ) => *mutable || *mutability == crate::origin::Mutability::Immutable,
                        _ => actual_origin == expected_origin,
                    }
            }
            (actual, expected)
                if tuple_elements(actual).is_some() && tuple_elements(expected).is_some() =>
            {
                let actual = tuple_elements(actual).expect("tuple elements");
                let expected = tuple_elements(expected).expect("tuple elements");
                actual.len() == expected.len()
                    && actual
                        .iter()
                        .zip(expected)
                        .all(|(actual, expected)| Self::storage_value_coerces(actual, expected))
            }
            (actual, expected)
                if list_element(actual).is_some() && list_element(expected).is_some() =>
            {
                Self::storage_value_coerces(
                    list_element(actual).expect("list element"),
                    list_element(expected).expect("list element"),
                )
            }
            // A capturing closure must not erase its environment into plain
            // `def(...)` storage (a field or collection element): the stored
            // value's capture origins would silently leave loan tracking.
            // Call-position downward funargs deliberately keep the erasing
            // coercion; only storage rejects it.
            (
                Ty::Func {
                    environment: crate::origin::CallableEnvironment::Capturing(_),
                    ..
                },
                Ty::Func {
                    environment: crate::origin::CallableEnvironment::Default,
                    ..
                },
            ) => false,
            _ => coerces(from, to),
        }
    }

    /// Mark every syntax leaf that must lower to a reference handle rather than
    /// an ordinary read-through value.  MIR consumes these checked adjustments;
    /// it never has to rediscover aggregate reference semantics from source AST.
    pub(super) fn mark_reference_storage_uses(&self, expr: &Expr, expected: &Ty) {
        match expected {
            Ty::Ref(reference) => {
                // The outer `Transfer` retains its Move adjustment.  Mark the
                // transferred place itself so MIR loads the stored reference
                // handle instead of reading through it to the referent.
                let expr = match &expr.kind {
                    ExprKind::Transfer(inner) | ExprKind::Named { value: inner, .. } => inner,
                    _ => expr,
                };
                self.reference_value_uses.borrow_mut().insert(
                    expr.source_span(),
                    reference.mutability == crate::origin::Mutability::Mutable,
                );
            }
            expected if tuple_elements(expected).is_some() => {
                let expected_elements = tuple_elements(expected).expect("tuple elements");
                let values = match &expr.kind {
                    ExprKind::TupleLit(values) => Some(values.as_slice()),
                    ExprKind::Call { name, args, .. } if name == "Tuple" => Some(args.as_slice()),
                    _ => None,
                };
                if let Some(values) = values {
                    for (value, expected) in values.iter().zip(expected_elements) {
                        self.mark_reference_storage_uses(value, expected);
                    }
                }
            }
            expected if list_element(expected).is_some() => {
                let expected_element = list_element(expected).expect("list element");
                let values = match &expr.kind {
                    ExprKind::ListLit(values) => Some(values.as_slice()),
                    ExprKind::Call { name, args, .. } if name == "List" => Some(args.as_slice()),
                    _ => None,
                };
                if let Some(values) = values {
                    for value in values {
                        self.mark_reference_storage_uses(value, expected_element);
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn infer_impl(&self, expr: &Expr) -> Result<Ty, TypeError> {
        match &expr.kind {
            ExprKind::Int(_) => Ok(Ty::IntLiteral),
            ExprKind::Float(_) => Ok(Ty::FloatLiteral),
            ExprKind::Bool(_) => Ok(Ty::Bool),
            ExprKind::Str(_) => Ok(Ty::StringLiteral),
            ExprKind::None => Ok(Ty::None),
            ExprKind::Uninitialized => Err(TypeError::InvariantViolation(
                "uninitialized marker reached expression inference".to_string(),
            )),
            ExprKind::TypeValue(_) => Err(TypeError::Unsupported(
                "function types as compile-time values".to_string(),
            )),
            ExprKind::Spread(_) => Err(TypeError::Unsupported(
                "call spread outside a specialized type pack".to_string(),
            )),
            ExprKind::Invoke {
                callee,
                param_args,
                args,
                kwargs,
            } => {
                if let Some(result) =
                    self.infer_variant_invoke(expr.source_span(), callee, param_args, args, kwargs)
                {
                    return result;
                }
                // Parameterized method syntax is parsed as `Invoke(Member)` so
                // that ordinary indexing remains unambiguous. Keep it a direct
                // method call semantically: bound methods do not become
                // first-class/escaping values merely because their compile-time
                // arguments are explicit.
                if let ExprKind::Member { object, field } = &callee.kind {
                    match self.infer_method_call(
                        expr.source_span(),
                        object,
                        field,
                        MethodCallArguments::parameterized(param_args, args, kwargs),
                    ) {
                        Ok(result) => return Ok(result),
                        Err(TypeError::NoSuchMethod { .. }) => {}
                        Err(error) => return Err(error),
                    }
                }
                let callable = self.infer(callee)?;
                let target = self.indirect_callable_target(&callable);
                let (ret, _, error) = self.infer_callable_ty(
                    &expr.source_span(),
                    "<callable>",
                    callable.clone(),
                    param_args,
                    args,
                    kwargs,
                )?;
                self.record_call_environment_effects(
                    expr.source_span(),
                    &callable,
                    param_args,
                    args,
                    kwargs,
                )?;
                if let Some(target) = target {
                    self.overload_targets
                        .borrow_mut()
                        .insert(expr.source_span(), target);
                }
                if let Some(error) = error.filter(|ty| *ty != Ty::Never) {
                    self.record_call_effect(expr.source_span(), error.clone());
                    self.require_error("call through a raising callable", error)?;
                }
                Ok(ret)
            }
            ExprKind::BraceLit(entries) => {
                if entries.is_empty() {
                    return Err(TypeError::Unsupported(
                        "an empty '{}' display needs a Dict[K, V] type annotation".to_string(),
                    ));
                }
                let dictionary = entries[0].1.is_some();
                if entries
                    .iter()
                    .any(|(_, value)| value.is_some() != dictionary)
                {
                    return Err(TypeError::Unsupported(
                        "set elements and dictionary key/value pairs cannot be mixed".to_string(),
                    ));
                }
                let keys = entries
                    .iter()
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>();
                let key_ty = self.infer_list_elem(&keys)?;
                for key in &keys {
                    self.check_consuming(key, &key_ty, "collection display element")?;
                }
                if !self.is_hashable(&key_ty) {
                    return Err(TypeError::TraitNotSatisfied {
                        param: "K".to_string(),
                        ty: key_ty.to_string(),
                        trait_name: "Hashable".to_string(),
                        reason: self.trait_failure_reason(&key_ty, "Hashable"),
                    });
                }
                if !dictionary {
                    let result = set_type(key_ty);
                    self.record_collection_construction(expr.source_span(), &result);
                    return Ok(result);
                }
                let values = entries
                    .iter()
                    .filter_map(|(_, value)| value.clone())
                    .collect::<Vec<_>>();
                let value_ty = self.infer_list_elem(&values)?;
                for value in &values {
                    self.check_consuming(value, &value_ty, "dictionary display value")?;
                }
                let result = dict_type(key_ty, value_ty);
                self.record_collection_construction(expr.source_span(), &result);
                Ok(result)
            }
            ExprKind::Comprehension { .. } => self
                .expression_types
                .borrow()
                .get(&expr.source_span())
                .cloned()
                .ok_or_else(|| {
                    TypeError::InvariantViolation(
                        "comprehension reached inference before scoped checking".to_string(),
                    )
                }),
            ExprKind::Identifier(name) => {
                self.check_capture_access(name, false)?;
                if let Some(owner) = self.lookup_owner(name) {
                    self.expression_bindings
                        .borrow_mut()
                        .insert(expr.source_span(), owner);
                }
                if self
                    .lookup_owner(name)
                    .is_some_and(|owner| self.uninitialized.borrow().contains(&owner))
                {
                    return Err(TypeError::Unsupported(format!(
                        "variable '{name}' may be uninitialized"
                    )));
                }
                self.lookup(name)
                    .map(|ty| match ty {
                        Ty::Ref(reference) => (*reference.referent).clone(),
                        other => other.clone(),
                    })
                    .ok_or_else(|| TypeError::UndefinedVariable(name.clone()))
            }
            ExprKind::Prefix(op, operand) => self.infer_prefix(*op, operand),
            ExprKind::Infix(op, left, right) => {
                self.infer_infix(Some(expr.source_span()), *op, left, right)
            }
            ExprKind::Call {
                name,
                param_args,
                args,
                kwargs,
            } => self.infer_call(expr.source_span(), name, param_args, args, kwargs),
            ExprKind::Member { object, field } => self.infer_member(object, field),
            ExprKind::MethodCall {
                object,
                method,
                args,
                kwargs,
            } => self.infer_method_call(
                expr.source_span(),
                object,
                method,
                MethodCallArguments::ordinary(args, kwargs),
            ),
            ExprKind::Index { object, index } => {
                // A variant projection whose alternative is spelled with a
                // struct name (`v[String]`): the name no longer parses as a
                // type token, so it arrives as an ordinary index identifier;
                // reinterpret it as the projection's type argument, mirroring
                // the `TypeApply` arm below.
                if let ExprKind::Identifier(vname) = &object.kind
                    && let ExprKind::Identifier(tname) = &index.kind
                    && self.structs.contains_key(tname)
                    && let Some(Ty::Variant(alternatives)) = self.lookup(vname).cloned()
                {
                    self.check_capture_access(vname, false)?;
                    let arg = crate::ast::ParamArg::Value((**index).clone());
                    let (index, result) =
                        self.variant_alternative(&alternatives, std::slice::from_ref(&arg))?;
                    if let Some(owner) = self.lookup_owner(vname) {
                        self.expression_bindings
                            .borrow_mut()
                            .insert(expr.source_span(), owner);
                    }
                    self.operation_adjustments.borrow_mut().insert(
                        expr.source_span(),
                        crate::checked::SemanticAdjustment::VariantProject {
                            alternatives,
                            index,
                        },
                    );
                    self.record_interior_reference(expr.source_span(), expr, "value");
                    // A projection value read copies a Copyable payload out of
                    // the variant's storage rather than aliasing it.
                    if self.is_copyable(&result) {
                        self.copy_place_value_uses
                            .borrow_mut()
                            .insert(expr.source_span());
                    }
                    return Ok(result);
                }
                self.infer_index(expr.source_span(), object, index)
            }
            // A dynamic/indexed expression does not designate independently
            // movable storage. Current Mojo permits `^` there only when the
            // result is implicitly copyable (the transfer is effectively a
            // copy); accepting a linear element would duplicate its destructor.
            // Compiler-private Tuple storage is the exception: each indexed
            // element is a tracked owned slot, which is how whole heterogeneous
            // packs and public Tuple's private backing field transfer linear
            // elements exactly once.
            ExprKind::Transfer(inner) => {
                let ty = self.infer(inner)?;
                let private_tuple_element = match &inner.kind {
                    ExprKind::Index { object, .. } => {
                        matches!(self.infer(object), Ok(Ty::Tuple(_)))
                    }
                    _ => false,
                };
                if place_has_index(inner)
                    && !self.is_implicitly_copyable(&ty)
                    && !private_tuple_element
                {
                    return Err(TypeError::Unsupported(
                        "cannot transfer a non-implicitly-copyable indexed value; the expression does not designate independently movable storage"
                            .to_string(),
                    ));
                }
                Ok(ty)
            }
            // An empty list literal needs a contextual element type; the
            // context-aware paths never reach this uncontextualized inference.
            ExprKind::ListLit(elems) if elems.is_empty() => Err(TypeError::CannotInferTypeParam {
                name: "List".to_string(),
                param: "T".to_string(),
            }),
            ExprKind::ListLit(elems) => {
                let result = list_type(self.infer_list_elem(elems)?);
                self.record_collection_construction(expr.source_span(), &result);
                Ok(result)
            }
            // A tuple literal keeps each element's own type (heterogeneous).
            ExprKind::TupleLit(elems) => {
                let tys = elems
                    .iter()
                    .map(|e| self.infer(e))
                    .collect::<Result<Vec<_>, _>>()?;
                let result = self.public_tuple_type(tys);
                self.record_collection_construction(expr.source_span(), &result);
                Ok(result)
            }
            // Walrus `name := value` types as `value`; MIR marks execution as
            // unsupported. The name is not bound here — `infer` is read-only — so
            // a program that *uses* the walrus-bound name later won't type-check.
            ExprKind::Named { value, .. } => self.infer(value),
            // Ternary `a if cond else b`: `cond` must be `Bool`; the branches must
            // have a common type (the result type).
            ExprKind::IfExpr {
                cond,
                then_branch,
                else_branch,
            } => {
                let ct = self.infer(cond)?;
                if ct != Ty::Bool {
                    return Err(TypeError::TypeMismatch {
                        expected: "Bool".to_string(),
                        found: ct.to_string(),
                        context: "conditional-expression condition".to_string(),
                    });
                }
                let tt = self.infer(then_branch)?;
                let et = self.infer(else_branch)?;
                common_branch_ty(&tt, &et).ok_or_else(|| TypeError::TypeMismatch {
                    expected: tt.to_string(),
                    found: et.to_string(),
                    context: "conditional-expression branches".to_string(),
                })
            }
            // Chained comparison `a < b < c`: each adjacent pair must compare to a
            // `Bool` (same rules as a single comparison); the result is `Bool`.
            ExprKind::Compare { first, rest } => {
                let mut left: &Expr = first;
                for (op, right) in rest {
                    if self.infer_infix(None, *op, left, right)? != Ty::Bool {
                        return Err(TypeError::BadOperator {
                            op: infix_symbol(*op).to_string(),
                            operands: "a chained comparison must compare to Bool".to_string(),
                        });
                    }
                    left = right;
                }
                Ok(Ty::Bool)
            }
            // Slice `object[lower:upper:step]` on a `List`/`String`: each present
            // bound must be `Int`; the result is the same sequence type.
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                explicit_step,
            } => self.infer_slice_subscript(
                expr.source_span(),
                object,
                lower.as_deref(),
                upper.as_deref(),
                step.as_deref(),
                *explicit_step,
            ),
            ExprKind::MultiIndex { object, args } => {
                self.infer_multi_subscript(expr.source_span(), object, args)
            }
            ExprKind::TString { parts, .. } => {
                // A t-string is a lazy `TString` value over an interleaved
                // element pack: literal segments as compile-time strings and
                // interpolation snapshots at their checked types.  A
                // non-Copyable place snapshots as a creation-time formatted
                // string instead — the specialized pack constructor consumes
                // its arguments, and the place must stay usable afterward.
                let mut elements = Vec::with_capacity(parts.len());
                for part in parts {
                    match part {
                        TStringPart::Literal(_) => elements.push(Ty::StringLiteral),
                        TStringPart::Expr(value) => {
                            let ty = self.infer(value)?;
                            if !self.conforms_to(&ty, "Writable") {
                                return Err(TypeError::TraitNotSatisfied {
                                    param: "interpolation".to_string(),
                                    ty: ty.to_string(),
                                    trait_name: "Writable".to_string(),
                                    reason: self.trait_failure_reason(&ty, "Writable"),
                                });
                            }
                            let ty = default_literal(&ty);
                            if is_place_expr(value) && !self.is_copyable(&ty) {
                                elements.push(Ty::StringLiteral);
                            } else {
                                elements.push(ty);
                            }
                        }
                    }
                }
                Ok(self.public_tstring_type(elements))
            }
            // A parameterized type is not a runtime value; it is only valid as a
            // static-method receiver (`UnsafePointer[T].alloc(…)`), typed in
            // `infer_method_call`.
            ExprKind::TypeApply { name, args } => {
                if let Some(specialized) = self.infer_specialized_callable_value(
                    expr.source_span(),
                    name,
                    args,
                    None,
                    true,
                )? {
                    Ok(specialized)
                } else if let Some(Ty::Variant(alternatives)) = self.lookup(name).cloned() {
                    self.check_capture_access(name, false)?;
                    let (index, result) = self.variant_alternative(&alternatives, args)?;
                    if let Some(owner) = self.lookup_owner(name) {
                        self.expression_bindings
                            .borrow_mut()
                            .insert(expr.source_span(), owner);
                    }
                    self.operation_adjustments.borrow_mut().insert(
                        expr.source_span(),
                        crate::checked::SemanticAdjustment::VariantProject {
                            alternatives,
                            index,
                        },
                    );
                    self.record_interior_reference(expr.source_span(), expr, "value");
                    // A projection value read copies a Copyable payload out of
                    // the variant's storage rather than aliasing it.
                    if self.is_copyable(&result) {
                        self.copy_place_value_uses
                            .borrow_mut()
                            .insert(expr.source_span());
                    }
                    Ok(result)
                } else {
                    Err(TypeError::TypeMismatch {
                        expected: "a value".to_string(),
                        found: format!("the type '{name}[…]'"),
                        context: "a parameterized type is not a value".to_string(),
                    })
                }
            }
        }
    }

    /// Recognize compiler-known parameterized `Variant` operations. The parser
    /// preserves their type arguments on the invoke; checked metadata records
    /// every selected tag and whether the runtime operation is checked or unsafe.
    pub(super) fn infer_variant_invoke(
        &self,
        span: SourceSpan,
        callee: &Expr,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Option<Result<Ty, TypeError>> {
        let ExprKind::Member { object, field } = &callee.kind else {
            return None;
        };
        let object_ty = match self.infer(object) {
            Ok(ty) => ty,
            Err(error) => return Some(Err(error)),
        };
        let Ty::Variant(alternatives) = object_ty else {
            return None;
        };
        if !matches!(
            field.as_str(),
            "isa"
                | "is_type_supported"
                | "set"
                | "take"
                | "unsafe_take"
                | "replace"
                | "unsafe_replace"
        ) {
            return None;
        }
        Some((|| {
            if !kwargs.is_empty() {
                return Err(TypeError::BadCall {
                    func: format!("Variant.{field}"),
                    reason: "keyword arguments are not supported".to_string(),
                });
            }
            match field.as_str() {
                "isa" => {
                    let (index, _) = self.variant_alternative(&alternatives, param_args)?;
                    if !args.is_empty() {
                        return Err(TypeError::ArityMismatch {
                            name: "Variant.isa".to_string(),
                            expected: 0,
                            got: args.len(),
                        });
                    }
                    self.operation_adjustments.borrow_mut().insert(
                        span,
                        crate::checked::SemanticAdjustment::VariantIs {
                            alternatives,
                            index,
                        },
                    );
                    Ok(Ty::Bool)
                }
                "is_type_supported" => {
                    if param_args.len() != 1 {
                        return Err(TypeError::WrongTypeArgCount {
                            name: "Variant.is_type_supported".to_string(),
                            expected: 1,
                            got: param_args.len(),
                        });
                    }
                    if !args.is_empty() {
                        return Err(TypeError::ArityMismatch {
                            name: "Variant.is_type_supported".to_string(),
                            expected: 0,
                            got: args.len(),
                        });
                    }
                    let requested =
                        self.type_param_argument(&param_args[0], "Variant.is_type_supported")?;
                    self.operation_adjustments.borrow_mut().insert(
                        span,
                        crate::checked::SemanticAdjustment::VariantTypeSupported {
                            supported: alternatives.contains(&requested),
                        },
                    );
                    Ok(Ty::Bool)
                }
                "set" => {
                    let (index, alternative) =
                        self.variant_alternative(&alternatives, param_args)?;
                    if args.len() != 1 {
                        return Err(TypeError::ArityMismatch {
                            name: "Variant.set".to_string(),
                            expected: 1,
                            got: args.len(),
                        });
                    }
                    self.check_place(object)?;
                    let actual = self.infer(&args[0])?;
                    if !self.record_implicit_conversion(&args[0], &actual, &alternative)? {
                        return Err(TypeError::TypeMismatch {
                            expected: alternative.to_string(),
                            found: actual.to_string(),
                            context: "argument to 'Variant.set'".to_string(),
                        });
                    }
                    self.check_consuming(&args[0], &actual, "argument to 'Variant.set'")?;
                    self.operation_adjustments.borrow_mut().insert(
                        span.clone(),
                        crate::checked::SemanticAdjustment::VariantSet {
                            alternatives,
                            index,
                        },
                    );
                    self.record_interior_invalidation(span, object);
                    Ok(Ty::None)
                }
                "take" | "unsafe_take" => {
                    let (index, alternative) =
                        self.variant_alternative(&alternatives, param_args)?;
                    if !args.is_empty() {
                        return Err(TypeError::ArityMismatch {
                            name: format!("Variant.{field}"),
                            expected: 0,
                            got: args.len(),
                        });
                    }
                    if !is_place_expr(object) {
                        return Err(TypeError::BadCall {
                            func: format!("Variant.{field}"),
                            reason: "consuming receiver must be an owned place".to_string(),
                        });
                    }
                    self.operation_adjustments.borrow_mut().insert(
                        span.clone(),
                        crate::checked::SemanticAdjustment::VariantTake {
                            alternatives,
                            index,
                            checked: field == "take",
                        },
                    );
                    self.record_interior_invalidation(span, object);
                    Ok(alternative)
                }
                "replace" | "unsafe_replace" => {
                    if param_args.len() != 2 {
                        return Err(TypeError::WrongTypeArgCount {
                            name: format!("Variant.{field}"),
                            expected: 2,
                            got: param_args.len(),
                        });
                    }
                    if args.len() != 1 {
                        return Err(TypeError::ArityMismatch {
                            name: format!("Variant.{field}"),
                            expected: 1,
                            got: args.len(),
                        });
                    }
                    let input = self.type_param_argument(&param_args[0], "Variant.replace")?;
                    let output = self.type_param_argument(&param_args[1], "Variant.replace")?;
                    let input_index = alternatives
                        .iter()
                        .position(|alternative| alternative == &input)
                        .ok_or_else(|| TypeError::TypeMismatch {
                            expected: format!("one of {}", Ty::Variant(alternatives.clone())),
                            found: input.to_string(),
                            context: "Variant replacement input type".to_string(),
                        })?;
                    let output_index = alternatives
                        .iter()
                        .position(|alternative| alternative == &output)
                        .ok_or_else(|| TypeError::TypeMismatch {
                            expected: format!("one of {}", Ty::Variant(alternatives.clone())),
                            found: output.to_string(),
                            context: "Variant replacement output type".to_string(),
                        })?;
                    self.check_place(object)?;
                    if field == "replace" && !self.is_implicitly_deletable(&input) {
                        return Err(TypeError::TraitNotSatisfied {
                            param: "Tin".to_string(),
                            ty: input.to_string(),
                            trait_name: "ImplicitlyDeletable".to_string(),
                            reason: Some(
                                "checked replacement must be able to delete the incoming value if the active tag mismatches"
                                    .to_string(),
                            ),
                        });
                    }
                    let actual = self.infer(&args[0])?;
                    if !self.record_implicit_conversion(&args[0], &actual, &input)? {
                        return Err(TypeError::TypeMismatch {
                            expected: input.to_string(),
                            found: actual.to_string(),
                            context: format!("argument to 'Variant.{field}'"),
                        });
                    }
                    self.check_consuming(
                        &args[0],
                        &actual,
                        &format!("argument to 'Variant.{field}'"),
                    )?;
                    self.operation_adjustments.borrow_mut().insert(
                        span.clone(),
                        crate::checked::SemanticAdjustment::VariantReplace {
                            alternatives,
                            input_index,
                            output_index,
                            checked: field == "replace",
                        },
                    );
                    self.record_interior_invalidation(span, object);
                    Ok(output)
                }
                _ => unreachable!("checked Variant operation"),
            }
        })())
    }

    pub(super) fn variant_alternative(
        &self,
        alternatives: &[Ty],
        args: &[crate::ast::ParamArg],
    ) -> Result<(usize, Ty), TypeError> {
        if args.len() != 1 {
            return Err(TypeError::WrongTypeArgCount {
                name: "Variant operation".to_string(),
                expected: 1,
                got: args.len(),
            });
        }
        let requested = self.type_param_argument(&args[0], "Variant operation")?;
        alternatives
            .iter()
            .position(|alternative| alternative == &requested)
            .map(|index| (index, requested.clone()))
            .ok_or_else(|| TypeError::TypeMismatch {
                expected: format!("one of {}", Ty::Variant(alternatives.to_vec())),
                found: requested.to_string(),
                context: "Variant operation type".to_string(),
            })
    }

    /// Infer a collection display against an expected collection type. Empty
    /// displays need this context to choose their family, and non-empty numeric
    /// displays need it to materialize elements (for example `{1, 2}` as
    /// `Set[Float64]`) instead of merely coercing the aggregate shell.
    ///
    /// Candidate scoring uses `record = false`; after overload selection the
    /// checked path repeats this with `record = true`, retaining the chosen root
    /// type and any element conversions for HIR/MIR.
    pub(super) fn infer_with_expected(
        &self,
        expression: &Expr,
        expected: &Ty,
        record: bool,
    ) -> Result<Ty, TypeError> {
        // Compiler-synthesized slice descriptors have no standalone source
        // expression. Their exact protocol type is installed by
        // `synthetic_slice_descriptor`; consume that checked fact directly.
        if matches!(expression.kind, ExprKind::None)
            && let Some(ty) = self
                .expression_types
                .borrow()
                .get(&expression.source_span())
                .cloned()
        {
            return Ok(ty);
        }
        if let ExprKind::TypeApply { name, args } = &expression.kind
            && matches!(expected, Ty::Func { .. })
            && let Some(specialized) = self.infer_specialized_callable_value(
                expression.source_span(),
                name,
                args,
                Some(expected),
                record,
            )?
        {
            return Ok(specialized);
        }
        // Reference-typed values normally read through to their referents in
        // expression position.  A reference-typed parameter is a storage
        // context, just like a reference field or aggregate element: infer and
        // forward the handle itself.  This matters for generated Tuple
        // `consume_elements`, whose element type may itself be `ref[...] T`.
        if matches!(expected, Ty::Ref(_)) {
            let actual = self.infer_storage_value(expression, expected)?;
            if !Self::storage_value_coerces(&actual, expected) {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: actual.to_string(),
                    context: "reference-valued argument".to_string(),
                });
            }
            if record {
                self.mark_reference_storage_uses(expression, expected);
            }
            return Ok(actual);
        }
        // A list literal bound to callable-element storage checks each
        // element with the storage rule: a capturing closure must not erase
        // its environment into plain `def(...)` element storage.
        if let ExprKind::ListLit(values) = &expression.kind
            && let Some(element) = list_element(expected)
            && matches!(element, Ty::Func { .. } | Ty::GenericFunc { .. })
        {
            for (index, value) in values.iter().enumerate() {
                let actual = self.infer_with_expected(value, element, record)?;
                if !Self::storage_value_coerces(&actual, element) {
                    return Err(TypeError::TypeMismatch {
                        expected: element.to_string(),
                        found: actual.to_string(),
                        context: format!("element {} of List", index + 1),
                    });
                }
            }
            if record {
                self.record_collection_construction(expression.source_span(), expected);
                self.expression_types
                    .borrow_mut()
                    .insert(expression.source_span(), expected.clone());
            }
            return Ok(expected.clone());
        }
        // Current Mojo solves direct collection-annotation holes from the
        // literal initializer (`List[_]`, bare `List`, `Dict[String, _]`). The
        // solve is intentionally shallow: nested holes remain non-concrete.
        let solved_expected = match (&expression.kind, expected) {
            (ExprKind::ListLit(values), expected)
                if list_element(expected).is_some_and(|element| *element == Ty::Infer) =>
            {
                if values.is_empty() {
                    return Err(TypeError::CannotInferTypeParam {
                        name: "List".to_string(),
                        param: "T".to_string(),
                    });
                }
                Some(list_type(self.infer_list_elem(values)?))
            }
            (ExprKind::BraceLit(entries), expected)
                if set_element(expected).is_some_and(|element| *element == Ty::Infer)
                    && entries.iter().all(|(_, value)| value.is_none()) =>
            {
                if entries.is_empty() {
                    return Err(TypeError::CannotInferTypeParam {
                        name: "Set".to_string(),
                        param: "T".to_string(),
                    });
                }
                let keys = entries
                    .iter()
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>();
                Some(set_type(self.infer_list_elem(&keys)?))
            }
            (ExprKind::BraceLit(entries), expected)
                if dict_elements(expected).is_some()
                    && (entries.is_empty() || entries.iter().all(|(_, value)| value.is_some())) =>
            {
                let (expected_key, expected_value) =
                    dict_elements(expected).expect("dictionary arguments");
                if (expected_key == &Ty::Infer || expected_value == &Ty::Infer)
                    && entries.is_empty()
                {
                    return Err(TypeError::CannotInferTypeParam {
                        name: "Dict".to_string(),
                        param: "K or V".to_string(),
                    });
                }
                let actual_keys = entries
                    .iter()
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>();
                let actual_values = entries
                    .iter()
                    .filter_map(|(_, value)| value.clone())
                    .collect::<Vec<_>>();
                let key = if expected_key == &Ty::Infer {
                    self.infer_list_elem(&actual_keys)?
                } else {
                    expected_key.clone()
                };
                let value = if expected_value == &Ty::Infer {
                    self.infer_list_elem(&actual_values)?
                } else {
                    expected_value.clone()
                };
                (key != *expected_key || value != *expected_value).then(|| dict_type(key, value))
            }
            _ => None,
        };
        let expected = solved_expected.as_ref().unwrap_or(expected);

        // A tuple display against a tuple-typed context checks each element
        // contextually, so an element may convert implicitly (a string
        // literal where the nominal String element is expected).
        if let (ExprKind::TupleLit(values), Some(expected_elements)) =
            (&expression.kind, tuple_elements(expected))
        {
            if values.len() != expected_elements.len() {
                return Err(TypeError::ArityMismatch {
                    name: "Tuple".to_string(),
                    expected: expected_elements.len(),
                    got: values.len(),
                });
            }
            for (value, element) in values.iter().zip(expected_elements) {
                let actual = self.infer_with_expected(value, element, record)?;
                let compatible = if record {
                    self.record_implicit_conversion(value, &actual, element)?
                } else {
                    self.value_coerces(&actual, element)
                        || self.implicit_conversion_target(&actual, element)?.is_some()
                };
                if !compatible {
                    return Err(TypeError::TypeMismatch {
                        expected: element.to_string(),
                        found: actual.to_string(),
                        context: "Tuple display element".to_string(),
                    });
                }
                self.check_consuming(value, &actual, "Tuple display element")?;
            }
            if record {
                self.record_collection_construction(expression.source_span(), expected);
                self.expression_types
                    .borrow_mut()
                    .insert(expression.source_span(), expected.clone());
            }
            return Ok(expected.clone());
        }

        let elements: Option<Vec<(&Expr, &Ty, &'static str)>> =
            if let (ExprKind::ListLit(values), Some(element)) =
                (&expression.kind, list_element(expected))
            {
                Some(
                    values
                        .iter()
                        .map(|value| (value, element, "collection display element"))
                        .collect(),
                )
            } else if let (ExprKind::BraceLit(entries), Some(element)) =
                (&expression.kind, set_element(expected))
            {
                entries.iter().all(|(_, value)| value.is_none()).then(|| {
                    entries
                        .iter()
                        .map(|(value, _)| (value, element, "collection display element"))
                        .collect()
                })
            } else if let (ExprKind::BraceLit(entries), Some((key, value))) =
                (&expression.kind, dict_elements(expected))
            {
                (entries.is_empty() || entries.iter().all(|(_, value)| value.is_some())).then(
                    || {
                        entries
                            .iter()
                            .flat_map(|(actual_key, actual_value)| {
                                [
                                    (actual_key, key, "dictionary display key"),
                                    (
                                        actual_value
                                            .as_ref()
                                            .expect("contextual dictionary entry has a value"),
                                        value,
                                        "dictionary display value",
                                    ),
                                ]
                            })
                            .collect()
                    },
                )
            } else {
                None
            };

        let Some(elements) = elements else {
            return self.infer(expression);
        };
        if let Some(element) = set_element(expected)
            && !self.is_hashable(element)
        {
            return Err(TypeError::TraitNotSatisfied {
                param: "T".to_string(),
                ty: element.to_string(),
                trait_name: "Hashable".to_string(),
                reason: self.trait_failure_reason(element, "Hashable"),
            });
        }
        if let Some((key, _)) = dict_elements(expected)
            && !self.is_hashable(key)
        {
            return Err(TypeError::TraitNotSatisfied {
                param: "K".to_string(),
                ty: key.to_string(),
                trait_name: "Hashable".to_string(),
                reason: self.trait_failure_reason(key, "Hashable"),
            });
        }

        for (value, element, context) in elements {
            let actual = self.infer_with_expected(value, element, record)?;
            let compatible = if record {
                self.record_implicit_conversion(value, &actual, element)?
            } else {
                self.value_coerces(&actual, element)
                    || self.implicit_conversion_target(&actual, element)?.is_some()
            };
            if !compatible {
                return Err(TypeError::TypeMismatch {
                    expected: element.to_string(),
                    found: actual.to_string(),
                    context: context.to_string(),
                });
            }
            self.check_consuming(value, &actual, context)?;
        }
        if record {
            self.record_collection_construction(expression.source_span(), expected);
            self.expression_types
                .borrow_mut()
                .insert(expression.source_span(), expected.clone());
        }
        Ok(expected.clone())
    }

    /// Infer the common element type of a non-empty list of expressions: numeric
    /// elements unify (widening literals), non-numeric elements must match; the
    /// result is materialized (a literal → its concrete default).
    pub(super) fn infer_list_elem(&self, elems: &[Expr]) -> Result<Ty, TypeError> {
        let mut acc: Option<Ty> = None;
        for e in elems {
            let ty = self.infer(e)?;
            acc = Some(match acc {
                None => ty,
                Some(cur) => common_elem(&cur, &ty).ok_or_else(|| TypeError::TypeMismatch {
                    expected: cur.to_string(),
                    found: ty.to_string(),
                    context: "list element".to_string(),
                })?,
            });
        }
        // A non-empty literal always sets `acc`; empty is handled by the caller.
        let element = acc.ok_or_else(|| {
            TypeError::InvariantViolation("empty list reached non-empty inference".to_string())
        })?;
        let materialized = default_literal(&element);
        for value in elems {
            let actual = self.infer(value)?;
            self.record_literal_materializations(value, &actual, &materialized)?;
        }
        Ok(materialized)
    }

    pub(super) fn record_collection_construction(&self, span: SourceSpan, target: &Ty) {
        let Ty::Struct(name, _) = target else {
            return;
        };
        let insert = if list_element(target).is_some() {
            Some(format!("{name}.append"))
        } else if set_element(target).is_some() {
            Some(format!("{name}.add"))
        } else if dict_elements(target).is_some() {
            Some(format!("{name}.__setitem__"))
        } else {
            None
        };
        self.operation_adjustments.borrow_mut().insert(
            span,
            crate::checked::SemanticAdjustment::ConstructCollection {
                target: target.clone(),
                insert,
            },
        );
    }

    /// Type a `List` construction: `List[T](args)` (explicit element type) or
    /// `List(args)` (element type inferred from the arguments — non-empty).
    pub(super) fn infer_list_construction(
        &self,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        if !param_args.is_empty() {
            let collection = self.list_type(param_args)?;
            let elem = list_element(&collection)
                .expect("List type construction has one type argument")
                .clone();
            for (i, arg) in args.iter().enumerate() {
                let aty = if self.type_contains_reference(&elem) {
                    self.infer_storage_value(arg, &elem)?
                } else {
                    self.infer(arg)?
                };
                if !Self::storage_value_coerces(&aty, &elem) {
                    return Err(TypeError::TypeMismatch {
                        expected: elem.to_string(),
                        found: aty.to_string(),
                        context: format!("element {} of List", i + 1),
                    });
                }
                self.record_literal_materializations(arg, &aty, &elem)?;
                self.mark_reference_storage_uses(arg, &elem);
            }
            return Ok(list_type(elem));
        }
        if args.is_empty() {
            return Err(TypeError::CannotInferTypeParam {
                name: "List".to_string(),
                param: "T".to_string(),
            });
        }
        Ok(list_type(self.infer_list_elem(args)?))
    }

    /// Type `Tuple(args...)` (element types inferred) and
    /// `Tuple[T1, ..., Tn](args...)` (fixed, element-wise checked).
    pub(super) fn infer_tuple_construction(
        &self,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
    ) -> Result<Ty, TypeError> {
        if param_args.is_empty() {
            return args
                .iter()
                .map(|arg| self.infer(arg))
                .collect::<Result<Vec<_>, _>>()
                .map(|elements| self.public_tuple_type(elements));
        }
        let tuple = self.tuple_type(param_args)?;
        let elements = tuple_elements(&tuple)
            .expect("Tuple type construction has type arguments")
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        if elements.len() != args.len() {
            return Err(TypeError::ArityMismatch {
                name: "Tuple".to_string(),
                expected: elements.len(),
                got: args.len(),
            });
        }
        for (index, (argument, expected)) in args.iter().zip(&elements).enumerate() {
            let actual = if self.type_contains_reference(expected) {
                self.infer_storage_value(argument, expected)?
            } else {
                self.infer(argument)?
            };
            if !Self::storage_value_coerces(&actual, expected) {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: actual.to_string(),
                    context: format!("element {} of Tuple", index + 1),
                });
            }
            self.mark_reference_storage_uses(argument, expected);
        }
        Ok(self.public_tuple_type(elements))
    }

    /// Select the concrete variadic-struct specialization after the compiler's
    /// discovery pass has materialized it. During discovery no such declaration
    /// exists yet, so the canonical `Tuple[T0, ...]` type remains available for
    /// collecting the exact specialization request.
    /// The checked type of a t-string occurrence: the concrete `TString`
    /// specialization when the compiler's discovery pass has materialized it,
    /// and the public nominal `TString[...]` spelling before that.
    pub(super) fn public_tstring_type(&self, elements: Vec<Ty>) -> Ty {
        let specialized = crate::comptime::tstring_specialization_symbol(&elements);
        let arguments = elements.iter().cloned().map(TyArg::Ty).collect::<Vec<_>>();
        match self.structs.get(&specialized) {
            Some(info) if info.fixed_arguments.as_ref() == Some(&arguments) => {
                Ty::Struct(specialized, arguments)
            }
            _ if self.declared_structs.contains(&specialized) => Ty::Struct(specialized, arguments),
            _ => crate::types::tstring_type(elements),
        }
    }

    pub(super) fn public_tuple_type(&self, elements: Vec<Ty>) -> Ty {
        let specialized = crate::comptime::tuple_specialization_symbol(&elements);
        let arguments = elements.iter().cloned().map(TyArg::Ty).collect::<Vec<_>>();
        match self.structs.get(&specialized) {
            Some(info) if info.fixed_arguments.as_ref() == Some(&arguments) => {
                Ty::Struct(specialized, arguments)
            }
            _ if self.declared_structs.contains(&specialized) => Ty::Struct(specialized, arguments),
            _ => nominal_tuple_type(elements),
        }
    }

    /// Replace every closed public Tuple nested in an instantiated generic
    /// result with the concrete declaration materialized by the compiler's
    /// discovery pass. Generic declarations themselves retain canonical
    /// `Tuple[T, ...]`; only a substituted use can select an executable nominal
    /// implementation.
    pub(super) fn canonicalize_public_tuple_types(&self, ty: Ty) -> Ty {
        if let Some(elements) = tuple_elements(&ty) {
            let elements = elements
                .into_iter()
                .cloned()
                .map(|element| self.canonicalize_public_tuple_types(element))
                .collect();
            return self.public_tuple_type(elements);
        }
        match ty {
            Ty::Struct(name, arguments) => Ty::Struct(
                name,
                arguments
                    .into_iter()
                    .map(|argument| match argument {
                        TyArg::Ty(ty) => TyArg::Ty(self.canonicalize_public_tuple_types(ty)),
                        value => value,
                    })
                    .collect(),
            ),
            Ty::ComptimeList(element) => {
                Ty::ComptimeList(Box::new(self.canonicalize_public_tuple_types(*element)))
            }
            Ty::Tuple(elements) => Ty::Tuple(
                elements
                    .into_iter()
                    .map(|element| self.canonicalize_public_tuple_types(element))
                    .collect(),
            ),
            Ty::RuntimePack(elements) => Ty::RuntimePack(
                elements
                    .into_iter()
                    .map(|element| self.canonicalize_public_tuple_types(element))
                    .collect(),
            ),
            Ty::VariadicPack(element) => {
                Ty::VariadicPack(Box::new(self.canonicalize_public_tuple_types(*element)))
            }
            Ty::Variant(alternatives) => Ty::Variant(
                alternatives
                    .into_iter()
                    .map(|alternative| self.canonicalize_public_tuple_types(alternative))
                    .collect(),
            ),
            Ty::Pointer { element, origin } => Ty::Pointer {
                element: Box::new(self.canonicalize_public_tuple_types(*element)),
                origin,
            },
            Ty::Ref(mut reference) => {
                reference.referent =
                    Box::new(self.canonicalize_public_tuple_types(*reference.referent));
                Ty::Ref(reference)
            }
            other => other,
        }
    }

    pub(super) fn infer_variant_construction(
        &self,
        span: SourceSpan,
        param_args: &[crate::ast::ParamArg],
        args: &[Expr],
        kwargs: &[crate::ast::KwArg],
    ) -> Result<Ty, TypeError> {
        if !kwargs.is_empty() {
            return Err(TypeError::BadCall {
                func: "Variant".to_string(),
                reason: "keyword arguments are not supported".to_string(),
            });
        }
        if args.len() != 1 {
            return Err(TypeError::ArityMismatch {
                name: "Variant".to_string(),
                expected: 1,
                got: args.len(),
            });
        }
        let Ty::Variant(alternatives) = self.variant_type(param_args)? else {
            return Err(TypeError::InvariantViolation(
                "Variant type construction did not produce a variant".to_string(),
            ));
        };
        let actual = self.infer(&args[0])?;
        let exact: Vec<_> = alternatives
            .iter()
            .enumerate()
            .filter(|(_, alternative)| **alternative == actual)
            .collect();
        // A bare literal first materializes to its ordinary scalar type. Current
        // Mojo therefore chooses `Int` for `Variant[Int, UInt](1)` instead of
        // treating both numeric conversions as equally good.
        let materialized = default_literal(&actual);
        let materialized_exact: Vec<_> = alternatives
            .iter()
            .enumerate()
            .filter(|(_, alternative)| **alternative == materialized)
            .collect();
        let candidates: Vec<_> = if !exact.is_empty() {
            exact
        } else if !materialized_exact.is_empty() {
            materialized_exact
        } else {
            alternatives
                .iter()
                .enumerate()
                .filter(|(_, alternative)| self.value_coerces(&actual, alternative))
                .collect()
        };
        let [(index, selected)] = candidates.as_slice() else {
            return Err(TypeError::BadCall {
                func: "Variant".to_string(),
                reason: if candidates.is_empty() {
                    format!("'{actual}' is not one of its declared alternatives")
                } else {
                    format!("'{actual}' matches more than one declared alternative")
                },
            });
        };
        if !self.record_implicit_conversion(&args[0], &actual, selected)? {
            return Err(TypeError::TypeMismatch {
                expected: selected.to_string(),
                found: actual.to_string(),
                context: "Variant payload".to_string(),
            });
        }
        self.operation_adjustments.borrow_mut().insert(
            span,
            crate::checked::SemanticAdjustment::ConstructVariant {
                alternatives: alternatives.clone(),
                index: *index,
            },
        );
        Ok(Ty::Variant(alternatives))
    }
}

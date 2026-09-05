//! Place validation, subscript/index inference and assignment (`__getitem__`/
//! `__setitem__`, slices, multi-index), pointer offset/write checks, and member
//! access inference. Extracted from `checker.rs`; see `docs/symbol-map.md`.

use super::*;

impl Checker {
    /// Validate an assignment **place** and return the type stored there. A place
    /// is a chain of field (`.x`) and index (`[i]`) accesses over a root that
    /// must be a mutable location: any variable, or `self` in a `mut self`
    /// method. Recursing on the object of each step verifies the whole chain is
    /// rooted at a mutable place (so `foo().x = e` or `self.x` in a read-only
    /// method are rejected). SIMD lane writes are not supported yet.
    pub(super) fn check_place(&self, place: &Expr) -> Result<Ty, TypeError> {
        let result = self.check_place_impl(place);
        if let Ok(ty) = &result {
            self.expression_types
                .borrow_mut()
                .insert(place.source_span(), ty.clone());
            if let ExprKind::Identifier(name) = &place.kind
                && let Some(owner) = self.lookup_owner(name)
            {
                self.expression_bindings
                    .borrow_mut()
                    .insert(place.source_span(), owner);
            }
            let storage_ty = self.place_storage_ty(place).or_else(|| Some(ty.clone()));
            if let Some(storage_ty) = storage_ty {
                self.expression_place_types
                    .borrow_mut()
                    .insert(place.source_span(), storage_ty);
            }
        }
        result
    }

    pub(super) fn place_storage_ty(&self, place: &Expr) -> Option<Ty> {
        match &place.kind {
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
            ExprKind::Index { object, index } => self.index_storage_ty(object, index),
            _ => None,
        }
    }

    /// Type physically stored at an index place, before the usual read-through
    /// rule for a reference element.  Tuple indices are compile-time constants,
    /// while homogeneous list/pointer storage has one element type.
    pub(super) fn index_storage_ty(&self, object: &Expr, index: &Expr) -> Option<Ty> {
        let object_ty = self.infer(object).ok()?;
        if let Some(elements) = tuple_elements(&object_ty) {
            let index = usize::try_from(self.eval_ct(index).ok()?.to_i64()?).ok()?;
            return elements.get(index).map(|element| (*element).clone());
        }
        if let Some(element) = list_element(&object_ty) {
            return Some(element.clone());
        }
        if let Some((_, value)) = dict_elements(&object_ty) {
            return Some(value.clone());
        }
        match object_ty {
            Ty::Tuple(elements) => {
                let index = usize::try_from(self.eval_ct(index).ok()?.to_i64()?).ok()?;
                elements.get(index).cloned()
            }
            Ty::Pointer { element, .. } => Some(*element),
            Ty::Simd { dtype, .. } => Some(simd_ty(dtype, 1)),
            _ => None,
        }
    }

    pub(super) fn check_place_impl(&self, place: &Expr) -> Result<Ty, TypeError> {
        match &place.kind {
            ExprKind::Identifier(name) => {
                if name == "self" && !self.self_mutable {
                    return Err(TypeError::ImmutableSelf);
                }
                if !self.is_binding_mutable(name) {
                    return Err(TypeError::ImmutableBinding(name.clone()));
                }
                self.lookup(name)
                    .map(|ty| match ty {
                        Ty::Ref(reference) => (*reference.referent).clone(),
                        other => other.clone(),
                    })
                    .ok_or_else(|| TypeError::UndefinedVariable(name.clone()))
            }
            ExprKind::Member { object, field } => {
                // The object must itself be a writable place (a struct value).
                self.check_place(object)?;
                // Reuse the field-typing logic (validates the field exists).
                self.infer_member(object, field)
            }
            // A type-keyed projection target (`v[Int] = x`, `v[Int] += x`,
            // `v[List[Int]][0] = x`): the accessor call's mutable reference
            // result is the place; its contract and reference are recorded
            // at this expression by the value-position inference.
            ExprKind::Index { object, index } if self.type_keyed_projection(object, index) => {
                let ExprKind::Identifier(name) = &object.kind else {
                    unreachable!("type-keyed projection roots at an identifier")
                };
                self.check_capture_access(name, true)?;
                if !self.is_binding_mutable(name) {
                    return Err(TypeError::ImmutableBinding(name.clone()));
                }
                let referent = self.infer(place)?;
                let reference = self
                    .selected_calls
                    .borrow()
                    .get(&place.source_span())
                    .and_then(|contract| contract.reference_result.clone());
                match reference {
                    Some(reference)
                        if reference.mutability != mojito_types::origin::Mutability::Immutable =>
                    {
                        // The place retains the accessor's handle; it is
                        // written through, never read out as a value. Writing
                        // the projected payload replaces it, so references
                        // into the previous payload's interior expire (the
                        // storage's own replacement rule).
                        self.reference_value_uses
                            .borrow_mut()
                            .insert(place.source_span(), true);
                        self.record_interior_invalidation(place.source_span(), object);
                        Ok(referent)
                    }
                    Some(_) => Err(TypeError::ImmutableBinding(
                        "immutable reference returned by '__getitem_param__'".to_string(),
                    )),
                    None => Err(TypeError::InvalidAssignTarget(format!(
                        "'{name}[…]' is a value, not a place"
                    ))),
                }
            }
            ExprKind::Index { object, index } => {
                let obj_ty = self.check_place(object)?;
                // A subscript-shaped Variant projection target (`storage[T] = x`
                // on a Variant-typed field, `v[String] = x`).
                if let Ty::Variant(alternatives) = &obj_ty
                    && let Some(arg) = self.variant_projection_type_argument(index)
                {
                    let alternatives = alternatives.clone();
                    let (index, alternative) =
                        self.variant_alternative(&alternatives, std::slice::from_ref(&arg))?;
                    self.operation_adjustments.borrow_mut().insert(
                        place.source_span(),
                        mojito_checked::checked::SemanticAdjustment::VariantProject {
                            alternatives,
                            index,
                        },
                    );
                    if let ExprKind::Identifier(name) = &object.kind
                        && let Some(owner) = self.lookup_owner(name)
                    {
                        self.expression_bindings
                            .borrow_mut()
                            .insert(place.source_span(), owner);
                    }
                    return Ok(alternative);
                }
                // The empty-subscript store `p[] = v` writes the pointee at
                // offset 0; the provenance must carry mutable capability.
                if matches!(index.kind, ExprKind::EmptySubscript) {
                    return match &obj_ty {
                        Ty::Pointer { element, origin } => {
                            self.check_pointer_write(origin)?;
                            Ok((**element).clone())
                        }
                        other => Err(TypeError::Unsupported(format!(
                            "an empty subscript ('value[]') is the pointer dereference; \
                             '{other}' is not a pointer"
                        ))),
                    };
                }
                self.prepare_index_argument(&obj_ty, index, "__setitem__", 0)?;
                if let Ty::Struct(name, _) = &obj_ty
                    && !self.structs.contains_key(name)
                {
                    if let Some(element) = list_element(&obj_ty) {
                        let idx_ty = self.infer(index)?;
                        if !self.is_index_type(&idx_ty) {
                            return Err(TypeError::TypeMismatch {
                                expected: "Indexer".to_string(),
                                found: idx_ty.to_string(),
                                context: "index".to_string(),
                            });
                        }
                        return Ok(match element.clone() {
                            Ty::Ref(reference) => *reference.referent,
                            element => element,
                        });
                    }
                    if let Some((key, value)) = dict_elements(&obj_ty) {
                        let idx_ty = self.infer(index)?;
                        if !coerces(&idx_ty, key) {
                            return Err(TypeError::TypeMismatch {
                                expected: key.to_string(),
                                found: idx_ty.to_string(),
                                context: "dictionary key".to_string(),
                            });
                        }
                        // A mapping setitem invalidates the receiver's interior
                        // generations in the focused builtin path too, matching
                        // the registered `mut self` contract: mapping mutation
                        // during iteration is rejected, not snapshot-tolerated.
                        // Recorded at the written place's site, where the
                        // assignment lowering consumes invalidation facts.
                        self.record_interior_invalidation(place.source_span(), object);
                        return Ok(match value.clone() {
                            Ty::Ref(reference) => *reference.referent,
                            value => value,
                        });
                    }
                    if tuple_elements(&obj_ty).is_some() {
                        return Err(TypeError::InvalidAssignTarget(
                            "Tuple elements are immutable".to_string(),
                        ));
                    }
                }
                // A user struct with `__setitem__(mut self, i, v)` is index-assignable:
                // `c[i] = e` → `c.__setitem__(i, e)`. The index must coerce to the
                // first parameter; the *target* type (what `e` must be) is the second.
                if let Ty::Struct(_, _) = &obj_ty {
                    let mut idx_ty = self.infer(index)?;
                    if self.has_index_normalization(index, &Ty::Int) {
                        idx_ty = Ty::Int;
                    }
                    let resolution = self.resolve_struct_setitem(&obj_ty, &[idx_ty], None)?;
                    if let Some(target) = resolution.lowered_name {
                        self.overload_targets
                            .borrow_mut()
                            .insert(place.source_span(), target);
                    }
                    self.subscript_descriptors
                        .borrow_mut()
                        .insert(place.source_span(), (vec![None], resolution.value_keyword));
                    return Ok(match resolution.return_type {
                        // Indexing a reference-valued container reads and writes
                        // through the stored handle. The physical storage type
                        // remains available through `place_storage_ty`; the
                        // assignment target is the referent.
                        Ty::Ref(reference) => *reference.referent,
                        value => value,
                    });
                }
                let elem = match &obj_ty {
                    // A pointer store `ptr[i] = e`: the target is the pointee type.
                    // An origin-bearing pointer designates one value and its
                    // provenance must carry mutable capability.
                    Ty::Pointer { element, origin } => {
                        self.check_pointer_offset(origin, index)?;
                        self.check_pointer_write(origin)?;
                        (**element).clone()
                    }
                    // A SIMD lane write `v[i] = e`: the target is the width-1 scalar.
                    Ty::Simd { dtype, .. } => simd_ty(*dtype, 1),
                    _ => return Err(TypeError::NotIndexable(obj_ty.to_string())),
                };
                let idx_ty = self.infer(index)?;
                if !self.is_index_type(&idx_ty) {
                    return Err(TypeError::TypeMismatch {
                        expected: "Indexer".to_string(),
                        found: idx_ty.to_string(),
                        context: "index".to_string(),
                    });
                }
                Ok(match elem {
                    Ty::Ref(reference) => *reference.referent,
                    other => other,
                })
            }
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                explicit_step,
            } => {
                let object_type = self.check_place(object)?;
                self.check_slice_bounds(lower.as_deref(), upper.as_deref(), step.as_deref())?;
                let kind = if *explicit_step {
                    SliceKind::StridedSlice
                } else {
                    SliceKind::ContiguousSlice
                };
                let descriptor = Ty::Struct(kind.type_name().to_string(), Vec::new());
                let resolution = self.resolve_struct_setitem(&object_type, &[descriptor], None)?;
                if let Some(target) = resolution.lowered_name {
                    self.overload_targets
                        .borrow_mut()
                        .insert(place.source_span(), target);
                }
                self.subscript_descriptors.borrow_mut().insert(
                    place.source_span(),
                    (vec![Some(kind)], resolution.value_keyword),
                );
                Ok(resolution.return_type)
            }
            ExprKind::MultiIndex { object, args } => {
                let object_type = self.check_place(object)?;
                let mut argument_types = Vec::with_capacity(args.len());
                let mut descriptors = Vec::with_capacity(args.len());
                for (position, argument) in args.iter().enumerate() {
                    match argument {
                        SubscriptArg::Keyword { name, .. }
                        | SubscriptArg::KeywordSlice { name, .. } => {
                            return Err(TypeError::Unsupported(format!(
                                "keyword subscript assignment ('[{name}=…] = value') is not supported; keyword subscripts are read-only"
                            )));
                        }
                        SubscriptArg::Index(value) => {
                            self.prepare_index_argument(
                                &object_type,
                                value,
                                "__setitem__",
                                position,
                            )?;
                            let mut argument_type = self.infer(value)?;
                            if self.has_index_normalization(value, &Ty::Int) {
                                argument_type = Ty::Int;
                            }
                            argument_types.push(argument_type);
                            descriptors.push(None);
                        }
                        SubscriptArg::Slice {
                            lower,
                            upper,
                            step,
                            explicit_step,
                        } => {
                            self.check_slice_bounds(
                                lower.as_deref(),
                                upper.as_deref(),
                                step.as_deref(),
                            )?;
                            let kind = if *explicit_step {
                                SliceKind::StridedSlice
                            } else {
                                SliceKind::ContiguousSlice
                            };
                            argument_types
                                .push(Ty::Struct(kind.type_name().to_string(), Vec::new()));
                            descriptors.push(Some(kind));
                        }
                    }
                }
                let resolution =
                    self.resolve_struct_setitem(&object_type, &argument_types, None)?;
                if let Some(target) = resolution.lowered_name {
                    self.overload_targets
                        .borrow_mut()
                        .insert(place.source_span(), target);
                }
                self.subscript_descriptors
                    .borrow_mut()
                    .insert(place.source_span(), (descriptors, resolution.value_keyword));
                Ok(resolution.return_type)
            }
            ExprKind::TypeApply { name, args } => {
                self.check_capture_access(name, true)?;
                if !self.is_binding_mutable(name) {
                    return Err(TypeError::ImmutableBinding(name.clone()));
                }
                let Ty::Variant(alternatives) = self
                    .lookup(name)
                    .cloned()
                    .ok_or_else(|| TypeError::UndefinedVariable(name.clone()))?
                else {
                    return Err(TypeError::InvalidAssignTarget(name.clone()));
                };
                let (index, alternative) = self.variant_alternative(&alternatives, args)?;
                self.operation_adjustments.borrow_mut().insert(
                    place.source_span(),
                    mojito_checked::checked::SemanticAdjustment::VariantProject {
                        alternatives,
                        index,
                    },
                );
                if let Some(owner) = self.lookup_owner(name) {
                    self.expression_bindings
                        .borrow_mut()
                        .insert(place.source_span(), owner);
                }
                Ok(alternative)
            }
            ExprKind::MethodCall { method, .. } => Err(TypeError::InvalidAssignTarget(format!(
                "the temporary result of method call '{method}()'"
            ))),
            ExprKind::Call { name, .. } => Err(TypeError::InvalidAssignTarget(format!(
                "the temporary result of call '{name}()'"
            ))),
            other => Err(TypeError::InvalidAssignTarget(format!("{:?}", other))),
        }
    }

    pub(super) fn check_slice_bounds(
        &self,
        lower: Option<&Expr>,
        upper: Option<&Expr>,
        step: Option<&Expr>,
    ) -> Result<(), TypeError> {
        for bound in [lower, upper, step].into_iter().flatten() {
            let found = self.infer(bound)?;
            if !coerces(&found, &Ty::Int) {
                return Err(TypeError::TypeMismatch {
                    expected: "Int".to_string(),
                    found: found.to_string(),
                    context: "slice bound".to_string(),
                });
            }
        }
        Ok(())
    }

    /// A slice descriptor is a real runtime argument to `__getitem__`/
    /// `__setitem__`, but its source syntax is spread across optional bounds.
    /// Give overload/origin/effect checking one typed synthetic argument while
    /// retaining descriptor construction separately for MIR.
    pub(super) fn synthetic_slice_descriptor(&self, span: &SourceSpan, kind: SliceKind) -> Expr {
        let mut expression = Expr::new(ExprKind::None, span.span);
        expression.source = span.source.clone();
        self.expression_types.borrow_mut().insert(
            expression.source_span(),
            Ty::Struct(kind.type_name().to_string(), Vec::new()),
        );
        expression
    }

    /// Whether `expression` is the checker-only value standing in for source
    /// slice syntax while selecting `__getitem__`/`__setitem__`. Current Mojo
    /// permits descriptor-family widening (for example `ContiguousSlice` to
    /// `Slice`), but does not feed a slice literal through an arbitrary user
    /// `@implicit` constructor. Keeping that boundary here also prevents a
    /// conversion for a nonexistent expression from being attached to the
    /// enclosing subscript's source span.
    pub(super) fn is_synthetic_slice_descriptor(&self, expression: &Expr) -> bool {
        matches!(expression.kind, ExprKind::None)
            && matches!(
                self.expression_types
                    .borrow()
                    .get(&expression.source_span()),
                Some(Ty::Struct(name, args))
                    if matches!(
                        name.as_str(),
                        "Slice" | "ContiguousSlice" | "StridedSlice"
                    ) && args.is_empty()
            )
    }

    /// Check one nominal `object[...] = value` as the exact selected
    /// `__setitem__` invocation. `Ok(None)` leaves primitive pointer/SIMD and
    /// compiler-private storage assignments on their intrinsic place path.
    pub(super) fn check_nominal_subscript_assignment(
        &self,
        target: &Expr,
        value: &Expr,
    ) -> Result<Option<Ty>, TypeError> {
        let (object, mut arguments, descriptors) = match &target.kind {
            // A type-keyed projection target writes through the accessor's
            // reference on the ordinary place path.
            ExprKind::Index { object, index } if self.type_keyed_projection(object, index) => {
                return Ok(None);
            }
            ExprKind::Index { object, index } => {
                let object_ty = self.infer(object)?;
                if !matches!(&object_ty, Ty::Struct(name, _) if self.structs.contains_key(name)) {
                    return Ok(None);
                }
                // A base that is itself a reference result (`v[List[Int]][0]
                // = x` on the self-hosted `Variant`) is borrowed for the
                // setter like any method receiver.
                if self.infer_reference_value(object).is_some() {
                    self.borrowed_reference_receivers
                        .borrow_mut()
                        .insert(object.source_span());
                }
                self.prepare_index_argument(&object_ty, index, "__setitem__", 0)?;
                self.infer(index)?;
                (object.as_ref(), vec![index.as_ref().clone()], vec![None])
            }
            ExprKind::Slice {
                object,
                lower,
                upper,
                step,
                explicit_step,
            } => {
                let object_ty = self.infer(object)?;
                if !matches!(&object_ty, Ty::Struct(name, _) if self.structs.contains_key(name)) {
                    return Ok(None);
                }
                self.check_slice_bounds(lower.as_deref(), upper.as_deref(), step.as_deref())?;
                let kind = if *explicit_step {
                    SliceKind::StridedSlice
                } else {
                    SliceKind::ContiguousSlice
                };
                let descriptor = self.synthetic_slice_descriptor(&target.source_span(), kind);
                (object.as_ref(), vec![descriptor], vec![Some(kind)])
            }
            ExprKind::MultiIndex {
                object,
                args: source,
            } => {
                let object_ty = self.infer(object)?;
                if !matches!(&object_ty, Ty::Struct(name, _) if self.structs.contains_key(name)) {
                    return Ok(None);
                }
                let mut arguments = Vec::with_capacity(source.len());
                let mut descriptors = Vec::with_capacity(source.len());
                for (position, argument) in source.iter().enumerate() {
                    match argument {
                        SubscriptArg::Keyword { name, .. }
                        | SubscriptArg::KeywordSlice { name, .. } => {
                            return Err(TypeError::Unsupported(format!(
                                "keyword subscript assignment ('[{name}=…] = value') is not supported; keyword subscripts are read-only"
                            )));
                        }
                        SubscriptArg::Index(index) => {
                            self.prepare_index_argument(
                                &object_ty,
                                index,
                                "__setitem__",
                                position,
                            )?;
                            self.infer(index)?;
                            arguments.push(index.clone());
                            descriptors.push(None);
                        }
                        SubscriptArg::Slice {
                            lower,
                            upper,
                            step,
                            explicit_step,
                        } => {
                            self.check_slice_bounds(
                                lower.as_deref(),
                                upper.as_deref(),
                                step.as_deref(),
                            )?;
                            let kind = if *explicit_step {
                                SliceKind::StridedSlice
                            } else {
                                SliceKind::ContiguousSlice
                            };
                            arguments
                                .push(self.synthetic_slice_descriptor(&target.source_span(), kind));
                            descriptors.push(Some(kind));
                        }
                    }
                }
                (object.as_ref(), arguments, descriptors)
            }
            _ => return Ok(None),
        };

        let object_ty = self.infer(object)?;
        // `receiver[index] = value` without any `__setitem__`: current Mojo
        // writes through a mutable-reference-returning `__getitem__` (upstream
        // `Array` has no setter). Slice and multi-index assignment keep the
        // setter requirement.
        let has_setitem = matches!(&object_ty, Ty::Struct(name, _) if self
            .structs
            .get(name)
            .is_some_and(|info| info.methods.contains_key("__setitem__")));
        if !has_setitem && matches!(&target.kind, ExprKind::Index { .. }) {
            return self
                .check_subscript_reference_assignment(target, value, &object_ty)
                .map(Some);
        }
        let index_argument_count = arguments.len();
        let value_keyword = self.select_subscript_set_call_shape(&object_ty, &arguments, value)?;
        let kwargs = if value_keyword {
            vec![mojito_ast::ast::KwArg {
                name: "value".to_string(),
                value: value.clone(),
            }]
        } else {
            arguments.push(value.clone());
            Vec::new()
        };
        // List element replacement is allocation-stable.  Its projected write
        // is recorded after this check as `list[AnyIndex]`, preserving the
        // List's `element` generation while still expiring interiors nested in
        // the replaced element. Slice assignment and arbitrary user setters
        // retain the conservative whole-receiver mutation effect.
        let preserves_receiver_interiors =
            matches!(&target.kind, ExprKind::Index { .. }) && list_element(&object_ty).is_some();
        let call = if preserves_receiver_interiors {
            MethodCallArguments::interior_preserving(&arguments, &kwargs)
        } else {
            MethodCallArguments::ordinary(&arguments, &kwargs)
        };
        self.infer_method_call(target.source_span(), object, "__setitem__", call)?;
        let contract = self
            .selected_calls
            .borrow()
            .get(&target.source_span())
            .cloned()
            .ok_or_else(|| {
                TypeError::InvariantViolation(
                    "nominal subscript assignment lost its selected call contract".to_string(),
                )
            })?;
        if contract.receiver_convention != Some(ArgConvention::Mut) {
            return Err(TypeError::TypeMismatch {
                expected: "a 'mut self' __setitem__".to_string(),
                found: "read-only self".to_string(),
                context: format!("index assignment on '{object_ty}'"),
            });
        }
        let value_source = if value_keyword {
            mojito_checked::checked::CheckedCallArgumentSource::Keyword(0)
        } else {
            mojito_checked::checked::CheckedCallArgumentSource::Positional(index_argument_count)
        };
        let target_ty = contract
            .arguments
            .iter()
            .find(|argument| argument.source == value_source)
            .map(|argument| argument.parameter_ty.clone())
            .ok_or_else(|| {
                TypeError::InvariantViolation(
                    "selected __setitem__ contract has no assignment-value slot".to_string(),
                )
            })?;
        self.subscript_descriptors
            .borrow_mut()
            .insert(target.source_span(), (descriptors, value_keyword));
        let target_ty = match target_ty {
            Ty::Ref(reference) => *reference.referent,
            ty => ty,
        };
        self.expression_types
            .borrow_mut()
            .insert(target.source_span(), target_ty.clone());
        self.expression_place_types
            .borrow_mut()
            .insert(target.source_span(), target_ty.clone());
        Ok(Some(target_ty))
    }

    /// Check `receiver[index] = value` against a mutable-reference-returning
    /// `__getitem__`: the getter contract is selected by ordinary subscript
    /// inference, the right-hand side is typed against the referent, and the
    /// recorded fact is an augmented reference write with no operator step
    /// (`setter: None`, `inplace: None`), so MIR finishes with `WriteRef`.
    fn check_subscript_reference_assignment(
        &self,
        target: &Expr,
        value: &Expr,
        object_ty: &Ty,
    ) -> Result<Ty, TypeError> {
        let element = self.infer(target)?;
        let site = target.source_span();
        let getter = self
            .selected_calls
            .borrow()
            .get(&site)
            .cloned()
            .ok_or_else(|| {
                TypeError::InvariantViolation(
                    "subscript reference assignment lost its getter contract".to_string(),
                )
            })?;
        let Some(reference) = &getter.reference_result else {
            // A value-returning getter offers no write-through path; without a
            // setter the receiver is simply not index-assignable.
            return Err(TypeError::NotIndexable(object_ty.to_string()));
        };
        // A provably immutable reference rejects here; a parametric-mut
        // reference (`Origin[mut=m]`) is judged per instantiation at the
        // enclosing method's call sites instead.
        if reference.mutability == mojito_types::origin::Mutability::Immutable {
            return Err(TypeError::ImmutableBinding(
                "immutable reference returned by '__getitem__'".to_string(),
            ));
        }
        let found = self.infer_with_expected(value, &element, true)?;
        if !self.record_implicit_conversion(value, &found, &element)? {
            return Err(TypeError::TypeMismatch {
                expected: element.to_string(),
                found: found.to_string(),
                context: format!("index assignment on '{object_ty}'"),
            });
        }
        self.check_consuming(value, &found, "index assignment")?;
        self.subscript_descriptors
            .borrow_mut()
            .entry(site.clone())
            .or_insert((vec![None], false));
        self.expression_types
            .borrow_mut()
            .insert(site.clone(), element.clone());
        self.expression_place_types
            .borrow_mut()
            .insert(site.clone(), element.clone());
        self.operation_adjustments.borrow_mut().insert(
            site,
            mojito_checked::checked::SemanticAdjustment::AugmentedSubscript(Box::new(
                mojito_checked::checked::CheckedAugmentedSubscript {
                    getter,
                    setter: None,
                    inplace: None,
                    operand_ty: element.clone(),
                    result_ty: element.clone(),
                    value_source: None,
                },
            )),
        );
        Ok(element)
    }

    /// Select how assignment syntax supplies the implicit RHS to
    /// `__setitem__`. Both the final-positional and `value=` shapes participate
    /// in ordinary overload scoring, including RHS-driven generic inference.
    /// A signature which accepts both shapes is considered only once and uses
    /// the conventional positional ABI; distinct equally-ranked signatures are
    /// still an ambiguous overload.
    pub(super) fn select_subscript_set_call_shape(
        &self,
        receiver: &Ty,
        index_arguments: &[Expr],
        value: &Expr,
    ) -> Result<bool, TypeError> {
        let Ty::Struct(name, type_arguments) = receiver else {
            return Err(TypeError::NotIndexable(receiver.to_string()));
        };
        let info = self
            .structs
            .get(name)
            .ok_or_else(|| TypeError::NotIndexable(receiver.to_string()))?;
        let signatures = info
            .methods
            .get("__setitem__")
            .ok_or_else(|| TypeError::NotIndexable(receiver.to_string()))?;
        let receiver_substitution = struct_subst(&info.decls, type_arguments);
        let mut positional_arguments = index_arguments.to_vec();
        positional_arguments.push(value.clone());
        let keyword_arguments = vec![mojito_ast::ast::KwArg {
            name: "value".to_string(),
            value: value.clone(),
        }];
        let no_keywords: &[mojito_ast::ast::KwArg] = &[];
        let shapes = [
            (false, positional_arguments.as_slice(), no_keywords),
            (true, index_arguments, keyword_arguments.as_slice()),
        ];

        // Keep only the best ABI for each declaration. A normal fixed setter
        // accepts its final parameter both positionally and by name; counting
        // those as two overloads would make every such assignment ambiguous.
        let mut best_by_signature: Vec<Option<(usize, bool)>> = vec![None; signatures.len()];
        for (signature_index, signature) in signatures.iter().enumerate() {
            if !signature.has_self {
                continue;
            }
            let receiver_params = signature
                .params
                .iter()
                .map(|parameter| substitute(parameter, &receiver_substitution))
                .collect::<Vec<_>>();
            let receiver_variadic = signature
                .variadic
                .as_deref()
                .map(|parameter| substitute(parameter, &receiver_substitution));
            let receiver_kw_variadic = signature
                .kw_variadic
                .as_deref()
                .map(|parameter| substitute(parameter, &receiver_substitution));
            for (value_keyword, arguments, keywords) in shapes {
                let Ok((params, variadic, kw_variadic, _, mut method_arguments)) = self
                    .instantiate_method_generics(
                        &format!("{name}.__setitem__"),
                        signature,
                        &receiver_params,
                        receiver_variadic.as_ref(),
                        receiver_kw_variadic.as_ref(),
                        &[],
                        arguments,
                        keywords,
                    )
                else {
                    continue;
                };
                for (declaration, argument) in info.decls.iter().zip(type_arguments) {
                    method_arguments.insert(
                        declaration.name().trim_start_matches('*').to_string(),
                        argument.clone(),
                    );
                }
                if !self.method_constraints_apply(signature, &method_arguments) {
                    continue;
                }
                let Ok(scored) = self.score_method_call(
                    signature,
                    &params,
                    variadic.as_ref(),
                    kw_variadic.as_ref(),
                    arguments,
                    keywords,
                ) else {
                    continue;
                };
                let candidate = (scored.rank, value_keyword);
                let replace =
                    best_by_signature[signature_index].is_none_or(|current| candidate < current);
                if replace {
                    best_by_signature[signature_index] = Some(candidate);
                }
            }
        }

        let candidates = best_by_signature.into_iter().flatten().collect::<Vec<_>>();
        let Some(best_rank) = candidates.iter().map(|(rank, _)| *rank).min() else {
            return Err(TypeError::BadCall {
                func: format!("{name}.__setitem__"),
                reason: "no overload matches the supplied indices and assignment value".to_string(),
            });
        };
        let mut selected = candidates
            .into_iter()
            .filter(|(rank, _)| *rank == best_rank);
        let (_, value_keyword) = selected.next().expect("at least one best setter shape");
        if selected.next().is_some() {
            return Err(TypeError::BadCall {
                func: format!("{name}.__setitem__"),
                reason: "ambiguous subscript-assignment overload".to_string(),
            });
        }
        Ok(value_keyword)
    }

    pub(super) fn infer_slice_subscript(
        &self,
        span: SourceSpan,
        object: &Expr,
        lower: Option<&Expr>,
        upper: Option<&Expr>,
        step: Option<&Expr>,
        explicit_step: bool,
    ) -> Result<Ty, TypeError> {
        self.check_slice_bounds(lower, upper, step)?;
        let kind = if explicit_step {
            SliceKind::StridedSlice
        } else {
            SliceKind::ContiguousSlice
        };
        let object_type = self.infer(object)?;
        let result = match &object_type {
            // Positional slicing on a StringLiteral value is gone at the
            // audited head along with nominal String positional slicing;
            // only the keyword-unit spellings remain.
            Ty::StringLiteral => {
                return Err(TypeError::Unsupported(
                    "String positional slicing was removed in current Mojo; spell the \
                     unit explicitly: 's[byte=a:b]' or 's[codepoint=a:b]'"
                        .to_string(),
                ));
            }
            Ty::Struct(name, _)
                if !self.structs.contains_key(name) && list_element(&object_type).is_some() =>
            {
                object_type.clone()
            }
            // Positional String slicing was removed in current Mojo (like
            // bare positional `s[i]`): the byte or codepoint unit is spelled
            // explicitly through a keyword slice.
            Ty::Struct(name, arguments)
                if arguments.is_empty() && mojito_symbol::symbol::is_stdlib_string_struct(name) =>
            {
                return Err(TypeError::Unsupported(
                    "String positional slicing was removed in current Mojo; spell the \
                     unit explicitly: 's[byte=a:b]' or 's[codepoint=a:b]'"
                        .to_string(),
                ));
            }
            Ty::Struct(..) | Ty::Param { .. } => {
                let descriptor = self.synthetic_slice_descriptor(&span, kind);
                self.infer_method_call(
                    span.clone(),
                    object,
                    "__getitem__",
                    MethodCallArguments::ordinary(std::slice::from_ref(&descriptor), &[]),
                )?
            }
            _ => return Err(TypeError::NotIndexable(object_type.to_string())),
        };
        // A view-typed slice result (a Span sub-slice) stays borrowed from
        // its receiver's sources: the result binding inherits the
        // receiver's loans instead of owning fresh storage.
        if self.type_carries_loans(&result) {
            self.operation_adjustments.borrow_mut().insert(
                span.clone(),
                mojito_checked::checked::SemanticAdjustment::BorrowViewResult,
            );
        }
        self.subscript_descriptors
            .borrow_mut()
            .insert(span, (vec![Some(kind)], false));
        Ok(result)
    }

    pub(super) fn infer_multi_subscript(
        &self,
        span: SourceSpan,
        object: &Expr,
        arguments: &[SubscriptArg],
    ) -> Result<Ty, TypeError> {
        let object_type = self.infer(object)?;
        // `ptr[unsafe_offset=i]` is current Mojo's keyword spelling of the
        // indexed dereference (the positional `ptr[i]` is its deprecated
        // form). It is the only keyword subscript a pointer has.
        if let Ty::Pointer { element, origin } = &object_type {
            if let [SubscriptArg::Keyword { name, value }] = arguments
                && name == "unsafe_offset"
            {
                self.check_pointer_offset(origin, value)?;
                let index = self.infer(value)?;
                if !self.is_index_type(&index) {
                    return Err(TypeError::TypeMismatch {
                        expected: "Indexer".to_string(),
                        found: index.to_string(),
                        context: "index".to_string(),
                    });
                }
                return Ok((**element).clone());
            }
            return Err(TypeError::Unsupported(
                "a Pointer keyword subscript is spelled exactly \
                 'pointer[unsafe_offset=i]'"
                    .to_string(),
            ));
        }
        if !matches!(object_type, Ty::Struct(..) | Ty::Param { .. }) {
            return Err(TypeError::NotIndexable(object_type.to_string()));
        }
        let mut actual_arguments = Vec::with_capacity(arguments.len());
        let mut keyword_arguments = Vec::new();
        let mut descriptors = Vec::with_capacity(arguments.len());
        for (position, argument) in arguments.iter().enumerate() {
            match argument {
                SubscriptArg::Index(value) => {
                    self.prepare_index_argument(&object_type, value, "__getitem__", position)?;
                    self.infer(value)?;
                    actual_arguments.push(value.clone());
                    descriptors.push(None);
                }
                SubscriptArg::Keyword { name, value } => {
                    // A keyword subscript binds a keyword-only `__getitem__`
                    // parameter through ordinary structural call binding; the
                    // index-normalization coercion stays positional-only.
                    self.infer(value)?;
                    keyword_arguments.push(mojito_ast::ast::KwArg {
                        name: name.clone(),
                        value: value.clone(),
                    });
                    descriptors.push(None);
                }
                SubscriptArg::Slice {
                    lower,
                    upper,
                    step,
                    explicit_step,
                } => {
                    self.check_slice_bounds(lower.as_deref(), upper.as_deref(), step.as_deref())?;
                    let kind = if *explicit_step {
                        SliceKind::StridedSlice
                    } else {
                        SliceKind::ContiguousSlice
                    };
                    actual_arguments.push(self.synthetic_slice_descriptor(&span, kind));
                    descriptors.push(Some(kind));
                }
                // A keyword slice (`s[byte=a:b]`) binds a keyword-only
                // slice-descriptor `__getitem__` parameter through the same
                // structural call binding as a keyword subscript.
                SubscriptArg::KeywordSlice {
                    name,
                    lower,
                    upper,
                    step,
                    explicit_step,
                } => {
                    self.check_slice_bounds(lower.as_deref(), upper.as_deref(), step.as_deref())?;
                    let kind = if *explicit_step {
                        SliceKind::StridedSlice
                    } else {
                        SliceKind::ContiguousSlice
                    };
                    keyword_arguments.push(mojito_ast::ast::KwArg {
                        name: name.clone(),
                        value: self.synthetic_slice_descriptor(&span, kind),
                    });
                    descriptors.push(Some(kind));
                }
            }
        }
        let result = self.infer_method_call(
            span.clone(),
            object,
            "__getitem__",
            MethodCallArguments::ordinary(&actual_arguments, &keyword_arguments),
        )?;
        // A view-typed subscript result (a StringSpan keyword slice) stays
        // borrowed from its receiver's sources.
        if self.type_carries_loans(&result) {
            self.operation_adjustments.borrow_mut().insert(
                span.clone(),
                mojito_checked::checked::SemanticAdjustment::BorrowViewResult,
            );
        }
        self.subscript_descriptors
            .borrow_mut()
            .insert(span, (descriptors, false));
        Ok(result)
    }

    /// Resolve `receiver[indices...] = value`. The assignment value is the final
    /// regular parameter for a fixed-arity `__setitem__`. A variadic setitem uses
    /// Mojo's `*indices, *, value: T` shape, so lowering must pass the value through
    /// the keyword-only slot while the source indices fill the variadic pack.
    pub(super) fn resolve_struct_setitem(
        &self,
        receiver: &Ty,
        arguments: &[Ty],
        value: Option<&Ty>,
    ) -> Result<SubscriptResolution, TypeError> {
        let Ty::Struct(name, type_arguments) = receiver else {
            return Err(TypeError::NotIndexable(receiver.to_string()));
        };
        let info = self
            .structs
            .get(name)
            .ok_or_else(|| TypeError::NotIndexable(receiver.to_string()))?;
        // A closed receiver instance assigns through the per-instantiation
        // clone family (`__setitem__$y3:Int`) when it exists.
        let method = self
            .instance_method_clone(name, "__setitem__", type_arguments)
            .unwrap_or_else(|| "__setitem__".to_string());
        let signatures = info
            .methods
            .get(&method)
            .ok_or_else(|| TypeError::NotIndexable(receiver.to_string()))?;
        let substitution = struct_subst(&info.decls, type_arguments);
        let mut matches = Vec::new();
        let mut saw_read_only = false;
        let mut value_mismatches = Vec::new();

        for signature in signatures
            .iter()
            .filter(|signature| signature.has_self && signature.decls.is_empty())
        {
            if !matches!(
                signature.self_convention,
                Some(mojito_ast::ast::ArgConvention::Mut)
            ) {
                saw_read_only = true;
                continue;
            }
            let parameters: Vec<_> = signature
                .params
                .iter()
                .map(|parameter| substitute(parameter, &substitution))
                .collect();
            if parameters.is_empty() {
                continue;
            }
            let variadic = signature
                .variadic
                .as_deref()
                .map(|parameter| substitute(parameter, &substitution));

            let (value_index, value_keyword, fixed_index_count) = if variadic.is_some() {
                let Some(value_index) = signature.names.iter().position(|name| name == "value")
                else {
                    continue;
                };
                let fixed_index_count = signature.variadic_index.unwrap_or(0);
                // The currently published variadic operator shape has only a
                // keyword-only `value` parameter after `*indices`.
                if value_index < fixed_index_count || parameters.len() != fixed_index_count + 1 {
                    continue;
                }
                (value_index, true, fixed_index_count)
            } else {
                (parameters.len() - 1, false, parameters.len() - 1)
            };
            if arguments.len() < fixed_index_count
                || (variadic.is_none() && arguments.len() != fixed_index_count)
            {
                continue;
            }

            let mut score = 0;
            let mut compatible = true;
            for (actual, expected) in arguments
                .iter()
                .take(fixed_index_count)
                .zip(parameters.iter().take(fixed_index_count))
            {
                if !coerces(actual, expected) {
                    compatible = false;
                    break;
                }
                score += conversion_count(actual, expected);
            }
            if compatible && let Some(element) = &variadic {
                for actual in arguments.iter().skip(fixed_index_count) {
                    if !coerces(actual, element) {
                        compatible = false;
                        break;
                    }
                    score += conversion_count(actual, element);
                }
            }
            if compatible && let Some(actual) = value {
                let expected = &parameters[value_index];
                if !self.value_coerces(actual, expected)
                    && self.implicit_conversion_target(actual, expected)?.is_none()
                {
                    value_mismatches.push(expected.clone());
                    compatible = false;
                } else {
                    score += conversion_count(actual, expected);
                }
            }
            if compatible {
                matches.push((
                    overload_rank(score, variadic.is_some(), parameters.len(), false),
                    signature,
                    parameters[value_index].clone(),
                    value_keyword,
                ));
            }
        }

        matches.sort_by_key(|(score, _, _, _)| *score);
        let Some((best, signature, value_type, value_keyword)) = matches.first() else {
            if let (Some(found), Some(expected)) = (value, value_mismatches.first()) {
                return Err(TypeError::TypeMismatch {
                    expected: expected.to_string(),
                    found: found.to_string(),
                    context: format!("assignment value for '{name}.__setitem__'"),
                });
            }
            if saw_read_only {
                return Err(TypeError::TypeMismatch {
                    expected: "a 'mut self' __setitem__".to_string(),
                    found: "read-only self".to_string(),
                    context: format!("index assignment on '{name}'"),
                });
            }
            return Err(TypeError::NotIndexable(receiver.to_string()));
        };
        if matches.get(1).is_some_and(|(score, _, _, _)| score == best) {
            return Err(TypeError::BadCall {
                func: format!("{name}.__setitem__"),
                reason: "ambiguous subscript-assignment overload".to_string(),
            });
        }
        Ok(SubscriptResolution {
            return_type: value_type.clone(),
            lowered_name: (signatures.len() > 1 || method != "__setitem__").then(|| {
                if signatures.len() == 1 {
                    return format!("{name}.{method}");
                }
                method_lowered_name(
                    name,
                    &method,
                    signature,
                    self.self_instance_ty(name).as_ref(),
                )
            }),
            value_keyword: *value_keyword,
        })
    }

    /// Type a subscript over tuples, SIMD, lists, pointers, or a user-defined
    /// `__getitem__` implementation.
    pub(super) fn infer_index(
        &self,
        span: SourceSpan,
        object: &Expr,
        index: &Expr,
    ) -> Result<Ty, TypeError> {
        let obj_ty = self.infer(object)?;
        // A reference-typed base (a `ref` field projected off a collection
        // element, or reached through a binding) indexes its referent:
        // select the subscript on the referent type; the VM reads through
        // the handle at dispatch (the read twin of the ref-field store's
        // second dereference).
        let obj_ty = match obj_ty {
            Ty::Ref(reference) => (*reference.referent).clone(),
            other => other,
        };
        // The empty subscript `p[]` dereferences a pointer at offset 0 (always
        // in provenance, so no offset check); any other receiver rejects here
        // rather than dispatching an accessor with the marker as its argument.
        if matches!(index.kind, ExprKind::EmptySubscript) {
            return match &obj_ty {
                Ty::Pointer { element, .. } => Ok((**element).clone()),
                other => Err(TypeError::Unsupported(format!(
                    "an empty subscript ('value[]') is the pointer dereference; \
                     '{other}' is not a pointer"
                ))),
            };
        }
        // Compiler-private inline uninit storage: `storage[0]` designates the
        // payload place (upstream's `self._array[0]`), assuming initialization
        // — an uninitialized payload traps at the VM. Bundled-crossing only.
        if let Some(element) = mojito_types::types::uninit_storage_element(&obj_ty) {
            if !self.bundled_stdlib_declaration {
                return Err(TypeError::Unsupported(format!(
                    "'{}' is compiler-private storage; use MaybeUninit from std.memory",
                    mojito_types::types::UNINIT_STORAGE_TYPE_NAME
                )));
            }
            if !matches!(&index.kind, ExprKind::Int(literal) if literal.to_i64() == Some(0)) {
                return Err(TypeError::Unsupported(
                    "inline uninit storage holds a single payload; index it as 'storage[0]'"
                        .to_string(),
                ));
            }
            return Ok(element.clone());
        }
        self.prepare_index_argument(&obj_ty, index, "__getitem__", 0)?;
        // A generated Tuple declaration may occur later than the generic body
        // currently being checked (the bundled List slice overload is one such
        // body). Phase one has already validated and retained that Tuple's
        // concrete element identity, so select its conventional generated
        // accessor now instead of falling through to the metadata-only Tuple
        // shortcut and losing executable dispatch.
        if let Ty::Struct(name, _) = &obj_ty
            && !self.structs.contains_key(name)
            && self
                .predeclared_generated_tuple_arguments
                .contains_key(name)
        {
            let elements = tuple_elements(&obj_ty).ok_or_else(|| {
                TypeError::InvariantViolation(format!(
                    "predeclared generated Tuple '{name}' lost its element types"
                ))
            })?;
            let exact = self.eval_ct(index).map_err(|_| TypeError::TypeMismatch {
                expected: "a compile-time Int index".to_string(),
                found: "a runtime value".to_string(),
                context: format!("variadic struct '{name}' subscript"),
            })?;
            let k = exact.to_i64().ok_or_else(|| TypeError::TypeMismatch {
                expected: format!("a pack index in 0..{}", elements.len()),
                found: exact.to_string(),
                context: format!("variadic struct '{name}' subscript"),
            })?;
            if k < 0 || k as usize >= elements.len() {
                return Err(TypeError::TypeMismatch {
                    expected: format!("a pack index in 0..{}", elements.len()),
                    found: k.to_string(),
                    context: format!("variadic struct '{name}' subscript"),
                });
            }
            let element = elements[k as usize].clone();
            let value_receiver = !is_place_expr(object);
            let method = if value_receiver {
                if !self.is_implicitly_copyable(&element) {
                    return Err(TypeError::NonCopyable {
                        ty: obj_ty.to_string(),
                        context: "indexing an rvalue Tuple requires an implicitly copyable element"
                            .to_string(),
                    });
                }
                format!("__getitem_param_value__${k}")
            } else {
                format!("__getitem_param__${k}")
            };
            let target = format!("{name}.{method}");
            self.overload_targets
                .borrow_mut()
                .insert(span.clone(), target.clone());
            if value_receiver {
                let result_ty = match &element {
                    Ty::Ref(reference) => reference.referent.as_ref().clone(),
                    value => value.clone(),
                };
                self.selected_calls.borrow_mut().insert(
                    span,
                    mojito_checked::checked::CheckedCallContract {
                        target,
                        raises: None,
                        result_ty: result_ty.clone(),
                        result_adapter: None,
                        receiver_requires_place: false,
                        receiver_elided: false,
                        receiver_convention: None,
                        arguments: Vec::new(),
                        captures: Vec::new(),
                        reference_result: None,
                        parameter_arguments: Vec::new(),
                        param_decls: Vec::new(),
                        boundary: mojito_checked::checked::CheckedCallBoundary::default(),
                    },
                );
                return Ok(result_ty);
            }
            let reference = match element {
                // The generated accessor for a reference-valued element
                // forwards that element's original handle rather than creating
                // an outer reference to the Tuple's private storage slot.
                Ty::Ref(reference) => reference,
                referent => {
                    let receiver = self.reference_actual(object)?;
                    mojito_types::origin::RefTy {
                        referent: Box::new(referent),
                        origin: receiver.origin,
                        mutability: receiver.mutability,
                    }
                }
            };
            let result = (*reference.referent).clone();
            self.selected_calls.borrow_mut().insert(
                span.clone(),
                mojito_checked::checked::CheckedCallContract {
                    target,
                    raises: None,
                    result_ty: Ty::Ref(reference.clone()),
                    result_adapter: None,
                    receiver_requires_place: true,
                    receiver_elided: false,
                    receiver_convention: Some(ArgConvention::Ref),
                    arguments: Vec::new(),
                    captures: Vec::new(),
                    reference_result: Some(reference.clone()),
                    parameter_arguments: Vec::new(),
                    param_decls: Vec::new(),
                    boundary: mojito_checked::checked::CheckedCallBoundary::default(),
                },
            );
            self.operation_adjustments.borrow_mut().insert(
                span,
                mojito_checked::checked::SemanticAdjustment::ReferenceResult { reference },
            );
            return Ok(result);
        }
        // Current Mojo permits a general compile-time parameter subscript hook,
        // not only the dependent accessor family synthesized for variadic
        // structs. Pass the source index as a checked value parameter and no
        // runtime argument; ordinary generic-method specialization then leaves
        // an exact callable target for HIR/MIR.
        if let Ty::Struct(name, _) = &obj_ty
            && self
                .structs
                .get(name)
                .is_some_and(|info| info.methods.contains_key("__getitem_param__"))
        {
            let parameter_arguments = [mojito_ast::ast::ParamArg::Value(index.clone())];
            let result = self.infer_method_call(
                span,
                object,
                "__getitem_param__",
                MethodCallArguments::parameterized(&parameter_arguments, &[], &[]),
            )?;
            return Ok(match result {
                Ty::Ref(reference) => *reference.referent,
                value => value,
            });
        }
        // A specialized variadic struct exposes one concrete dependent
        // accessor per pack element. Resolve it before the generic Tuple
        // element shortcut below: generated public Tuples retain nominal
        // element metadata too, but executable indexing must dispatch through
        // the checked ordinary method rather than a VM tuple intrinsic.
        if let Ty::Struct(name, _) = &obj_ty
            && let Some(info) = self.structs.get(name)
            && let Some(family) = dependent_index_accessor_family(info)
        {
            let count = (0..)
                .take_while(|k| info.methods.contains_key(&format!("{}${k}", family.place)))
                .count();
            let exact = self.eval_ct(index).map_err(|_| TypeError::TypeMismatch {
                expected: "a compile-time Int index".to_string(),
                found: "a runtime value".to_string(),
                context: format!("variadic struct '{name}' subscript"),
            })?;
            let k = exact.to_i64().ok_or_else(|| TypeError::TypeMismatch {
                expected: format!("a pack index in 0..{count}"),
                found: exact.to_string(),
                context: format!("variadic struct '{name}' subscript"),
            })?;
            if k < 0 || k as usize >= count {
                return Err(TypeError::TypeMismatch {
                    expected: format!("a pack index in 0..{count}"),
                    found: k.to_string(),
                    context: format!("variadic struct '{name}' subscript"),
                });
            }
            let value_receiver = !is_place_expr(object);
            let value_method = format!("{}${k}", family.value);
            let method = if value_receiver && info.methods.contains_key(&value_method) {
                value_method
            } else {
                format!("{}${k}", family.place)
            };
            let ret = self.infer_method_call(
                span.clone(),
                object,
                &method,
                MethodCallArguments::ordinary(&[], &[]),
            )?;
            self.overload_targets
                .borrow_mut()
                .insert(span, format!("{name}.{method}"));
            return Ok(ret);
        }
        // A tuple is heterogeneous, so its index must be a **compile-time** `Int`
        // constant — the result type is that element's type.
        let tuple_elements = tuple_elements(&obj_ty).or_else(|| match &obj_ty {
            // Compiler-private heterogeneous pack storage uses the same static
            // per-index typing rule as public Tuple, but never method dispatch.
            Ty::Tuple(elements) => Some(elements.iter().collect()),
            _ => None,
        });
        if let Some(elems) = tuple_elements {
            let exact = self.eval_ct(index).map_err(|_| TypeError::TypeMismatch {
                expected: "a compile-time Int index".to_string(),
                found: "a runtime value".to_string(),
                context: "tuple index".to_string(),
            })?;
            let i = exact.to_i64().ok_or_else(|| TypeError::TypeMismatch {
                expected: format!("a tuple index in 0..{}", elems.len()),
                found: exact.to_string(),
                context: "tuple index".to_string(),
            })?;
            if i < 0 || i as usize >= elems.len() {
                return Err(TypeError::TypeMismatch {
                    expected: format!("a tuple index in 0..{}", elems.len()),
                    found: i.to_string(),
                    context: "tuple index".to_string(),
                });
            }
            return Ok(match elems[i as usize] {
                Ty::Ref(reference) => (*reference.referent).clone(),
                element => element.clone(),
            });
        }
        // A homogeneous runtime variadic collector is compiler-private Tuple
        // storage with one repeated element type. Unlike a heterogeneous pack,
        // its index may vary at runtime without changing the result type.
        if let Ty::VariadicPack(element) = &obj_ty {
            let idx_ty = self.infer(index)?;
            if !self.is_index_type(&idx_ty) {
                return Err(TypeError::TypeMismatch {
                    expected: "Indexer".to_string(),
                    found: idx_ty.to_string(),
                    context: "variadic-pack index".to_string(),
                });
            }
            return Ok(match &**element {
                Ty::Ref(reference) => (*reference.referent).clone(),
                element => element.clone(),
            });
        }
        if let Ty::Struct(name, _) = &obj_ty
            && !self.structs.contains_key(name)
        {
            // A discovery-round abstract scalar range subscripts by Int to
            // its scalar element; the fixpoint rewrite registers the
            // concrete struct with its ordinary `__getitem__` before any
            // lowering consumes this shortcut.
            if let Some((_, dtype)) = mojito_types::types::scalar_range_parts(&obj_ty) {
                let idx_ty = self.infer(index)?;
                if !self.is_index_type(&idx_ty) {
                    return Err(TypeError::TypeMismatch {
                        expected: "Indexer".to_string(),
                        found: idx_ty.to_string(),
                        context: "index".to_string(),
                    });
                }
                return Ok(simd_ty(dtype, 1));
            }
            if let Some(element) = list_element(&obj_ty) {
                let idx_ty = self.infer(index)?;
                if !self.is_index_type(&idx_ty) {
                    return Err(TypeError::TypeMismatch {
                        expected: "Indexer".to_string(),
                        found: idx_ty.to_string(),
                        context: "index".to_string(),
                    });
                }
                self.record_interior_reference(span, object, "element");
                return Ok(match element.clone() {
                    Ty::Ref(reference) => *reference.referent,
                    element => element,
                });
            }
            if let Some((key, value)) = dict_elements(&obj_ty) {
                let idx_ty = self.infer(index)?;
                if !coerces(&idx_ty, key) {
                    return Err(TypeError::TypeMismatch {
                        expected: key.to_string(),
                        found: idx_ty.to_string(),
                        context: "dictionary key".to_string(),
                    });
                }
                self.record_replacing_interior_reference(span, object, "value");
                return Ok(match value.clone() {
                    Ty::Ref(reference) => *reference.referent,
                    value => value,
                });
            }
        }
        // A user struct with `__getitem__` is subscriptable: `c[i]` →
        // `c.__getitem__(i)`, typed by the method (the index need not be `Int`).
        if matches!(obj_ty, Ty::Struct(..)) {
            if let Ty::Struct(name, _) = &obj_ty
                && self
                    .structs
                    .get(name)
                    .is_some_and(|info| info.methods.contains_key("__getitem__"))
            {
                let result = self.infer_struct_getitem_call(
                    span,
                    object,
                    std::slice::from_ref(index),
                    &obj_ty,
                )?;
                // A source `ref[...] T` subscript result is a reference handle
                // only in a reference-valued context. Ordinary expression
                // inference reads through it to `T`; `RefDecl` and aggregate
                // storage recover the checked handle from the interior-origin
                // and place-type facts recorded above.
                return Ok(match result {
                    Ty::Ref(reference) => *reference.referent,
                    value => value,
                });
            }
            return Err(TypeError::NotIndexable(obj_ty.to_string()));
        }
        // Bounded type parameters use the same method-selection path as
        // concrete nominal receivers. This retains the requirement's error,
        // convention, capture, and reference-origin contract instead of
        // projecting only its parameter/result types.
        if matches!(obj_ty, Ty::Param { .. }) {
            return self.infer_method_call(
                span,
                object,
                "__getitem__",
                MethodCallArguments::ordinary(std::slice::from_ref(index), &[]),
            );
        }
        // The result of indexing: a SIMD lane, a List element, or a pointer pointee.
        let result = match &obj_ty {
            Ty::Simd { dtype, .. } => simd_ty(*dtype, 1),
            Ty::Pointer { element, origin } => {
                self.check_pointer_offset(origin, index)?;
                (**element).clone()
            }
            _ => return Err(TypeError::NotIndexable(obj_ty.to_string())),
        };
        let idx_ty = self.infer(index)?;
        if !self.is_index_type(&idx_ty) {
            return Err(TypeError::TypeMismatch {
                expected: "Indexer".to_string(),
                found: idx_ty.to_string(),
                context: "index".to_string(),
            });
        }
        Ok(match result {
            Ty::Ref(reference) => *reference.referent,
            other => other,
        })
    }

    /// Select and record the nominal `__getitem__` contract for a struct
    /// subscript read — the shared entry for `infer_index`'s tail and the bare
    /// element-call re-dispatch, generalized to the multi-index arity. Records
    /// the interior-reference fact for list/dict-backed receivers and returns
    /// the getter's raw result type; callers read through `Ty::Ref`.
    pub(super) fn infer_struct_getitem_call(
        &self,
        span: SourceSpan,
        object: &Expr,
        indices: &[Expr],
        obj_ty: &Ty,
    ) -> Result<Ty, TypeError> {
        let result = self.infer_method_call(
            span.clone(),
            object,
            "__getitem__",
            MethodCallArguments::ordinary(indices, &[]),
        )?;
        if list_element(obj_ty).is_some() {
            self.record_interior_reference(span, object, "element");
        } else if dict_elements(obj_ty).is_some() {
            self.record_replacing_interior_reference(span, object, "value");
        }
        Ok(result)
    }

    /// An origin-bearing pointer to a precise place designates exactly one
    /// checked value, so only offset 0 can be dereferenced; any other offset
    /// would be out-of-provenance access that Mojo leaves undefined. A
    /// multi-element provenance — an interior-generation domain or an origin
    /// parameter — dereferences at any offset, with the VM's arena bounds
    /// check as the dynamic backstop.
    pub(super) fn check_pointer_offset(
        &self,
        origin: &mojito_types::origin::PointerOrigin,
        index: &Expr,
    ) -> Result<(), TypeError> {
        if origin.as_origin().is_none() || origin.multi_element() {
            return Ok(());
        }
        match self.eval_ct(index) {
            Ok(value) if value.is_zero() => Ok(()),
            _ => Err(TypeError::Unsupported(
                "an origin-bearing Pointer designates a single value; only \
                 offset 0 can be dereferenced"
                    .to_string(),
            )),
        }
    }

    /// Reject a write through a pointer whose provenance does not carry
    /// statically known mutable capability. A symbolic parameter mutability is
    /// writable: storage coercion only admits mutable places into fields whose
    /// declared mutability is not explicitly immutable.
    pub(super) fn check_pointer_write(
        &self,
        origin: &mojito_types::origin::PointerOrigin,
    ) -> Result<(), TypeError> {
        if origin.statically_mutable() == Some(false) {
            return Err(TypeError::Unsupported(
                "cannot write through a Pointer with an immutable origin".to_string(),
            ));
        }
        Ok(())
    }

    /// Whether a value can be normalized to the VM's index representation.
    /// Numeric literals/Int use the identity path; an opaque `Indexer` or a
    /// concrete conformer supplies `__mlir_index__() -> Int`.
    pub(super) fn is_index_type(&self, ty: &Ty) -> bool {
        coerces(ty, &Ty::Int)
            || matches!(ty, Ty::Param { bounds, .. } if bounds.iter().any(|bound| bound == "Indexer"))
            || matches!(ty, Ty::Struct(..))
                && self.struct_dunder(ty, "__mlir_index__", &[]) == Some(Ok(Ty::Int))
    }

    /// Resolve the exact normalization method for a non-Int `Indexer`. The
    /// abstract trait symbol is retained for a bounded generic and retargeted by
    /// ordinary checked method dispatch once its runtime receiver is known.
    pub(super) fn index_normalization_target(&self, ty: &Ty) -> Option<String> {
        match ty {
            Ty::Struct(name, arguments) => {
                let info = self.structs.get(name)?;
                let substitution = struct_subst(&info.decls, arguments);
                let methods = info.methods.get("__mlir_index__")?;
                let selected = methods.iter().find(|method| {
                    method.has_self
                        && method.params.is_empty()
                        && substitute(&method.ret, &substitution) == Ty::Int
                })?;
                Some(if methods.len() == 1 {
                    format!("{name}.__mlir_index__")
                } else {
                    method_lowered_name(
                        name,
                        "__mlir_index__",
                        selected,
                        self.self_instance_ty(name).as_ref(),
                    )
                })
            }
            Ty::Param { bounds, .. } => {
                let methods = self.lookup_trait_methods(bounds, "__mlir_index__", 0);
                let selected = methods
                    .iter()
                    .find(|method| method.params.is_empty() && method.ret == Ty::Int)?;
                Some(method_lowered_name(
                    "__trait_dispatch",
                    "__mlir_index__",
                    selected,
                    None,
                ))
            }
            _ => None,
        }
    }

    pub(super) fn has_index_normalization(&self, expression: &Expr, expected: &Ty) -> bool {
        *expected == Ty::Int
            && self
                .implicit_conversions
                .borrow()
                .get(&expression.source_span())
                .is_some_and(|target| mojito_symbol::symbol::is_index_normalization_symbol(target))
    }

    /// Prepare a subscript argument for checked lowering. A nominal receiver
    /// normalizes an Indexer only when no `__getitem__` or `__setitem__`
    /// candidate accepts its source type at this argument position and an Int
    /// candidate exists there, preserving direct user overloads. Positions in a
    /// `*indices` tail use the variadic element contract. Primitive pointer/SIMD
    /// and homogeneous private-pack indexing has a fixed Int contract and
    /// therefore always records the checked normalization fallback.
    pub(super) fn prepare_index_argument(
        &self,
        receiver: &Ty,
        expression: &Expr,
        method: &str,
        position: usize,
    ) -> Result<(), TypeError> {
        let actual = self.infer(expression)?;
        if coerces(&actual, &Ty::Int) {
            return Ok(());
        }
        let Some(target) = self.index_normalization_target(&actual) else {
            return Ok(());
        };
        let Ty::Struct(name, arguments) = receiver else {
            if matches!(
                receiver,
                Ty::Pointer { .. } | Ty::Simd { .. } | Ty::VariadicPack(_)
            ) {
                self.implicit_conversions
                    .borrow_mut()
                    .insert(expression.source_span(), target);
            }
            return Ok(());
        };
        let Some(info) = self.structs.get(name) else {
            return Ok(());
        };
        let Some(signatures) = info.methods.get(method) else {
            return Ok(());
        };
        let substitution = struct_subst(&info.decls, arguments);
        let mut accepts_source = false;
        let mut accepts_int = false;
        for signature in signatures.iter().filter(|signature| signature.has_self) {
            let parameter = match signature.variadic_index {
                Some(variadic_index) if position >= variadic_index => signature.variadic.as_deref(),
                _ => signature.params.get(position),
            };
            let Some(parameter) = parameter else {
                continue;
            };
            let expected = substitute(parameter, &substitution);
            accepts_source |= self.value_coerces(&actual, &expected)
                || self
                    .implicit_conversion_target(&actual, &expected)?
                    .is_some();
            accepts_int |= expected == Ty::Int;
        }
        if !accepts_source && accepts_int {
            self.implicit_conversions
                .borrow_mut()
                .insert(expression.source_span(), target);
        }
        Ok(())
    }

    /// Type a field access `object.field`. On a generic struct value the field
    /// type has the struct's type arguments substituted in (`Pair[Int].left :
    /// Int`).
    pub(super) fn infer_member(&self, object: &Expr, field: &str) -> Result<Ty, TypeError> {
        // `Self.n` reads the enclosing struct's value parameter (an `Int`).
        if let ExprKind::Identifier(s) = &object.kind
            && s == "Self"
        {
            if let Some(self_ty) = &self.self_ty {
                self.expression_types
                    .borrow_mut()
                    .insert(object.source_span(), self_ty.clone());
            }
            return match self.self_decls.iter().find(|d| d.name() == field) {
                Some(ParamDecl::Value { .. }) => Ok(Ty::Int),
                _ => Err(TypeError::UnknownSelfParam(field.to_string())),
            };
        }
        // `T.size` where `T` is a generic type parameter and a bound trait
        // requires `comptime size: Int`: expression-level access to an associated
        // compile-time value. Type-valued associated members remain type-position
        // only (`T.Element` in annotations).
        if let ExprKind::Identifier(name) = &object.kind
            && let Some(parameter) = self.lookup_tparam(name)
            && let Ty::Param { bounds, .. } = &parameter
            && let Some(ty) = self.lookup_trait_assoc_value_ty(bounds, field)
        {
            self.expression_types
                .borrow_mut()
                .insert(object.source_span(), parameter);
            return Ok(ty);
        }
        let obj_ty = self.infer(object)?;
        // Projecting a field through a reference-returning expression borrows
        // that referent for the projection; it does not first create an owned
        // copy of the whole value. Retain the handle exactly as method and
        // chained-subscript receivers do, including for linear referents.
        if let Some(reference) = self.infer_reference_value(object) {
            self.reference_value_uses.borrow_mut().insert(
                object.source_span(),
                reference.mutability == mojito_types::origin::Mutability::Mutable,
            );
        }
        if matches!(&obj_ty, Ty::Struct(name, args) if matches!(name.as_str(), "Slice" | "ContiguousSlice" | "StridedSlice") && args.is_empty())
            && matches!(field, "start" | "end" | "step")
        {
            return Ok(Ty::Struct("Optional".to_string(), vec![TyArg::Ty(Ty::Int)]));
        }
        if let Ty::Struct(sname, targs) = &obj_ty {
            let info = self.structs.get(sname).ok_or_else(|| {
                TypeError::InvariantViolation(format!("struct '{sname}' was not registered"))
            })?;
            if let Some((_, fty)) = info.fields.iter().find(|(n, _)| n == field) {
                let subst = struct_subst(&info.decls, targs);
                return Ok(match substitute(fty, &subst) {
                    Ty::Ref(reference) => *reference.referent,
                    value => value,
                });
            }
        }
        Err(TypeError::NoSuchField {
            object_type: obj_ty.to_string(),
            field: field.to_string(),
        })
    }
}
